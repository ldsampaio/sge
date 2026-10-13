---
phase: 21-smtp-send-live-gate-live-01-sandboxed
plan: "02"
subsystem: live-gate-smtp-send
tags: [live-gate, smtp, expansion, sandboxed, LIVE-01, verification-only, failure-modes, draft-send, secret-scan, send-audit, cleanup, report]
requires: ["phase-13-send-pipeline", "phase-22-harness-pattern", "21-01-tracer"]
provides: ["failure-mode-legs-7-8-9", "draft-send-binary-verdict", "send-audit-five-rules", "secret-scan-db-dump", "cleanup-retry-manual", "closing-report-block"]
affects: ["21-VALIDATION", "human-live-run"]
tech-stack:
  added: []
  patterns: ["timeout-transport-fault-decorator", "reconcile-not-resend-call-count-witness", "crash-claim-triage-uncertain", "binary-draft-verdict", "in-bin-db-dump-scan", "wrapper-re-verification", "retry-then-manual-cleanup", "finally-path-report"]
key-files:
  created: []
  modified:
    - src-tauri/src/bin/live_gate_21.rs
    - scripts/live-21.sh
    - src-tauri/tests/live_gate_21_safety.rs
decisions:
  - "Draft-send STEP verdict is binary on the AUTO path with zero harness-side discard: any orphan (local row present or server SEARCH hit) FAILs BY DESIGN and routes to gap closure — never a PASS with a GAP footnote (per D-16, RESEARCH A6 re-verified at execution: no delete_draft call exists in flush_one_row/flush_send_queue, only the draft_id Message-ID-reuse arm in enqueue_send)"
  - "Secret scan runs in-bin over a full SQLite dump BEFORE cleanup removes the temp DB (wrapper cannot dump a deleted file); the wrapper re-checks the SCAN-DB line plus the /tmp log — passwords travel as fixed-substring needles, never printed, WHICH artifact matched is named"
  - "Cleanup closes the loop the LIST snapshot cannot: post-cleanup re-SEARCH of every test Message-ID in INBOX + Sent must return zero hits; sandbox folder DELETE with 3x 5s/15s/45s retry, real-Drafts fallback keeps tracked-UID expunge only and is never DELETEd"
  - "REPORT prints on the finally path (inside cleanup) so a mid-run FAIL still ends with STEP outcomes, autosave branch, draft binary verdict + DRAFT-03 GAP note, call counts, and the A-01..A-09 checklist against evidence STEPs"
  - "A-05 asserted as uncertain post-flush-triage (flush-pass triage promotes stranded sending to uncertain first), superseding the UI-SPEC queued wording; A-07 via flush outcomes + AUDIT-CALLS given the snapshot sent_unfiled hard-code 0 (recorded gap)"
metrics:
  duration: "~45 min"
  completed: "2026-10-13"
commits: 2
plan_head_before: 18aa1f4
actuals:
  tokens: 14155
  tasks: 3
  commits: 2
status: complete
---

# Phase 21 Plan 02: Failure Modes + Draft-Send + Safety Gates + Report Summary

Expansion: prove the failure-mode legs (offline flush with visible backoff, DATA-timeout uncertain reconcile, kill-mid-sending recovery), the DRAFT-03 draft-send no-orphan leg, and the safety gates (secret scan, send audit, cleanup) — then report. Verification-only over the 21-01 harness; no new capabilities, no new tables, no new transports, no new UI.

## One-liner

Failure-mode legs with call-count no-resend proof, binary draft-send verdict, five-rule send audit, in-bin full-DB secret scan, retry-then-manual cleanup with re-SEARCH, and a finally-path REPORT covering every STEP plus the A-01..A-09 checklist — all offline-green, ready for the human live run.

## Tasks Completed

| # | Name | Commit | Files |
|---|------|--------|-------|
| 1 | Offline flush + DATA-timeout uncertain + kill-mid-sending recovery (D-09/D-10/D-11) — completed by prior session, verified intact this session | 18aa1f4 | live_gate_21.rs, live_gate_21_safety.rs |
| 2 | Draft-send no-orphan + secret scan + no-unprefixed-send audit (D-16/D-13/D-14) | 0c6e7c6 | live_gate_21.rs, live-21.sh, live_gate_21_safety.rs |
| 3 | Cleanup with retry-then-manual + run report (D-15) | 95082b9 | live_gate_21.rs, live_gate_21_safety.rs |

