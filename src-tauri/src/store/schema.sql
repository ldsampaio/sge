-- SGE canonical store schema (Phase 2, Plan 02-01).
--
-- Lifted verbatim from .planning/research/ARCHITECTURE.md "Concrete SQLite
-- Table Sketch" (normative for the local-store phase). rusqlite_migration v1
-- applies this whole script to a fresh database; the module keeps WAL on.
--
-- M1 invariants encoded here:
--   * No secrets in any table — only username/mailbox metadata (threat T-02-02).
--   * messages keyed on (mailbox_id, uid); Message-ID is a nullable display
--     field, never a key (threat T-02-01).
--   * attachment_parts holds metadata only; bytes live on disk (D-attachments).
--   * message_bodies stores SANITIZED html + decoded text only; body_complete
--     distinguishes a fetched body from a header-only row.

-- WAL for concurrent reader (UI) + writer (sync worker).
PRAGMA journal_mode = WAL;

CREATE TABLE mailboxes (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,          -- 'INBOX' (M1); others later
  uid_validity  INTEGER NOT NULL DEFAULT 0,
  uid_next      INTEGER NOT NULL DEFAULT 0,
  highest_modseq INTEGER,                      -- NULL if server lacks CONDSTORE
  last_sync_at  TEXT                           -- ISO8601 UTC
);
-- sync_state folds into mailboxes (one row per folder); no separate table
-- needed at M1 scale.

CREATE TABLE messages (
  id            INTEGER PRIMARY KEY,
  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,              -- IMAP UID, unique per mailbox
  message_id    TEXT,                          -- RFC822 Message-ID (nullable, not trusted as key)
  subject       TEXT NOT NULL DEFAULT '',
  from_addr     TEXT NOT NULL DEFAULT '',
  to_addrs      TEXT NOT NULL DEFAULT '',      -- JSON array (M1-simple; normalize later)
  cc_addrs      TEXT NOT NULL DEFAULT '[]',
  date_utc      TEXT NOT NULL,                 -- INTERNALDATE preferred over Date: header
  flags         TEXT NOT NULL DEFAULT '[]',    -- JSON: Seen, Flagged, Answered...
  has_attachments INTEGER NOT NULL DEFAULT 0,  -- from BODYSTRUCTURE, drives paperclip icon
  preview       TEXT NOT NULL DEFAULT '',      -- first ~200 chars of text/plain, for list + FTS
  UNIQUE (mailbox_id, uid)
);
CREATE INDEX idx_messages_mailbox_uid ON messages(mailbox_id, uid);
CREATE INDEX idx_messages_date ON messages(mailbox_id, date_utc DESC);

CREATE TABLE message_bodies (
  message_id    INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  body_text     TEXT,                          -- text/plain decoded
  body_html     TEXT,                          -- SANITIZED html only (never raw)
  body_complete INTEGER NOT NULL DEFAULT 0,    -- 1 when fetched; 0 = header-only row
  fetched_at    TEXT
);

CREATE TABLE attachment_parts (
  id            INTEGER PRIMARY KEY,
  message_id    INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_number   TEXT NOT NULL,                 -- MIME part path e.g. '2', '1.2'
  filename      TEXT NOT NULL DEFAULT '',
  mime_type     TEXT NOT NULL DEFAULT 'application/octet-stream',
  size_bytes    INTEGER NOT NULL DEFAULT 0,
  local_path    TEXT,                          -- set after save_attachment; NULL = not downloaded
  UNIQUE (message_id, part_number)
);

-- Full-text search over cached headers+preview (M1 local search requirement).
CREATE VIRTUAL TABLE messages_fts USING fts5(subject, from_addr, preview,
  content='messages', content_rowid='id');
CREATE TRIGGER msg_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_addr, preview)
  VALUES (new.id, new.subject, new.from_addr, new.preview);
END;
CREATE TRIGGER msg_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, preview)
  VALUES ('delete', old.id, old.subject, old.from_addr, old.preview);
END;
