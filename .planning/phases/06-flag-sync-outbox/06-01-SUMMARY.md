---
phase: 06-flag-sync-outbox
plan: '01'
subsystem: flag-sync
tags: [imap, sqlite-outbox, uid-store, seen-flag, reconcile, tauri-command]

requires:
  - phase: phase-5-keyring-auth
    provides: [os-keyring-credentials, active_account-session]
  - phase: phase-2-sync-worker
    provides: [header-sweep-worker, SyncSession-trait, store-queries]
provides:
  - set_seen Tauri command with optimistic write + durable enqueue + immediate STORE
  - flag_outbox durable queue (M2) with latest-wins UPSERT and RFC 4549 replay
  - SessionManager single-session ownership with mailbox-scoped leases
  - pending-wins reconcile gate + post-sync replay in the sync worker
affects: [06-02-frontend-toggle, 06-03-peek-audit, phase-7-folders]

actuals:
  tokens: 16675
  tasks: 3
  commits: 3

tech-stack:
  added: []
  patterns: [optimistic-write-with-durable-outbox, pending-wins-reconcile, single-session-ownership, spawn_blocking-plus-block_on]

key-files:
  created: [src-tauri/src/imap/manager.rs]
  modified: [src-tauri/src/store/mod.rs, src-tauri/src/store/queries.rs, src-tauri/src/imap/mod.rs, src-tauri/src/sync/worker.rs, src-tauri/src/commands/sync.rs, src-tauri/src/lib.rs]

key-decisions:
  - "Outbox replay is a SyncWorker method taking `&mut dyn SyncSession` so worker tests drive it with MockSession and production passes a manager lease session"
  - "set_seen returns Ok with acked=false (not Err) when offline-queued; queueing is normal operation, surfaced via pending_count + detail text"
  - "Opportunistic replay in set_seen runs only after a successful immediate STORE to avoid doubling a 30s connect timeout while offline"
  - "Command-path replay passes live_uids=None (no fresh SEARCH); absent-UID pruning happens on the next full sync"

patterns-established:
  - "UID-only flag addressing: every flag write goes through uid_store with the two seen_store_arg literals; no sequence-number call exists on any flag path"
  - "Store lock never held across .await: replay and reconcile take brief sync lock scopes around set_seen awaits"

requirements-completed: [FLAG-01, FLAG-02]

coverage:
  - id: D1
    description: "Seen toggle persists optimistically, queues durably, and is confirmed on the server after sync (FLAG-01)"
    requirement: "FLAG-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#empty_prior_flags_toggle_applies_target_state"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#replay_acks_queued_ops_in_order"
        status: pass
    human_judgment: false
  - id: D2
    description: "Offline toggles queue durably and replay on reconnect with a pending indicator until acknowledged (FLAG-02)"
    requirement: "FLAG-02"
    verification:
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#replay_failure_stays_queued_with_error"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#pending_wins_reconcile_preserves_optimistic_flags"
        status: pass
    human_judgment: false
  - id: D3
    description: "Flag toggle never flaps or lands on the wrong message under concurrent sync (UID-only STORE, pending-wins)"
    verification:
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#mock_set_seen_records_uid_store_calls"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#outbox_enqueue_latest_wins"
        status: pass
    human_judgment: false
  - id: D4
    description: "Live end-to-end Seen toggle against mail.utfpr.edu.br:993 (real server acknowledgement, offline-queue replay over a real reconnect)"
    verification: []
    human_judgment: true
    rationale: "No live IMAP server is reachable from the unit-test sandbox; wire behavior of uid_store drain/reconnect and keyring credential flow needs a human-run sync against the real account"

duration: ~1 session
completed: 2026-10-04
status: complete
---

# Phase 6 Plan 1: Backend Flag-Write Foundation Summary

**UID STORE Seen write path behind a single-session SessionManager, durable SQLite outbox with RFC 4549 replay, pending-wins reconcile, and the set_seen Tauri command with pending count in sync_status.**

## Performance

- **Duration:** ~1 session
- **Completed:** 2026-10-04
- **Tasks:** 3 completed
- **Files modified:** 9 (1 created, 8 modified)
- **Tests:** 83 passed, 0 failed, 1 ignored (full `cargo test` suite)

## Accomplishments

- Durable `flag_outbox` queue at schema v2 (M2 migration, v1 text untouched) with latest-wins UPSERT, pending-set/drop/count CRUD, and canonical `\Seen` local-write helpers
- `SyncSession::set_seen` via `uid_store` with `+FLAGS.SILENT (\Seen)` / `-FLAGS.SILENT (\Seen)` literals and mandatory stream drain, plus `SessionManager` single-session ownership with mailbox-scoped leases and reconnect-once retry
- `set_seen` Tauri command (optimistic write + enqueue, immediate STORE, opportunistic replay) with `pending_count` in `sync_status`; worker pending-wins gate and epoch-checked replay hooked post-sync and on empty mailbox

## Task Commits

Each task was committed atomically:

1. **Task 1: Outbox storage** - `68b8889` (feat)
2. **Task 2: Session write path** - `3677ed5` (feat)
3. **Task 3: Wire command and reconcile** - `6d578f5` (feat)

## Files Created/Modified

