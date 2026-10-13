---
phase: 22-draft-seen-round-trip-live-gate-live-02
plan: "02"
subsystem: live-gate-draft-seen
tags: [live-gate, replay, resurrection, seen-round-trip, secret-scan, cleanup, LIVE-02, verification-only]
requires:
  - phase: 22-01
    provides: live_gate_22-bin (4-STEP runner, exit 0/1/2, env-only creds) + GateSession decorator + one-copy helpers + 10 offline doubles
  - phase: phase-12-drafts
    provides: one-copy save (APPEND-new + expunge-old, draft_uid tracking)
  - phase: phase-06-flag-sync
    provides: replay_outbox / replay_imap_outbox entries, FakeSession/MockSession double patterns
provides:
  - live_gate_22 full 13-STEP run (STEPS 4-12: one-copy legs, replay-order, double-sync, Seen both ways, D-16 audit)
  - secret-scan + prefix-audit + order/peek wrapper gates with cleanup retry (--cleanup-only)
  - 19 offline safety doubles green with no creds (9 new in live_gate_22::{replay,resurrection,seen,safety,cleanup})
affects: [live-validation-bundle, 22-live-run-with-creds]
actuals:
  tokens: 16383
  tasks: 3
  commits: 2
tech-stack:
  added: []
  patterns: [pending-depth-structural-assert, transcript-order-predicate, fixed-substring-secret-scan, prefix-plus-self-audit, dump-before-cleanup, cleanup-only-retry-path]
key-files:
  created: []
  modified:
    - src-tauri/src/bin/live_gate_22.rs
    - src-tauri/tests/live_gate_22_safety.rs
    - scripts/live-22.sh
key-decisions:
  - "Full 13-STEP run including 22-01's deferred one-copy live legs (STEPS 4-8) so the verifier's STEP 1-13 contract holds end to end"
  - "Second sandbox {wire}-arc for the move target — no real folder is ever a replay destination"
  - "Live replay-order asserts end-state (depth 1+ to 0, UID moved not resurrected); log order asserted by the wrapper over the worker's own imap replay: lines"
  - "Bin --cleanup-only <sandbox> gives the wrapper an idempotent cleanup-retry path (rerunning the full bin would mint a new sandbox, never clean the old one)"
  - "FTS5 virtual tables excluded from the SQLite dump (SELECT * unreliable on them; they mirror messages with no credential surface)"
patterns-established:
  - "Offline toggle dual-shape: GateSession decorator for session-level legs, outbox-seed + sync-pass for reconnect legs — both hit the real replay entries"
  - "Module-grouped safety doubles (live_gate_22::{replay,resurrection,seen,safety,cleanup}) so plan verify filters match verbatim"
requirements-completed: [LIVE-02]
coverage:
  - id: D1
    description: "Offline doubles for replay-order, double-sync resurrection, and Seen both-directions reconcile"
    requirement: "LIVE-02"
    verification:
      - kind: unit
        ref: "src-tauri/tests/live_gate_22_safety.rs#live_gate_22::{replay,resurrection,seen} — 5/5 pass"
        status: pass
    human_judgment: false
  - id: D2
    description: "Secret-scan predicate and SGE-TEST- prefix/self-recipient audit predicate doubles"
    requirement: "LIVE-02"
    verification:
      - kind: unit
        ref: "src-tauri/tests/live_gate_22_safety.rs#live_gate_22::safety — 2/2 pass; SCAN-MECHANISM-OK canary check"
        status: pass
    human_judgment: false
  - id: D3
    description: "Cleanup backoff schedule and SQLite dump-writer doubles"
    requirement: "LIVE-02"
    verification:
      - kind: unit
        ref: "src-tauri/tests/live_gate_22_safety.rs#live_gate_22::cleanup — 2/2 pass"
        status: pass
    human_judgment: false
  - id: D4
    description: "Live STEPS 4-12 legs (one-copy, kill-reconcile, offline-edit, content, replay-order, double-sync, Seen, audit) through real worker entries"
    requirement: "LIVE-02"
    verification:
      - kind: other
        ref: "live_gate_22 binary exits 2 SKIP with no creds and zero IMAP traffic; wrapper exits 2 SKIP"
        status: pass
    human_judgment: true
    rationale: "Live legs only execute against mail.utfpr.edu.br with user-supplied git-ignored src-tauri/.live-test.env; run scripts/live-22.sh with creds to execute"
  - id: D5
    description: "Wrapper gates: log+dump secret scan, replay-order evidence, non-PEEK transcript gate, cleanup retry with manual-steps fallback"
    requirement: "LIVE-02"
    verification:
      - kind: other
        ref: "bash -n scripts/live-22.sh clean; grep manual-cleanup branches present in wrapper and bin"
        status: pass
    human_judgment: true
    rationale: "Full gate proof (SCAN-VERDICT PASS, LIST restored, sandbox deleted) requires a live run with creds"
