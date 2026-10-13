# Phase 20: Batch Reorganization - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning
**Mode:** Autonomous smart-discuss (recommended answers auto-accepted; no interactive session)

<domain>
## Phase Boundary

The user reorganizes the whole account in one explicit run: no per-email
confirmation, live progress, persisted per-category report, journaled +
resumable + undo-batch. A loop over the PROVEN Phase 18 confirm path with
dialog off, progress on.

</domain>

<decisions>
## Implementation Decisions

### Run shape
- `batch_classify(scope)` (whole account or per-folder): pre-create FULL
  `Auto/` tree up front (all tops+children), bulk-enqueue all candidates,
  chunk 25–50 with per-chunk UIDVALIDITY re-check (shift ⇒ abort-to-resync
  for that folder, run marked interrupted).
- Auto-confirm at/above threshold (same gate as Phase 17); below ⇒
  `A Classificar` remainder (never moved).
- Progress: per-chunk events (done/total, current folder); UI progress bar +
  cancel (cooperative flag like `sync_cancel`).

### Journal + resume + undo (structural, not stretch)
- Per-message journal rows (message_id, from_folder, to_folder, label,
  chunk, undone flag) written BEFORE each move; crash/kill mid-run ⇒
  resume continues after last journaled row (no dupes/skips — idempotent
  by journal check, interplay with `imap_outbox` verified in tests).
- `undo_batch(run_id)`: reverse journaled moves (to→from) in reverse order;
  restores originals; marks run undone. Persisted per-category report
  (moved counts, A-Classificar remainder list) survives undo.
- `batch_runs` state machine: running → done | interrupted | undone.

### Scale guard
- Live large-folder gate against the real throttling UTFPR server: chunk
  sizing tuned at plan time (start 25, document server behavior).
- Secondary labels never move in batch either (primary only).

### Agent Discretion
- Exact chunk size within 25–50 after live observation; progress event
  cadence (per-chunk vs per-N).
- Cancel granularity (chunk boundary only — recommended).

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- Phase 18 confirm path (dialog off = same backend call); Phase 10
  move+outbox; Phase 11 CREATE; Phase 16 `batch_runs` table (+ journal
  columns added here — M14 or ALTER in-plan, planner picks); Phase 17
  bridge/worker/suggest.
- `sync_cancel` cooperative-cancel pattern.

### Integration Points
- New commands: `batch_classify`, `batch_status`, `batch_resume`,
  `undo_batch`, `batch_report`, `cancel_batch`.
- Frontend: BatchProgress (bar + counts + report view + undo button).

</code>

<specifics>
## Specific Ideas

- Batch is explicitly UNCONFIRMED by design (whole-account op with report +
  undo) — distinct from single-email always-confirmed (Phase 18). Both
  documented in the UI copy so the trust model is legible.
- Dry-run + retry-failed: DEFERRED (v1.3.x), not this phase.

</specifics>

<deferred>
## Deferred Ideas

Batch dry-run + retry-failed (v1.3.x). Override history view (still later).
</deferred>
