# Phase 9 — UID Backfill (Wave 1: Gap detection + convergence)

## Summary

**Wave:** 1 of 1
**Status:** ✅ Complete (implemented 2026-10-05 gap closure; supersedes the retracted 2026-10-04 summary, which described work that did not exist)
**Depends on:** Phase 8 (Poll + Manual Refresh)
**Requirements:** SYNC-04

## Tasks Completed

**Task 1 — UID range-diff logic** (`src-tauri/src/sync/worker.rs`, `store/queries.rs`, `store/mod.rs`):
- After each batch FETCH, requested − returned = gap set → one immediate in-pass re-fetch; `gap_refetches` counted on `SyncSummary`
- Still-missing UIDs earn strikes (`record_fetch_strike`); at `TOMBSTONE_STRIKES` (3) they are excluded from future sweeps (no infinite loop); `clear_tombstone` on success; `prune_tombstones` when they vanish from SEARCH
- Step 6 expunge diff runs over server ∪ tombstoned so a skip never reads as an expunge
- M4 migration: `fetch_tombstones` table + `mailboxes.sweeps_since_full` (SCHEMA_VERSION 4)

**Task 2 — Convergence** (`src-tauri/src/sync/worker.rs`, `sync/mod.rs`):
- Skip shortcut: server UID set == local AND no epoch bump AND no pending outbox ops → no envelope FETCH, `converged = true`, counter +1
- Periodic full sweep every `FULL_SWEEP_EVERY` (5) passes (counter reset) so remote flag changes surface; tombstoned UIDs retried there
- `SyncSummary` gains `fetched / gap_refetches / converged`; done-line logs all three
- Verified: `uid_gap`, `convergence_test`, `tombstoned_uids_stop_being_requested`, `m4_adds_tombstones_and_sweep_counter`

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml uid_gap          # 1 passed
cargo test --manifest-path src-tauri/Cargo.toml convergence_test # 1 passed
cargo test --manifest-path src-tauri/Cargo.toml                  # 100 passed, 1 ignored, 0 failed
```

## Files Modified

- `src-tauri/src/sync/worker.rs` — convergence shortcut, range-diff refetch, strikes, sweep-set filter, expunge union, mock range-awareness + flaky modes, 3 gate tests, double-sync test updated to convergence semantics
- `src-tauri/src/sync/mod.rs` — SyncSummary fetched/gap_refetches/converged
- `src-tauri/src/store/mod.rs` — M4 + tests (SCHEMA_VERSION 4)
- `src-tauri/src/store/queries.rs` — all_local_uids, tombstone + counter helpers
- `.planning/phases/09-uid-backfill/09-01-PLAN.md` — as-built notes