duration: ~90 min
completed: 2026-10-13
status: complete
---

# Phase 22 Plan 02: Replay, Resurrection, Seen + Safety Gates Summary

Full 13-STEP live run (`live_gate_22` bin + `live-22.sh` wrapper): pre-sweep replay with pending-until-acked, double-sync no-resurrection, Seen round-trip both directions online and replayed, secret scan + prefix audit gates, dump-backed scan surface, retry-then-manual cleanup with run report — all green offline (19/19 doubles, no creds), live execution SKIP-clean pending user creds.

## Performance

- **Duration:** ~90 min
- **Tasks:** 3
- **Files modified:** 3
- **Commits:** 2 (measured `aef3acf..HEAD`)

## Accomplishments

- STEPS 4-8 live legs completing 22-01's deferred one-copy proof: append-1, one-copy supersede, kill-reconcile (lease-drop + max-wins), offline-edit (dirty-draft flush, latest wins), PEEK-only content assert — all via `save_draft_copy_in` / `sync_with_borrowed` / `fetch_body`, UIDs re-derived per step
- STEP 9 replay-order: move op into second sandbox `{wire}-arc`, pending depth 1+ → 0 across the real full pass, UID moved never resurrected same-pass; wrapper checks the worker's `imap replay:` line in the captured log
- STEP 10 delete-double-sync: real discard path then two full syncs, UID absent from SEARCH and local row gone both passes, arc torn down
- STEP 11 Seen both ways: `set_seen_in` online read/unread plus `replay_outbox` queued-and-acked both directions with epoch/absent drop rules armed, FLAGS asserted via `fetch_envelopes` only
- STEP 12 D-16 audit: in-run subjects + `drafts` table + `send_queue` addrs (strict JSON-cell parse, any row must be self-only)
- Wrapper: scan over log + SQLite dump for both passwords and username (`grep -F`, single read then unset, SECRET-LEAK names artifact), order-evidence and non-PEEK transcript gates, `--cleanup-only` retry on 5s/15s/45s before manual steps, SCAN-VERDICT report tail
- Bin: SQLite dump writer (pre-cleanup, `DUMP <path>` line), factored `delete_with_backoff`, `--cleanup-only` path, REPORT block; `TOTAL_STEPS` 4 → 13

## Task Commits

Each task's work was committed atomically (bin spans Tasks 1+3, wrapper Tasks 2+3 — mapping explicit):

