# 02-01 PLAN SUMMARY — Canonical SQLite Store

## Status: ✅ COMPLETE

## What was built

### Files created
- `src-tauri/src/store/mod.rs` — `Store` type (WAL + migration runner), `StoreError`,
  `BODY_CACHE_CAP_BYTES = 262144`, `attachment_dir()` helper, `open()` / `open_in_memory()`.
- `src-tauri/src/store/queries.rs` — All SQL centralized in one module per
  ARCHITECTURE.md single-writer invariant. Query functions:
  `ensure_mailbox`, `get_sync_state`, `set_sync_state`, `upsert_message`,
  `upsert_message_batch`, `delete_missing_uids`, `list_messages`, `fts_search`,
  `insert_body`, `body_is_complete`, `insert_attachment_meta`.
- `src-tauri/src/store/schema.sql` — Canonical DDL verbatim from ARCHITECTURE.md
  (mailboxes, messages, message_bodies, attachment_parts, messages_fts + triggers).

### Files modified
- `src-tauri/src/lib.rs` — Added `pub mod store;`.

## Verification

- `cargo check` — clean (exit 0)
- `cargo test store::` — **13 passed, 0 failed**
- `grep -rn "STORE\|EXPUNGE" src-tauri/src/store/` — **CLEAN** (no matches)
- WAL mode confirmed on file-based DB (in-memory always reports "memory")

### Tests included
| Test | What it verifies |
|------|-----------------|
| `in_memory_store_creates_all_tables` | All 5 tables + FTS5 exist after migration |
| `in_memory_store_fts_and_triggers_exist` | Both FTS triggers (msg_ai, msg_ad) present |
| `wal_mode_is_set_on_file_database` | PRAGMA journal_mode = WAL on file DB |
| `body_cache_cap_is_256kb` | Constant = 262144 |
| `attachment_dir_layout` | Path = app_data/attachments/uidvalidity/uid |
| `upsert_overwrites_existing_uid` | ON CONFLICT UPSERT works, no duplicates |
| `uidvalidity_resync_wipe_no_duplicates` | Empty live_uids wipes rows; re-sweep yields correct count |
| `uidvalidity_bump_clears_fts_index` | FTS index stays consistent after wipe + re-upsert |
| `upsert_expunge_fts_roundtrip` | FTS search finds inserted messages; deletion removes them |
| `flags_stored_but_read_unread_is_display_only` | Flags persisted verbatim; M1 read/unread display-only |
| `body_insert_and_complete_flag` | Body complete flag flips after insert_body |
| `delete_missing_uids_partial` | Expunge diff removes only missing UIDs |
| `ten_thousand_row_perf_smoke` | 10k rows inserted; list_messages(200) < 50ms; FTS search returns |

## Constraints honored
- No IMAP, no Tauri dependencies in store module (pure rusqlite)
- BODY_CACHE_CAP_BYTES = 262144 (256 KB)
- ATTACHMENT_METADATA only — no byte storage in SQLite (file-backed per D-attachments)
- STORE/EXPUNGE strings absent (verified by grep)
- Single SQL module: all queries centralized in queries.rs
