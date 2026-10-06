//! Local SQLite store for the sync engine.
//!
//! WAL + versioned migrations, messages/bodies/attachment-metadata tables,
//! and a populated FTS5 index. This module owns the connection; all SQL
//! statements live in [`queries`] (single-writer invariant — no raw SQL
//! escapes `queries.rs`).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use rusqlite_migration::{Error as MigrationError, Migrations, M};
use thiserror::Error;

pub mod queries;
/// Per CONTEXT discretion allowance; exported for `bodies.rs`.
pub const BODY_CACHE_CAP_BYTES: usize = 262144;

/// Schema version managed by rusqlite_migration.
pub const SCHEMA_VERSION: u32 = 7;

// v1 = full schema.sql (canonical DDL from ARCHITECTURE.md)
// M2 = flag_outbox durable queue (Phase 6, Plan 06-01). The schema.sql v1
// baseline text stays untouched; forward migrations append below.
// NOTE: rusqlite_migration 2.x takes Vec<M>, which is not const-constructable,
// so we build the Migrations inline in apply_migrations().

/// Errors from the store layer.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(#[from] MigrationError),
    #[error("store path error: {0}")]
    Path(String),
}

/// Convenience alias used throughout the crate for store operations.
pub type StoreResult<T> = Result<T, StoreError>;

/// Connection owner. All SQL queries live in [`queries`].
///
/// `Store` is not `Clone` — the single-writer invariant is enforced
/// by wrapping a single connection in `Arc<Mutex<Store>>` at the
/// Tauri State layer (see `lib.rs`).
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) a SQLite database at `path` with WAL mode and
    /// migrations applied. Production path.
    pub fn open<P: AsRef<Path>>(path: P) -> StoreResult<Self> {
        let parent = path
            .as_ref()
            .parent()
            .ok_or_else(|| StoreError::Path("database path has no parent directory".into()))?;
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Path(format!("create_dir_all failed: {e}")))?;
        }

        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        Self::apply_migrations(&mut conn)?;
        Ok(Store { conn })
    }

    /// Open an in-memory SQLite database with migrations applied.
    /// Used by unit tests that don't need persistence.
    pub fn open_in_memory() -> StoreResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        Self::apply_migrations(&mut conn)?;
        Ok(Store { conn })
    }

    fn apply_migrations(conn: &mut Connection) -> StoreResult<()> {
        // WAL for concurrent reader (UI) + writer (sync worker)
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        let migrations = Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
            M::up(M7_IMAP_OUTBOX_SQL),
        ]);
        migrations.to_latest(conn)?;
        Ok(())
    }

    /// Borrow the underlying connection for query functions in `queries.rs`.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Mutable access for batch write paths that need a transaction.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::open_in_memory().expect("in-memory default store should always succeed")
    }
}

/// M2 forward migration: durable Seen-flag outbox (Phase 6, Plan 06-01).
///
/// Queues optimistic flag toggles made while offline so they replay on
/// reconnect (RFC 4549 drop rules enforced by the replay engine: whole
/// mailbox queue drops on UIDVALIDITY bump, single ops drop when the UID
/// is absent). `UNIQUE(mailbox_id, uid)` collapses rapid toggle flapping
/// to latest-wins — intermediate states are intentionally not replayed.
const M2_FLAG_OUTBOX_SQL: &str = concat!(
    "CREATE TABLE flag_outbox (",
    "  id            INTEGER PRIMARY KEY,",
    "  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,",
    "  uid           INTEGER NOT NULL,",
    "  seen          INTEGER NOT NULL,",
    "  uid_validity  INTEGER NOT NULL,",
    "  created_at    TEXT NOT NULL DEFAULT (datetime('now')),",
    "  attempts      INTEGER NOT NULL DEFAULT 0,",
    "  last_error    TEXT,",
    "  UNIQUE (mailbox_id, uid)",
    ");",
    "CREATE INDEX idx_outbox_mailbox ON flag_outbox(mailbox_id);",
);

