# Phase 8: Poll + Manual Refresh - Context

**Gathered:** 2026-10-05
**Status:** Ready for planning
**Mode:** Auto-generated (reconstructed — implementation from 08-01 plan exists, context file was missing after archival)

<domain>
## Phase Boundary

User's mail stays fresh without thinking about sync and never corrupts from overlapping syncs.
Depends on Phase 7. Requirement: SYNC-03.

Scope: single-flight guard on the single SessionManager-owned session, poll timer
(configurable, default 5–10 min), manual refresh through the same `request_sync()`
code path as the poll timer, honest "reconnecting…" state on session expiry with
resume after keyring re-read + re-SELECT. IDLE/CONDSTORE stay out of scope.

</domain>

<decisions>
## Implementation Decisions

### Single-flight before timer
The single-flight guard ships before any poll timer fires — a poll firing mid-sync
(or mid flag-STORE) never overlaps; one sync runs at a time on the single session.

### One code path
Poll timer and manual refresh share one `request_sync()` entry — no second sync path.

### Poll cadence default unresolved
60–120 s vs 5–10 min default was unresolved at milestone planning — validate during
verification (see 08-01-PLAN.md Task 2: 5 minutes default).

</decisions>

<code_context>
## Existing Code Insights

SyncWorker with SessionManager-owned session from Phase 6; per-folder state from
Phase 7. Poll guard is an EXECUTING AtomicBool on the sync worker; timer invokes
`start_sync()` defaulting to INBOX for v1.1. See 08-01-SUMMARY.md for as-built notes.

</code>

<specifics>
## Specific Ideas

- "Get Mail" button in SyncStatus.tsx through the same code path as the poll timer,
  disabled while single-flight flag is set.
- Emit SyncEvent::Poll on timer fire; SyncSummary logs convergence state.

</specifics>

<deferred>
## Deferred Ideas

None — IDLE push and CONDSTORE fast path are milestone-level deferred items.

</deferred>