## Resumed execution (prior executor interrupted mid-plan)

This session resumed with Task 1 DONE (commit `18aa1f4`) and partial Task-2 work uncommitted in the tree. Verified Task 1 intact first (`git show --stat 18aa1f4`; `retry` filter 6/6 green) — no re-implementation, no repair needed. The uncommitted partial work (draft-send leg, audit block, wrapper audit/scan stages, 4 audit/scan fixtures) compiled and its tests passed; two gaps remained and were completed this session: the in-bin `SCAN-DB` leg the wrapper already expected (helpers existed, no call site) and all of Task 3 (REPORT, cleanup hardening). Commits were split per task by reconstructing the Task-2 intermediate state (verified green) before committing, then restoring and committing Task 3.

## What Was Built

**Task 1 (prior session, verified this session)** — `TimeoutTransport` fault decorator (armed: `Uncertain`, zero inner calls), STEPs 7/8/9: gate-closed enqueue → reopen flush with backoff-bounds-visible schedule and exactly-once arrival; armed-flush uncertain hold + `reconcile_uncertain_sends` confirming with transport `send_calls` UNCHANGED (SEARCH-before-resend); kill-mid-sending claim → `crash_recover` → flush-pass triage to uncertain → reconcile-miss requeue → backoff-waited single send (failed stays 0). TOTAL_STEPS 8→12, canonical `log_step` outcome log with exit 0/1 contract, 34 safety + 17 bin tests green at handoff.

**Task 2 — draft-send + audit + scan (commit `0c6e7c6`)**:
- STEP 10 `draft-send`: sandbox-first APPEND (account-root CREATE attempt; real-Drafts fallback with tracked-UID cleanup, fallback recorded verbatim), `enqueue_send` with `draft_id` (Message-ID reuse asserted, `deduped == false`), queued `.eml` APPENDed as the server copy, production flush must deliver once, then the BINARY verdict — local `get_draft` gone AND draft-wire re-SEARCH zero hits AND `harness_discard_ran == false` (constant, no discard call exists on the path) → PASS; else FAIL BY DESIGN with the flush-outcome evidence in the detail line.
- Send audit (D-14, logged as STEP 10 `send-audit`): all five rules over temp queue rows + rendered bytes, printed as `AUDIT-ROW` / `AUDIT-EML` / `AUDIT-SUBJECT` / `AUDIT-MID` / `AUDIT-CALLS` lines (`AUDIT summary: PASS|FAIL`); any violation aborts to cleanup.
- Secret scan (D-13): `dump_store_for_scan` renders EVERY temp table; `scan_text_has_secret` fixed-substring match (regex metachars safe, empty secrets never match); the scan runs in-bin as STEP 12 `secret-scan` printing `SCAN-DB tables=N rows=M result=PASS|FAIL` (+ `SECRET-LEAK` naming the artifact, never the secret) and failing the run on any hit.
- `scripts/live-21.sh`: AUDIT stage re-verifies D-09 flush-done visibility, bin summary, recipients (`to==[self]`, cc/bcc empty), subjects, Bcc:-absence, MID uniqueness, call-count reconcile; SCAN stage greps the log for both password values plus the `SECRET-LEAK` marker and requires the in-bin `SCAN-DB … PASS`; secrets unset before exit; exit 2 SKIP / `--seed-only` paths skip both stages.
- Offline fixtures (4 safety tests): recipient rule (incl. case/whitespace tolerance, malformed JSON), subject-prefix + Bcc:-header-line vs body-mention/folded-continuation + MID-duplicate rules, planted credential substrings with regex metachars (partial passwords do NOT match), dump coverage of ALL temp tables.

**Task 3 — cleanup + REPORT (commit `95082b9`)**:
- Cleanup (D-15, T-21-05, always on the finally path): secret-scan leg first (DB still present), INBOX + Sent tracked-UID expunges (3x 5s/15s/45s via `expunge_tracked`), draft server-copy expunge via its own wire, sandbox-folder DELETE with the same backoff (real-Drafts fallback: never DELETE), post-cleanup re-SEARCH of every test Message-ID in INBOX + Sent (zero hits required), re-LIST equals pre-run snapshot; exhaustion prints exact manual steps (run tag, folders, Message-IDs, INBOX/Sent/draft UIDs, webmail path) and exits 1.
- REPORT (printed inside cleanup, even after mid-run FAIL): `REPORT` header with pass count, `REPORT-STEP` per outcome, `autosave-branch` (`probe_detail` naming deduped-in-Sent vs APPENDed-to-Sent + `REPORT-PROBE` per MID), draft-send binary verdict with the DRAFT-03 GAP note on FAIL, `REPORT-CALLS` per leg with expected deltas, and the `REPORT-CHECK` A-01..A-09 checklist against evidence STEPs (existing `OutboxBadgeWired` + `SendStatus` copy referenced verbatim, no new UI).
- Offline fixtures (2 safety tests): REPORT covers steps/calls/draft-verdict/GAP-note/all nine checklist IDs; manual steps name MIDs, UIDs, folders, sandbox, and the webmail path.

