//! All SQL for the local store lives here (single-SQL-module invariant).
//!
//! Per the architectual sketch, every `messages` write path — upsert,
//! expunge-diff, body/attachment insert — is channelled through the
//! functions below. No raw SQL escapes this module.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use super::{StoreError, StoreResult};

// ── Data types ───────────────────────────────────────────────────

/// A cached message header row as surfaced to the UI layer.
#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
    pub uid: u32,
    /// Raw wire mailbox name (modified UTF-7) the message belongs to —
    /// lets global search jump to the right folder.
    pub mailbox: String,
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

/// Canonical `\Seen` flag spelling (case-sensitive, backslash form).
///
/// Used for both the SQLite JSON `flags` column and the IMAP STORE
/// argument — a single constant so the two can never drift apart.
pub const SEEN_FLAG: &str = "\\Seen";

/// Rewrite a stored flags JSON string with the target Seen state.
///
/// Adds `\Seen` when `seen` is true, removes every occurrence when false,
/// preserving all other flags and their order. An empty or unparseable
/// prior value starts from an empty set, so toggling a message with no
/// prior flag state still applies the target state.
pub fn set_seen_flag(flags_json: &str, seen: bool) -> String {
    let mut flags = parse_flags(flags_json);
    flags.retain(|f| f != SEEN_FLAG);
    if seen {
        flags.push(SEEN_FLAG.to_string());
    }
    serde_json::to_string(&flags).unwrap_or_else(|_| "[]".to_string())
}

/// `true` when the `\Seen` flag is absent — the message is unread.
///
/// Local reads are display-only; writes go through `SessionManager`
/// (`imap/manager.rs`) via UID STORE, never through this helper.
pub fn is_unread(flags_json: &str) -> bool {
    !parse_flags(flags_json).iter().any(|f| f == SEEN_FLAG)
}

// ── mailbox / sync-state ─────────────────────────────────────────

/// Look up the integer `mailboxes.id` for a name without creating it.
/// Returns `None` when the mailbox was never synced (read-only paths like
/// `sync_status` must not create rows as a side effect).
pub fn mailbox_id(conn: &Connection, name: &str) -> StoreResult<Option<u64>> {
    match conn.query_row(
        "SELECT id FROM mailboxes WHERE name = ?1",
        rusqlite::params![name],
        |row| row.get::<_, u64>(0),
    ) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

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
        "SELECT IFNULL(last_sync_at, ''), uid_validity, uid_next,
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

/// Cache a folder's `STATUS` datum after a successful sweep (FOLD-02).
/// Creates the row when missing; stamps `status_synced_at` (NOT
/// `last_sync_at` — that stays reserved for message syncs so the sidebar
/// badge fallback keeps working). The sidebar badge shows the dynamic
/// local unread count once synced and falls back to this server datum for
/// never-synced folders.
pub fn set_mailbox_status(
    conn: &Connection,
    mailbox: &str,
    uid_validity: u32,
    uid_next: u32,
    unseen: u32,
) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO mailboxes (name, uid_validity, uid_next, unseen_count, status_synced_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(name) DO UPDATE SET
           uid_validity     = excluded.uid_validity,
           uid_next         = excluded.uid_next,
           unseen_count     = excluded.unseen_count,
           status_synced_at = excluded.status_synced_at",
        rusqlite::params![mailbox, uid_validity, uid_next, unseen],
    )?;
    Ok(())
}

/// Record a folder's LIST hierarchy delimiter (`/`, `.`, …) for tree
/// rendering (M6). Empty means flat/unknown.
pub fn set_mailbox_delimiter(
    conn: &Connection,
    mailbox: &str,
    delimiter: &str,
) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter)
         VALUES (?1, 0, 0, ?2)
         ON CONFLICT(name) DO UPDATE SET delimiter = excluded.delimiter",
        rusqlite::params![mailbox, delimiter],
    )?;
    Ok(())
}

/// Record a folder's resolved role + LIST attributes (M8, Plan 11-03).
///
/// `role` is one of `inbox|trash|sent|drafts|custom`
/// (`imap::roles::Role::as_str`); `attributes` the space-joined LIST
/// attributes. A CACHE rewritten on every LIST refresh — never trusted
/// without one (T-11-07).
pub fn set_mailbox_role(
    conn: &Connection,
    name: &str,
    role: &str,
    attributes: &str,
) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO mailboxes (name, uid_validity, uid_next, role, attributes)
         VALUES (?1, 0, 0, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET
           role = excluded.role,
           attributes = excluded.attributes",
        rusqlite::params![name, role, attributes],
    )?;
    Ok(())
}

/// Escape SQLite LIKE wildcards so a folder prefix matches literally —
/// `Pai` must never match `Pai2` (T-11-05).
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Rename a cached mailbox row plus its whole subtree (Plan 11-02).
///
/// Keeps `mailbox_id`, `uid_validity`, and messages untouched; children
/// move via prefix UPDATEs (`old + delimiter` → `new + delimiter`).
/// Returns rows touched. Children share the parent's delimiter; an exact
/// `Pai2` row never matches a `Pai` prefix (LIKE-escape).
pub fn rename_mailbox_cache(
    conn: &Connection,
    old: &str,
    new: &str,
    delimiter: &str,
) -> StoreResult<usize> {
    let mut touched = conn.execute(
        "UPDATE mailboxes SET name = ?1 WHERE name = ?2",
        rusqlite::params![new, old],
    )?;
    if !delimiter.is_empty() {
        let old_prefix = format!("{old}{delimiter}");
        let new_prefix = format!("{new}{delimiter}");
        let pattern = format!("{}%", escape_like(&old_prefix));
        let children: Vec<String> = {
            let mut stmt =
                conn.prepare("SELECT name FROM mailboxes WHERE name LIKE ?1 ESCAPE '\\'")?;
            let mut rows = stmt.query(rusqlite::params![pattern])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(row.get(0)?);
            }
            out
        };
        for child in children {
            // LIKE is ASCII case-insensitive, folder names are not: keep
            // only true byte-prefix children (`PAI/X` must not follow a
            // `Pai` → `Novo` rename). LIKE matched the literal prefix, so
            // slicing at the prefix byte length lands on a char boundary.
            if !child.starts_with(&old_prefix) {
                continue;
            }
            let new_name = format!("{new_prefix}{}", &child[old_prefix.len()..]);
            touched += conn.execute(
                "UPDATE mailboxes SET name = ?1 WHERE name = ?2",
                rusqlite::params![new_name, child],
            )?;
        }
    }
    Ok(touched)
}

/// Delete a cached mailbox row and its dependent cache state (Plan 11-02).
///
/// Messages cascade via FK; both durable outboxes (`flag_outbox`,
/// `imap_outbox`) drop in the same lock section. Children must already be
/// gone (the command refuses deletes with subfolders) — deleting a parent
/// row never orphans: callers assert zero remaining `LIKE` rows in tests.
pub fn delete_mailbox_cache(conn: &Connection, name: &str) -> StoreResult<()> {
    let id: Option<u64> = conn
        .query_row(
            "SELECT id FROM mailboxes WHERE name = ?1",
            rusqlite::params![name],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(mailbox_id) = id {
        drop_outbox_for_mailbox(conn, mailbox_id)?;
        drop_imap_outbox_for_mailbox(conn, mailbox_id)?;
    }
    conn.execute(
        "DELETE FROM mailboxes WHERE name = ?1",
        rusqlite::params![name],
    )?;
    Ok(())
}

/// A cached mailbox row as surfaced to the UI layer.
#[derive(Debug, Clone, Serialize)]
pub struct MailboxRow {
    pub id: u64,
    /// Raw wire name (modified UTF-7) — protocol use only.
    pub name: String,
    /// Decoded display name for the folder tree.
    pub display_name: String,
    /// LIST hierarchy delimiter ('' = flat).
    pub delimiter: String,
    /// Resolved role (`inbox|trash|sent|drafts|custom`, M8 cache — empty
    /// means not yet resolved, never assumed).
    pub role: String,
    /// Space-joined LIST attributes (M8 cache, e.g. `\HasNoChildren`).
    pub attributes: String,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub last_sync_at: Option<String>,
    pub unread_count: i64,
    /// Last `STATUS (UNSEEN)` datum (M3). Badge fallback for never-synced folders.
    pub unseen_count: i64,
}

/// List all mailboxes cached locally, with per-folder unread counts.
///
/// Returns rows ordered by name. Unread count is computed from
/// `messages.flags` JSON (presence of `\\Seen` flag → read).
pub fn list_mailboxes(conn: &Connection) -> StoreResult<Vec<MailboxRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, delimiter, role, attributes, uid_validity, uid_next, last_sync_at, unseen_count,
                (SELECT COUNT(*) FROM messages m
                 WHERE m.mailbox_id = mailboxes.id
                 AND NOT EXISTS (
                   SELECT 1 FROM json_each(m.flags)
                   WHERE json_each.value = '\\\\Seen'
                 ))
         FROM mailboxes
         ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            let display_name = crate::imap::mutf7::decode_modified_utf7(&name);
            Ok(MailboxRow {
                id: row.get(0)?,
                name,
                display_name,
                delimiter: row.get(2)?,
                role: row.get(3)?,
                attributes: row.get(4)?,
                uid_validity: row.get(5)?,
                uid_next: row.get(6)?,
                last_sync_at: row.get(7)?,
                unseen_count: row.get(8)?,
                unread_count: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sql)?;
    Ok(rows)
}

/// Count unread messages in a named mailbox (FOLD-02).
pub fn count_unread(conn: &Connection, mailbox: &str) -> StoreResult<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages m
         JOIN mailboxes mb ON m.mailbox_id = mb.id
         WHERE mb.name = ?1
         AND NOT EXISTS (
           SELECT 1 FROM json_each(m.flags)
           WHERE json_each.value = '\\\\Seen'
         )",
        rusqlite::params![mailbox],
        |row| row.get(0),
    )?;
    Ok(count)
}

