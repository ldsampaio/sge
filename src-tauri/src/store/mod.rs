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
pub const SCHEMA_VERSION: u32 = 11;

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
            M::up(M8_ROLES_SQL),
            M::up(M9_DRAFTS_SQL),
            M::up(M10_SEND_QUEUE_SQL),
            M::up(M11_SENT_UNFILED_SQL),
            M::up(M12_CLASSIFY_SQL),
            M::up(M13_EXCLUSIONS_SQL),
            M::up(M14_REVIEW_SETTINGS_SQL),
            M::up(M15_DISMISSED_SQL),
            M::up(M16_BATCH_ITEMS_SQL),
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

/// M8 forward migration: role + attributes bookkeeping per folder
/// (Phase 11, Plan 11-03).
///
/// `role` holds the resolved folder role (`inbox|trash|sent|drafts|custom`
/// per `imap::roles::Role::as_str`); `attributes` the space-joined LIST
/// attributes for `\Noselect`/`\Noinferiors`/SPECIAL-USE checks. Both are a
/// CACHE: recomputed on every LIST refresh and persisted here, so guards
/// stay role-aware across restarts without trusting stale values (the
/// refresh always rewrites them — T-11-07). Defaults keep pre-M8 rows
/// meaningful (`custom` is never assumed — empty means "not yet resolved").
const M8_ROLES_SQL: &str = concat!(
    "ALTER TABLE mailboxes ADD COLUMN role TEXT NOT NULL DEFAULT '';",
    "ALTER TABLE mailboxes ADD COLUMN attributes TEXT NOT NULL DEFAULT '';",
);

/// M9 forward migration: local-first drafts backing store
/// (Phase 12, Plan 12-01).
///
/// One row per compose session (`id` = UI uuid). `message_id` is stable
/// per session and is the `UID SEARCH HEADER Message-ID` reconcile key
/// (async-imap 0.11 swallows APPENDUID, so UID discovery always goes
/// through SEARCH — see RESEARCH §1). `server_uid` is the last APPENDed
/// copy's UID (`NULL` = never APPENDed); `dirty = 1` rows ARE the
/// offline queue (no separate outbox — the reconnect pass flushes them).
/// `attachments` holds staging refs only (picker UI is Phase 14).
const M9_DRAFTS_SQL: &str = concat!(
    "CREATE TABLE drafts (",
    "  id            TEXT PRIMARY KEY,",
    "  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,",
    "  message_id    TEXT NOT NULL UNIQUE,",
    "  subject       TEXT NOT NULL DEFAULT '',",
    "  body          TEXT NOT NULL DEFAULT '',",
    "  recipients_to TEXT NOT NULL DEFAULT '',",
    "  recipients_cc TEXT NOT NULL DEFAULT '',",
    "  recipients_bcc TEXT NOT NULL DEFAULT '',",
    "  dirty         INTEGER NOT NULL DEFAULT 1,",
    "  server_uid    INTEGER,",
    "  attachments   TEXT NOT NULL DEFAULT '[]',",
    "  updated_at    TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "CREATE INDEX idx_drafts_mailbox ON drafts(mailbox_id);",
    "CREATE INDEX idx_drafts_dirty ON drafts(dirty);",
);

