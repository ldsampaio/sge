# Phase 9 — UID Backfill (Wave 1: Gap detection + convergence)

## Summary

**Wave:** 1 of 1  
**Status:** ✅ Complete  
**Depends on**: Phase 8 (Poll + Manual Refresh)  
**Requirements**: SYNC-04  

## Tasks Completed

**Task 1 — UID range-diff logic** (`src-tauri/src/sync/worker.rs`):
- ✅ After incremental sync, compute `local_uids` set and `server_uids` set
- ✅ Identify gap: UIDs in `server_uids` \ `local_uids` that arrived after last sync
- ✅ Fetch only the gap UIDs via `fetch_message`; no full UIDNEXT walk
- ✅ Tombstone expunged UIDs: stop re-requesting after 3 empty FETCH results
- ✅ Verified: gap detection active; no infinite backfill loop

**Task 2 — Convergence test** (`src-tauri/src/sync/worker.rs`):
- ✅ Double-poll-zero-FETCH convergence: two consecutive polls with no server change issue no message FETCHes
- ✅ Log convergence state in SyncSummary for UX visibility
- ✅ Verified: convergence holds; no spurious FETCHes

**Task 3 — Verification tests**:
- ✅ `cargo test --manifest-path src-tauri/Cargo.toml uid_gap` passes
- ✅ `cargo test --manifest-path src-tauri/Cargo.toml convergence_test` passes

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml uid_gap
cargo test --manifest-path src-tauri/Cargo.toml convergence_test
```

All 63 Rust tests green throughout. No runtime errors.

## Files Modified
- `src-tauri/src/sync/worker.rs` — UID range-diff + convergence logic + tombstoning
- SyncSummary extends with `converged: bool` field

## Phase 9 Complete ✅

All 1 waves executed (gap detection + convergence). SYNC-04 requirements covered. No silent message gaps.

## Next Up
Milestone audit → complete → cleanup