1. **Task 1: Replay-order + double-sync resurrection + Seen both directions** - `51a34f8` (feat, with Task 3's bin paths: dump/backoff/`--cleanup-only`/REPORT)
2. **Task 2: Secret scan gate + SGE-TEST- prefix audit in wrapper** - `1de336b` (feat, with Task 3's wrapper retry + report tail)
3. **Task 3: Cleanup discipline + LIST restore + run report** - `51a34f8` + `1de336b` (bin paths and wrapper retry/report committed with Tasks 1-2; doubles in `live_gate_22::cleanup`)

**Plan metadata:** 22-02-PLAN.md (no separate docs commit — plan pre-existed)

## Files Created/Modified

- `src-tauri/src/bin/live_gate_22.rs` - STEPS 4-12 live legs, shared predicates (`pending_depth`, `replay_logged_before_sweep`, `scan_text_for_secrets`, `audit_test_mail`), dump writer, backoff + `--cleanup-only`, REPORT (+957 lines)
- `src-tauri/tests/live_gate_22_safety.rs` - 9 new doubles in `live_gate_22::{replay,resurrection,seen,safety,cleanup}` modules; FakeSession gains Seen tracking + move recording + MOVE capability (+370 lines)
- `scripts/live-22.sh` - secret scan (log+dump+username), order/PEEK gates, cleanup retry, report tail (+91/-9)

## Decisions Made

- Full 13-STEP run now (not just the three expansion steps): 22-01 left its one-copy live legs as helpers-only, and the plan verification demands STEP 1-13 PASS — expanding to STEPS 4-8 was required, not optional (Rule 2).
- `{wire}-arc` second sandbox as the move target: the imap_outbox move replay needs a destination and no real folder may ever be one; prefix guard holds, arc is created/torn down inside STEPS 9-10 with explicit LIST-delta allowance.
- Live order assert is end-state (depth + moved-location); transcript order asserted by the wrapper over real worker log lines — the bin cannot capture its own stderr lines in-process, and fabricating transcript fixtures in the live leg would be dishonest.
- `--cleanup-only` instead of wrapper bin-rerun: a rerun mints a new timestamped sandbox and abandons the old one; the targeted retry is the only idempotent shape.
- Doubles grouped in `mod live_gate_22::{...}` so the plan's `<verify>` filters (`live_gate_22::replay` etc.) match verbatim via `cargo test -- -- <filters>` (this toolchain's `cargo test` takes one positional filter; extras pass after `--`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] FakeSession lacked the MOVE capability, forcing the COPY fallback that errors on unknown UIDs**
- **Found during:** Task 1 (replay double)
- **Issue:** `replay_imap_outbox` capability-gates to UID MOVE only with `MOVE` advertised; the double offered `IMAP4rev1+UIDPLUS`, took the COPY→STORE→expunge dance, and `store_deleted` failed with "no such uid"
- **Fix:** Advertise `MOVE` in FakeSession capabilities (modern-server shape, matching the live target)
- **Files modified:** src-tauri/tests/live_gate_22_safety.rs
- **Verification:** `live_gate_22::replay` 2/2 pass
- **Committed in:** `51a34f8`

**2. [Rule 1 - Bug] SQLite dump writer choked on the FTS5 virtual table**
- **Found during:** Task 1 (dump double)
- **Issue:** `SELECT * FROM "messages_fts"` fails with "no such column: T.body_text" — FTS5 quirk, not a data problem
- **Fix:** Skip `VIRTUAL TABLE`s in the dump query + documented rationale (mirrors `messages`, no credential surface)
- **Files modified:** src-tauri/src/bin/live_gate_22.rs
- **Verification:** `live_gate_22::cleanup` 2/2 pass
- **Committed in:** `51a34f8`

**3. [Rule 1 - Bug] `mail_parser 0.11` `MessageParser::parse` returns `Option`, not `Result`**
- **Found during:** Task 1 (bin compile)
- **Issue:** `.map_err()` on `Option` — two call sites in the offline-edit/content legs
- **Fix:** `.ok_or_else()` with plain-language messages
- **Files modified:** src-tauri/src/bin/live_gate_22.rs
- **Verification:** `cargo check --bin live_gate_22` clean
- **Committed in:** `51a34f8`

**4. [Rule 1 - Bug] rusqlite `query_map` temporaries outliving `stmt` borrow**
- **Found during:** Task 1 (bin compile)
- **Issue:** Chained `query_map(...)?...collect()?` at block tail holds the borrow past `stmt` drop (E0597, five sites)
- **Fix:** Bind the mapped iterator to a local first, collect, return the local
- **Files modified:** src-tauri/src/bin/live_gate_22.rs
- **Verification:** `cargo check --bin live_gate_22` clean
- **Committed in:** `51a34f8`

**5. [Rule 2 - Missing critical] STEPS 4-8 live legs absent (22-01 shipped helpers only)**
- **Found during:** Task 1 (wiring the 13-STEP run)
- **Issue:** Plan verification requires a STEP 1-13 live run, but only STEPS 1-3 + cleanup existed as live legs; without 4-8 the run cannot prove LIVE-02's one-copy half
- **Fix:** Implemented append-1 / one-copy / kill-reconcile / offline-edit / content legs reusing the same worker entries (no new IMAP verbs, no new tables)
- **Files modified:** src-tauri/src/bin/live_gate_22.rs
- **Verification:** Bin builds; SKIP-clean without creds; legs reviewable, live proof pending creds (coverage D4)
- **Committed in:** `51a34f8`

**6. [Rule 1 - Bug] Wrapper cleanup-retry tested `tee`'s exit, not the bin's**
- **Found during:** Task 2 (wrapper review)
- **Issue:** `if cargo ... | tee -a` branches on the pipeline's last command (`tee`), masking `--cleanup-only` failures
- **Fix:** Capture `RETRY_CODE="${PIPESTATUS[0]}"` and branch on it; reset CODE=0 only when no STEP FAIL lines exist
- **Files modified:** scripts/live-22.sh
- **Verification:** `bash -n` clean; logic reviewed
- **Committed in:** `1de336b`

---

**Total deviations:** 6 auto-fixed (4 bugs, 1 missing-critical, 1 review-caught bug)
**Impact on plan:** All required for correctness or for the plan's own STEP 1-13 verification. No scope creep — no new tables, verbs, capabilities, or UI surface.

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: new-mutating-entry | src-tauri/src/bin/live_gate_22.rs (`--cleanup-only`) | New DELETE path beyond the plan's replay/discard helpers; mitigated — sandbox-prefix assert first, `--cleanup-only` refuses non-`SGE-Test-` names, same backoff + LIST-diff discipline as `cleanup` |

## Issues Encountered

- **Sibling-agent interference (out of scope, not fixed):** a parallel phase-23 agent committed `src-tauri/src/store/mod.rs` + `queries.rs` (M18, `aef3acf`) mid-plan; twice the lib failed to compile transiently (`M18_FILTERS_SQL` missing, then `FilterError` mismatch), blocking test runs for minutes. Left untouched per scope boundary; verified green before and after (19/19 at clean HEAD). Pre-commit HEAD check initially refused (`main` protected) but `.planning/config.json` sets `git.allow_default_branch_commits: true` and 22-01 committed on `main` — proceeded per the documented override.
- **Live run SKIPs (expected):** no `src-tauri/.live-test.env` in this environment — bin exits 2, wrapper exits 2, zero IMAP traffic. Never created a creds file.

## Known Stubs

None — no placeholder values, TODOs, or unwired surfaces in this plan's files. Live legs are complete implementations whose *execution* awaits creds (tracked as coverage D4/D5 with `human_judgment: true`, not stubs).

## User Setup Required

None - no external service configuration required. Live run (human): copy `src-tauri/.live-test.env.example` to `src-tauri/.live-test.env`, fill creds, run `bash scripts/live-22.sh` (test mail only to self, sandbox auto-cleaned, SECRET-LEAK/order/PEEK gates enforced, manual steps printed only if cleanup retries exhaust).

## Next Phase Readiness

- LIVE-02 expansion complete offline; the remaining step is the credentialed live run (human with the env file) asserting STEP 1-13 PASS, SCAN-VERDICT PASS, LIST restored, sandbox deleted.
- 22-01 + 22-02 together: `cargo test --test live_gate_22_safety` 19/19 green baseline for the live-validation bundle.
- Watch item: sibling phase-23 work shares `src-tauri/src/store/` — coordinate before the live run in case migrations shift the temp-Store surface.

---
*Phase: 22-draft-seen-round-trip-live-gate-live-02*
*Completed: 2026-10-13*

## Self-Check: PASSED
- SUMMARY file exists on disk; commits `51a34f8` + `1de336b` are ancestors of HEAD; no stub patterns in plan files (single "placeholder" hit is prose in Known Stubs).