/// M10 forward migration: durable send queue (Phase 13, Plan 13-01).
///
/// One row per outgoing mail (`id` = queue uuid). `message_id` is assigned
/// once at enqueue and is `UNIQUE` — double-invoke dedupes on it, and
/// retries resend the identical `.eml` bytes (never re-render). Envelope
/// recipients ride as JSON (`to_addrs`/`cc_addrs`/`bcc_addrs`; BCC is
/// envelope-only, never in headers). `eml_path` points at the immutable
/// `<app_data>/outbox/<id>.eml` render. `state` is the flush machine
/// (`queued|sending|sent|failed|uncertain`); `failed` is terminal-with-
/// manual-retry, `uncertain` means reconcile-not-resend. `draft_id` links
/// the DRAFT-03 send transaction (`NULL` = composed outside drafts).
/// Crash recovery resets `sending` → `queued` at launch before any flush.
const M10_SEND_QUEUE_SQL: &str = concat!(
    "CREATE TABLE send_queue (",
    "  id            TEXT PRIMARY KEY,",
    "  message_id    TEXT NOT NULL UNIQUE,",
    "  from_addr     TEXT NOT NULL DEFAULT '',",
    "  to_addrs      TEXT NOT NULL DEFAULT '[]',",
    "  cc_addrs      TEXT NOT NULL DEFAULT '[]',",
    "  bcc_addrs     TEXT NOT NULL DEFAULT '[]',",
    "  eml_path      TEXT NOT NULL DEFAULT '',",
    "  state         TEXT NOT NULL DEFAULT 'queued'",
    "    CHECK (state IN ('queued','sending','sent','failed','uncertain')),",
    "  attempts      INTEGER NOT NULL DEFAULT 0,",
    "  next_retry_at TEXT,",
    "  last_error    TEXT,",
    "  draft_id      TEXT,",
    "  created_at    TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "CREATE INDEX idx_send_queue_state ON send_queue(state);",
    "CREATE INDEX idx_send_queue_next_retry ON send_queue(next_retry_at);",
);

/// M11 forward migration: `sent_unfiled` send state (Phase 13, Plan 13-03).
///
/// SMTP accepted the bytes but the Sent APPEND did not land (or has not run
/// yet) — APPEND-only retry, never re-SMTP-send. SQLite cannot ALTER a CHECK
/// constraint, so the table is rebuilt: copy all 13 columns into the new
/// shape (CHECK extended with `sent_unfiled`), drop, rename, re-index.
/// Row-preserving by construction (same columns, plain INSERT..SELECT).
const M11_SENT_UNFILED_SQL: &str = concat!(
    "CREATE TABLE send_queue_new (",
    "  id            TEXT PRIMARY KEY,",
    "  message_id    TEXT NOT NULL UNIQUE,",
    "  from_addr     TEXT NOT NULL DEFAULT '',",
    "  to_addrs      TEXT NOT NULL DEFAULT '[]',",
    "  cc_addrs      TEXT NOT NULL DEFAULT '[]',",
    "  bcc_addrs     TEXT NOT NULL DEFAULT '[]',",
    "  eml_path      TEXT NOT NULL DEFAULT '',",
    "  state         TEXT NOT NULL DEFAULT 'queued'",
    "    CHECK (state IN ('queued','sending','sent','failed','uncertain','sent_unfiled')),",
    "  attempts      INTEGER NOT NULL DEFAULT 0,",
    "  next_retry_at TEXT,",
    "  last_error    TEXT,",
    "  draft_id      TEXT,",
    "  created_at    TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "INSERT INTO send_queue_new (id, message_id, from_addr, to_addrs, cc_addrs, \
       bcc_addrs, eml_path, state, attempts, next_retry_at, last_error, \
       draft_id, created_at) \
     SELECT id, message_id, from_addr, to_addrs, cc_addrs, bcc_addrs, \
       eml_path, state, attempts, next_retry_at, last_error, draft_id, \
       created_at FROM send_queue;",
    "DROP TABLE send_queue;",
    "ALTER TABLE send_queue_new RENAME TO send_queue;",
    "CREATE INDEX idx_send_queue_state ON send_queue(state);",
    "CREATE INDEX idx_send_queue_next_retry ON send_queue(next_retry_at);",
);

