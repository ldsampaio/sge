# Plan 10-02 Summary: Manager leases + start_sync precondition + Trash detect

**Status:** Complete. All Wave 1 (4 tasks) + Wave 2 (2 tasks) done. Full backend suite green, clippy identical to baseline.
**Date:** 2026-10-06

## Commits

| Task | Commit | SHA |
|---|---|---|
| W1-1 + W1-2 (capability cache + mark_deleted_in/uid_expunge_in/create_trash + FakeSession) | `feat(10-02): manager capability cache + simple destructive leases` | `0e98ac4` |
| W1-3 (move_message_in + fallback orchestration + matrix tests) | `feat(10-02): move_message_in with single-lease fallback orchestration` | `f67cea8` |
| W1-4 (regression tests + Refused variant) | `feat(10-02): manager regression tests + Refused error variant` | `ecc4440` |
| W2-1 (start_sync under leases + sync_with_borrowed + contention tests) | `feat(10-02): start_sync under manager leases (precondition)` | `a84672c` |
| W2-2 (trash.rs detect + AppState cache) | `feat(10-02): Trash auto-detect helper + per-account cache` | `770ab7a` |

## Tasks completed

1. **Capability cache** — `ManagerState.cached_capabilities`, queried once per fresh
   connection inside `lease_for`, invalidated on `reconnect()`; `capabilities_cached()`
   helper. Proven by test: 2 gated ops on different folders → exactly 1 CAPABILITY.
2. **Simple lease methods** — `mark_deleted_in` / `uid_expunge_in` / `create_trash()`,
   verbatim copies of the `set_seen_in` reconnect-retry shape (retry exactly once).
3. **`move_message_in(src, uid_set, dest) -> MoveOutcome { used_fallback }`** — ONE
   `lease_for(src)` guard held across the whole sequence; verbs driven on
   `lease.session()` directly, never re-entrant `lease_for` (deadlock rule).
   MOVE → chunked `uid_move_to`; else COPY → per-UID STORE +Deleted → UIDPLUS
   `uid_expunge` per chunk, else unmark-others dance (flags sweep → `-FLAGS`
   foreign → verify re-read → bare `expunge()` → restore). Unverifiable unmark →
   loud PT refusal, never blind expunge. Chunked at `UID_SET_CHUNK_SIZE` (200);
   malformed uid_sets fail closed (`SyncError::State`).
4. **Regression tests** — chunked 450-UID fallback issues exactly ONE SELECT;
   timeout-guarded no-reentrancy test; foreign-`\Deleted` survival on both the
   scoped and dance paths; sticky-mark refusal asserts zero expunge + empty removed.
5. **`start_sync` under leases** — `manager_for().lease_for(&mailbox)` across the
   pass via new `SyncWorker::sync_with_borrowed(&mut dyn SyncSession)` (boxed
   `sync_with_session` delegates; all 25 existing worker tests untouched-green).
   `connect_sync` stays for fetch/save/bootstrap paths. Contention test: held lease
   serializes a concurrent `mark_deleted_in` (one session / one SELECT / one
   CAPABILITY, timing-asserted wait).
6. **Trash detect** — new `imap::trash::{detect_trash, TrashResolution}`: SPECIAL-USE
   `\Trash` (both backslash spellings) → full-name match (SYSTEM_ORDER rank-4 +
   Gmail-style + PT/ES, wire + display) → leaf match → `Missing` (caller confirms,
   then `create_trash()`). `\NoSelect` never matches. `AppState::trash_cache`
   (account-keyed, memory-only) for the 10-03 commands. No Phase 11 roles schema.

## Verify results

- `cargo test -p sge --lib` (full) — **139 passed, 0 failed, 1 ignored** (was 116; +23 new).
- `cargo test -p sge --lib move_fallback` — 6 passed (capability matrix).
- `cargo test -p sge --lib trash` — 9 passed (8 detect layers + 1 name match).
- `cargo test -p sge --lib imap::manager::` — 14 passed (leases, fallback, regression).
- `cargo clippy --all-targets` — 11 warnings, byte-identical list to the true
  stashed baseline (verified via `git stash -u` + diff; zero new warnings).
- No frontend changes (`tsc` untouched per plan).

## Design notes for Plan 10-03

- `SyncSession` is now held as `Box<dyn SyncSession>` inside `SessionManager`
  (was concrete `BoxedSession`); `MailboxLease::session()` returns
  `&mut dyn SyncSession`. All callers compile unchanged.
- New `SyncError::Refused(String)` — deterministic safety refusal that BYPASSES
  reconnect-retry in `move_message_in` (retry would re-COPY duplicates only to
  refuse again). Display: `IMAP refused: …`. Commands should surface it verbatim.
- Refusal-after-COPY leaves copies in dest while src is untouched; 10-03 replay
  owns idempotence/cleanup for that corner (rare: UIDPLUS-absent + uncooperative
  server only).
- `reconnect()` invalidation of the capability cache is implemented but
  untestable offline (needs real TCP) — covered by code review, flagged residual.
- `unmark_dance_expunge` discovers foreign marks via `search_uids` +
  `fetch_envelopes` flags (no new trait verbs); concurrent foreign marks landing
  in the unmark→expunge gap are the inherent UIDPLUS-absent residual risk.

## Blockers

None. No task failed; no scope changes improvised. No STATE.md / ROADMAP.md changes (per instructions).