## Test Report

- `cargo test --test live_gate_21_safety retry` — 6/6 (Task 1 intact, incl. armed-uncertain, reconcile hit/miss, crash triage, backoff bounds).
- `cargo test --test live_gate_21_safety safety` — 6/6 (4 audit/scan fixtures + 2 report/manual fixtures).
- `cargo test --bin live_gate_21` — 19/19 (17 prior + report rendering + manual-steps naming).
- Full `cargo test --manifest-path src-tauri/Cargo.toml` — 328 lib + 19 bin + 42 live-21-safety (+ live-22 safety 10) green, zero failures; `bash -n scripts/live-21.sh` clean; `git ls-files --error-unmatch` confirms all three asserted paths git-tracked; logs/dumps confined to `/tmp`.
- No frontend changes — `tsc` surface untouched (pre-existing unrelated frontend dirt in the tree left alone).

## Pending human live run (NOT executed this session)

`scripts/live-21.sh` was syntax-checked only. The live gate (real sends to self under the sandbox contract, ending with cleanup + REPORT) is the plan's human step — creds live in the git-ignored `src-tauri/.live-test.env`, and live traffic is out of scope for the automated verifies. Code-level expectation for the human run, stated honestly per the binary-verdict rule: the draft-send STEP is expected to FAIL BY DESIGN (prod flush performs no `delete_draft`; RESEARCH A6 re-verified), routing to gap closure (implement `delete_draft`-in-flush as a fix, or explicit D-16 deferral with adjusted success criteria). Recorded as an `unrun-verify` entry in `.planning/WINDOWS.md` so the ship gate stays closed until the live run executes.

## Deviations from Plan

None — plan executed exactly as written. Execution notes (not plan changes):

1. **Resume, not rework:** prior session's uncommitted partial Task-2 work was adopted after verifying Task 1 intact and the partial work green; gaps (in-bin scan leg, all of Task 3) were completed this session. No plan content altered.
2. **Main-branch commits:** per `.planning/config.json` `git.allow_default_branch_commits: true` (WINDOWS ledger id 1, phase-6 convention: solo local repo, all prior phases on main) — the pre-commit protected-branch assertion passed via the configured override.
3. **Rule 2 visibility widening:** `TrackedMail` queue/snapshot fields (`message_ids`, `inbox_uids`, `sent_uids`, `unfiled`, `sent_wire`) made `pub` like the pre-existing `pub` tracker fields, so the offline harness tests can drive `render_report` / `manual_cleanup_steps` through the `#[path]`-included bin — test-only accessibility, zero prod-code change.

## Auth gates

None — all verifies run credential-free (offline doubles green, exit-2 SKIP contract intact; `.live-test.env` contents never read, printed, or echoed).

## Known Stubs

None — no placeholder values, TODOs, or unwired surfaces in this plan's scope. The draft-send FAIL-BY-DESIGN path is specified behavior (binary verdict), not a stub.

## Threat Flags

None — no new security surface beyond the plan's threat register: no new endpoints, auth paths, or schema changes (verification-only harness over the 21-01 surface). Credential values are held in-memory as scan needles only (`scan_secrets`, cloned from the keyring/env config) and never printed; T-21-01..T-21-05 mitigations implemented as specified.

## Self-Check: PASSED

- `src-tauri/src/bin/live_gate_21.rs`, `scripts/live-21.sh`, `src-tauri/tests/live_gate_21_safety.rs` — all FOUND and git-tracked.
- Commits `18aa1f4` (Task 1, prior session), `0c6e7c6` (Task 2), `95082b9` (Task 3) — all FOUND via `git log`.
- Measured: `git rev-list --count 18aa1f4..HEAD` = 2 (this session's commits); diff `18aa1f4..HEAD` over plan files = 56623 chars (~14155 tokens).
