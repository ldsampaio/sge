---
phase: 30-batch-trust-pair
plan: 01
subsystem: batch-preview
tags: [batch, preview, dry-run, stale-resolution, uidvalidity]
dependency_graph:
  requires: [phase-20-batch-journal, phase-24-archive-as-batch, phase-29-dry-run-contract]
  provides: [shared-preview-plan, resolve-for-move, retry-core-helpers]
  affects: [30-02-retry-ui, batch-report]
tech_stack:
  added: []
  patterns: [transient-dry-run-plan, same-section-re-resolution, transcript-tripwire]
key_files:
  created: []
  modified: [src-tauri/src/classify/batch.rs, src-tauri/src/commands/classify.rs, src-tauri/src/lib.rs]
decisions:
  - "Preview decision without sidecar: cached label rows decide when fresh; otherwise a headless taxonomy keyword match over cached classify fields. The sidecar suggester is never invoked by preview (zero IMAP, zero writes, deterministic headless tests)."
  - "Failure derived from moved=0 alone (planner's discretion choice) — no error-text column, no migration."
  - "500-row cap applies to items and skipped lists independently, with truncated + total_candidates."
metrics:
  duration: "~45 min"
  completed: 2026-10-14
  tasks: 3
  commits: 1
status: complete
---

# Phase 30 Plan 01: Shared Batch Preview + Stale Re-Resolution Summary

**One-liner:** Transient shared dry-run preview (`preview_batch`/`preview_archive` over one `preview_plan()`) with zero writes, plus confirm-path UID re-resolution (`resolve_for_move`) with stale-drop counts — BATCH-03 backend proven headless.

## What Was Built

- **Shared preview core** (`src-tauri/src/classify/batch.rs`): `preview_plan(conn, kind)` serves both legs. Classify leg runs `collect_candidates` + cached-label/keyword decisions through the shared `dest_display_for` helper (the real run loop was refactored to call the same helper — preview and execute cannot diverge). Archive leg runs the archive selector + `archive_dest_for` with `kind_tag: "archive"`. Per-item shape `{message_id, from_folder, to_folder, dest_display, basis, skipped_reason}` plus `total_candidates`, `truncated`, `remainder`. `PREVIEW_CAP=500`.
- **Writes off (structural):** preview functions take `&Connection` read-only paths only — no `batch_runs`/`batch_items`/label/override writes, no IMAP verbs, no SyncGate interaction.
- **Confirm hardening** (`src-tauri/src/commands/classify.rs`): both `batch_classify` and `archive_run` loops now re-resolve every item via `resolve_for_move` in the journal lock section; the MOVE verb takes only the re-resolved `(folder, UID)` pair. Stale items drop with `stale_dropped` counted (chunk-level epoch skips count too). `totals_json` gains `stale_dropped`.
- **Report surfacing:** `BatchReport` gains `stale_dropped`, `retry_skipped` (parsed from backend `totals_json`, default 0), and `failed` (`moved=0` count — backend truth for the 30-02 retry button).
- **Retry core helpers (BATCH-04 backend half, used by 30-02):** `check_retry_entry` (done/interrupted-only gate, plain-language refusals), `failed_count`, run-level single-flight guard (`try_claim_retry`/`release_retry`).
- **Commands registered:** `preview_batch`, `preview_archive` in the Tauri invoke handler.

## Tests

All headless (`cargo test --lib classify::batch::` — 8/8 pass; full suite 429 passed, 0 failed):

| Test | Asserts |
|------|---------|
| `preview_writes_nothing` | DB diff empty after both preview legs; classify basis carries label; gibberish → review-bucket; archive items carry `archive` basis + `Auto/Archive/` dests; all basis strings redaction-clean |
| `commit_counts_match_preview` | Journaling preview destinations == preview plan (parity) |
| `stale_preview_drop` | Location-shift + epoch-shift items drop (`stale_dropped == N`), transcript contains zero MOVE verbs for stale UIDs; fresh control resolves with identical pair |
| `live_preview_gate_skips_offline` | SKIPs cleanly without `SGE_LIVE_IMAP_USER`, never touches network |

## Deviations from Plan

None - plan executed exactly as written.

## Known Stubs

None.

## Threat Flags

None — all new surface was in the plan's threat model (T-30-01 mitigated by same-section re-resolution + transcript test; T-30-02 by redaction-sanitized basis strings + `looks_clean` assertion; T-30-03 by the 500-row cap + zero-verb preview; T-30-04 by the shared `dest_display_for` + parity test). Zero new deps.

## Self-Check: PASSED

- `preview_plan`, `resolve_for_move`, `preview_batch`, `preview_archive` all present and compiled.
- Commit `9757cad` verified in `git log`.
- Full lib suite: 429 passed, 0 failed.
