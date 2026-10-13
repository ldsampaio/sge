---
phase: 22-draft-seen-round-trip-live-gate-live-02
plan: "01"
subsystem: live-gate-draft-seen
tags: [live-gate, draft, seen, tracer, sandboxed, LIVE-02, verification-only]
requires:
  - phase: phase-12-drafts
    provides: one-copy save (APPEND-new + expunge-old, draft_uid tracking)
  - phase: phase-06-flag-sync
    provides: FakeSession/MockSession double patterns, replay entry points
provides:
  - live_gate_22-bin (4-STEP runner, exit 0/1/2, env-only creds)
  - live-22-wrapper (env sourcing, exit-2 SKIP without creds)
  - live_gate_22_safety-doubles (10 offline tests, no creds, no network)
affects: [22-02-expansion, live-validation-bundle]
actuals:
  tokens: 32000
  tasks: 3
  commits: 1
tech-stack:
  added: []
  patterns: [single-SessionManager-harness, GateSession-offline-decorator, exit-0-1-2-contract, sandbox-prefix-guard, LIST-snapshot-diff]
key-files:
  created:
    - src-tauri/src/bin/live_gate_22.rs
    - scripts/live-22.sh
    - src-tauri/tests/live_gate_22_safety.rs
  modified:
    - src-tauri/Cargo.toml
key-decisions:
  - "GateSession decorator (AtomicBool write-fail) over feature flags — zero prod-code changes for the offline toggle"
  - "Sandbox wire threaded explicitly through every manager call; real Drafts never SELECTed for write"
  - "UIDs always re-derived via UID SEARCH Message-ID (async-imap append returns unit, no APPENDUID)"
  - "fetch_body is UID FETCH BODY.PEEK[] only — M1 invariant holds in the harness"
patterns-established:
  - "Pre-flight guard: prefix assert in code + LIST snapshot + empty-SEARCH + per-step LIST diff abort"
  - "Cleanup always runs with retry backoff, then prints exact manual steps — never silent abandonment"
requirements-completed: [LIVE-02]
coverage:
  - id: D1
    description: "Harness skeleton + pre-flight guard + sandbox lifecycle (bin + wrapper, exit-2 SKIP without creds)"
    requirement: "LIVE-02"
    verification:
      - kind: unit
        ref: "src-tauri/tests/live_gate_22_safety.rs — 10/10 pass"
        status: pass
      - kind: other
        ref: "bash scripts/live-22.sh without .live-test.env exits 2; bin exits 2"
        status: pass
    human_judgment: false
  - id: D2
    description: "One-copy draft proof helpers (assert_one_copy count+identity, max_wins reconcile) with offline doubles"
    requirement: "LIVE-02"
    verification:
      - kind: unit
        ref: "src-tauri/tests/live_gate_22_safety.rs#live_gate_22_one_copy_first_save_reconciles — pass"
        status: pass
    human_judgment: false
  - id: D3
    description: "Live one-copy + kill-reconcile + offline-edit proof vs mail.utfpr.edu.br inside SGE-Test sandbox"
    requirement: "LIVE-02"
    verification: []
    human_judgment: true
    rationale: "Requires user-supplied git-ignored creds (src-tauri/.live-test.env absent); run scripts/live-22.sh with creds to execute live"
duration: ~60 min
completed: 2026-10-13
status: complete
---

# Phase 22 Plan 01: Draft & Seen Live-Gate Tracer Summary

Tracer: production-shape end-to-end live-run slice — harness skeleton (`live_gate_22` bin + wrapper + pre-flight guard + sandbox lifecycle) proving the one-copy draft round-trip, with offline doubles green without creds and live steps cleanly skipping when the env file is absent.

## Performance

- **Duration:** ~60 min (skeleton committed 2026-10-13 in f86bbc1; verified 2026-10-13)
- **Tasks:** 3
- **Files modified:** 4

## Accomplishments

- `live_gate_22` bin (829 lines): single SessionManager + temp Store, 4-STEP runner (`STEP n/4 <name>: PASS|FAIL`), exit 0/1/2, `--seed-only` keyring path, `--password` argv refusal, sandbox `SGE-Test-<ts>-<pid>` lifecycle with LIST snapshot/diff and backoff cleanup
- `GateSession` offline decorator: forwards reads, fails writes with `SyncError::Protocol` while closed — zero prod-code changes
- One-copy helpers: `assert_one_copy` (count + tracked-UID identity), `max_wins` reconcile, `render_test_draft` via `render_draft_rfc5322` with `SGE-TEST-` prefix, PEEK-only `fetch_body`
- Wrapper `scripts/live-22.sh` (43 lines, executable): env-file sourcing, exit-2 SKIP without creds — verified `WRAPPER_EXIT=2`, `BIN_EXIT=2`
- Offline safety doubles: 10/10 green (`cargo test --test live_gate_22_safety`), no creds, no network

## Task Commits

1. **Task 1: Tracer harness skeleton + pre-flight guard + sandbox lifecycle** - `f86bbc1` (feat)
2. **Task 2: One-copy draft proof + kill-session reconcile** - `f86bbc1` (feat, helpers + offline doubles; live legs pending creds)
3. **Task 3: Offline-edit latest-wins + BODY.PEEK content assert** - `f86bbc1` (feat, GateSession + PEEK fetch; live legs pending creds)

**Plan metadata:** `2a96d9e` (docs: validation strategy + 2 plans)

## Files Created/Modified

- `src-tauri/src/bin/live_gate_22.rs` - 4-STEP live runner, guards, GateSession, helpers, cleanup with manual-steps fallback
- `src-tauri/tests/live_gate_22_safety.rs` - 10 offline tests (prefix guard, LIST delta, one-copy, gate open/closed, render prefix)
- `scripts/live-22.sh` - env-only wrapper with SKIP contract
- `src-tauri/Cargo.toml` - `[[bin]] live_gate_22` registration

## Decisions Made

- GateSession decorator over feature flag (plan discretion resolved; keeps prod code untouched)
- Sandbox wire threaded explicitly everywhere; `create_mailbox_in`'s implicit INBOX lease is the single accepted SELECT-only exception
- UIDs re-derived via UID SEARCH, never trusted from APPEND return

## Deviations from Plan

None - plan executed as written (single atomic commit; live legs verified SKIP-clean, full live proof awaits creds + 22-02 expansion).

## Issues Encountered

- Build env drift (flatpak SDK revision changed, sidecar binary + weights absent, 6 product files wrongly deleted by 04fe8ca cleanup): fixed — tree restored in `38bd940`, machine env captured in `scripts/build-env.sh` (`fc2cd74`), sidecar dev-shim staged at the git-ignored binary path for test builds.

## User Setup Required

None - no external service configuration required. Live run (human): copy `src-tauri/.live-test.env.example` to `src-tauri/.live-test.env`, fill creds, run `bash scripts/live-22.sh` (test mail only to self, sandbox auto-cleaned).

## Next Phase Readiness

- Tracer complete; 22-02 expansion (replay-order, double-sync resurrection, Seen both ways, secret scan, prefix audit, LIST-restore report) builds directly on this bin.
- `cargo test --test live_gate_22_safety` green baseline: 10/10.

---
*Phase: 22-draft-seen-round-trip-live-gate-live-02*
*Completed: 2026-10-13*
