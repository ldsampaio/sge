---
phase: 26-live-validation-bundle-live-03-sandboxed
plan: "01"
subsystem: testing
tags: [live-gate, sandbox, imap, offline-doubles, LIVE-03]

# Dependency graph
requires:
  - phase: 21-smtp-send-live-gate
    provides: STEP-runner + exit-contract + wrapper-gate shape cloned for live_gate_26
  - phase: 22-draft-seen-round-trip-live-gate
    provides: GateSession decorator, ReplaySummary/pending-depth predicates, secret-scan precedent
provides:
  - live_gate_26 bin proving v1.1 read-side deferred gates 6/7/8 inside one shared sandbox
  - scripts/live-26.sh wrapper with scan/PEEK/order/SCAN-DB/REPORT gates
  - 24 offline doubles + 7 in-bin unit tests, exit-2 SKIP without creds
affects: [26-02 (v1.3 legs + 21/22 citation expansion), gsd-verify-work 6/7/8]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 23700
  tasks: 3
  commits: 1

# Tech tracking
tech-stack:
  added: []
  patterns: [STEP-runner live-gate clone, per-leg REPORT rows, in-bin SCAN-DB dump scan, mod-grouped safety doubles]

key-files:
  created:
    - src-tauri/src/bin/live_gate_26.rs
    - src-tauri/tests/live_gate_26_safety.rs
    - scripts/live-26.sh
  modified:
    - src-tauri/Cargo.toml

key-decisions:
  - "Single feat commit for all three tasks: the plan's file sets overlap fully (one-file harness), so atomic-per-task commits would misattribute; verified per-task instead"
  - "In-bin SCAN-DB scans passwords only (usernames excluded: temp Store holds self addresses, would false-positive); wrapper keeps the full user+pass grep per 22 precedent"
  - "Gate-7 badge assert is cached-STATUS == server-STATUS (deterministic); local unread_count reported as evidence, not hard-failed (server eventual-consistency)"
  - "Gate-8a arrival uses the raw session APPEND path (second logical sender path, still self-only) while gate-6 uses save_draft_copy_in — distinct prod verbs, no duplication"

patterns-established:
  - "Per-leg REPORT gate=verdict=evidence rows alongside STEP lines (D-46 incomplete-run detection)"
  - "UIDVALIDITY epoch re-check at every leg entry with loud abort on generation change"

requirements-completed: [LIVE-03]

# Coverage metadata (#1602) — one entry per shipped deliverable.
coverage:
  - id: D1
    description: "live_gate_26 bin compiles as [[bin]]; missing env file exits 2 SKIP with zero wire traffic"
    requirement: "LIVE-03"
    verification:
      - kind: unit
        ref: "cargo test --manifest-path src-tauri/Cargo.toml --bin live_gate_26 (7 passed)"
        status: pass
      - kind: manual_procedural
        ref: "cargo run --bin live_gate_26 with src-tauri/.live-test.env absent → SKIP exit 2"
        status: pass
    human_judgment: false
  - id: D2
    description: "Gates 6/7/8 live legs implemented with STEP verdict lines + REPORT rows (Seen round-trip, tree/browse/badges, arrival tick + reconnect)"
    requirement: "LIVE-03"
    verification:
      - kind: unit
        ref: "src-tauri/tests/live_gate_26_safety seen/tree/arrival leg-shape doubles (24 passed)"
        status: pass
    human_judgment: true
    rationale: "Live STEPs execute only with human-supplied creds via scripts/live-26.sh; server-side asserts (FLAGS verify, LIST compare, tick pickup) need a human-witnessed live run"
  - id: D3
    description: "Offline doubles suite green with no creds and no network; wrapper passes bash -n with scan/PEEK/order gates wired to FAIL"
    requirement: "LIVE-03"
    verification:
      - kind: unit
        ref: "cargo test --manifest-path src-tauri/Cargo.toml --test live_gate_26_safety (24 passed)"
        status: pass
      - kind: other
        ref: "bash -n scripts/live-26.sh + grep for SECRET-LEAK/PEEK-FAIL/ORDER-FAIL/SCAN-DB/REPORT-FAIL gates"
        status: pass
    human_judgment: false

# Metrics
duration: 7min
completed: 2026-10-13
status: complete
plan_head_before: 24b25736e9d2525c435e36c2125dcf3ba8646c18
plan_head_after: b16394a
---

# Phase 26 Plan 01: Live-Gate Tracer for Read-Side Gates Summary

**Sandboxed live_gate_26 harness proving v1.1 deferred gates 6/7/8 (Seen round-trip, folder tree/browse/badges, arrival tick + reconnect) inside one shared SGE-Test sandbox, with 24 offline doubles green and exit-2 SKIP without creds**

## Performance

- **Duration:** 7 min
- **Started:** 2026-10-13T21:16:30Z
- **Completed:** 2026-10-13T21:23:12Z
- **Tasks:** 3
- **Files modified:** 4

## Accomplishments

- `live_gate_26` bin (STEP 1–9, exit 0/1/2, `--seed-only`/`--cleanup-only`, `--password` refusal) driving existing manager/lease entry points only — zero new prod IMAP verbs
- Gates 6/7/8 legs each emit mandatory `STEP n/9` verdict + `REPORT gate= verdict= evidence=` row; cleanup emits in-bin `SCAN-DB` line + `DUMP` path + final REPORT
- `scripts/live-26.sh` wrapper: env-file sourcing, keyring seed-once, secret scan (log+dump), PEEK gate, replay-before-sweep order check, SCAN-DB presence check, REPORT-row check, 3× cleanup retry, SCAN-VERDICT tail
- Offline suite green with no creds/network: 7 in-bin unit tests + 24 safety doubles in `mod live_gate_26::{seen,tree,arrival,safety,cleanup}`
- Live run pending human creds (`src-tauri/.live-test.env` absent; never created by agent); no creds file exists