/// M12 forward migration: classification store (Phase 16, Plan 16-02).
///
/// Foundation for the v1.3 Auto-Classify lane. Pointer-only by design:
/// `labels` carries category IDs + confidence, NEVER snippet/body/evidence
/// text (privacy is structural — the schema cannot hold content even if a
/// later phase tries). `label_overrides` is append-only from day one (the
/// history VIEW lands later; the log must already exist). `classify_queue`
/// is the behind-sync work list with UIDVALIDITY epoch hygiene (same drop
/// rule as `imap_outbox`: whole-folder rows drop on UIDVALIDITY bump).
/// `batch_runs` exists now so the milestone needs exactly one migration.
const M12_CLASSIFY_SQL: &str = concat!(
    "CREATE TABLE taxonomy (",
    "  id           INTEGER PRIMARY KEY CHECK (id = 1),",
    "  version      INTEGER NOT NULL,",
    "  name         TEXT NOT NULL DEFAULT '',",
    "  json         TEXT NOT NULL,",
    "  installed_at TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "CREATE TABLE labels (",
    "  message_id   INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,",
    "  primary_id   TEXT NOT NULL,",
    "  secondary_id TEXT,",
    "  confidence   REAL NOT NULL DEFAULT 0,",
    "  threshold    REAL NOT NULL DEFAULT 0.6,",
    "  stale        INTEGER NOT NULL DEFAULT 0,",
    "  labeled_at   TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "CREATE INDEX idx_labels_primary ON labels(primary_id);",
    "CREATE TABLE label_overrides (",
    "  id           INTEGER PRIMARY KEY,",
    "  message_id   INTEGER NOT NULL,",
    "  from_id      TEXT NOT NULL,",
    "  to_id        TEXT NOT NULL,",
    "  at           TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "CREATE INDEX idx_overrides_message ON label_overrides(message_id);",
    "CREATE TABLE classify_queue (",
    "  id           INTEGER PRIMARY KEY,",
    "  message_id   INTEGER NOT NULL,",
    "  folder       TEXT NOT NULL,",
    "  status       TEXT NOT NULL DEFAULT 'pending'",
    "    CHECK (status IN ('pending','processing','done','failed')),",
    "  attempts     INTEGER NOT NULL DEFAULT 0,",
    "  uidvalidity  INTEGER NOT NULL DEFAULT 0,",
    "  enqueued_at  TEXT NOT NULL DEFAULT (datetime('now')),",
    "  UNIQUE (message_id, folder)",
    ");",
    "CREATE INDEX idx_classify_queue_status ON classify_queue(status, folder);",
    "CREATE TABLE batch_runs (",
    "  id           INTEGER PRIMARY KEY,",
    "  started_at   TEXT NOT NULL DEFAULT (datetime('now')),",
    "  finished_at  TEXT,",
    "  state        TEXT NOT NULL DEFAULT 'running'",
    "    CHECK (state IN ('running','done','interrupted','undone')),",
    "  totals_json  TEXT NOT NULL DEFAULT '{}'",
    ");",
);

/// M13 forward migration: auto-classify folder exclusions (Phase 17,
/// SIDE-03). Per-folder opt-out; the automatic pass skips these folders
/// entirely while manual `classify_message` still works on them.
const M13_EXCLUSIONS_SQL: &str = concat!(
    "CREATE TABLE classify_excluded_folders (",
    "  folder       TEXT PRIMARY KEY,",
    "  reason       TEXT NOT NULL DEFAULT '',",
    "  excluded_at  TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
);

/// M14 forward migration: review flag + settings (Phase 18, Plan 18-01).
///
/// `labels.needs_review` persists the suggester's routing (gate + keyword
/// veto) so confirm can distinguish "confident suggestion" from "review
/// item" WITHOUT recomputing. `classify_settings` holds `threshold` and
/// the `auto_root` marker (ours-vs-theirs `Auto` collision answer).
const M14_REVIEW_SETTINGS_SQL: &str = concat!(
    "ALTER TABLE labels ADD COLUMN needs_review INTEGER NOT NULL DEFAULT 0;",
    "CREATE TABLE classify_settings (",
    "  key          TEXT PRIMARY KEY,",
    "  value        TEXT NOT NULL DEFAULT ''",
    ");",
);

/// M15 forward migration: suggestion dismissal (Phase 18, Plan 18-01).
///
/// Dismissed suggestions leave the mail alone: the hook only enqueues
/// UNLABELED rows (dismissed rows keep their label), and the review list
/// filters them out. Confirm stays possible (user changed mind).
const M15_DISMISSED_SQL: &str =
    "ALTER TABLE labels ADD COLUMN dismissed INTEGER NOT NULL DEFAULT 0;";

/// M16 forward migration: batch journal (Phase 20, Plan 20-01).
///
/// Per-message journal enabling resume (skip journaled) + undo-batch
/// (reverse `moved` rows). `moved` distinguishes journaled-intent from
/// journaled-fact so a crash between journal-write and MOVE retries the
/// row instead of double-moving it.
const M16_BATCH_ITEMS_SQL: &str = concat!(
    "CREATE TABLE batch_items (",
    "  id           INTEGER PRIMARY KEY,",
    "  run_id       INTEGER NOT NULL REFERENCES batch_runs(id) ON DELETE CASCADE,",
    "  message_id   INTEGER NOT NULL,",
    "  from_folder  TEXT NOT NULL,",
    "  to_folder    TEXT NOT NULL,",
    "  label_id     TEXT NOT NULL,",
    "  chunk        INTEGER NOT NULL DEFAULT 0,",
    "  moved        INTEGER NOT NULL DEFAULT 0,",
    "  undone       INTEGER NOT NULL DEFAULT 0",
    ");",
    "CREATE INDEX idx_batch_items_run ON batch_items(run_id, id);",
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
    fn schema_version_is_11_with_sent_unfiled() {
        assert_eq!(SCHEMA_VERSION, 11);
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
        let role_cols: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('mailboxes') WHERE name IN ('role', 'attributes')",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert_eq!(role_cols, 2, "mailboxes.role + attributes should exist at schema v8");
        let send_check: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'send_queue'",
                [],
                |r| r.get(0),
            )
            .expect("query should succeed");
        assert!(
            send_check.contains("sent_unfiled"),
            "send_queue CHECK should admit sent_unfiled at schema v11"
        );
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
    fn m8_adds_role_and_attributes_preserving_rows() {
        // Simulate a v7 database (through M7, as shipped after Plan 11-02).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
            M::up(M7_IMAP_OUTBOX_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // Seed v7 rows with delimiter values.
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter) \
             VALUES ('INBOX', 100, 4, ''), ('Pai/Sub', 100, 2, '/')",
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

        // Forward-upgrade with the production set — only M8 applies.
        Store::apply_migrations(&mut conn).unwrap();

        // v7 rows survive with data intact and role/attributes defaulted.
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1, "forward migration must preserve cached rows");
        let (name, delim, role, attrs): (String, String, String, String) = conn
            .query_row(
                "SELECT name, delimiter, role, attributes FROM mailboxes WHERE name = 'Pai/Sub'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(name, "Pai/Sub");
        assert_eq!(delim, "/", "M6 delimiter values survive the M8 upgrade");
        assert_eq!(role, "", "role defaults empty (not-yet-resolved, never assumed)");
        assert_eq!(attrs, "");
    }

    #[test]
    fn m9_adds_drafts_preserving_rows() {
        // Simulate a v8 database (through M8, as shipped after Phase 11).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
            M::up(M7_IMAP_OUTBOX_SQL),
            M::up(M8_ROLES_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // Seed v8 rows: mailbox + cached message + role values.
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter, role, attributes) \
             VALUES ('INBOX', 100, 4, '', 'inbox', '')",
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

        // Forward-upgrade with the production set — only M9 applies.
        Store::apply_migrations(&mut conn).unwrap();

        // v8 rows survive the upgrade.
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1, "forward migration must preserve cached rows");
        let role: String = conn
            .query_row("SELECT role FROM mailboxes WHERE name = 'INBOX'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(role, "inbox", "M8 role values survive the M9 upgrade");
        // M9 surface exists with all 12 columns.
        let cols: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('drafts') WHERE name IN \
                 ('id','mailbox_id','message_id','subject','body','recipients_to',\
                  'recipients_cc','recipients_bcc','dirty','server_uid',\
                  'attachments','updated_at')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cols, 12, "drafts table must have all 12 columns");
        for (kind, name) in [
            ("table", "drafts"),
            ("index", "idx_drafts_mailbox"),
            ("index", "idx_drafts_dirty"),
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{kind} {name} should exist at schema v9");
        }
        // Fresh drafts table starts empty.
        let drafts: i64 = conn
            .query_row("SELECT COUNT(*) FROM drafts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(drafts, 0);
    }

    #[test]
    fn m10_adds_send_queue_preserving_rows() {
        // Simulate a v9 database (through M9, as shipped after Phase 12).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
            M::up(M7_IMAP_OUTBOX_SQL),
            M::up(M8_ROLES_SQL),
            M::up(M9_DRAFTS_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // Seed v9 rows: mailbox + cached message + queued flag op + draft.
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter, role, attributes) \
             VALUES ('INBOX', 100, 4, '', 'inbox', '')",
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
        conn.execute(
            "INSERT INTO drafts (id, mailbox_id, message_id, subject, body) \
             VALUES ('draft-1', ?1, '<draft-1@sge.local>', 'Hi', 'hello')",
            rusqlite::params![mb],
        )
        .unwrap();

        // Forward-upgrade with the production set — M10 + M11 apply.
        Store::apply_migrations(&mut conn).unwrap();

        // v9 rows survive the upgrade.
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1, "forward migration must preserve cached rows");
        let ops: i64 = conn
            .query_row("SELECT COUNT(*) FROM flag_outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ops, 1, "forward migration must preserve flag_outbox rows");
        let role: String = conn
            .query_row("SELECT role FROM mailboxes WHERE name = 'INBOX'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(role, "inbox", "M8 role values survive the M10 upgrade");
        let drafts: i64 = conn
            .query_row("SELECT COUNT(*) FROM drafts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(drafts, 1, "forward migration must preserve draft rows");
        // M10 surface exists with all 13 columns.
        let cols: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('send_queue') WHERE name IN \
                 ('id','message_id','from_addr','to_addrs','cc_addrs','bcc_addrs',\
                  'eml_path','state','attempts','next_retry_at','last_error',\
                  'draft_id','created_at')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cols, 13, "send_queue table must have all 13 columns");
        for (kind, name) in [
            ("table", "send_queue"),
            ("index", "idx_send_queue_state"),
            ("index", "idx_send_queue_next_retry"),
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{kind} {name} should exist at schema v10");
        }
        // Fresh send_queue table starts empty.
        let queued: i64 = conn
            .query_row("SELECT COUNT(*) FROM send_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(queued, 0);
    }

    #[test]
    fn m12_adds_classify_tables_preserving_rows() {
        // Simulate a v11 database (through M11, as shipped after Phase 14).
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            M::up(M3_UNSEEN_COUNT_SQL),
            M::up(M4_BACKFILL_SQL),
            M::up(M5_STATUS_TS_SQL),
            M::up(M6_DELIMITER_SQL),
            M::up(M7_IMAP_OUTBOX_SQL),
            M::up(M8_ROLES_SQL),
            M::up(M9_DRAFTS_SQL),
            M::up(M10_SEND_QUEUE_SQL),
            M::up(M11_SENT_UNFILED_SQL),
        ])
        .to_latest(&mut conn)
        .unwrap();
        // Seed v11 rows: mailbox + cached message + send_queue row.
        conn.execute(
            "INSERT INTO mailboxes (name, uid_validity, uid_next, delimiter, role, attributes) \
             VALUES ('INBOX', 100, 4, '', 'inbox', '')",
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
            "INSERT INTO send_queue (id, message_id, from_addr, eml_path) \
             VALUES ('q-1', '<q@sge.local>', 'a@x.com', '/tmp/x.eml')",
            [],
        )
        .unwrap();

        // Forward-upgrade with the production set — only M12 applies.
        Store::apply_migrations(&mut conn).unwrap();

        // v11 rows survive the upgrade.
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1, "forward migration must preserve cached rows");
        let queued: i64 = conn
            .query_row("SELECT COUNT(*) FROM send_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(queued, 1, "forward migration must preserve send_queue rows");
        for (kind, name) in [
            ("table", "taxonomy"),
            ("table", "labels"),
            ("index", "idx_labels_primary"),
            ("table", "label_overrides"),
            ("index", "idx_overrides_message"),
            ("table", "classify_queue"),
            ("index", "idx_classify_queue_status"),
            ("table", "batch_runs"),
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{kind} {name} should exist at schema v12");
        }
        // Fresh classify tables start empty.
        for table in ["taxonomy", "labels", "label_overrides", "classify_queue", "batch_runs"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} must start empty");
        }
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
