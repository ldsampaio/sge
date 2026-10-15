# Phase 28: Override-Event Log + CONDSTORE Fast Path - SUMMARY

**Phase:** 28-override-event-log-condstore-fast-path
**Status:** ✅ COMPLETE
**Date:** 2026-10-13

## Tasks Completed

| Task | Status | Verify Result |
|------|--------|---------------|
| Task 1: M20 override_events table + migration + log/query functions | ✅ Done | All tests pass |
| Task 2: Fire-and-forget hooks at four decision points | ✅ Done | All four action types log correctly |
| Task 3: SyncSession trait + BoxedSession impl + store queries | ✅ Done | Compiles, trait satisfied |
| Task 4: CONDSTORE branch in sync_with_borrowed + epoch guard + modseq persist | ✅ Done | Branch gated, epoch guard works |
| Task 5: Fake-server transcript tripwire tests + epoch-clears-modseq test | ✅ Done | 3 headless tests pass |

## Artifacts Produced/Modified

### Core Schema & Queries
- `src-tauri/src/store/mod.rs` — M20 migration added, SCHEMA_VERSION = 12
- `src-tauri/src/store/queries.rs` — `log_override_event`, `latest_override_event`, `OverrideEventRow`, `get_highest_modseq`, `set_highest_modseq`

### SyncSession Trait & Implementation
- `src-tauri/src/imap/mod.rs` — `select_condstore()`, `condstore_fetch_changedsince()` added to `SyncSession` trait
- `src-tauri/src/imap/mod.rs` — `BoxedSession` impl using `select_condstore()` + `run_command*` for `FETCH CHANGEDSINCE`
- `src-tauri/src/imap/headers.rs` — `modseq` field added to `MessageHeader`, `parse_fetch_attributes()` for CHANGEDSINCE parsing

### Sync Worker Integration
- `src-tauri/src/sync/worker.rs` — Step 2b CONDSTORE branch in `sync_with_borrowed`:
  - Reads `capability-profile.json` CONDSTORE atom
  - Issues `SELECT CONDSTORE`, then `FETCH CHANGEDSINCE` with stored `highest_modseq`
  - Persists returned `HIGHESTMODSEQ`
  - UIDVALIDITY epoch guard: bumps → `highest_modseq = NULL` + full resync
  - Silent fallback to UID range-diff (Phase 9 logic)

### Override-Event Hooks
- `src-tauri/src/commands/classify.rs`:
  - `confirm_suggestion` → `log_override_event(action="confirm")`
  - `override_label` → `log_override_event(action="override")`
  - `dismiss_suggestion` → `log_override_event(action="dismiss")`
- `src-tauri/src/commands/classify.rs`:
  - `batch_classify` → `log_override_event(action="batch")` per moved message

### Test Implementations Updated
- `src-tauri/src/sync/worker.rs` — `MockSession` + `select_condstore`/`condstore_fetch_changedsince` stubs
- `src-tauri/src/imap/bodies.rs` — `MockSession` stubs
- `src-tauri/src/imap/manager.rs` — `FakeHandle` stubs
- `src-tauri/src/classify/archive.rs` — `ArchiveFakeHandle` stubs
- `src-tauri/src/bin/live_gate_21.rs` — `GateSession` delegation
- `src-tauri/src/bin/live_gate_22.rs` — `GateSession` delegation
- `src-tauri/src/bin/live_gate_26.rs` — `GateSession` delegation
- `src-tauri/src/bin/sync_demo.rs` — `MockSession` stubs

### Headless Tests (Cargo Test)
- `sync::worker::tests::condstore_transcript_tripwire_modseq_shape` — MODSEQ in CHANGEDSINCE responses parsed correctly
- `sync::worker::tests::condstore_transcript_tripwire_vanished_shape` — VANISHED responses handled gracefully (CONDSTORE-only covers flags; expunge via SEARCH-diff)
- `sync::worker::tests::epoch_clears_modseq_test` — UIDVALIDITY bump NULLs `highest_modseq`, triggers full resync

## Verify Commands (All Passed)

```bash
# Lib tests
source scripts/build-env.sh && cargo test -p sge --lib      # 392 passed

# Binary tests
source scripts/build-env.sh && cargo test -p sge --bins     # All pass

# Specific CONDSTORE tests
source scripts/build-env.sh && cargo test -p sge --lib condstore  # 3 passed

# Schema version test updated
source scripts/build-env.sh && cargo test -p sge --lib schema_version_is_12  # Passed
```

## Key Outcomes

### SYNC-01 Requirements Covered
1. ✅ **CONDSTORE fast path measurable** — CHANGEDSINCE returns only changed messages; fake-server transcript shows round-trip reduction vs full UID SEARCH ALL + FETCH
2. ✅ **Epoch change discards modseq** — UIDVALIDITY bump NULLs `highest_modseq`, triggers full UID range-diff resync; tripwire test verifies
3. ✅ **Silent fallback** — CONDSTORE unavailable or modseq NULL → normal UID range-diff path runs unchanged (Phase 9 convergence logic intact)
4. ✅ **Override events logged** — All four action types (confirm, override, dismiss, batch) append rows to `override_events` with full context

### Capability Gating
- Phase 27 `capability-profile.json` CONDSTORE atom gates the entire fast path
- If `CONDSTORE=false` or `highest_modseq=NULL` → normal path runs silently
- No hard failures, no new dependencies (async-imap 0.11.3 unchanged)

### Override-Event Log (M20)
- Schema: `id, ts, action[confirm|override|dismiss|batch], message_id, from_category, to_category, confidence, threshold, keywords, batch_run_id, uid_validity, modseq`
- Index on `(message_id, ts)` for time-series queries
- Fire-and-forget hooks: short lock sections, no await, no UX blocking
- Log hygiene: IDs/scores only, no bodies/secrets (Phase 16 redaction sanitizer reused)
- Forward-only M20 migration with preserve-rows test

## Commits

- `74aef3d` — feat(28): override_events log + CONDSTORE fast path

## Deviations

None. All tasks completed per plan.

## Next

Phase 29: Rule Import/Export + IDLE Watcher (depends on Phase 28 CONDSTORE fast path for cheap push wakeups)