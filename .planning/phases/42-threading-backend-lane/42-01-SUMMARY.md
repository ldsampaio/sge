---
phase: 42-threading-backend-lane
plan: 01
subsystem: database
tags: [sqlite, rusqlite-migration, imap-threading, alter-add-column, schema-version-bump]

requires:
  - phase: 42-threading-backend-lane (42-CONTEXT.md)
    provides: locked decisions D-01..D-05 (per-folder done-bit, NULL-sentinel semantics)

provides:
  - "M26 forward migration: nullable messages columns in_reply_to / references_json / thread_id + mailboxes.refs_backfilled_at done-bit + idx_messages_thread(mailbox_id, thread_id) index"
  - "SCHEMA_VERSION 26 with renamed gate test schema_version_is_26_with_m26_threading and fresh/upgrade fixture pair"
  - "UPSERT_MESSAGE_SQL + upsert_message carrying in_reply_to/references_json (13 params, NULL-stays-NULL routing)"
  - "InsertMessage extended with the two threading fields (batch path wired, no construction sites yet)"

affects: [42-02, 42-03, 42-04, 42-05, 42-06, 42-07]

actuals:
  tokens: 6430
  tasks: 2
  commits: 2
  plan_head_before: eb0eefe5c09b584e4cfaaf38a34626c1850d3788

tech-stack:
  added: []
  patterns:
    - "ALTER-only forward migrations (M4 :260 precedent) — never copy CREATE TABLE of baseline tables into a migration"
    - "Truncated-baseline upgrade fixture (migrations_through_m25() + production apply_migrations + fingerprint comparison) cloned from m23_llm_settings_gate_tests"
    - "Single-writer upsert SQL: one UPSERT_MESSAGE_SQL constant shared by upsert_message and upsert_messages_batch; arity parity enforced by compile+test"

key-files:
  created: []
  modified:
    - src-tauri/src/store/mod.rs
    - src-tauri/src/store/queries.rs
    - src-tauri/src/sync/worker.rs
    - src-tauri/src/imap/bodies.rs
    - src-tauri/src/classify/batch.rs
    - src-tauri/src/classify/filing.rs
    - src-tauri/src/classify/archive.rs
    - src-tauri/src/classify/worker.rs
    - src-tauri/src/commands/sync.rs
    - src-tauri/src/commands/classify.rs
    - src-tauri/tests/live_gate_26_safety.rs
    - src-tauri/tests/live_gate_22_safety.rs

key-decisions:
  - "mailboxes done-bit modeled as ALTER column refs_backfilled_at TEXT (NULL = folder backfill not done), not state.json (D-01)"
  - "thread index keyed (mailbox_id, thread_id) — bare thread_id never a grouping key (D-04/THR-04)"
  - "ON CONFLICT routes both new columns via plain excluded.field — NULL never coalesced to empty-string chain sentinel (D-04)"
  - "All 48 pre-existing upsert call sites pass None,None (threading not yet computed); the compute lane in 42-02 populates them"

requirements-completed: [THR-01, THR-04]

coverage:
  - id: D1
    description: "M26 columns + idx_messages_thread + refs_backfilled_at present on fresh install at SCHEMA_VERSION 26"
    requirement: THR-01
    verification:
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#store::m26_threading_gate_tests::m26_fresh_install_adds_thread_columns"
        status: pass
    human_judgment: false
  - id: D2
    description: "v25 database with seeded rows upgrades to 26 byte-identical per prior row; new columns NULL; done-bit NULL"
    requirement: THR-01
    verification:
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#store::m26_threading_gate_tests::m26_upgrade_from_v25_preserves_rows"
        status: pass
    human_judgment: false
  - id: D3
    description: "Upsert path persists in_reply_to/references_json with NULL-stays-NULL conflict routing"
    requirement: THR-04
    verification:
      - kind: unit
        ref: "src-tauri/src/store/queries.rs#upsert filter: 5 passed"
        status: pass
    human_judgment: false
  - id: D4
    description: "Backfill resumability (interrupted per-folder backfill resumes from rows lacking references_json) — structural design guarantee"
    requirement: THR-04
    verification: []
    human_judgment: true
    rationale: "Plan 42-01 assigns this as a backstop; proven later in plan 42-05 via the mock session — not verifiable at this commit"

metrics:
  duration: 23min
  completed: 2026-10-20
status: complete
---

# Phase 42 Plan 01: M26 Threading Storage Base + Upsert Growth Summary