// ── UID backfill (Phase 9) ─────────────────────────────────────

/// Strikes after which a UID stops being re-requested (tombstoned).
pub const TOMBSTONE_STRIKES: i64 = 3;

/// Passes between periodic full sweeps: remote flag changes on an otherwise
/// unchanged UID set still surface regularly (convergence shortcut blind
/// spot mitigation).
pub const FULL_SWEEP_EVERY: i64 = 5;

/// All locally cached UIDs for a mailbox (convergence set-diff).
pub fn all_local_uids(conn: &Connection, mailbox_id: u64) -> StoreResult<Vec<u32>> {
    let mut stmt = conn.prepare("SELECT uid FROM messages WHERE mailbox_id = ?1")?;
    let rows = stmt
        .query_map(rusqlite::params![mailbox_id], |row| row.get::<_, u32>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sql)?;
    Ok(rows)
}

/// Record one empty-FETCH strike for a UID; returns the new strike count.
/// A UID that FETCHes successfully must have its tombstone cleared via
/// [`clear_tombstone`] instead.
pub fn record_fetch_strike(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<i64> {
    conn.execute(
        "INSERT INTO fetch_tombstones (mailbox_id, uid, strikes)
         VALUES (?1, ?2, 1)
         ON CONFLICT(mailbox_id, uid) DO UPDATE SET
           strikes = strikes + 1,
           updated_at = datetime('now')",
        rusqlite::params![mailbox_id, uid],
    )?;
    let strikes: i64 = conn.query_row(
        "SELECT strikes FROM fetch_tombstones WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get(0),
    )?;
    Ok(strikes)
}

/// Clear a UID's tombstone (fetched successfully, or vanished from SEARCH).
pub fn clear_tombstone(conn: &Connection, mailbox_id: u64, uid: u32) -> StoreResult<()> {
    conn.execute(
        "DELETE FROM fetch_tombstones WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
    )?;
    Ok(())
}

/// UIDs at or above [`TOMBSTONE_STRIKES`]: excluded from future sweeps
/// until pruned (vanished from SEARCH) or recovered (periodic full sweep).
pub fn tombstoned_uids(conn: &Connection, mailbox_id: u64) -> StoreResult<Vec<u32>> {
    let mut stmt = conn.prepare(
        "SELECT uid FROM fetch_tombstones WHERE mailbox_id = ?1 AND strikes >= ?2",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![mailbox_id, TOMBSTONE_STRIKES],
            |row| row.get::<_, u32>(0),
        )?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::Sql)?;
    Ok(rows)
}

/// Drop tombstones for UIDs no longer on the server (expunged — Step 6
/// deletes their messages, so the strike record is stale).
pub fn prune_tombstones(
    conn: &Connection,
    mailbox_id: u64,
    live_uids: &[u32],
) -> StoreResult<()> {
    if live_uids.is_empty() {
        conn.execute(
            "DELETE FROM fetch_tombstones WHERE mailbox_id = ?1",
            rusqlite::params![mailbox_id],
        )?;
        return Ok(());
    }
    let placeholders = "?,".repeat(live_uids.len());
    let in_clause = &placeholders[..placeholders.len() - 1];
    let sql = format!(
        "DELETE FROM fetch_tombstones WHERE mailbox_id = ?1 AND uid NOT IN ({in_clause})"
    );
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(live_uids.len() + 1);
    params.push(&mailbox_id);
    for uid in live_uids {
        params.push(uid);
    }
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}

/// Consecutive converged (sweep-skipped) passes for a mailbox.
pub fn sweeps_since_full(conn: &Connection, mailbox_id: u64) -> StoreResult<i64> {
    let n: i64 = conn.query_row(
        "SELECT sweeps_since_full FROM mailboxes WHERE id = ?1",
        rusqlite::params![mailbox_id],
        |row| row.get(0),
    )?;
    Ok(n)
}

/// Reset (full sweep ran) or bump (converged skip) the sweep counter.
pub fn set_sweeps_since_full(
    conn: &Connection,
    mailbox_id: u64,
    value: i64,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE mailboxes SET sweeps_since_full = ?1 WHERE id = ?2",
        rusqlite::params![value, mailbox_id],
    )?;
    Ok(())
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
        // Explicit dependent deletes: `PRAGMA foreign_keys` is off by
        // default, so ON DELETE CASCADE cannot be relied on for
        // message_bodies/attachment_parts — orphans would survive the wipe.
        conn.execute(
            "DELETE FROM attachment_parts WHERE message_id IN \
             (SELECT id FROM messages WHERE mailbox_id = ?1)",
            rusqlite::params![mailbox_id],
        )?;
        conn.execute(
            "DELETE FROM message_bodies WHERE message_id IN \
             (SELECT id FROM messages WHERE mailbox_id = ?1)",
            rusqlite::params![mailbox_id],
        )?;
        let n = conn
            .execute(
                "DELETE FROM messages WHERE mailbox_id = ?1",
                rusqlite::params![mailbox_id],
            )
            .map_err(StoreError::from)?;
        // Wipe path (UIDVALIDITY bump): queued delete/move UIDs belong to
        // the dead generation — replaying them would move the wrong
        // messages (RFC 4549, same rule as the flag queue).
        drop_imap_outbox_for_mailbox(conn, mailbox_id)?;
        return Ok(n);
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
        "DELETE FROM attachment_parts WHERE message_id IN \
         (SELECT m.id FROM messages m WHERE m.mailbox_id = ?1 \
          AND m.uid NOT IN (SELECT uid FROM sge_live_uids))",
        rusqlite::params![mailbox_id],
    )?;
    conn.execute(
        "DELETE FROM message_bodies WHERE message_id IN \
         (SELECT m.id FROM messages m WHERE m.mailbox_id = ?1 \
          AND m.uid NOT IN (SELECT uid FROM sge_live_uids))",
        rusqlite::params![mailbox_id],
    )?;
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

