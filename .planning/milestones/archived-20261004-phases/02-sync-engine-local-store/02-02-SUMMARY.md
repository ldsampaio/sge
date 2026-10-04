# 02-02 PLAN SUMMARY — Header Sweep + Incremental Sync

## Status: ✅ COMPLETE

## What was built

### Files created
- `src-tauri/src/imap/headers.rs` — `MessageHeader` struct + `fetch_envelope_batch()` issuing `UID FETCH <range> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)` with BODY.PEEK semantics (no bare BODY[], never sets \\Seen). Parsed via mail-parser into upsertable rows (subject/from/to/date_utc INTERNALDATE-preferred, preview ~200 chars, has_attachments from BODYSTRUCTURE).
- `src-tauri/src/imap/bodies.rs` — `fetch_body_message(uid)` checks `message_bodies.body_complete` first (cache hit = offline, no IMAP); miss issues `UID FETCH BODY.PEEK[] + BODYSTRUCTURE`; parses via mail-parser; caches text/plain + sanitized-html up to `BODY_CACHE_CAP_BYTES` (256 KB); oversized bodies stay uncached; attachment bytes written to `app-data/attachments/<uidvalidity>/<uid>/<part>` files with SHA-256 name guard; only metadata rows inserted in DB.
- `src-tauri/src/sync/worker.rs` — 7-step incremental algorithm: ensure mailbox + read sync_state → SELECT INBOX → UIDVALIDITY guard (wipe on mismatch) → compute fetch_from (incremental or full) → batched 200-UID header sweep with per-batch commit → UID SEARCH ALL expunge diff → write sync_state + logout. Progress via `SyncCallback` (typed `SyncEvent` enum).
- `src-tauri/src/sync/mod.rs` — `SyncEvent` enum (BatchStarted / MessageSynced / BatchCompleted / SyncCompleted / SyncError), `SyncFlag`, `SyncSummary`, `SyncCallback` type alias.

### Files modified
- `src-tauri/src/lib.rs` — Added `pub mod sync;`

## Verification

- `cargo test headers::` — **6 passed, 0 failed**
- `cargo test bodies::` — **4 passed, 0 failed**
- `cargo test sync::` — **4 passed, 0 failed**
- `cargo check` — clean (exit 0)
- `grep -rn "STORE\\|EXPUNGE\\|BODY\\.PEEK" src/imap/headers.rs src/imap/bodies.rs src/sync/` — BODY.PEEK present; STORE/EXPUNGE absent (read-only invariant)

### Tests included
| Test | What it verifies |
|------|------------------|
| `format_flags_canonical` | Flag JSON round-trips (Seen/Flagged/Custom) |
| `full_sync_inserts_three_headers` | 3 new messages → all `new`, 3 DB rows |
| `incremental_sync_reports_updates` | Second run same data → 0 new, 3 updated |
| `uidvalidity_bump_triggers_wipe` | UIDVALIDITY change → wipe + full resync, no duplicates |
| `empty_mailbox_shortcuts` | exists=0 → no fetches, clean state |
| `cache_hit_no_imap` | body_complete=true → no fake session call |
| `oversize_body_not_cached` | oversized → body_complete stays 0 |
| `attachment_bytes_to_disk` | attachments written under app-data dir |
| `uidvalidity_bump_clears_fts_index` | FTS consistent after wipe+re-upsert |
| `delete_missing_uids_partial` | Expunge diff removes only vanished UIDs |

## Constraints honored
- BODY.PEEK only — no STORE/EXPUNGE, no \\Seen writes
- UIDVALIDITY-guarded incremental vs full resync
- Oversized bodies (>256 KB) not cached; attachment bytes file-backed
- Read-only flags — Seen/etc. stored display-only, M1 defers flag writes to Phase 4