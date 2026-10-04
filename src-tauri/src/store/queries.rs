//! All SQL for the local store lives here (single-SQL-module invariant).
//!
//! Per the architectual sketch, every `messages` write path — upsert,
//! expunge-diff, body/attachment insert — is channelled through the
//! functions below. No raw SQL escapes this module.

use rusqlite::{Connection, Row};
use serde::Serialize;

use super::{StoreError, StoreResult};

// ── Data types ───────────────────────────────────────────────────

/// A cached message header row as surfaced to the UI layer.
#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
    pub uid: u32,
    pub subject: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub date_utc: String,
    pub flags: String,
    pub has_attachments: bool,
    pub preview: String,
}

/// Parse the stored JSON flags string into a `Vec<String>`.
pub fn parse_flags(flags_json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(flags_json).unwrap_or_default()
}

/// `true` when the `\Seen` flag is absent — the message is unread.
///
/// Read/unread is **display-only** in M1: flags are stored on the row but
/// the client never writes `\Seen` back to the server (D-flags).
pub fn is_unread(flags_json: &str) -> bool {
    !parse_flags(flags_json).iter().any(|f| f == "\\Seen")
}

// ── mailbox / sync-state ─────────────────────────────────────────

/// Get-or-create a mailbox row by name; returns its integer id.
///
/// `uid_validity` and `uid_next` default to 0 (meaning "never synced").
pub fn ensure_mailbox(conn: &Connection, name: &str) -> StoreResult<u64> {
    conn.execute(
        "INSERT INTO mailboxes (name, uid_validity, uid_next)
         VALUES (?1, 0, 0)
         ON CONFLICT(name) DO NOTHING",
        rusqlite::params![name],
    )?;
    let id: u64 = conn.query_row(
        "SELECT id FROM mailboxes WHERE name = ?1",
        rusqlite::params![name],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Returns `(uid_validity, uid_next)` if the mailbox has been synced
/// at least once, or `None` on first run.
pub fn get_sync_state(conn: &Connection, mailbox: &str) -> StoreResult<Option<(u32, u32)>> {
    let result = conn.query_row(
        "SELECT uid_validity, uid_next FROM mailboxes WHERE name = ?1",
        rusqlite::params![mailbox],
        |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
    );
    match result {
        Ok(state) => Ok(Some(state)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

/// Persist `uid_validity` + `uid_next` after a successful sweep,
/// stamping `last_sync_at` with the current UTC time.
pub fn set_sync_state(
    conn: &Connection,
    mailbox: &str,
    uid_validity: u32,
    uid_next: u32,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE mailboxes
         SET uid_validity = ?1,
             uid_next     = ?2,
             last_sync_at = datetime('now')
         WHERE name = ?3",
        rusqlite::params![uid_validity, uid_next, mailbox],
    )?;
    Ok(())
}

/// Sync status for the UI: last sync timestamp, UIDVALIDITY, UIDNEXT,
/// and current message count for the named mailbox.
pub fn sync_status(
    conn: &Connection,
    mailbox: &str,
) -> StoreResult<Option<(String, u32, u32, i64)>> {
    match conn.query_row(
        "SELECT last_sync_at, uid_validity, uid_next,
                (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id = mailboxes.id)
         FROM mailboxes WHERE name = ?1",
        rusqlite::params![mailbox],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

// ── messages (upsert / expunge) ──────────────────────────────────

const UPSERT_MESSAGE_SQL: &str = concat!(
    "INSERT INTO messages ",
    "(mailbox_id, uid, message_id, subject, from_addr, to_addrs, ",
    " cc_addrs, date_utc, flags, has_attachments, preview) ",
    "VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ",
    "ON CONFLICT(mailbox_id, uid) DO UPDATE SET ",
    "  message_id      = excluded.message_id, ",
    "  subject         = excluded.subject, ",
    "  from_addr       = excluded.from_addr, ",
    "  to_addrs        = excluded.to_addrs, ",
    "  cc_addrs        = excluded.cc_addrs, ",
    "  date_utc        = excluded.date_utc, ",
    "  flags           = excluded.flags, ",
    "  has_attachments = excluded.has_attachments, ",
    "  preview         = excluded.preview",
);

/// Upsert a single message header row.  Keyed on `(mailbox_id, uid)`
/// so re-swarms overwrite in place rather than duplicating.
///
/// This is the **single writer** for the sync sweep (no second SQL path).
pub fn upsert_message(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    message_id: Option<&str>,
    subject: &str,
    from_addr: &str,
    to_addrs: &str,
    cc_addrs: &str,
    date_utc: &str,
    flags: &str,
    has_attachments: bool,
    preview: &str,
) -> StoreResult<()> {
    conn.execute(
        UPSERT_MESSAGE_SQL,
        rusqlite::params![
            mailbox_id,
            uid,
            message_id,
            subject,
            from_addr,
            to_addrs,
            cc_addrs,
            date_utc,
            flags,
            has_attachments,
            preview,
        ],
    )?;
    Ok(())
}

/// Batch upsert of message header rows.
///
/// Takes `&Connection` (not `&mut`) — each item is a single UPSERT
/// statement, so no transaction wrapper is needed. The caller can wrap
/// a larger sweep in a transaction if it owns `&mut Connection`; the
/// single-writer invariant is preserved because all SQL routes through
/// this module. The 200-UID batch commit boundary is enforced by the
/// sync worker (Plan 02-02), not here.
pub fn upsert_messages_batch(
    conn: &Connection,
    mailbox_id: u64,
    messages: &[InsertMessage<'_>],
) -> StoreResult<()> {
    for msg in messages {
        conn.execute(
            UPSERT_MESSAGE_SQL,
            rusqlite::params![
                mailbox_id,
                msg.uid,
                msg.message_id,
                &msg.subject,
                &msg.from_addr,
                &msg.to_addrs,
                &msg.cc_addrs,
                &msg.date_utc,
                &msg.flags,
                msg.has_attachments,
                &msg.preview,
            ],
        )?;
    }
    Ok(())
}

/// A message header ready for insertion.
pub struct InsertMessage<'a> {
    pub uid: u32,
    pub message_id: Option<&'a str>,
    pub subject: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub date_utc: String,
    pub flags: String,
    pub has_attachments: bool,
    pub preview: String,
}

/// Delete messages for `mailbox_id` whose `uid` is **not** in `live_uids`.
///
/// Empty slice → wipe every row for the mailbox (UIDVALIDITY-bump path).
/// Returns the number of rows deleted (for sync summary stats).
/// This is plain row deletion mirroring server-side mailbox removals;
/// no IMAP flag-write verbs appear in this module.
///
/// Implementation note: the live set goes through a TEMP TABLE instead of
/// a `NOT IN (?, ?, …)` list, so mailboxes of any size work regardless of
/// the SQLite bound-variables limit.
pub fn delete_missing_uids(
    conn: &Connection,
    mailbox_id: u64,
    live_uids: &[u32],
) -> StoreResult<usize> {
    if live_uids.is_empty() {
        return conn
            .execute(
                "DELETE FROM messages WHERE mailbox_id = ?1",
                rusqlite::params![mailbox_id],
            )
            .map_err(StoreError::from);
    }

    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS sge_live_uids(uid INTEGER PRIMARY KEY);
         DELETE FROM sge_live_uids;",
    )?;
    {
        let mut stmt = conn.prepare("INSERT OR IGNORE INTO sge_live_uids(uid) VALUES (?1)")?;
        for chunk in live_uids.chunks(500) {
            for uid in chunk {
                stmt.execute(rusqlite::params![uid])?;
            }
        }
    }
    conn.execute(
        "DELETE FROM messages WHERE mailbox_id = ?1 AND uid NOT IN (SELECT uid FROM sge_live_uids)",
        rusqlite::params![mailbox_id],
    )
    .map_err(StoreError::from)
}

/// Return the set of UIDs from `uids` that already exist for `mailbox_id`.
/// Used by the sync worker to distinguish new vs updated messages in a
/// batch. Empty input short-circuits to an empty vec.
pub fn existing_uids(
    conn: &Connection,
    mailbox_id: u64,
    uids: &[u32],
) -> StoreResult<Vec<u32>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = "?,".repeat(uids.len());
    let in_clause = &placeholders[..placeholders.len() - 1];
    let sql = format!(
        "SELECT uid FROM messages WHERE mailbox_id = ?1 AND uid IN ({in_clause})"
    );
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(uids.len() + 1);
    params.push(&mailbox_id);
    for uid in uids {
        params.push(uid);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params.as_slice(), |row| row.get::<_, u32>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Look up the integer `messages.id` for a given mailbox + IMAP UID.
pub fn find_message_id(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<Option<u64>> {
    match conn.query_row(
        "SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, u64>(0),
    ) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

// ── message list / search ────────────────────────────────────────────

fn row_to_message(row: &Row<'_>) -> Result<MessageRow, rusqlite::Error> {
    Ok(MessageRow {
        uid: row.get::<_, u32>(0)?,
        subject: row.get::<_, String>(1)?,
        from_addr: row.get::<_, String>(2)?,
        to_addrs: row.get::<_, String>(3)?,
        date_utc: row.get::<_, String>(4)?,
        flags: row.get::<_, String>(5)?,
        has_attachments: row.get::<_, bool>(6)?,
        preview: row.get::<_, String>(7)?,
    })
}

/// Look up a single message row by UID (for the reader).
pub fn get_message_by_uid(
    conn: &Connection,
    mailbox: &str,
    uid: u32,
) -> StoreResult<Option<MessageRow>> {
    match conn.query_row(
        "SELECT m.uid, m.subject, m.from_addr, m.to_addrs, m.date_utc, \
         m.flags, m.has_attachments, m.preview \
         FROM messages m JOIN mailboxes mb ON m.mailbox_id = mb.id \
         WHERE mb.name = ?1 AND m.uid = ?2",
        rusqlite::params![mailbox, uid],
        row_to_message,
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

/// Metadata for a single attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentInfo {
    pub name: String,
    pub size: u64,
    pub content_type: String,
    pub part_number: String,
}

/// List attachment metadata for a message (Phase 4 reader cache).
pub fn list_attachments(
    conn: &Connection,
    message_id: u64,
) -> StoreResult<Vec<AttachmentInfo>> {
    let mut stmt = conn.prepare_cached(
        "SELECT part_number, filename, mime_type, size_bytes \
         FROM attachment_parts WHERE message_id = ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![message_id], |row| {
        Ok(AttachmentInfo {
            part_number: row.get::<_, String>(0)?,
            name: row.get::<_, String>(1)?,
            content_type: row.get::<_, String>(2)?,
            size: row.get::<_, u64>(3)?,
        })
    })?;
    rows.map(|r| r.map_err(StoreError::Sql)).collect()
}

/// List messages for a mailbox, newest-first (`date_utc DESC, uid DESC`).
///
/// `offset` is used for pagination (infinite scroll): pass 0 for the first
/// batch, then the running count for subsequent batches.
pub fn list_messages(
    conn: &Connection,
    mailbox: &str,
    limit: usize,
    offset: usize,
) -> StoreResult<Vec<MessageRow>> {
    let sql = concat!(
        "SELECT m.uid, m.subject, m.from_addr, m.to_addrs, ",
        "m.date_utc, m.flags, m.has_attachments, m.preview ",
        "FROM messages m ",
        "JOIN mailboxes mb ON m.mailbox_id = mb.id ",
        "WHERE mb.name = ?1 ",
        "ORDER BY m.date_utc DESC, m.uid DESC ",
        "LIMIT ?2 OFFSET ?3",
    );
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        rusqlite::params![mailbox, limit as i64, offset as i64],
        row_to_message,
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Full-text search over `messages_fts` with BM25 ranking, scoped to a mailbox.
pub fn fts_search(conn: &Connection, mailbox: &str, query: &str) -> StoreResult<Vec<MessageRow>> {
    let sql = concat!(
        "SELECT m.uid, m.subject, m.from_addr, m.to_addrs, ",
        "m.date_utc, m.flags, m.has_attachments, m.preview ",
        "FROM messages_fts ",
        "JOIN messages m ON messages_fts.rowid = m.id ",
        "JOIN mailboxes mb ON m.mailbox_id = mb.id ",
        "WHERE messages_fts MATCH ?1 AND mb.name = ?2 ",
        "ORDER BY rank",
    );
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![query, mailbox], row_to_message)?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// ── bodies ───────────────────────────────────────────────────────

const SET_BODY_SQL: &str = concat!(
    "INSERT INTO message_bodies (message_id, body_text, body_html, body_complete, fetched_at) ",
    "VALUES (?1, ?2, ?3, 1, datetime('now')) ",
    "ON CONFLICT(message_id) DO UPDATE SET ",
    "  body_text     = excluded.body_text, ",
    "  body_html     = excluded.body_html, ",
    "  body_complete = 1, ",
    "  fetched_at    = excluded.fetched_at",
);

/// Insert or replace sanitized body text/html for a message.
/// Marks `body_complete = 1`.
pub fn insert_body(
    conn: &Connection,
    message_id: u64,
    body_text: Option<&str>,
    body_html: Option<&str>,
) -> StoreResult<()> {
    conn.execute(
        SET_BODY_SQL,
        rusqlite::params![message_id, body_text, body_html],
    )?;
    Ok(())
}

/// Check whether a message body has been fully fetched.
/// Returns `false` if no body row exists yet (header-only).
pub fn body_is_complete(conn: &Connection, message_id: u64) -> StoreResult<bool> {
    let result = conn.query_row(
        "SELECT body_complete FROM message_bodies WHERE message_id = ?1",
        rusqlite::params![message_id],
        |row| row.get::<_, i64>(0),
    );
    match result {
        Ok(v) => Ok(v != 0),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

// ── attachment metadata ──────────────────────────────────────────

const INSERT_ATTACHMENT_SQL: &str = concat!(
    "INSERT INTO attachment_parts ",
    "(message_id, part_number, filename, mime_type, size_bytes, local_path) ",
    "VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
);

/// Insert attachment **metadata only** — bytes live on disk (D-attachments).
/// `local_path` is NULL until the file is downloaded.
pub fn insert_attachment_meta(
    conn: &Connection,
    message_id: u64,
    part_number: &str,
    filename: &str,
    mime_type: &str,
    size_bytes: u64,
) -> StoreResult<()> {
    conn.execute(
        INSERT_ATTACHMENT_SQL,
        rusqlite::params![message_id, part_number, filename, mime_type, size_bytes],
    )?;
    Ok(())
}

// ── tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::time::Instant;

    /// Helper: count messages for a mailbox.
    fn count_messages(conn: &Connection, mailbox_id: u64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE mailbox_id = ?1",
            rusqlite::params![mailbox_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Helper: upsert a minimal header row.
    fn insert_msg(conn: &Connection, mb_id: u64, uid: u32, subject: &str, from: &str, flags: &str) {
        upsert_message(
            conn,
            mb_id,
            uid,
            None,
            subject,
            from,
            "[]",
            "[]",
            "2024-01-01T00:00:00Z",
            flags,
            false,
            "preview",
        )
        .unwrap();
    }

    // ── Task 1 verification: schema exists ──────────────────────

    #[test]
    fn all_tables_present_after_migration() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(tables.contains(&"mailboxes".to_string()));
        assert!(tables.contains(&"messages".to_string()));
        assert!(tables.contains(&"message_bodies".to_string()));
        assert!(tables.contains(&"attachment_parts".to_string()));
        assert!(tables.contains(&"messages_fts".to_string()));
    }

    // ── Task 2 verification: round-trip upsert/expunge/search ───

    #[test]
    fn upsert_expunge_fts_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        // Insert 3 rows
        insert_msg(conn, mb_id, 1, "Hello", "alice@example.com", "[]");
        insert_msg(conn, mb_id, 2, "World", "bob@example.com", "[]");
        insert_msg(conn, mb_id, 3, "Test", "carol@example.com", "[]");
        assert_eq!(count_messages(conn, mb_id), 3);

        // Expunge: UID 2 vanishes → delete_missing_uids removes exactly 1
        delete_missing_uids(conn, mb_id, &[1, 3]).unwrap();
        assert_eq!(count_messages(conn, mb_id), 2);

        // Check that UID 2 is gone, 1 and 3 remain
        let uids: Vec<u32> = conn
            .prepare("SELECT uid FROM messages WHERE mailbox_id = ?1 ORDER BY uid")
            .unwrap()
            .query_map(rusqlite::params![mb_id], |r| r.get::<_, u32>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(uids, vec![1, 3]);

        // FTS search finds the sender offline (no IMAP needed)
        let results = fts_search(conn, "INBOX", "alice").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].from_addr, "alice@example.com");
    }

    #[test]
    fn upsert_overwrites_existing_uid() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        insert_msg(conn, mb_id, 1, "Old", "a@x.com", "[]");

        // Same UID, different subject → should overwrite, not duplicate
        upsert_message(
            conn,
            mb_id,
            1,
            None,
            "New",
            "a@x.com",
            "[]",
            "[]",
            "2024-01-01T00:00:00Z",
            "[]",
            false,
            "new preview",
        )
        .unwrap();

        assert_eq!(count_messages(conn, mb_id), 1);
        let rows = list_messages(conn, "INBOX", 100, 0).unwrap();
        assert_eq!(rows[0].subject, "New");
    }

    // ── Task 3 verification: UIDVALIDITY resync wipe ───────────

    #[test]
    fn uidvalidity_resync_wipe_no_duplicates() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        // Initial sync under uid_validity A = 100
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        set_sync_state(conn, "INBOX", 100, 10).unwrap();

        insert_msg(conn, mb_id, 1, "Msg1", "a@x.com", "[]");
        insert_msg(conn, mb_id, 2, "Msg2", "b@x.com", "[]");
        insert_msg(conn, mb_id, 3, "Msg3", "c@x.com", "[]");
        assert_eq!(count_messages(conn, mb_id), 3);

        // Server bumps UIDVALIDITY → wipe mailbox + reset
        delete_missing_uids(conn, mb_id, &[]).unwrap(); // empty set = wipe all
        set_sync_state(conn, "INBOX", 999, 1).unwrap();
        assert_eq!(count_messages(conn, mb_id), 0);

        // Re-sweep with new UID set (1, 2 only — 3 is gone)
        insert_msg(conn, mb_id, 1, "New1", "n@x.com", "[]");
        insert_msg(conn, mb_id, 2, "New2", "n2@x.com", "[]");
        assert_eq!(count_messages(conn, mb_id), 2);

        // No duplicates — unique(mailbox_id, uid) held
        let state = get_sync_state(conn, "INBOX").unwrap();
        assert_eq!(state, Some((999, 1)));
    }

    #[test]
    fn uidvalidity_bump_clears_fts_index() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        insert_msg(conn, mb_id, 1, "StaleSubject", "old@x.com", "[]");

        // Wipe → old content must disappear from FTS
        delete_missing_uids(conn, mb_id, &[]).unwrap();
        let stale = fts_search(conn, "INBOX", "StaleSubject").unwrap();
        assert!(stale.is_empty(), "FTS must be empty after wipe");
    }

    // ── Task 3 verification: 10k-row perf smoke ─────────────────

    #[test]
    fn ten_thousand_row_perf_smoke() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        for i in 1..=10_000u32 {
            upsert_message(
                conn,
                mb_id,
                i,
                None,
                &format!("Subject {}", i),
                &format!("user{}@example.com", i % 100),
                "[]",
                "[]",
                "2024-06-01T12:00:00Z",
                "[]",
                i % 5 == 0,
                &format!("preview {}", i),
            )
            .unwrap();
        }

        // list_messages limit-200 should be well under 50 ms
        let start = Instant::now();
        let rows = list_messages(conn, "INBOX", 200, 0).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(rows.len(), 200);
        assert!(
            elapsed.as_millis() < 50,
            "list_messages 200 rows took {:?} (expected < 50 ms)",
            elapsed
        );

        // FTS search must return non-empty
        let results = fts_search(conn, "INBOX", "user42").unwrap();
        assert!(!results.is_empty(), "FTS should find user42 hits");
    }

    // ── Task 3 verification: flags stored, read/unread display-only ──

    #[test]
    fn flags_stored_but_read_unread_is_display_only() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        // Read message (has \Seen)
        insert_msg(conn, mb_id, 1, "Read", "a@x.com", r#"["\\Seen"]"#);
        // Unread message (no flags)
        insert_msg(conn, mb_id, 2, "Unread", "b@x.com", "[]");
        // Flagged + Seen
        insert_msg(
            conn,
            mb_id,
            3,
            "Flagged",
            "c@x.com",
            r#"["\\Flagged","\\Seen"]"#,
        );

        // Insert path must not error on Seen/Flagged input
        let rows = list_messages(conn, "INBOX", 100, 0).unwrap();
        assert_eq!(rows.len(), 3);

        // Flags persisted verbatim
        assert!(rows
            .iter()
            .any(|r| r.flags.contains("\\Seen") && r.subject == "Read"));
        assert!(rows.iter().any(|r| r.flags == "[]"));
        assert!(rows
            .iter()
            .any(|r| r.flags.contains("\\Flagged") && r.flags.contains("\\Seen")));

        // is_unread helper reads flags but never writes them back
        assert!(!is_unread(&rows[0].flags)); // Read → seen
        assert!(is_unread(&rows[1].flags)); // Unread → no Seen
    }

    // ── body + attachment metadata ─────────────────────────────

    #[test]
    fn body_insert_and_complete_flag() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        upsert_message(
            conn,
            mb_id,
            1,
            None,
            "S",
            "a@x.com",
            "[]",
            "[]",
            "2024-01-01T00:00:00Z",
            "[]",
            false,
            "p",
        )
        .unwrap();

        // Header-only: body not yet fetched
        let msg_id: u64 = conn
            .query_row(
                "SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = 1",
                rusqlite::params![mb_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!body_is_complete(conn, msg_id).unwrap());

        // Fetch body
        insert_body(conn, msg_id, Some("plain text"), Some("sanitized html")).unwrap();
        assert!(body_is_complete(conn, msg_id).unwrap());

        // Attachment metadata only (no bytes in DB)
        insert_attachment_meta(conn, msg_id, "2", "report.pdf", "application/pdf", 102_000)
            .unwrap();
        let att_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attachment_parts WHERE message_id = ?1",
                rusqlite::params![msg_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(att_count, 1);
    }
}