/// Current `flags` JSON for a message row, if it exists.
/// Used by the pending-wins reconcile gate to keep optimistic local flags.
pub fn message_flags(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<Option<String>> {
    match conn.query_row(
        "SELECT flags FROM messages WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, String>(0),
    ) {
        Ok(flags) => Ok(Some(flags)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(StoreError::Sql(e)),
    }
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

/// RFC822 Message-ID display field for a cached row (dest-side Seen
/// resolution matches on it after a move assigns a new UID). Never a key.
pub fn message_rfc_id(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<Option<String>> {
    match conn.query_row(
        "SELECT message_id FROM messages WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(mid) => Ok(mid),
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
        mailbox: row.get::<_, String>(8)?,
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
         m.flags, m.has_attachments, m.preview, mb.name \
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
        "m.date_utc, m.flags, m.has_attachments, m.preview, mb.name ",
        "FROM messages m ",
        "JOIN mailboxes mb ON m.mailbox_id = mb.id ",
        "WHERE mb.name = ?1 AND m.pending_delete = 0 ",
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

/// Full-text search over `messages_fts` with BM25 ranking.
/// `mailbox = None` searches the whole account (all folders); `Some(name)`
/// scopes to one folder. Every row carries its folder in `mailbox` so
/// global results can jump to the right folder.
pub fn fts_search(
    conn: &Connection,
    mailbox: Option<&str>,
    query: &str,
) -> StoreResult<Vec<MessageRow>> {
    let sql = concat!(
        "SELECT m.uid, m.subject, m.from_addr, m.to_addrs, ",
        "m.date_utc, m.flags, m.has_attachments, m.preview, mb.name ",
        "FROM messages_fts ",
        "JOIN messages m ON messages_fts.rowid = m.id ",
        "JOIN mailboxes mb ON m.mailbox_id = mb.id ",
        "WHERE messages_fts MATCH ?1 ",
        "AND (?2 IS NULL OR mb.name = ?2) ",
        "AND m.pending_delete = 0 ",
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

// ── flag outbox (durable offline queue) ──────────────────────────

/// One queued Seen toggle awaiting server acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxOp {
    pub id: u64,
    pub mailbox_id: u64,
    pub uid: u32,
    pub seen: bool,
    pub uid_validity: u32,
    pub attempts: i64,
    pub last_error: Option<String>,
}

/// Enqueue (or collapse) a Seen toggle for `mailbox_id` + `uid`.
///
/// `UNIQUE(mailbox_id, uid)` makes the latest toggle win: a rapid
/// read→unread→read flap converges to the final state and replays once
/// instead of once per tap. Re-enqueue refreshes the op epoch
/// (`uid_validity`) and resets the retry counters.
pub fn enqueue_outbox(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    seen: bool,
    uid_validity: u32,
) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO flag_outbox (mailbox_id, uid, seen, uid_validity)
          VALUES (?1, ?2, ?3, ?4)
          ON CONFLICT(mailbox_id, uid) DO UPDATE SET
            seen         = excluded.seen,
            uid_validity = excluded.uid_validity,
            attempts     = 0,
            last_error   = NULL",
        rusqlite::params![mailbox_id, uid, seen, uid_validity],
    )?;
    Ok(())
}

/// UIDs with a queued (unacknowledged) op for `mailbox_id`.
///
/// The sync worker gates its step-5 upsert loop on this set so pending
/// optimistic flags are never clobbered by a concurrent server sweep.
pub fn pending_uids(conn: &Connection, mailbox_id: u64) -> StoreResult<Vec<u32>> {
    let mut stmt =
        conn.prepare("SELECT uid FROM flag_outbox WHERE mailbox_id = ?1 ORDER BY uid")?;
    let rows = stmt
        .query_map(rusqlite::params![mailbox_id], |row| {
            row.get::<_, u32>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// All queued ops for `mailbox_id` in creation order (replay order).
pub fn list_outbox(conn: &Connection, mailbox_id: u64) -> StoreResult<Vec<OutboxOp>> {
    let mut stmt = conn.prepare(
        "SELECT id, mailbox_id, uid, seen, uid_validity, attempts, last_error
          FROM flag_outbox WHERE mailbox_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![mailbox_id], |row| {
            Ok(OutboxOp {
                id: row.get::<_, u64>(0)?,
                mailbox_id: row.get::<_, u64>(1)?,
                uid: row.get::<_, u32>(2)?,
                seen: row.get::<_, bool>(3)?,
                uid_validity: row.get::<_, u32>(4)?,
                attempts: row.get::<_, i64>(5)?,
                last_error: row.get::<_, Option<String>>(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Delete the queued op for `mailbox_id` + `uid` (server acknowledged).
/// Returns the number of rows deleted.
pub fn delete_outbox_op(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<usize> {
    conn.execute(
        "DELETE FROM flag_outbox WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
    )
    .map_err(StoreError::from)
}

/// Drop the whole mailbox queue (UIDVALIDITY-bump path, RFC 4549).
/// Returns the number of rows deleted.
pub fn drop_outbox_for_mailbox(conn: &Connection, mailbox_id: u64) -> StoreResult<usize> {
    conn.execute(
        "DELETE FROM flag_outbox WHERE mailbox_id = ?1",
        rusqlite::params![mailbox_id],
    )
    .map_err(StoreError::from)
}

/// Record a failed replay attempt (bumps `attempts`, stores the error text).
pub fn record_outbox_error(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    err: &str,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE flag_outbox
          SET attempts = attempts + 1, last_error = ?1
          WHERE mailbox_id = ?2 AND uid = ?3",
        rusqlite::params![err, mailbox_id, uid],
    )?;
    Ok(())
}

/// Number of queued (unacknowledged) ops for `mailbox_id`.
/// Surfaced in `sync_status` as the pending indicator.
pub fn outbox_count(conn: &Connection, mailbox_id: u64) -> StoreResult<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM flag_outbox WHERE mailbox_id = ?1",
        rusqlite::params![mailbox_id],
        |row| row.get(0),
    )
    .map_err(StoreError::from)
}

// ── imap outbox (durable offline delete/move queue, Phase 10) ──────

/// Queued delete/move operation kinds stored in `imap_outbox.op`.
pub const IMAP_OP_DELETE: &str = "delete";
pub const IMAP_OP_MOVE: &str = "move";

/// One queued delete/move awaiting server acknowledgement (pre-sweep replay).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapOutboxOp {
    pub id: u64,
    pub mailbox_id: u64,
    pub uid: u32,
    /// `'delete'` (move-to-Trash) or `'move'` (move to `dest_mailbox`).
    pub op: String,
    /// Target raw wire name. Always set: the resolved Trash for `delete`,
    /// the picker wire name for `move`.
    pub dest_mailbox: Option<String>,
    /// Pending Seen state captured from a dropped `flag_outbox` row at
    /// move-enqueue time; applied at the destination UID on replay.
    /// `None` for `delete` (MOVE/COPY preserve flags server-side).
    pub seen_intent: Option<bool>,
    pub uid_validity: u32,
    pub attempts: i64,
    pub last_error: Option<String>,
}

/// Enqueue (or collapse) a delete/move for `mailbox_id` + `uid`.
///
/// Latest-wins per `(mailbox_id, uid)` like the flag outbox. As part of the
/// same store-lock section the caller must hold, the same-key `flag_outbox`
/// row is consumed: a flag write to a soon-moved message is moot, and a
/// `move` carries its pending Seen state along as `seen_intent` for
/// dest-side apply at replay (a `delete` needs none — MOVE/COPY preserve
/// flags server-side). Re-enqueue refreshes epoch + dest and resets retry
/// counters.
pub fn enqueue_imap_outbox(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    op: &str,
    dest_mailbox: Option<&str>,
    uid_validity: u32,
) -> StoreResult<()> {
    let pending_seen: Option<bool> = match conn.query_row(
        "SELECT seen FROM flag_outbox WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, bool>(0),
    ) {
        Ok(seen) => Some(seen),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(StoreError::Sql(e)),
    };
    conn.execute(
        "DELETE FROM flag_outbox WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
    )?;
    let seen_intent: Option<bool> = if op == IMAP_OP_MOVE {
        pending_seen
    } else {
        None
    };
    conn.execute(
        "INSERT INTO imap_outbox (mailbox_id, uid, op, dest_mailbox, seen_intent, uid_validity)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6)
          ON CONFLICT(mailbox_id, uid) DO UPDATE SET
            op           = excluded.op,
            dest_mailbox = excluded.dest_mailbox,
            seen_intent  = excluded.seen_intent,
            uid_validity = excluded.uid_validity,
            attempts     = 0,
            last_error   = NULL",
        rusqlite::params![mailbox_id, uid, op, dest_mailbox, seen_intent, uid_validity],
    )?;
    Ok(())
}

/// UIDs with a queued delete/move for `mailbox_id`.
///
/// Joins the convergence gate alongside [`pending_uids`]: a queued delete
/// must never read as "converged".
pub fn pending_imap_uids(conn: &Connection, mailbox_id: u64) -> StoreResult<Vec<u32>> {
    let mut stmt =
        conn.prepare("SELECT uid FROM imap_outbox WHERE mailbox_id = ?1 ORDER BY uid")?;
    let rows = stmt
        .query_map(rusqlite::params![mailbox_id], |row| {
            row.get::<_, u32>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// All queued delete/move ops for `mailbox_id` in creation order.
pub fn list_imap_outbox(conn: &Connection, mailbox_id: u64) -> StoreResult<Vec<ImapOutboxOp>> {
    let mut stmt = conn.prepare(
        "SELECT id, mailbox_id, uid, op, dest_mailbox, seen_intent, uid_validity, attempts, last_error
          FROM imap_outbox WHERE mailbox_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![mailbox_id], |row| {
            Ok(ImapOutboxOp {
                id: row.get::<_, u64>(0)?,
                mailbox_id: row.get::<_, u64>(1)?,
                uid: row.get::<_, u32>(2)?,
                op: row.get::<_, String>(3)?,
                dest_mailbox: row.get::<_, Option<String>>(4)?,
                seen_intent: row.get::<_, Option<bool>>(5)?,
                uid_validity: row.get::<_, u32>(6)?,
                attempts: row.get::<_, i64>(7)?,
                last_error: row.get::<_, Option<String>>(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Delete the queued delete/move for `mailbox_id` + `uid` (acknowledged or
/// undone). Returns the number of rows deleted.
pub fn delete_imap_outbox_op(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<usize> {
    conn.execute(
        "DELETE FROM imap_outbox WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
    )
    .map_err(StoreError::from)
}

/// Drop the whole mailbox delete/move queue (UIDVALIDITY-bump path, RFC 4549).
/// Returns the number of rows deleted.
pub fn drop_imap_outbox_for_mailbox(
    conn: &Connection,
    mailbox_id: u64,
) -> StoreResult<usize> {
    conn.execute(
        "DELETE FROM imap_outbox WHERE mailbox_id = ?1",
        rusqlite::params![mailbox_id],
    )
    .map_err(StoreError::from)
}

/// Record a failed replay attempt (bumps `attempts`, stores the error text).
pub fn record_imap_outbox_error(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    err: &str,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE imap_outbox
          SET attempts = attempts + 1, last_error = ?1
          WHERE mailbox_id = ?2 AND uid = ?3",
        rusqlite::params![err, mailbox_id, uid],
    )?;
    Ok(())
}

/// Number of queued delete/move ops for `mailbox_id`.
/// Added to the flag-outbox depth in `sync_status` for the pending indicator.
pub fn imap_outbox_count(conn: &Connection, mailbox_id: u64) -> StoreResult<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM imap_outbox WHERE mailbox_id = ?1",
        rusqlite::params![mailbox_id],
        |row| row.get(0),
    )
    .map_err(StoreError::from)
}

// ── optimistic delete/move hidden state ────────────────────────────

/// Set or clear the `pending_delete` hidden flag for a message row.
///
/// Delete/move sets it (row stays for undo, filtered from list/search);
/// undo clears it; ack leaves it set until the sweep's expunge-diff removes
/// the row. Missing rows are a no-op (expunged between tap and write).
pub fn set_pending_delete(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    pending: bool,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE messages SET pending_delete = ?1 WHERE mailbox_id = ?2 AND uid = ?3",
        rusqlite::params![pending, mailbox_id, uid],
    )?;
    Ok(())
}

/// `true` when the row carries the optimistic hidden flag (or the row is
/// gone — callers treat missing as not-pending).
pub fn is_pending_delete(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<bool> {
    match conn.query_row(
        "SELECT pending_delete FROM messages WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, bool>(0),
    ) {
        Ok(pending) => Ok(pending),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(StoreError::Sql(e)),
    }
}

/// Undo a still-queued delete/move: clear the hidden flag and drop the
/// outbox row. Valid only while the op is still queued (until next sync);
/// returns `true` when an op was pending and the row was restored.
pub fn undo_pending_op(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
) -> StoreResult<bool> {
    let had_op: bool = conn.query_row(
        "SELECT COUNT(*) FROM imap_outbox WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, i64>(0),
    )? > 0;
    if !had_op {
        return Ok(false);
    }
    set_pending_delete(conn, mailbox_id, uid, false)?;
    delete_imap_outbox_op(conn, mailbox_id, uid)?;
    Ok(true)
}

/// Delete local rows for exactly `uids` (confirmed expunge path).
///
/// Removes dependent `attachment_parts`/`message_bodies` rows explicitly —
/// `PRAGMA foreign_keys` is off by default in this codebase, so FK cascade
/// cannot be relied on — and drops any queued flag/imap ops for the UIDs.
/// The FTS `msg_ad` trigger cleans the index automatically. Returns the
/// UIDs actually removed (for attachment-dir cleanup by the caller).
pub fn expunge_uids_local(
    conn: &Connection,
    mailbox_id: u64,
    uids: &[u32],
) -> StoreResult<Vec<u32>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = "?,".repeat(uids.len());
    let in_clause = &placeholders[..placeholders.len() - 1];
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(uids.len() + 1);
    params.push(&mailbox_id);
    for uid in uids {
        params.push(uid);
    }
    let existing: Vec<u32> = {
        let sql = format!(
            "SELECT uid FROM messages WHERE mailbox_id = ?1 AND uid IN ({in_clause})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params.as_slice(), |row| row.get::<_, u32>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if existing.is_empty() {
        return Ok(Vec::new());
    }
    // Rebuild the IN clause from the rows actually present (placeholder
    // count must match the bound params exactly).
    let placeholders = "?,".repeat(existing.len());
    let in_clause = &placeholders[..placeholders.len() - 1];
    let mut rm_params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(existing.len() + 1);
    rm_params.push(&mailbox_id);
    for uid in &existing {
        rm_params.push(uid);
    }
    let rm_sql = |table: &str, id_col: &str| {
        format!(
            "DELETE FROM {table} WHERE {id_col} IN \
             (SELECT m.id FROM messages m WHERE m.mailbox_id = ?1 AND m.uid IN ({in_clause}))"
        )
    };
    conn.execute(&rm_sql("attachment_parts", "message_id"), rm_params.as_slice())?;
    conn.execute(&rm_sql("message_bodies", "message_id"), rm_params.as_slice())?;
    let del_sql = format!(
        "DELETE FROM messages WHERE mailbox_id = ?1 AND uid IN ({in_clause})"
    );
    conn.execute(&del_sql, rm_params.as_slice())?;
    let op_sql = format!(
        "DELETE FROM flag_outbox WHERE mailbox_id = ?1 AND uid IN ({in_clause})"
    );
    conn.execute(&op_sql, rm_params.as_slice())?;
    let imap_sql = format!(
        "DELETE FROM imap_outbox WHERE mailbox_id = ?1 AND uid IN ({in_clause})"
    );
    conn.execute(&imap_sql, rm_params.as_slice())?;
    Ok(existing)
}

// ── local Seen write (optimistic UI) ───────────────────────────────

/// Apply the target Seen state to the local `messages.flags` JSON.
///
/// Optimistic-write half of `set_seen`: the row keeps every other flag
/// and only the `\Seen` membership changes. Missing rows are a no-op
/// (the message may have been expunged between tap and write).
pub fn set_local_seen(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    seen: bool,
) -> StoreResult<()> {
    let current: Option<String> = match conn.query_row(
        "SELECT flags FROM messages WHERE mailbox_id = ?1 AND uid = ?2",
        rusqlite::params![mailbox_id, uid],
        |row| row.get::<_, String>(0),
    ) {
        Ok(flags) => Some(flags),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(StoreError::Sql(e)),
    };
    if let Some(flags) = current {
        let next = set_seen_flag(&flags, seen);
        conn.execute(
            "UPDATE messages SET flags = ?1 WHERE mailbox_id = ?2 AND uid = ?3",
            rusqlite::params![next, mailbox_id, uid],
        )?;
    }
    Ok(())
}

// ── drafts (local-first compose sessions, Phase 12) ─────────────────

/// One compose session row: the editor backing store (DRAFT-01).
///
/// `server_uid` is the last APPENDed copy's UID (`None` = never
/// persisted); `dirty` marks rows needing a server write (offline queue).
/// Serialized over IPC by `get_draft` (Phase 13 DRAFT-03 consumes
/// `server_uid` + `dirty` for the send transaction).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftRow {
    pub id: String,
    pub mailbox_id: u64,
    pub message_id: String,
    pub subject: String,
    pub body: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub dirty: bool,
    pub server_uid: Option<u32>,
    pub attachments: String,
    pub updated_at: String,
}

fn draft_from_row(row: &Row<'_>) -> Result<DraftRow, rusqlite::Error> {
    Ok(DraftRow {
        id: row.get::<_, String>(0)?,
        mailbox_id: row.get::<_, u64>(1)?,
        message_id: row.get::<_, String>(2)?,
        subject: row.get::<_, String>(3)?,
        body: row.get::<_, String>(4)?,
        to: row.get::<_, String>(5)?,
        cc: row.get::<_, String>(6)?,
        bcc: row.get::<_, String>(7)?,
        dirty: row.get::<_, bool>(8)?,
        server_uid: row.get::<_, Option<u32>>(9)?,
        attachments: row.get::<_, String>(10)?,
        updated_at: row.get::<_, String>(11)?,
    })
}

const DRAFT_COLUMNS: &str = "id, mailbox_id, message_id, subject, body, \
     recipients_to, recipients_cc, recipients_bcc, dirty, server_uid, \
     attachments, updated_at";

/// Insert or replace a compose session, marking it dirty (`dirty = 1`).
///
/// Local-first: called BEFORE any network attempt, so the row survives
/// offline saves. `message_id` is stable per session (generated once at
/// row creation) — never regenerated on re-save, or the SEARCH-reconcile
/// would orphan the previous server copy (T-12-03).
#[allow(clippy::too_many_arguments)]
pub fn upsert_draft(
    conn: &Connection,
    id: &str,
    mailbox_id: u64,
    message_id: &str,
    subject: &str,
    body: &str,
    to: &str,
    cc: &str,
    bcc: &str,
) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO drafts (id, mailbox_id, message_id, subject, body, \
          recipients_to, recipients_cc, recipients_bcc, dirty, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
           mailbox_id    = excluded.mailbox_id, \
           subject       = excluded.subject, \
           body          = excluded.body, \
           recipients_to = excluded.recipients_to, \
           recipients_cc = excluded.recipients_cc, \
           recipients_bcc = excluded.recipients_bcc, \
           dirty         = 1, \
           updated_at    = datetime('now')",
        rusqlite::params![id, mailbox_id, message_id, subject, body, to, cc, bcc],
    )?;
    Ok(())
}

/// Load one compose session by id. `None` when the row is missing (the
/// command layer falls back to the rare server copy in that case).
pub fn get_draft(conn: &Connection, id: &str) -> StoreResult<Option<DraftRow>> {
    let sql = format!("SELECT {DRAFT_COLUMNS} FROM drafts WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let row = stmt
        .query_row(rusqlite::params![id], draft_from_row)
        .optional()?;
    Ok(row)
}

/// Delete a compose session row (discard path). Returns rows deleted.
pub fn delete_draft(conn: &Connection, id: &str) -> StoreResult<usize> {
    conn.execute("DELETE FROM drafts WHERE id = ?1", rusqlite::params![id])
        .map_err(StoreError::from)
}

/// All dirty rows in creation order (the reconnect flush queue).
pub fn list_dirty_drafts(conn: &Connection) -> StoreResult<Vec<DraftRow>> {
    let sql = format!("SELECT {DRAFT_COLUMNS} FROM drafts WHERE dirty = 1 ORDER BY updated_at, id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], draft_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Mark a session clean after its server copy is confirmed: `dirty = 0`,
/// `server_uid` updated to the reconciled UID.
pub fn mark_draft_clean(
    conn: &Connection,
    id: &str,
    server_uid: u32,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE drafts SET dirty = 0, server_uid = ?1 WHERE id = ?2",
        rusqlite::params![server_uid, id],
    )?;
    Ok(())
}

/// Depth of the dirty-draft queue (added to `sync_status.pending_count`).
pub fn dirty_draft_count(conn: &Connection) -> StoreResult<i64> {
    conn.query_row("SELECT COUNT(*) FROM drafts WHERE dirty = 1", [], |row| {
        row.get(0)
    })
    .map_err(StoreError::from)
}

/// Depth of the dirty-draft queue for one mailbox (per-folder pending).
pub fn dirty_draft_count_for(conn: &Connection, mailbox_id: u64) -> StoreResult<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM drafts WHERE dirty = 1 AND mailbox_id = ?1",
        rusqlite::params![mailbox_id],
        |row| row.get(0),
    )
    .map_err(StoreError::from)
}

// ── send queue (durable outbox, Phase 13) ──────────────────────────

/// One queued outgoing mail: the exactly-once foundation (SEND-04).
///
/// `message_id` is assigned once at enqueue and is `UNIQUE` — double-invoke
/// dedupes on it, retries resend the identical `.eml` bytes. `state` is one
/// of `queued|sending|sent|failed|uncertain`; `failed` is terminal-with-
/// manual-retry, `uncertain` means reconcile-not-resend (never blind retry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SendRow {
    pub id: String,
    pub message_id: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub bcc_addrs: String,
    pub eml_path: String,
    pub state: String,
    pub attempts: i64,
    pub next_retry_at: Option<String>,
    pub last_error: Option<String>,
    pub draft_id: Option<String>,
    pub created_at: String,
}

fn send_from_row(row: &Row<'_>) -> Result<SendRow, rusqlite::Error> {
    Ok(SendRow {
        id: row.get::<_, String>(0)?,
        message_id: row.get::<_, String>(1)?,
        from_addr: row.get::<_, String>(2)?,
        to_addrs: row.get::<_, String>(3)?,
        cc_addrs: row.get::<_, String>(4)?,
        bcc_addrs: row.get::<_, String>(5)?,
        eml_path: row.get::<_, String>(6)?,
        state: row.get::<_, String>(7)?,
        attempts: row.get::<_, i64>(8)?,
        next_retry_at: row.get::<_, Option<String>>(9)?,
        last_error: row.get::<_, Option<String>>(10)?,
        draft_id: row.get::<_, Option<String>>(11)?,
        created_at: row.get::<_, String>(12)?,
    })
}

const SEND_COLUMNS: &str = "id, message_id, from_addr, to_addrs, cc_addrs, \
     bcc_addrs, eml_path, state, attempts, next_retry_at, last_error, \
     draft_id, created_at";

/// Queue states stored in `send_queue.state`.
pub const SEND_STATE_QUEUED: &str = "queued";
pub const SEND_STATE_SENDING: &str = "sending";
pub const SEND_STATE_SENT: &str = "sent";
pub const SEND_STATE_FAILED: &str = "failed";
pub const SEND_STATE_UNCERTAIN: &str = "uncertain";

/// Insert one send row. Dedupe happens in `send_queue.rs` (check
/// [`get_send_row_by_message_id`] first): a concurrent double-insert hits
/// the `message_id` UNIQUE constraint and the caller reconciles to the
/// existing row instead of failing the enqueue.
#[allow(clippy::too_many_arguments)]
pub fn enqueue_send_row(
    conn: &Connection,
    id: &str,
    message_id: &str,
    from_addr: &str,
    to_addrs: &str,
    cc_addrs: &str,
    bcc_addrs: &str,
    eml_path: &str,
    draft_id: Option<&str>,
) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO send_queue (id, message_id, from_addr, to_addrs, cc_addrs, \
           bcc_addrs, eml_path, state, draft_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8)",
        rusqlite::params![
            id, message_id, from_addr, to_addrs, cc_addrs, bcc_addrs, eml_path, draft_id
        ],
    )?;
    Ok(())
}

/// Load one queued mail by queue id. `None` when the id is unknown (the
/// command layer maps this to `send-missing`).
pub fn get_send_row(conn: &Connection, id: &str) -> StoreResult<Option<SendRow>> {
    let sql = format!("SELECT {SEND_COLUMNS} FROM send_queue WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let row = stmt
        .query_row(rusqlite::params![id], send_from_row)
        .optional()?;
    Ok(row)
}

/// Load one queued mail by Message-ID — the double-invoke dedupe key.
pub fn get_send_row_by_message_id(
    conn: &Connection,
    message_id: &str,
) -> StoreResult<Option<SendRow>> {
    let sql = format!("SELECT {SEND_COLUMNS} FROM send_queue WHERE message_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let row = stmt
        .query_row(rusqlite::params![message_id], send_from_row)
        .optional()?;
    Ok(row)
}

/// Mails due for a flush pass: `queued` with no schedule or a schedule at
/// or before `now` (`%Y-%m-%d %H:%M:%S` UTC, same shape as `datetime('now')`
/// so lexicographic comparison is chronological). Oldest first.
pub fn list_due_sends(conn: &Connection, now: &str) -> StoreResult<Vec<SendRow>> {
    let sql = format!(
        "SELECT {SEND_COLUMNS} FROM send_queue \
         WHERE state = 'queued' AND (next_retry_at IS NULL OR next_retry_at <= ?1) \
         ORDER BY created_at, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params![now], send_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Transition a row to a new state (`queued|sending|sent|failed|uncertain`).
/// The CHECK constraint rejects anything else.
pub fn set_send_state(conn: &Connection, id: &str, state: &str) -> StoreResult<()> {
    conn.execute(
        "UPDATE send_queue SET state = ?1 WHERE id = ?2",
        rusqlite::params![state, id],
    )?;
    Ok(())
}

/// Record one failed attempt: `attempts + 1` with the next schedule and the
/// plain-language error. `next_retry_at = None` leaves the row unscheduled
/// (used when parking as terminal `failed` — never auto-retried).
pub fn record_send_attempt(
    conn: &Connection,
    id: &str,
    next_retry_at: Option<&str>,
    last_error: Option<&str>,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE send_queue \
         SET attempts = attempts + 1, next_retry_at = ?1, last_error = ?2 \
         WHERE id = ?3",
        rusqlite::params![next_retry_at, last_error, id],
    )?;
    Ok(())
}

/// Crash recovery: flip every `sending` row back to `queued` before any
/// flush pass runs (a crash mid-send must re-send, never strand). `sent` /
/// `failed` / `uncertain` are untouched — they already reached a verdict.
/// Returns the number of rows flipped.
pub fn reset_sending_to_queued(conn: &Connection) -> StoreResult<usize> {
    conn.execute(
        "UPDATE send_queue SET state = 'queued' WHERE state = 'sending'",
        [],
    )
    .map_err(StoreError::from)
}

/// Per-state depths for `send_status` (`queued, sending, sent, failed,
/// uncertain`) in a single scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SendCounts {
    pub queued: i64,
    pub sending: i64,
    pub sent: i64,
    pub failed: i64,
    pub uncertain: i64,
}

impl SendCounts {
    /// Mails still needing transport (the outbox badge number).
    pub fn pending(&self) -> i64 {
        self.queued + self.sending
    }
}

/// Count rows per state for the outbox badge + failed-retry surface.
pub fn send_state_counts(conn: &Connection) -> StoreResult<SendCounts> {
    let row = conn.query_row(
        "SELECT \
           SUM(CASE WHEN state = 'queued' THEN 1 ELSE 0 END), \
           SUM(CASE WHEN state = 'sending' THEN 1 ELSE 0 END), \
           SUM(CASE WHEN state = 'sent' THEN 1 ELSE 0 END), \
           SUM(CASE WHEN state = 'failed' THEN 1 ELSE 0 END), \
           SUM(CASE WHEN state = 'uncertain' THEN 1 ELSE 0 END) \
         FROM send_queue",
        [],
        |r| {
            Ok(SendCounts {
                queued: r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                sending: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                sent: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                failed: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                uncertain: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        },
    )?;
    Ok(row)
}

/// Ids currently `sending` — flush-entry triage (Plan 13-02): rows stranded
/// by a crash between the SMTP accept and the local verdict. The flush pass
/// resets them via [`reset_sending_to_queued`] and immediately parks them as
/// `uncertain` (reconcile-not-resend) instead of blindly re-sending.
pub fn list_sending_send_ids(conn: &Connection) -> StoreResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM send_queue WHERE state = 'sending'")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Rows awaiting Sent reconcile (`uncertain` verdicts): Sent SEARCH by
/// Message-ID decides sent vs. single re-send. Oldest first.
pub fn list_uncertain_sends(conn: &Connection) -> StoreResult<Vec<SendRow>> {
    let sql = format!(
        "SELECT {SEND_COLUMNS} FROM send_queue \
         WHERE state = 'uncertain' ORDER BY created_at, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], send_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Park a row as `uncertain` with the reconcile reason. Attempts are
/// untouched — the requeue after a reconcile miss counts the attempt, so a
/// verdict here must not double-count.
pub fn mark_send_uncertain(conn: &Connection, id: &str, reason: &str) -> StoreResult<()> {
    conn.execute(
        "UPDATE send_queue SET state = 'uncertain', last_error = ?1 WHERE id = ?2",
        rusqlite::params![reason, id],
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
        let results = fts_search(conn, Some("INBOX"), "alice").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].from_addr, "alice@example.com");
    }

    /// Global search (`mailbox = None`) spans all folders; every row
    /// carries its folder so results can jump to it.
    #[test]
    fn fts_search_none_scopes_whole_account() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        let inbox = ensure_mailbox(conn, "INBOX").unwrap();
        let sent = ensure_mailbox(conn, "Sent").unwrap();
        insert_msg(conn, inbox, 1, "Boletim mensal", "escola@example.com", "[]");
        insert_msg(conn, sent, 2, "Boletim resposta", "eu@example.com", "[]");

        // Scoped search: one folder only.
        let scoped = fts_search(conn, Some("INBOX"), "Boletim").unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].mailbox, "INBOX");

        // Global search: both folders, folder attributed per row.
        let mut all = fts_search(conn, None, "Boletim").unwrap();
        assert_eq!(all.len(), 2);
        all.sort_by(|a, b| a.mailbox.cmp(&b.mailbox));
        assert_eq!(all[0].mailbox, "INBOX");
        assert_eq!(all[1].mailbox, "Sent");
    }

    /// Delimiter persistence + display-name decoding for the folder tree.
    #[test]
    fn mailbox_tree_fields_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();

        ensure_mailbox(conn, "Orienta&AOcA9Q-es").unwrap();
        set_mailbox_delimiter(conn, "Orienta&AOcA9Q-es", "/").unwrap();
        set_mailbox_delimiter(conn, "INBOX", "/").unwrap();

        let rows = list_mailboxes(conn).unwrap();
        let folder = rows
            .iter()
            .find(|r| r.name == "Orienta&AOcA9Q-es")
            .expect("folder cached");
        assert_eq!(folder.display_name, "Orientações");
        assert_eq!(folder.delimiter, "/");
        let inbox = rows.iter().find(|r| r.name == "INBOX").expect("inbox");
        assert_eq!(inbox.display_name, "INBOX");
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
        let stale = fts_search(conn, Some("INBOX"), "StaleSubject").unwrap();
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
        let results = fts_search(conn, Some("INBOX"), "user42").unwrap();
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

    // ── Phase 6: flag outbox (latest-wins, pending set, delete, drop) ──

    #[test]
    fn outbox_enqueue_latest_wins() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        enqueue_outbox(conn, mb_id, 7, true, 100).unwrap();
        // Rapid flap: unread then read again — latest toggle wins, one row.
        enqueue_outbox(conn, mb_id, 7, false, 100).unwrap();
        enqueue_outbox(conn, mb_id, 7, true, 100).unwrap();

        let ops = list_outbox(conn, mb_id).unwrap();
        assert_eq!(ops.len(), 1, "flapping must collapse to a single op");
        assert_eq!(ops[0].uid, 7);
        assert!(ops[0].seen, "latest toggle (seen=true) must win");
    }

    #[test]
    fn outbox_pending_set_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        assert!(pending_uids(conn, mb_id).unwrap().is_empty());
        enqueue_outbox(conn, mb_id, 3, true, 100).unwrap();
        enqueue_outbox(conn, mb_id, 9, false, 100).unwrap();

        let mut pending = pending_uids(conn, mb_id).unwrap();
        pending.sort_unstable();
        assert_eq!(pending, vec![3, 9]);

        // Other mailboxes are isolated.
        let other = ensure_mailbox(conn, "Sent").unwrap();
        assert!(pending_uids(conn, other).unwrap().is_empty());
    }

    #[test]
    fn outbox_single_op_delete() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        enqueue_outbox(conn, mb_id, 3, true, 100).unwrap();
        enqueue_outbox(conn, mb_id, 9, false, 100).unwrap();

        // Ack UID 3 → only UID 9 remains.
        let deleted = delete_outbox_op(conn, mb_id, 3).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(pending_uids(conn, mb_id).unwrap(), vec![9]);

        // Deleting a UID with no op is a no-op success.
        let deleted = delete_outbox_op(conn, mb_id, 3).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn outbox_mailbox_drop() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        enqueue_outbox(conn, mb_id, 1, true, 100).unwrap();
        enqueue_outbox(conn, mb_id, 2, false, 100).unwrap();
        assert_eq!(outbox_count(conn, mb_id).unwrap(), 2);

        // UIDVALIDITY bump → whole mailbox queue drops (RFC 4549).
        let dropped = drop_outbox_for_mailbox(conn, mb_id).unwrap();
        assert_eq!(dropped, 2);
        assert!(pending_uids(conn, mb_id).unwrap().is_empty());
        assert_eq!(outbox_count(conn, mb_id).unwrap(), 0);
    }

    #[test]
    fn empty_prior_flags_toggle_applies_target_state() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        insert_msg(conn, mb_id, 1, "NoFlags", "a@x.com", "[]");
        insert_msg(conn, mb_id, 2, "SeenAlready", "b@x.com", r#"["\\Seen"]"#);

        // Mark read from empty flags → \Seen appears in canonical form.
        set_local_seen(conn, mb_id, 1, true).unwrap();
        let row = get_message_by_uid(conn, "INBOX", 1).unwrap().unwrap();
        assert_eq!(parse_flags(&row.flags), vec!["\\Seen".to_string()]);
        assert!(!is_unread(&row.flags));

        // Mark unread from empty flags → stays empty, still unread.
        set_local_seen(conn, mb_id, 1, false).unwrap();
        let row = get_message_by_uid(conn, "INBOX", 1).unwrap().unwrap();
        assert_eq!(parse_flags(&row.flags), Vec::<String>::new());
        assert!(is_unread(&row.flags));

        // Mark unread on a Seen row → \Seen removed, other flags kept.
        insert_msg(
            conn,
            mb_id,
            3,
            "Flagged",
            "c@x.com",
            r#"["\\Flagged","\\Seen"]"#,
        );
        set_local_seen(conn, mb_id, 3, false).unwrap();
        let row = get_message_by_uid(conn, "INBOX", 3).unwrap().unwrap();
        assert_eq!(parse_flags(&row.flags), vec!["\\Flagged".to_string()]);
        assert!(is_unread(&row.flags));

        // Toggling a UID with no local row is a no-op, not an error.
        set_local_seen(conn, mb_id, 999, true).unwrap();

        // Pure helper round-trips.
        assert_eq!(set_seen_flag("not-json{{{", true), r#"["\\Seen"]"#);
        assert_eq!(set_seen_flag("[]", false), "[]");
    }

    // ── Phase 10 Plan 10-03 Wave 1: imap_outbox + pending_delete ──

    #[test]
    fn imap_outbox_enqueue_latest_wins() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        enqueue_imap_outbox(conn, mb_id, 7, IMAP_OP_DELETE, Some("Trash"), 100).unwrap();
        // Re-enqueue as move: latest op wins, one row, counters reset.
        record_imap_outbox_error(conn, mb_id, 7, "boom").unwrap();
        enqueue_imap_outbox(conn, mb_id, 7, IMAP_OP_MOVE, Some("Archive"), 100).unwrap();

        let ops = list_imap_outbox(conn, mb_id).unwrap();
        assert_eq!(ops.len(), 1, "re-enqueue must collapse to a single op");
        assert_eq!(ops[0].uid, 7);
        assert_eq!(ops[0].op, IMAP_OP_MOVE);
        assert_eq!(ops[0].dest_mailbox.as_deref(), Some("Archive"));
        assert_eq!(ops[0].attempts, 0, "re-enqueue resets retry counters");
        assert_eq!(ops[0].last_error, None);
        assert_eq!(imap_outbox_count(conn, mb_id).unwrap(), 1);
        assert_eq!(pending_imap_uids(conn, mb_id).unwrap(), vec![7]);
    }

    #[test]
    fn imap_enqueue_consumes_flag_op_capturing_seen_for_move() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        // Queued Seen toggle for uid 5 (read), plain message for uid 6.
        enqueue_outbox(conn, mb_id, 5, true, 100).unwrap();
        enqueue_outbox(conn, mb_id, 6, false, 100).unwrap();

        // Move uid 5: flag row consumed, Seen intent captured.
        enqueue_imap_outbox(conn, mb_id, 5, IMAP_OP_MOVE, Some("Archive"), 100).unwrap();
        assert!(
            pending_uids(conn, mb_id).unwrap().contains(&6)
                && !pending_uids(conn, mb_id).unwrap().contains(&5),
            "same-key flag op must be gone after move enqueue"
        );
        let ops = list_imap_outbox(conn, mb_id).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].seen_intent, Some(true));

        // Delete uid 6: flag row consumed, no Seen intent stored
        // (MOVE/COPY preserve flags server-side).
        enqueue_imap_outbox(conn, mb_id, 6, IMAP_OP_DELETE, Some("Trash"), 100).unwrap();
        assert!(pending_uids(conn, mb_id).unwrap().is_empty());
        let ops = list_imap_outbox(conn, mb_id).unwrap();
        let del = ops.iter().find(|o| o.uid == 6).unwrap();
        assert_eq!(del.seen_intent, None);
    }

    #[test]
    fn imap_outbox_single_op_delete_and_mailbox_drop() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();

        enqueue_imap_outbox(conn, mb_id, 3, IMAP_OP_DELETE, Some("Trash"), 100).unwrap();
        enqueue_imap_outbox(conn, mb_id, 9, IMAP_OP_MOVE, Some("Archive"), 100).unwrap();

        let deleted = delete_imap_outbox_op(conn, mb_id, 3).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(pending_imap_uids(conn, mb_id).unwrap(), vec![9]);
        assert_eq!(delete_imap_outbox_op(conn, mb_id, 3).unwrap(), 0);

        // UIDVALIDITY bump → whole mailbox queue drops (RFC 4549).
        let dropped = drop_imap_outbox_for_mailbox(conn, mb_id).unwrap();
        assert_eq!(dropped, 1);
        assert_eq!(imap_outbox_count(conn, mb_id).unwrap(), 0);
    }

    #[test]
    fn pending_delete_hidden_from_list_and_fts_until_undo() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        insert_msg(conn, mb_id, 1, "Visible", "a@x.com", "[]");
        insert_msg(conn, mb_id, 2, "Hidden", "b@x.com", "[]");

        set_pending_delete(conn, mb_id, 2, true).unwrap();
        assert!(is_pending_delete(conn, mb_id, 2).unwrap());
        assert!(!is_pending_delete(conn, mb_id, 1).unwrap());

        let rows = list_messages(conn, "INBOX", 100, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].uid, 1);
        let hits = fts_search(conn, Some("INBOX"), "Hidden").unwrap();
        assert!(hits.is_empty(), "pending rows must not surface in FTS");

        // Ack path leaves the flag set (sweep expunge-diff removes the row).
        enqueue_imap_outbox(conn, mb_id, 2, IMAP_OP_DELETE, Some("Trash"), 100).unwrap();
        // Undo while still queued restores the row to list + FTS.
        assert!(undo_pending_op(conn, mb_id, 2).unwrap());
        assert!(!is_pending_delete(conn, mb_id, 2).unwrap());
        assert!(pending_imap_uids(conn, mb_id).unwrap().is_empty());
        let rows = list_messages(conn, "INBOX", 100, 0).unwrap();
        assert_eq!(rows.len(), 2);
        let hits = fts_search(conn, Some("INBOX"), "Hidden").unwrap();
        assert_eq!(hits.len(), 1);

        // Undo with nothing queued is a no-op false.
        assert!(!undo_pending_op(conn, mb_id, 1).unwrap());
        // Missing rows are a no-op, not an error.
        set_pending_delete(conn, mb_id, 999, true).unwrap();
        assert!(!is_pending_delete(conn, mb_id, 999).unwrap());
    }

    #[test]
    fn expunge_removes_fts_bodies_and_parts() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        insert_msg(conn, mb_id, 1, "GoneSoon", "a@x.com", "[]");
        insert_msg(conn, mb_id, 2, "StaysHere", "b@x.com", "[]");
        let msg_id: u64 = conn
            .query_row(
                "SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = 1",
                rusqlite::params![mb_id],
                |r| r.get(0),
            )
            .unwrap();
        insert_body(conn, msg_id, Some("text"), Some("<p>html</p>")).unwrap();
        insert_attachment_meta(conn, msg_id, "2", "f.pdf", "application/pdf", 10).unwrap();
        enqueue_outbox(conn, mb_id, 1, true, 100).unwrap();
        enqueue_imap_outbox(conn, mb_id, 1, IMAP_OP_DELETE, Some("Trash"), 100).unwrap();

        let removed = expunge_uids_local(conn, mb_id, &[1, 999]).unwrap();
        assert_eq!(removed, vec![1], "only present UIDs report removed");
        assert!(fts_search(conn, Some("INBOX"), "GoneSoon").unwrap().is_empty());
        let bodies: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_bodies WHERE message_id = ?1",
                rusqlite::params![msg_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bodies, 0, "explicit body delete (FK pragma is off)");
        let parts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attachment_parts WHERE message_id = ?1",
                rusqlite::params![msg_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parts, 0, "explicit parts delete (FK pragma is off)");
        // Queued ops for the expunged UID drop with it.
        assert!(pending_imap_uids(conn, mb_id).unwrap().is_empty());
        // Survivor untouched.
        assert_eq!(fts_search(conn, Some("INBOX"), "StaysHere").unwrap().len(), 1);
        assert!(expunge_uids_local(conn, mb_id, &[]).unwrap().is_empty());
    }

    #[test]
    fn expunge_diff_wipe_clears_dependents_and_imap_queue() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "INBOX").unwrap();
        insert_msg(conn, mb_id, 1, "Old", "a@x.com", "[]");
        let msg_id: u64 = conn
            .query_row(
                "SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = 1",
                rusqlite::params![mb_id],
                |r| r.get(0),
            )
            .unwrap();
        insert_body(conn, msg_id, Some("t"), None).unwrap();
        insert_attachment_meta(conn, msg_id, "2", "f.pdf", "application/pdf", 5).unwrap();
        enqueue_imap_outbox(conn, mb_id, 1, IMAP_OP_MOVE, Some("Archive"), 100).unwrap();

        // Wipe path (UIDVALIDITY bump): rows + dependents + imap queue go.
        delete_missing_uids(conn, mb_id, &[]).unwrap();
        for table in ["messages", "message_bodies", "attachment_parts", "imap_outbox"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} must be empty after wipe");
        }
        assert!(fts_search(conn, Some("INBOX"), "Old").unwrap().is_empty());
    }

    // ── Plan 11-02: folder cache rename / delete ────────────────────

    #[test]
    fn rename_mailbox_cache_moves_subtree_preserving_rows() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let pai = ensure_mailbox(conn, "Pai").unwrap();
        set_mailbox_delimiter(conn, "Pai", "/").unwrap();
        let sub = ensure_mailbox(conn, "Pai/Sub").unwrap();
        set_mailbox_delimiter(conn, "Pai/Sub", "/").unwrap();
        let pai2 = ensure_mailbox(conn, "Pai2").unwrap();
        insert_msg(conn, pai, 1, "Old", "a@x.com", "[]");
        insert_msg(conn, sub, 2, "Kid", "b@x.com", "[]");
        set_sync_state(conn, "Pai", 100, 4).unwrap();

        let touched = rename_mailbox_cache(conn, "Pai", "Novo", "/").unwrap();
        assert_eq!(touched, 2, "exact row + one child, Pai2 untouched");

        // Exact `Pai2` never matched the prefix (LIKE-escape, T-11-05).
        let names: Vec<String> = conn
            .prepare("SELECT name FROM mailboxes ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(names.contains(&"Novo".to_string()));
        assert!(names.contains(&"Novo/Sub".to_string()));
        assert!(names.contains(&"Pai2".to_string()));
        assert!(!names.iter().any(|n| n.starts_with("Pai/")));

        // id / uid_validity / messages untouched (UIDs preserved, no refetch).
        let novo_id: u64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'Novo'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(novo_id, pai);
        let validity: u32 = conn
            .query_row("SELECT uid_validity FROM mailboxes WHERE name = 'Novo'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(validity, 100);
        assert_eq!(count_messages(conn, novo_id), 1);
        let sub_id: u64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'Novo/Sub'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sub_id, sub);
        assert_eq!(count_messages(conn, sub_id), 1);
        assert_eq!(count_messages(conn, pai2), 0);
    }

    #[test]
    fn rename_mailbox_cache_prefix_match_is_case_sensitive() {
        // SQLite LIKE is ASCII case-insensitive, folder names are not:
        // renaming `Pai` must leave a case-differing `PAI/X` subtree alone.
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        ensure_mailbox(conn, "Pai").unwrap();
        set_mailbox_delimiter(conn, "Pai", "/").unwrap();
        ensure_mailbox(conn, "Pai/Sub").unwrap();
        set_mailbox_delimiter(conn, "Pai/Sub", "/").unwrap();
        ensure_mailbox(conn, "PAI/X").unwrap();
        set_mailbox_delimiter(conn, "PAI/X", "/").unwrap();

        let touched = rename_mailbox_cache(conn, "Pai", "Novo", "/").unwrap();
        assert_eq!(touched, 2, "exact row + one true child, PAI/X untouched");

        let names: Vec<String> = conn
            .prepare("SELECT name FROM mailboxes ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(names.contains(&"Novo/Sub".to_string()));
        assert!(names.contains(&"PAI/X".to_string()));
        assert!(!names.iter().any(|n| n.starts_with("Novo/X")));
    }

    #[test]
    fn delete_mailbox_cache_cascades_and_drops_queues() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mb_id = ensure_mailbox(conn, "Velha").unwrap();
        let sibling = ensure_mailbox(conn, "Irmã").unwrap();
        insert_msg(conn, mb_id, 1, "Old", "a@x.com", "[]");
        enqueue_outbox(conn, mb_id, 1, true, 100).unwrap();
        enqueue_imap_outbox(conn, mb_id, 2, IMAP_OP_DELETE, Some("Trash"), 100).unwrap();
        insert_msg(conn, sibling, 9, "Keep", "c@x.com", "[]");

        delete_mailbox_cache(conn, "Velha").unwrap();

        // Row gone, messages cascaded, both outbox rows dropped.
        let gone: i64 = conn
            .query_row("SELECT COUNT(*) FROM mailboxes WHERE name = 'Velha'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(gone, 0);
        assert_eq!(count_messages(conn, mb_id), 0);
        assert_eq!(outbox_count(conn, mb_id).unwrap(), 0);
        assert_eq!(imap_outbox_count(conn, mb_id).unwrap(), 0);
        // Sibling folders and their messages survive.
        assert_eq!(count_messages(conn, sibling), 1);
        // No orphan LIKE rows remain under the deleted prefix.
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mailboxes WHERE name LIKE 'Velha/%' ESCAPE '\\'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn delete_mailbox_cache_missing_name_is_noop() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        // Unknown folder: no row, no id, nothing to drop — still Ok.
        delete_mailbox_cache(conn, "Nunca-Existiu").unwrap();
    }

    // ── Plan 11-03: role bookkeeping ─────────────────────────────

    #[test]
    fn set_mailbox_role_upsert_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        // Fresh M8 rows default empty (not-yet-resolved, never assumed).
        ensure_mailbox(conn, "Lixeira").unwrap();
        let rows = list_mailboxes(conn).unwrap();
        let row = rows.iter().find(|r| r.name == "Lixeira").unwrap();
        assert_eq!(row.role, "");
        assert_eq!(row.attributes, "");

        set_mailbox_role(conn, "Lixeira", "trash", "\\HasNoChildren \\Trash").unwrap();
        let rows = list_mailboxes(conn).unwrap();
        let row = rows.iter().find(|r| r.name == "Lixeira").unwrap();
        assert_eq!(row.role, "trash");
        assert_eq!(row.attributes, "\\HasNoChildren \\Trash");

        // Re-resolve overwrites (LIST is truth — T-11-07).
        set_mailbox_role(conn, "Lixeira", "custom", "\\HasNoChildren").unwrap();
        let rows = list_mailboxes(conn).unwrap();
        let row = rows.iter().find(|r| r.name == "Lixeira").unwrap();
        assert_eq!(row.role, "custom");

        // Upsert creates the row when missing (refresh path).
        set_mailbox_role(conn, "Nova", "custom", "").unwrap();
        assert!(list_mailboxes(conn).unwrap().iter().any(|r| r.name == "Nova"));
    }
}