/// M3 forward migration: STATUS UNSEEN cache per folder (Phase 7, Wave 2).
///
/// `unseen_count` holds the last `STATUS <folder> (UNSEEN)` datum written by
/// `queries::set_mailbox_status` during folder sync. The sidebar badge shows
/// the dynamic local unread count once a folder has synced (`last_sync_at`
/// present) and falls back to this cached server datum for never-synced
/// folders — so a fresh folder still shows an honest server-sourced signal
/// (FOLD-02). Defaults to 0; existing rows backfill harmlessly.
const M3_UNSEEN_COUNT_SQL: &str =
    "ALTER TABLE mailboxes ADD COLUMN unseen_count INTEGER NOT NULL DEFAULT 0;";

/// M4 forward migration: UID-backfill support (Phase 9).
///
/// `fetch_tombstones` records UIDs that repeatedly return empty FETCH
/// results although SEARCH still lists them (flaky server path): after
/// `TOMBSTONE_STRIKES` (see queries) the sweeper stops re-requesting them
/// — no infinite backfill loop — until they vanish from SEARCH (pruned) or
/// a periodic full sweep recovers them. `sweeps_since_full` counts
/// consecutive converged (sweep-skipped) passes so remote flag changes
/// still surface on a periodic full sweep.
const M4_BACKFILL_SQL: &str = concat!(
    "CREATE TABLE fetch_tombstones (",
    "  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,",
    "  uid           INTEGER NOT NULL,",
    "  strikes       INTEGER NOT NULL DEFAULT 1,",
    "  updated_at    TEXT NOT NULL DEFAULT (datetime('now')),",
    "  PRIMARY KEY (mailbox_id, uid)",
    ");",
    "CREATE INDEX idx_tombstone_mailbox ON fetch_tombstones(mailbox_id);",
    "ALTER TABLE mailboxes ADD COLUMN sweeps_since_full INTEGER NOT NULL DEFAULT 0;",
);

/// M5 forward migration: separate STATUS timestamp (Phase 7 audit fix).
///
/// `set_mailbox_status` (STATUS discovery) used to stamp `last_sync_at`,
/// which defeated the sidebar badge fallback: a discovered-but-never-
/// message-synced folder looked "synced" with local unread 0 instead of
/// showing the server UNSEEN datum. `status_synced_at` records STATUS
/// freshness; `last_sync_at` is now stamped only by message syncs
/// (`set_sync_state`), and the badge gates on it.
const M5_STATUS_TS_SQL: &str =
    "ALTER TABLE mailboxes ADD COLUMN status_synced_at TEXT;";

/// M6 forward migration: hierarchy delimiter per folder (folder tree).
///
/// `delimiter` holds the LIST hierarchy delimiter (`/`, `.`, …) so the
/// sidebar can nest subfolders offline. Written by `set_mailbox_delimiter`
/// during folder discovery; empty means flat (unknown — render top-level).
const M6_DELIMITER_SQL: &str =
    "ALTER TABLE mailboxes ADD COLUMN delimiter TEXT NOT NULL DEFAULT '';";

/// M7 forward migration: durable delete/move queue + optimistic hidden state
/// (Phase 10, Plan 10-03).
///
/// `imap_outbox` queues offline deletes/moves for pre-sweep replay with
/// RFC 4549 drop rules (whole-mailbox drop on UIDVALIDITY bump, single-op
/// drop when the UID is absent server-side). `UNIQUE(mailbox_id, uid)`
/// collapses rapid re-tries to latest-wins. `flag_outbox` (Phase 6
/// contract) is untouched — enqueueing a delete/move drops the same-key
/// flag row instead (a flag write to a soon-moved message is moot).
///
/// `messages.pending_delete` is the optimistic hidden flag: delete/move
/// sets it (row filtered from list/search, undo restores), the next sweep's
/// expunge-diff removes the row once the server confirms the move. Adding
/// a column leaves the `msg_ai`/`msg_ad` FTS triggers intact.
const M7_IMAP_OUTBOX_SQL: &str = concat!(
    "CREATE TABLE imap_outbox (",
    "  id            INTEGER PRIMARY KEY,",
    "  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,",
    "  uid           INTEGER NOT NULL,",
    "  op            TEXT NOT NULL CHECK (op IN ('delete','move')),",
    "  dest_mailbox  TEXT,",
    "  seen_intent   INTEGER,",
    "  uid_validity  INTEGER NOT NULL,",
    "  created_at    TEXT NOT NULL DEFAULT (datetime('now')),",
    "  attempts      INTEGER NOT NULL DEFAULT 0,",
    "  last_error    TEXT,",
    "  UNIQUE (mailbox_id, uid)",
    ");",
    "CREATE INDEX idx_imap_outbox_mailbox ON imap_outbox(mailbox_id);",
    "ALTER TABLE messages ADD COLUMN pending_delete INTEGER NOT NULL DEFAULT 0;",
);

