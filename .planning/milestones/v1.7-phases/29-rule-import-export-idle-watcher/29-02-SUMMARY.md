---
phase: 29-rule-import-export-idle-watcher
plan: 02
subsystem: idle-push
tags: [IDLE-01, idle, push, syncgate, capability-gating]
requires: [29-01, phase-27-capability-profile, phase-28-condstore]
provides: [idle-watcher, sync-wake, idle-status-commands]
affects: [29-03, phase-30-batch-trust]
tech-stack:
  added: []
  patterns: [scripted-transcript-driver, advisory-wakeup-only, capability-gated-spawn]
key-files:
  created: [src-tauri/src/imap/idle.rs]
  modified: [src-tauri/src/imap/mod.rs, src-tauri/src/sync/mod.rs, src-tauri/src/commands/sync.rs, src-tauri/src/lib.rs]
decisions:
  - "Watcher drives a small IdleDriver trait (SELECT/CAPABILITY/IDLE/DONE/LOGOUT); production wraps BoxedSession, tests replay scripted transcripts"
  - "New SyncWake type in sync/mod.rs (coalescing dirty flag + SyncGate dispatch) — no SyncWake existed before"
  - "Spawn lives behind idle_start command + spawn_if_allowed profile gate, not SyncWorker (no such constructor exists)"
  - "Stop-interruptible live wait (1s poll race, DONE on kill) via futures::select — no new deps"
metrics:
  duration: ~3h
  completed: 2026-10-14
status: complete
actuals:
  tokens: 68000
  tasks: 3
  commits: 1
commits:
  - bbeadd2: IdleWatcher + SyncWake + idle commands
plan_head_before: 0594d24
---

# Phase 29 Plan 02: IdleWatcher Core Summary

Manager-independent IDLE push (IDLE-01) delivered headlessly: a dedicated read-only connection runs SELECT→IDLE→DONE→LOGOUT cycles, wakes are advisory-only through `SyncWake` + `SyncGate`, the Phase-27 profile gates spawning (IDLE=false → poll-only, never spawns), and poll remains the untouched fallback sweeper.

## What Was Built

- **`src-tauri/src/imap/idle.rs`** (new, ~700 lines incl. tests): `IdleWatcher` (own `connect_sync` clone per cycle, never the manager lease), `IdleState` machine (Connecting/Selecting/Idling/Done/Reconnecting/Stopped/Unavailable), `drive_cycle` (begin→wait→unconditional `end_idle`), `LiveIdleDriver` (SELECT INBOX, live CAPABILITY re-check refusing non-IDLE servers, async-imap `idle()` handle, DONE+LOGOUT every exit, stop-interruptible wait within ~1s), `backoff_secs` (5s×2^attempt + deterministic jitter, 600s cap), `status_of` (Push/Polling/Offline mapping), `spawn_if_allowed` (profile gate), read-only-verb structural test.
- **`src-tauri/src/sync/mod.rs`**: new `SyncWake` (coalescing `request_sync`, `dispatch` through `SyncGate::try_begin` keeping the dirty flag on busy, request/dispatch counters) + 3 tests (burst→single pass, busy-keeps-dirty, quiet-never-dispatches).
- **`src-tauri/src/commands/sync.rs`**: `idle_status` (pure snapshot read), `idle_start` (gate → spawn + `idle-wakeup` emit callback), `idle_stop` (cooperative DONE window then abort) + 2 command-contract tests.
- **`src-tauri/src/lib.rs`**: `AppState::idle` slot (None = poll-only default) + 5 command registrations (3 idle + 2 portability).

## Tests

10 `imap::idle` tests (lifecycle verb order, DONE/reconnect/backoff transcript, 5-burst coalescing→1 pass, poll-fallback refusal, kill-mid-IDLE, SELECT-fail backoff, state chain, status mapping, read-only verbs, live-gate SKIP) + 3 SyncWake + 2 command tests. Full lib suite at commit: 420 passed, 0 failed; all `--bins` pass.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Correctness] `SyncWake` did not exist — created it**
- **Found during:** Task 1 (grep: only `SyncGate` in `sync/mod.rs`, zero `SyncWake`/`request_sync` in repo).
- **Fix:** New `SyncWake` type wrapping `Arc<SyncGate>` with coalescing dirty flag + `dispatch()`; watcher calls `request_sync` only, never sync. Matches the plan's contract exactly.

**2. [Rule 3 - Blocking] No `SyncWorker::start` exists — spawn via `idle_start` command**
- **Issue:** Plan assumed a `SyncWorker::start()` spawn point; sync runs per-`start_sync` command through the manager lease.
- **Fix:** `spawn_if_allowed` + `idle_start` command (UI hook starts best-effort on mount); `on_wake` callback emits `idle-wakeup` so the existing poll entry drains through the gate. 29-03 wires the frontend side.

**3. [Rule 3 - Blocking] `connect_sync` lives in `imap::session`, manager has no clone helper**
- **Fix:** Watcher clones `AccountConfig` and calls `connect_sync` directly per cycle (dedicated connection, structural isolation from the manager session).

**4. [Rule 1 - Bug] StopSource drop cancels the IDLE wait**
- **Found during:** Implementation (stop-token docs: token completes when source drops).
- **Fix:** `_stop_src` binding held across the wait future; stop races via `futures::select!` against a 1s flag poll, DONE sent before reporting Closed. Borrow-checker restructure (take-handle-out + free `wait_owned`/`release_handle` fns).

## Auth Gates

None.

## Known Stubs

None. The live network path (`LiveIdleDriver`) compiles but is exercised only behind the operator live-gate (SKIPs offline); all decision logic is transcript-tested.

## Threat Flags

None — T-29-05 (read-only verbs, structural test), T-29-06 (backoff cap + coalescing), T-29-08 (no direct sync path — verified by construction: watcher holds no manager/worker reference).

## Self-Check: PASSED

- `src-tauri/src/imap/idle.rs` exists; `idle` module registered; commands registered in `lib.rs` handler.
- Commit bbeadd2 exists.
- `cargo test -p sge --lib -- imap::idle` → 10 passed; `sync::` → all pass.
