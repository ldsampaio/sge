# 02-03 PLAN SUMMARY — Tauri IPC Bridge + Demo Harness

## Status: ✅ COMPLETE

## What was built

### Files created
- `src-tauri/src/commands/sync.rs` — Three Tauri commands:
  - `start_sync(on_event: Channel<SyncEvent>)` — loads stored creds + server config from OS keyring, opens IMAP session (BODY.PEEK only), runs SyncWorker on a dedicated blocking thread, streams typed `SyncEvent` progress over the Channel
  - `sync_status(state: AppState)` — reads `mailboxes.last_sync_at + counts` from SQLite (same row the worker writes), returns `SyncStatus` for frontend polling
  - `cancel_sync()` — cancellation flag (wired for Phase 3 poll timer)
- `src-tauri/src/bin/sync_demo.rs` — CLI harness proving all 3 Phase 2 success criteria headless against a deterministic `MockSession` (5 fixed messages): full sweep → incremental noop → UIDVALIDITY mismatch rejection
- `src/components/SyncStatus.tsx` — React component: "Up to date / Offline — last synced <timestamp>", "Sync Now" button with live progress events, "Cancel" button, error+retry
- `src/App.tsx` — Updated: SecuritySelector + LoginForm + SyncStatus wired in sequence

### Files modified
- `src-tauri/src/lib.rs` — `AppState` holds `Arc<Mutex<Store>>` (Send+Sync for Tauri); `invoke_handler` registers `start_sync`, `sync_status`, `cancel_sync`; `connect_account` saves server config after login
- `src-tauri/src/commands/mod.rs` — Declares `pub mod sync;`
- `src-tauri/Cargo.toml` — Added `[[bin]] sync_demo` entry

### Sync engine (from 02-01 + 02-02, verified)
- `src/imap/headers.rs` — `fetch_envelope_batch()` with BODY.PEEK, parsed via mail-parser
- `src/imap/bodies.rs` — `fetch_body_message()` cache + overflow guard + attachment file write
- `src/sync/worker.rs` — 7-step incremental algorithm (ensure mailbox → SELECT → UIDVALIDITY guard → compute fetch_from → batched 200-UID sweep → UID SEARCH ALL expunge diff → write sync_state + logout)
- `src/sync/mod.rs` — `SyncEvent` enum, `SyncFlag`, `SyncSummary`, `SyncCallback`

## Verification

- `cargo check` — clean (exit 0, lib + sync_demo bin)
- `cargo test` — **56 passed, 0 failed, 1 ignored** (up from 29; includes store, headers, bodies, sync workers)
- `grep -rn "STORE\\|EXPUNGE" src/` — GATE CLEAN (no STORE/EXPUNGE anywhere, read-only invariant)
- `npm run build` — **clean** (frontend TypeScript compiles + Vite bundles)
- `npm run lint` — clean
- `cargo run --bin sync_demo` — proves all 3 success criteria headless:
  1. Full sweep: 5 new messages → 5 rows in SQLite ✓
  2. Incremental: second run 0 new messages ✓
  3. UIDVALIDITY mismatch: rejected with error ✓

### Tests included
| Test | What it verifies |
|------|------------------|
| `format_flags_canonical` | Flag JSON round-trips |
| `full_sync_inserts_three_headers` | 3 new messages → all new, 3 DB rows |
| `incremental_sync_reports_updates` | Second run same data → 0 new, 3 updated |
| `uidvalidity_bump_triggers_wipe` | UIDVALIDITY change → wipe + full resync |
| `empty_mailbox_shortcuts` | exists=0 → no fetches |
| `cache_hit_no_imap` | body_complete=true → no fake session call |
| `oversize_body_not_cached` | oversized → body_complete stays 0 |
| `attachment_bytes_to_disk` | attachments under app-data dir |
| `uidvalidity_bump_clears_fts_index` | FTS consistent after wipe+re-upsert |
| `delete_missing_uids_partial` | Expunge diff removes only vanished UIDs |

## Constraints honored
- BODY.PEEK only — no STORE/EXPUNGE, no \Seen writes
- UIDVALIDITY-guarded incremental vs full resync
- Channel<SyncEvent> only for progress (no plain emit)
- No auto-login logic — launch always lands on login screen
- Credentials in OS keyring via keyring sync-secret-service
- ServerConfig stored separately in keyring (`sge-server-cfg`)

## Phase 2 success criteria (ROADMAP)
1. ✓ After first sync, INBOX headers readable from local SQLite + server copies preserved (BODY.PEEK)
2. ✓ Second sync is incremental (UIDVALIDITY-guarded; bump triggers full resync)
3. ✓ User sees sync progress (n/total events over Channel), up-to-date timestamp, offline badge