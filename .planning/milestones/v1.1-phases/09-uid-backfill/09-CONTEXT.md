# Phase 9: UID Backfill - Context

**Gathered:** 2026-10-05
**Status:** Ready for planning
**Mode:** Auto-generated (reconstructed — implementation from 09-01 plan exists, context file was missing after archival)

<domain>
## Phase Boundary

User never silently misses mail that arrived between syncs.
Depends on Phase 8. Requirement: SYNC-04.

Scope: gap detection via UID range-diff (not UIDNEXT walk) as a convergence property
of incremental sync; tombstoning of expunged-on-server UIDs after empty results (no
infinite backfill loop); double-poll-zero-FETCH convergence (two consecutive polls
with no server change issue no message FETCHes).

</domain>

<decisions>
## Implementation Decisions

### Range-diff, not UIDNEXT walk
Gaps detected by diffing server UID set against local UID set; only gap UIDs are
fetched via `fetch_message`.

### Tombstone after empty results
Expunged-on-server UIDs stop being re-requested after empty FETCH results —
tombstoned so the backfill loop terminates.

### Convergence is a property, not a phase
Backfill is a convergence property of the Phase 8 incremental sync path, not a
separate sync mode.

</decisions>

<code_context>
## Existing Code Insights

Incremental sync path from Phase 8 (single-flight `request_sync()`); UID sets from
the Phase 7 per-folder sync state. Gap + convergence logic lives in
`src-tauri/src/sync/worker.rs`; convergence state logged in SyncSummary.
See 09-01-SUMMARY.md for as-built notes.

</code>

<specifics>
## Specific Ideas

- `uid_gap` and `convergence_test` cargo tests gate the phase.
- Tombstone threshold: stop re-requesting after 3 empty FETCH results.

</specifics>

<deferred>
## Deferred Ideas

None — CONDSTORE/QRESYNC fast path remains a milestone-level deferred item.

</deferred>