- `src-tauri/src/imap/manager.rs` - SessionManager, MailboxLease, reconnect-once set_seen (created)
- `src-tauri/src/store/mod.rs` - SCHEMA_VERSION 2, M2 flag_outbox migration, forward-migration tests
- `src-tauri/src/store/queries.rs` - outbox CRUD, set_local_seen/set_seen_flag, mailbox_id/message_flags helpers, outbox tests
- `src-tauri/src/imap/mod.rs` - SyncSession::set_seen, seen_store_arg literals, BoxedSession impl with stream drain
- `src-tauri/src/sync/worker.rs` - replay_outbox engine, pending-wins gate, UIDVALIDITY outbox drop, replay hooks, 6 new tests
- `src-tauri/src/commands/sync.rs` - set_seen command, SetSeenResult, sync_status pending_count, shared credential loader
- `src-tauri/src/lib.rs` - AppState.session_manager slot, set_seen handler registration
- `src-tauri/src/imap/bodies.rs`, `src-tauri/src/bin/sync_demo.rs` - MockSession set_seen stubs (trait fallout)

## Decisions Made

- Outbox replay is a `SyncWorker` method taking `&mut dyn SyncSession` so worker tests drive it with MockSession and production passes a manager lease session.
- `set_seen` returns `Ok` with `acked=false` (not `Err`) when offline-queued; queueing is normal operation, surfaced via `pending_count` + `detail` text.
- Opportunistic replay in `set_seen` runs only after a successful immediate STORE to avoid doubling a 30s connect timeout while offline.
- Command-path replay passes `live_uids=None` (no fresh SEARCH); absent-UID pruning happens on the next full sync.
- Commits land directly on `main` per established repo practice (all prior feat commits on main; orchestrator ran without worktree isolation).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Two extra MockSession fixtures missing the new trait method**
- **Found during:** Task 2 (`set_seen` verify step — compile error E0046)
- **Issue:** Extending `SyncSession` with `set_seen` broke two more implementors outside the plan's file list: the `MockSession` in `imap/bodies.rs` tests and the one in `bin/sync_demo.rs`.
- **Fix:** Added no-op `set_seen` stubs to both fixtures.
- **Files modified:** `src-tauri/src/imap/bodies.rs`, `src-tauri/src/bin/sync_demo.rs`
- **Verification:** `cargo test set_seen` green (3 matched tests pass); full suite 83 passed.
- **Committed in:** `3677ed5` (part of task commit)

**2. [Rule 2 - Missing critical] Forward-migration coverage for v1 databases**
- **Found during:** Task 3 (plan verify: "migration M2 upgrades v1 databases forward")
- **Issue:** The plan's verify step requires M2 to upgrade v1 databases forward, but no test exercised a real v1→v2 upgrade path — only fresh in-memory migrations.
- **Fix:** Added `m2_upgrades_v1_database_forward_preserving_rows` test in `store/mod.rs`: builds a v1-only DB with a cached row, runs `apply_migrations`, asserts `flag_outbox` exists and the row survives.
- **Files modified:** `src-tauri/src/store/mod.rs`
- **Verification:** New test passes; full suite 83 passed.
- **Committed in:** `6d578f5` (part of task commit)

---

**Total deviations:** 2 auto-fixed (1 Rule 3, 1 Rule 2)
**Impact on plan:** Both auto-fixes necessary for correctness/verification. No scope creep.

## Issues Encountered

Repeated self-inflicted edit slip: three times an `Edit` call targeting a test-function opening line dropped the trailing `{`, breaking compilation. Each was caught immediately by re-reading the affected hunk and repaired before running tests. No impact on committed code (all commits compile + pass). Lesson: when inserting before/after a function signature line, anchor edits on unique context lines, not on the signature line itself.

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: T-6-01 mitigated | `src-tauri/src/imap/mod.rs`, `src-tauri/src/imap/manager.rs`, `src-tauri/src/commands/sync.rs` | New renderer→wire write path (`set_seen` uid → UID STORE). Mitigation landed: UID-only addressing (u32 from local DB row, fixed two-variant STORE literal, no sequence-number call anywhere — verified by grep + `mock_set_seen_records_uid_store_calls` + `set_seen_store_arg_*` tests) |
| threat_flag: T-6-02 mitigated | `src-tauri/src/sync/worker.rs`, `src-tauri/src/store/queries.rs` | New durable-queue→replay path. Mitigation landed: per-op epoch check with whole-queue drop on UIDVALIDITY mismatch, absent-UID single-op drop, covered by `replay_epoch_mismatch_drops_queue` + `replay_absent_uid_drops_single_op` tests |

T-6-03 (BODY.PEEK audit) is unchanged by this plan — the full audit test lands in plan 06-03 per the threat register. No new packages added (T-6-SC accept stands).

## Known Stubs

None — stub scan over the plan diff found no TODO/FIXME/placeholder/unimplemented markers. Every new symbol is wired: `set_seen` command → manager lease → `uid_store` drain → outbox ack delete; `sync_status.pending_count` reads `outbox_count`.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Ready for 06-02 (frontend read/unread toggle): `set_seen(uid, seen)` + `sync_status.pending_count` are registered IPC commands; `SetSeenResult.acked/pending_count/detail` gives the UI everything for optimistic toggle + pending indicator.
- Ready for 06-03 (BODY.PEEK audit): write path is UID-STORE-only; no FETCH paths were touched, so the audit baseline is unchanged.
- Live-server validation still open (D4 above): real UTFPR acknowledgement + offline replay need a human-run sync; UTFPR capability caveats (CONDSTORE/`.SILENT` acceptance) from STATE.md blockers remain.

---
*Phase: 06-flag-sync-outbox*
*Completed: 2026-10-04*

## Self-Check: PASSED

- All 9 source files exist on disk (manager.rs created, 8 modified).
- All 3 task commits exist (`68b8889`, `3677ed5`, `6d578f5` verified via `git log`).
- Full `cargo test` suite: 83 passed, 0 failed at commit time.