## Task Commits

All three tasks landed in one tracer commit (the plan's task file sets overlap fully — single-file harness; see Decisions):

1. **Task 1: Harness skeleton + wrapper + sandbox lifecycle (offline green)** — `b16394a` (feat)
2. **Task 2: Gates 6/7/8 read-side live legs** — `b16394a` (feat, same commit; verified via `... --test live_gate_26_safety seen` 4/4 + `--bin live_gate_26` 7/7)
3. **Task 3: Offline doubles suite + wrapper safety gates** — `b16394a` (feat, same commit; verified via full safety suite 24/24 + `bash -n`)

**Plan metadata:** this SUMMARY commit (docs, follows)

## Files Created/Modified

- `src-tauri/src/bin/live_gate_26.rs` (new, ~62KB) — STEP-runner, sandbox lifecycle, gates 6/7/8 legs, GateSession decorator, SCAN-DB dump scan, cleanup/report, 7 unit tests
- `src-tauri/tests/live_gate_26_safety.rs` (new, ~26KB) — FakeSession doubles in 5 mod groups, 24 tests
- `scripts/live-26.sh` (new, executable) — wrapper with scan/PEEK/order/SCAN-DB/REPORT/cleanup-retry gates
- `src-tauri/Cargo.toml` (modified) — `[[bin]] live_gate_26` registration mirroring 21/22 entries

## Decisions Made

- Single `feat(26-01)` commit for all three tasks: every task's `<files>` names the same four files, so per-task commits would slice one new-file harness arbitrarily and misattribute. Verified per-task (Task 1: bin tests + SKIP; Task 2: `seen` filter + bin; Task 3: full suite + `bash -n`) instead.
- In-bin SCAN-DB scans the password substring only; usernames excluded because the temp Store legitimately holds self addresses (false-positive risk). Documented in code; wrapper keeps the full credential grep per 22 precedent.
- Gate-7 badge hard-assert is STATUS-UNSEEN == locally cached datum (deterministic, same STATUS response); local `unread_count` derivation reported as evidence only, to avoid flaky live FAILs from server eventual consistency.
- Kept the 21/22 verbatim `starts_with` prefix guard (bare `SGE-Test-` passes) instead of hardening it: the plan mandates verbatim D-36 guards; LIST snapshot + delta checks own that case.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Test over-strict vs verbatim prefix guard**
- **Found during:** Task 3 (offline doubles verification — 23/24 green, 1 failure)
- **Issue:** New test asserted `assert_sandbox_prefix("SGE-Test-")` is refused, but the D-36 verbatim `starts_with` guard (same as live_gate_21/22) accepts the bare prefix
- **Fix:** Aligned the test with the contract (bare prefix accepted, `SGE-Test` without dash refused), with a comment citing the verbatim-guard rationale
- **Files modified:** src-tauri/tests/live_gate_26_safety.rs
- **Verification:** Full suite 24/24 green on re-run
- **Committed in:** b16394a (part of task commit)

---

**Total deviations:** 1 auto-fixed (1 bug — test/contract alignment, no prod impact)
**Impact on plan:** None on scope; guard semantics unchanged from the locked 21/22 contract.

## Issues Encountered

- **Sibling executor's transient breakage:** mid-execution, `cargo test` failed to compile the lib due to a type error (`&v` vs `v` in `.contains()`) in uncommitted Phase-24 WIP inside `src-tauri/src/store/queries.rs` (shared working tree, no worktree isolation). Per dispatch rules the file was not touched; after a 90s wait the sibling had fixed it and all verifies passed. No action taken, no files affected.
- **Concurrent cargo lock contention** ("Blocking waiting for file lock") from the parallel executor — cargo's own locking handled it; commands retried cleanly.

## User Setup Required

**Live run needs human-supplied credentials (standing Phase 26 user_setup).** To run the live gates:

1. Copy `src-tauri/.live-test.env.example` to `src-tauri/.live-test.env` (git-ignored, never committed) and fill in `SGE_LIVE_IMAP_USER`, `SGE_LIVE_IMAP_PASS`, `SGE_LIVE_SMTP_PASS`, `SGE_LIVE_SELF`
2. `scripts/live-26.sh --seed-only` (seeds OS keyring once), then `scripts/live-26.sh`
3. Expect STEPs 1–9 PASS plus per-gate REPORT rows; log + dump stay in `/tmp` only

## Next Phase Readiness

- Tracer complete: 26-02 can expand this bin shape with the v1.3 legs (confirm→MOVE→undo, RENAME/DELETE, batch/chunk probe) and the 21/22 citation rows per D-31
- Live `scripts/live-26.sh` run pending human creds — gates 6/7/8 server-side asserts unwitnessed until then
- No blockers; no stubs; threat register T-26-01..T-26-06 + T-26-SC all mitigated in code (no new dependencies)

---
*Phase: 26-live-validation-bundle-live-03-sandboxed*
*Completed: 2026-10-13*

## Self-Check: PASSED
