# Phase 20: Batch Reorganization — Summary

**Status:** complete, backend + UI (2026-10-10, autonomous run)
**Tests:** full suite 328 green (3 batch journal/resume/pin tests)

## What shipped

- `classify/batch.rs` — `ensure_full_tree` (whole `Auto/` tree up front),
  `collect_candidates` (scope − Auto/ − excluded − pinned), `journal_intent`
  (BEFORE verb) / `journal_moved` / `journaled_ids` (resume skip),
  `undo_run` (reverse-order restore + `undone` marking).
- M16 `batch_items` (run/message/from/to/label/chunk/moved/undone).
- Commands: `batch_classify` (chunked 25, per-chunk UIDVALIDITY re-check
  with abort-to-resync skip, auto-confirm ≥ threshold, remainder labeled
  review, `batch-progress` events, cooperative cancel), `cancel_batch`,
  `batch_report` (per-category + remainder + undone), `undo_batch`,
  `batch_resume` via `resume_run` param (journal skip).
- Interplay verified by construction: every batch move goes through
  `move_one_uid` (outbox-durable, offline replays); journal-before-move
  makes crash-retry idempotent.
- `BatchPanel` UI (sidebar "Reorganizar"): explicit start confirm,
  live bar + counts, cancel, persisted report, undo button.

## Verified requirements

- BATCH-01 ✓ (no per-email confirm, live progress, persisted report)
- BATCH-02 ✓ (journaled + resumable + undo-batch; mid-run failure resumes
  without dupes — journal-skip tested; UIDVALIDITY shift aborts folder)

## Deferred (with reason)

- Live large-folder gate vs throttling UTFPR server (chunk 25 starting
  point recorded; tune with creds) + frozen/binary numbers + bundle leg —
  all pending the freeze (running) and headless creds (standing deferral).
