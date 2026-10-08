---
phase: 13-send-pipeline
plan: 13-01
subsystem: send-queue
tags: [sqlite, rusqlite-migration, send-queue, exactly-once, backoff, mime]
requires:
  - phase: 12-drafts
    provides: M9 drafts table + new_message_id + RFC5322 renderer + preserve-rows pattern
provides:
  - M10 `send_queue` table + queue queries (enqueue/get/due-list/state/attempt/reset/counts)
  - `send_queue.rs` state machine (SendGate, backoff, dedupe, crash recovery, frozen contract)
affects: [13-02 (SMTP transport + uncertain reconcile), 13-03 (queue_send/retry_send/send_status commands), 14-compose (queued-send consumer)]
tech-stack:
  added: []
  patterns: [forward-only-migration-with-preserve-rows-test, message-id-unique-dedupe, crash-reset-before-flush, capped-jittered-backoff, envelope-only-bcc]
key-files:
  created:
    - src-tauri/src/send_queue.rs
  modified:
    - src-tauri/src/store/mod.rs
    - src-tauri/src/store/queries.rs
    - src-tauri/src/lib.rs
key-decisions:
  - "Envelope columns named from_addr/to_addrs/cc_addrs/bcc_addrs (messages-table convention) — `from` is a SQL keyword"
  - "retry bumps attempts via record_send_attempt (schedule kept for display); requeue_failed clears schedule to due-immediately"
  - "sent_unfiled frozen in send_status shape as 0 until Plan 13-02 owns the APPEND leg"
  - "No new dependencies (T-13-SC): clock-xorshift jitter instead of rand; mail-parser/chrono/serde already pinned"
decisions: []
metrics:
  duration: ~40min
  completed: 2026-10-08
status: complete
actuals:
  tokens: 12613
  tasks: 3
  commits: 3
  plan_head_before: d9787e0
commits:
  - 7f8d2e0
  - 8357f6d
  - e23b8c5
requirements-completed: [SEND-04]
coverage:
  - id: SEND-04
    description: "Outgoing mail queued offline survives restart and never duplicates on double-invoke"
    verification:
      - kind: unit
        ref: "src-tauri/src/send_queue.rs#double_enqueue_same_message_id_returns_existing_row"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#m10_adds_send_queue_preserving_rows"
        status: pass
    human_judgment: false
---

# Phase 13 Plan 01: Send-queue tracer Summary

**Durable send_queue store + enqueue/dedupe path + unit-testable state machine, no network I/O — 239 backend tests green**

## Performance

- **Duration:** ~40min wall
- **Started:** 2026-10-08
- **Completed:** 2026-10-08
- **Tasks:** 3 / 3
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments

- M10 `send_queue` table (13 columns, state CHECK, indexes on state + next_retry_at) migrates forward from v9 preserving mailbox/message/flag-outbox/draft rows; `SCHEMA_VERSION` 9→10
- Queue queries in `store/queries.rs`: `enqueue_send_row`, `get_send_row`, `get_send_row_by_message_id`, `list_due_sends`, `set_send_state`, `record_send_attempt`, `reset_sending_to_queued`, `send_state_counts`
- `send_queue.rs`: `SendGate` (separate from SyncGate, never holds IMAP session), `enqueue_send` (validate → Message-ID resolve → dedupe → render → 25 MB cap → immutable `.eml` → insert row), backoff (30 s base, 15 min cap, ±20 % jitter), `record_send_failure` / `mark_send_failed` / `requeue_failed` / `crash_recover`, `send_status_snapshot`
- Frozen command contract documented verbatim for Plan 13-03: `queue_send` / `retry_send` / `send_status` shapes + `send-too-large` / `send-no-recipient` / `send-missing` prefixes
- Full backend suite green: **239 passed, 0 failed, 1 ignored** (226 pre-existing + 13 new)

## Task Commits

1. **Task 1: End-to-end enqueue path** - `7f8d2e0` (feat: M10 migration + queries + send_queue module + lib registration)
2. **Task 2: State machine + dedupe + crash recovery tests** - `8357f6d` (test: 7 behavior tests incl. BCC stripping, injection, oversize, backoff bounds)
3. **Task 3: Full suite + frozen contract** - `e23b8c5` (docs: contract comments + snapshot helper/shape test; suite 239 green)

## Files Created/Modified

- `src-tauri/src/send_queue.rs` (created) — state machine, SendGate, enqueue path, backoff, crash recovery, contract docs, 12 unit tests
- `src-tauri/src/store/mod.rs` (modified) — `M10_SEND_QUEUE_SQL`, migration wiring, version test update, preserve-rows test
- `src-tauri/src/store/queries.rs` (modified) — `SendRow`, 8 queue query functions, `SendCounts`
- `src-tauri/src/lib.rs` (modified) — `pub mod send_queue` registration

## Decisions Made

- Envelope columns follow the messages-table convention (`from_addr`/`to_addrs`/…) — `from` would be a SQL keyword
- Manual retry bumps `attempts` (schedule retained for the failed-retry surface) but clears `next_retry_at` so the row is due immediately
- `sent_unfiled` is frozen in the `send_status()` shape now but reads 0 until Plan 13-02 builds the APPEND leg
- Jitter uses a clock-seeded xorshift — no new `rand` dependency per T-13-SC

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Test range moved `lo`/`hi` Strings (E0382)**
- **Found during:** Task 2 (first `send_queue` test compile)
- **Issue:** `(lo..=hi).contains(&scheduled)` moved both Strings, then the assert message borrowed them
- **Fix:** Replaced with `scheduled >= lo && scheduled <= hi` reference comparison
- **Files modified:** `src-tauri/src/send_queue.rs` (test only)
- **Commit:** `8357f6d`

**Total deviations:** 1 auto-fixed (test-only compile error, no behavior impact)

## Issues Encountered

- System pacman `rustc` is ABI-broken (same as Phase 12); used the user-space rustup toolchain (`~/.cargo/bin`) for all verification — environment only, no project change
- HEAD is on `main`, but `.planning/config.json` sets `git.allow_default_branch_commits: true`, so per-task commits on main are the permitted path here
- Pre-existing `.planning/STATE.md` modification (from discuss) left untouched per dispatch instructions

## Known Stubs

None introduced. (`sent_unfiled: 0` in the snapshot is an explicit forward-declared zero for Plan 13-02, not a stub — documented in code and contract.)

## Threat Flags

None — no new network surface (no SMTP, no new dependencies; `Cargo.toml`/`Cargo.lock` untouched). Threat-model mitigations verified: T-13-01 (forward-only M10, preserve-rows test green), T-13-02 (injection round-trip test green), T-13-03 (oversize refusal leaves no row/file), T-13-04 (UNIQUE dedupe + immutable bytes + race-reconcile path), T-13-SC (zero dependency delta confirmed by diff).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Plans 13-02/13-03 build on: `enqueue_send` (dedupe proven), `list_due_sends` + `SendGate` (flush pass skeleton), `crash_recover` (must run before any flush), `record_send_failure`/`mark_send_failed` (transport verdicts), `send_status_snapshot` (exact `send_status()` shape)
- Known limitation for 13-02: the `state` CHECK lacks `sent_unfiled` (APPEND-failure state) — extending it needs a table rebuild or CHECK-drop migration; 13-02 owns that decision

## Self-Check: PASSED

All 5 files found on disk; all 3 task commits (`7f8d2e0`, `8357f6d`, `e23b8c5`) present in history. Full suite re-verified at Task 3: 239 passed, 0 failed.
