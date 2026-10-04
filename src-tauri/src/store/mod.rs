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
pub const SCHEMA_VERSION: u32 = 2;

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
    fn schema_version_is_2_with_outbox_table() {
        assert_eq!(SCHEMA_VERSION, 2);
        let store = Store::open_in_memory().expect("migration should succeed");
        let conn = store.conn();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name = 'flag_outbox'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(count, 1, "flag_outbox table should exist at schema v2");
        let idx: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name = 'idx_outbox_mailbox'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(idx, 1, "outbox mailbox index should exist");
    }
}
