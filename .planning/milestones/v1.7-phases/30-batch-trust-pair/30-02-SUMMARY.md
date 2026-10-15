---
phase: 30-batch-trust-pair
plan: 02
subsystem: batch-retry-ui
tags: [batch, retry, exactly-once, preview-ui, single-flight]
dependency_graph:
  requires: [30-01-shared-preview]
  provides: [retry-failed-command, trust-pair-ui]
  affects: [phase-31-learning-dataset]
tech_stack:
  added: []
  patterns: [same-run-retry, run-level-single-flight, shared-preview-table]
key_files:
  created: [src/components/BatchPreviewTable.tsx]
  modified: [src-tauri/src/classify/batch.rs, src-tauri/src/commands/classify.rs, src-tauri/src/lib.rs, src/components/BatchPanel.tsx, src/components/ArchivePanel.tsx]
decisions:
  - "retry_failed returns an already-running RetryReport (skip) instead of an error for the second concurrent caller — double-click degrades to a status message, never a failure."
  - "Retry needs no sidecar: journal rows already carry to_folder + label_id, so retry re-issues MOVE verbs only. Archive vs classify verb chosen from the run's totals_json kind tag."
  - "Already-filed-but-unjournaled rows (verb fired, crash before journal_moved) are detected by current-folder == to_folder and recorded without a verb — crash-window exactly-once."
  - "Preview table shows the first 50 movable rows + first 20 skipped rows inline; totals carry the full counts (500-row cap bounds the IPC payload)."
metrics:
  duration: "~60 min"
  completed: 2026-10-14
  tasks: 3
  commits: 1
status: complete
---

# Phase 30 Plan 02: Retry-Only-Failures + Trust-Pair UI Summary

**One-liner:** Same-`run_id` retry over `moved=0` rows with exactly-once under double-click + burst-EXISTS stress, plus the shared preview table, [Run for real], retry-failed button, and stale/retry counts in BatchPanel and ArchivePanel.

## What Was Built

- **`retry_failed(run_id)` command** (`src-tauri/src/commands/classify.rs`, registered in `lib.rs`): entry gate via `check_retry_entry` (done/interrupted + ≥1 failed, plain-language refusals otherwise); run-level single-flight (`try_claim_retry` — second caller gets an `already-running` report); retry-start snapshot + per-row `resolve_for_move` re-resolution; already-at-destination rows journaled without verbs; fresh failures re-issued through `move_one_uid`/`archive_one` (existing SyncGate/outbox path, unchanged); `override_events` logged with `batch` action + `run_id` (Phase 31 dataset consistency); report accumulates on the original run (`totals_json` merges `retry_skipped`, state preserved, no new run row).
- **Retry stress tests** (`src-tauri/src/classify/batch.rs`, all headless over the real helpers): `retry_skips_moved` (zero verbs for `moved=1`, one MOVE per failed, no new run row), `double_click_retry_single_flight` (8-thread barrier race proves max concurrent holders == 1), `burst_exists_retry_skips_shifted` (shifted row skipped + counted, no verb for shifted UID, stays failed), `retry_entry_refuses_undone_and_empty` (undone/running/empty/unknown all refuse plainly), `live_retry_gate_skips_offline` (sandbox SKIP).
- **Trust-pair UI:** new shared `BatchPreviewTable.tsx` (from → to, basis, skipped-with-reason, remainder, truncated indicator, [Run for real] confirm). Both panels gain [Pré-visualizar] (invoke `preview_batch`/`preview_archive`), the shared table with confirm wired to the existing fresh-run commands (never replays rows), a `Repetir N falhas` button on done/interrupted runs with `failed > 0` (undone never offers retry), and stale-dropped + retry-skipped counts in the report. `tsc` clean.

## Tests

`cargo test --lib classify::batch::` 13/13 pass; full lib suite 434 passed, 0 failed; `cargo test --bins` compiles green (0 tests); `tsc --noEmit` clean.

## Deviations from Plan

None - plan executed exactly as written.

## Known Stubs

None.

## Threat Flags

None — T-30-05 mitigated by skip-before-verb + transcript test; T-30-06 by the single-flight guard + 8-thread overlap proof; T-30-07 by same-run accumulation + merged `retry_skipped`; T-30-08 by plain-language refusals + backend-enum-only UI strings. Zero new deps.

## Self-Check: PASSED

- `retry_failed` registered in the Tauri invoke handler and compiled.
- Commit `1899e60` verified in `git log`; no unintended deletions.
- Full lib suite 434/434; tsc clean.