/// Returns the app-data attachment directory for a given mailbox UID.
///
/// Files live under `<app_data>/attachments/<uid_validity>/<uid>/` —
/// never inside SQLite blobs (per D-attachments).
pub fn attachment_dir(app_data: &Path, uid_validity: u32, uid: u32) -> PathBuf {
    app_data
        .join("attachments")
        .join(uid_validity.to_string())
        .join(uid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_store_creates_all_tables() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let conn = store.conn();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='table' AND name IN ('mailboxes','messages','message_bodies','attachment_parts')",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(count, 4, "all base tables should exist");
    }

    #[test]
    fn in_memory_store_fts_and_triggers_exist() {
        let store = Store::open_in_memory().expect("migration should succeed");
        let conn = store.conn();

        // FTS5 virtual table
        let fts_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'messages_fts'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(fts_count, 1, "messages_fts FTS5 table should exist");

        // Triggers
        let trigger_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN ('msg_ai','msg_ad')",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(trigger_count, 2, "both FTS triggers should exist");
    }

    #[test]
    fn body_cache_cap_is_256kb() {
        assert_eq!(BODY_CACHE_CAP_BYTES, 262144);
    }

    #[test]
    fn wal_mode_is_set_on_file_database() {
        // In-memory SQLite always reports "memory" — WAL requires a file.
        let dir = std::env::temp_dir();
        let db_path = dir.join(format!("sge_wal_test_{}.db", std::process::id()));
        {
            let store = Store::open(&db_path).expect("should open");
            let journal: String = store
                .conn()
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .expect("query should succeed");
            assert_eq!(journal.to_lowercase(), "wal");
        }
        // Cleanup
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.to_str().unwrap_or_default()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.to_str().unwrap_or_default()));
    }

    #[test]
    fn attachment_dir_layout() {
        let dir = attachment_dir(std::path::Path::new("/tmp/sge"), 4321, 7);
        assert_eq!(dir, std::path::PathBuf::from("/tmp/sge/attachments/4321/7"));
    }

    #[test]
    fn schema_version_is_7_with_imap_outbox() {
        assert_eq!(SCHEMA_VERSION, 7);
        let store = Store::open_in_memory().expect("migration should succeed");
        let conn = store.conn();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'flag_outbox'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(count, 1, "flag_outbox table should exist at schema v6");
        let idx: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name = 'idx_outbox_mailbox'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(idx, 1, "outbox mailbox index should exist");
        let unseen_cols: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('mailboxes') WHERE name = 'unseen_count'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(unseen_cols, 1, "mailboxes.unseen_count should exist at schema v6");
    }

    #[test]
    fn m4_adds_tombstones_and_sweep_counter() {
        // Simulate a v3 database (schema.sql + M2 + M3, as shipped after Phase 7).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // Forward-upgrade with the production set — only M4 applies.
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        for (kind, name) in [
            ("table", "fetch_tombstones"),
            ("index", "idx_tombstone_mailbox"),
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{kind} {name} should exist at schema v6");
        }
        let cols: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('mailboxes') WHERE name = 'sweeps_since_full'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cols, 1, "mailboxes.sweeps_since_full should exist at schema v6");
    }

    #[test]
    fn m6_adds_delimiter_column() {
        // Simulate a v5 database (through M5).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('mailboxes') WHERE name = 'delimiter'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "mailboxes.delimiter should exist at schema v6");
        // Pre-M6 rows default to '' (flat — render top-level).
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next) VALUES ('INBOX', 100, 4)",
            [],
        )
        .unwrap();
        let d: String = conn
            .query_row(
                "SELECT delimiter FROM mailboxes WHERE name = 'INBOX'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(d, "");
    }

    #[test]
    fn m5_adds_status_synced_at() {
        // Simulate a v4 database (through M4, as shipped after Phase 9).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // STATUS discovery must NOT stamp last_sync_at (badge fallback).
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next) VALUES ('Sent', 10, 2)",
            [],
        )
        .unwrap();
        crate::store::queries::set_mailbox_status(&conn, "Sent", 10, 2, 4).unwrap();
        let (last, status_ts, unseen): (Option<String>, Option<String>, i64) = conn
            .query_row(
                "SELECT last_sync_at, status_synced_at, unseen_count FROM mailboxes WHERE name = 'Sent'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(last, None, "STATUS must not stamp last_sync_at");
        assert!(status_ts.is_some(), "STATUS stamps status_synced_at");
        assert_eq!(unseen, 4);
    }

    #[test]
    fn m3_adds_unseen_count_column() {
        // Simulate a v2 database (schema.sql + M2, as shipped after Phase 6).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next) VALUES ('INBOX', 100, 4)",
            [],
        )
        .unwrap();
        // Forward-upgrade the v2 database with the production migration set —
        // only the M3 delta applies (user_version 2 → 3).
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        let unseen: i64 = conn
            .query_row(
                "SELECT unseen_count FROM mailboxes WHERE name = 'INBOX'",
                [],
                |r| r.get(0),
            )
            .expect("unseen_count should default to 0 for pre-M3 rows");
        assert_eq!(unseen, 0);
    }

    #[test]
    fn m7_adds_imap_outbox_and_pending_delete_preserving_rows() {
        // Simulate a v6 database (through M6, as shipped after Plan 10-02).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // Seed rows under v6: a cached message + a queued flag toggle.
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next) VALUES ('INBOX', 100, 4)",
            [],
        )
        .unwrap();
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO messages (mailbox_id, uid, subject, from_addr, date_utc, flags, preview) \
             VALUES (?1, 1, 'Old', 'a@x.com', '2024-01-01T00:00:00Z', '[]', 'p')",
            rusqlite::params![mb],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO flag_outbox (mailbox_id, uid, seen, uid_validity) \
             VALUES (?1, 1, 1, 100)",
            rusqlite::params![mb],
        )
        .unwrap();

        // Forward-upgrade with the production set — only M7 applies.
        Store::apply_migrations(&mut conn).unwrap();

        // v6 rows survive the upgrade.
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1, "forward migration must preserve cached rows");
        let ops: i64 = conn
            .query_row("SELECT COUNT(*) FROM flag_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ops, 1, "forward migration must preserve flag_outbox rows");
        // M7 surface exists.
        for (kind, name) in [
            ("table", "imap_outbox"),
            ("index", "idx_imap_outbox_mailbox"),
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{kind} {name} should exist at schema v7");
        }
        // Pre-M7 rows read as not-pending (hidden flag defaults to 0).
        let pending: i64 = conn
            .query_row(
                "SELECT pending_delete FROM messages WHERE mailbox_id = ?1 AND uid = 1",
                rusqlite::params![mb],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
        // flag_outbox contract untouched: no op/dest columns.
        let extra: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('flag_outbox') \
                 WHERE name IN ('op', 'dest_mailbox', 'seen_intent')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(extra, 0, "flag_outbox schema must stay untouched");
    }

    #[test]
    fn m2_upgrades_v1_database_forward_preserving_rows() {
        // Simulate a v1 database (schema.sql only, as shipped before Phase 6).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![M::up(include_str!("schema.sql"))])
            .to_latest(&mut conn)
            .unwrap();
        // A message cached under v1.
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next) VALUES ('INBOX', 100, 4)",
            [],
        )
        .unwrap();
        let mb: i64 = conn
            .query_row("SELECT id FROM mailboxes WHERE name = 'INBOX'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO messages (mailbox_id, uid, subject, from_addr, date_utc, flags, preview) \
             VALUES (?1, 1, 'Old', 'a@x.com', '2024-01-01T00:00:00Z', '[]', 'p')",
            rusqlite::params![mb],
        )
        .unwrap();

        // Opening through the store applies M2 forward.
        Store::apply_migrations(&mut conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'flag_outbox'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "M2 must create flag_outbox on a v1 database");
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1, "forward migration must preserve cached rows");
    }
}