**Nullable thread-identity columns (`in_reply_to`, `references_json`, `thread_id`) landed via ALTER-only M26 migration with per-folder index `idx_messages_thread`, `mailboxes.refs_backfilled_at` done-bit, SCHEMA_VERSION 25→26 with fresh/upgrade fixture pair, and the 13-param upsert path carrying both header fields.**

## Performance

- **Duration:** ~23 min (eb0eefe → 0c966b9 commit span)
- **Started:** 2026-10-20T10:30:06-03:00 (plan-boundary commit)
- **Completed:** 2026-10-20T10:52:45-03:00
- **Tasks:** 2/2
- **Files modified:** 12 (plan-listed 2 + 10 mechanical-propagation files)

## Task Commits

Each task was committed atomically:

1. **Task 1: M26 migration + SCHEMA_VERSION bump + fresh/upgrade fixture pair** — `a0c4375` (feat)
2. **Task 2: Extend upsert path to carry in_reply_to and references_json** — `0c966b9` (feat)

**Plan metadata:** pending (docs commit below, separate).

## Files Created/Modified

- `src-tauri/src/store/mod.rs` — M26_THREADING_SQL const (5 ALTER/INDEX lines, zero CREATE TABLE), vec append, SCHEMA_VERSION 26, renamed gate test, m26_threading_gate_tests module (migrations_through_m25() helper + fingerprint helpers + fresh/upgrade fixture pair)
- `src-tauri/src/store/queries.rs` — UPSERT_MESSAGE_SQL 13 columns/params + DO UPDATE SET routing, upsert_message +2 Option<&str> params, InsertMessage +2 fields, batch params wired
- `src-tauri/src/sync/worker.rs`, `src-tauri/src/imap/bodies.rs`, `src-tauri/src/classify/*.rs`, `src-tauri/src/commands/*.rs`, `src-tauri/tests/live_gate_{22,26}_safety.rs` — mechanical `None, None` additions at 48 upsert_message call sites

## Decisions Made

- Locked D-01..D-05 honored as given (no re-decisions): done-bit as ALTER column on mailboxes; NULL `thread_id` = own-thread sentinel; index keyed `(mailbox_id, thread_id)`; NULL never coalesced in conflict routing.
- `refs_backfilled_at` typed TEXT (not INTEGER) per plan action text — NULL = not done; timestamp semantics reserved for the 42-05 epoch writer.
- Fixture byte-identity measured via baseline-column fingerprint SQL (v1 columns only), cloning the m23 gate-test shape — deterministic without rusqlite Value plumbing.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] upsert_message signature growth rippled to 48 call sites outside the plan's files_modified list**
- **Found during:** Task 2
- **Issue:** The plan mandates extending `upsert_message` params (and the shared `UPSERT_MESSAGE_SQL`), but Rust arity changes break every caller; 48 call sites across 10 additional files (classify/*, commands/*, sync/worker.rs, imap/bodies.rs, two live-gate integration suites) would not compile otherwise.
- **Fix:** Scripted paren-balanced insertion of `None, None,` (threading not yet computed — NULL semantics) at all 48 sites; `upsert_messages_batch` and `InsertMessage` wired through so the batch path keeps 13-param parity with the SQL.
- **Verification:** Full `cargo test -p sge` green (532 lib passed / 2 credential-gated ignored; all integration suites green including live_gate_22/26 safety at 19/48 passed).
- **Committed in:** `0c966b9` (part of task commit)

**Total deviations:** 1 auto-fixed (Rule 3 mechanical propagation — required for compilation; prohibited files `imap/references.rs` and `imap/mod.rs` untouched; `list_messages` payload untouched; schema.sql byte-unchanged).
**Impact on plan:** No scope creep — propagation is the mechanical consequence of the plan's mandated signature change.

## Issues Encountered

- Transient mid-session script error (non-idempotent re-run doubled `None, None` insertions): recovered by reverting the 11 affected files to HEAD and re-running the corrected one-pass script. No bad state shipped.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- 42-02 (References lane) can populate `in_reply_to`/`references_json` at the now-13-param call sites; `InsertMessage` is ready for construction sites.
- 42-04/05 (epoch/compute) have `thread_id` + `idx_messages_thread` + `refs_backfilled_at` substrate in place.
- 42-06 payload byte-compat has the additive-only list contract untouched.
- Deferred legs (unchanged from baseline): live-server IMAP legs need credentials — 42-07's job; not claimed here.

## Self-Check: PASSED
- a0c4375 / 0c966b9 present in git log
- SUMMARY present on disk
- 12 files changed across plan commits (matches Files/Modified list)

---
*Phase: 42-threading-backend-lane*
*Completed: 2026-10-20*
