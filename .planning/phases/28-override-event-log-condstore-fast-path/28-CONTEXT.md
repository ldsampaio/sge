# Phase 28: Override-Event Log + CONDSTORE Fast Path - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, batch proposals accepted)

<domain>
## Phase Boundary

Reconnects resync fast over CONDSTORE deltas while every correction is banked for future learning. Two headless lanes (no UI): (1) override_events log (M20) — append-only telemetry from every confirm/override/dismiss/batch decision; (2) CONDSTORE fast path inside sync_with_borrowed — CHANGEDSINCE FETCH with MODSEQ filtering, UIDVALIDITY-first epoch gating, silent fallback to UID range-diff. Capability gated by Phase 27 capability-profile.json (CONDSTORE atom). Both verified via cargo test (fake-server transcripts, epoch tripwires).

</domain>

<decisions>
## Implementation Decisions

### CONDSTORE Fast Path Wiring
- CHANGEDSINCE range: UID set from last sync to current uid_next, with MODSEQ filter; pass prev_modseq to FETCH CHANGEDSINCE; fall back to UID range-diff if unsupported
- UIDVALIDITY epoch change: on bump, set highest_modseq=NULL + full resync (wipe local + replay outboxes); transcript tripwire test with synthetic epoch bump
- MODSEQ persistence: after each successful CONDSTORE pass, write highest_modseq from SELECT response; on epoch change or CONDSTORE unavailable, NULL it
- Baseline fallback: UID range-diff (Phase 9 convergence logic) runs when CONDSTORE unsupported or modseq NULL — no hard failure, silent degrade

### Override-Event Log (M20)
- Schema: CREATE TABLE override_events (id INTEGER PK, ts TEXT NOT NULL, action TEXT NOT NULL CHECK(action IN ('confirm','override','dismiss','batch')), message_id TEXT NOT NULL, from_category TEXT, to_category TEXT, confidence REAL, threshold REAL, keywords TEXT, batch_run_id TEXT, uid_validity INTEGER, modseq INTEGER) — append-only, indexed on (message_id, ts)
- Hook insertion points: fire-and-forget after confirm_suggestion, override_label, dismiss_suggestion, batch_classify — pass context (mid, from/to category, confidence, threshold, keywords, batch_run_id)
- Migration: forward-only M20 migration following M2-M11 template with preserve-rows test — no schema changes to existing tables
- Log hygiene: IDs/scores only, never bodies (Pitfall 3); redaction sanitizer from Phase 16 reused

### Architecture & Integration
- SyncSession verbs: add condstore_fetch_changedsince(range: String, modseq: u64) -> Vec<MessageHeader> and select_condstore() -> bool to the SyncSession trait; impl for MockSession and real Session
- Capability gating: read CONDSTORE atom from capability-profile.json (Phase 27 output) at sync start; if false or highest_modseq NULL, skip CONDSTORE branch entirely
- Wire protocol: use async-imap 0.11.3 select_condstore + raw-string FETCH CHANGEDSINCE via run_command* escape hatch — no async-imap bump, no new deps
- Test strategy: fake-server transcript tripwire (synthetic MODSEQ shapes), epoch-clears-modseq test, convergence test with/without CONDSTORE — all cargo test, no live server needed

### the agent's Discretion
- Exact MODSEQ wire format (FETCH CHANGEDSINCE modseq vs UID range) — follow async-imap run_command* pattern
- Fake-server transcript design for tripwire tests — planner's choice
- override_events index strategy beyond (message_id, ts) — planner's choice
- M20 migration naming and preserve-rows test details — follow existing template

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/store/mod.rs` mailboxes table with `highest_modseq INTEGER` column (NULL if no CONDSTORE)
- `src-tauri/src/sync/worker.rs` sync_with_borrowed — Step 1-5 flow (SELECT, UIDVALIDITY guard, UID SEARCH, header sweep, flag sync)
- `src-tauri/src/imap/session.rs` Session::select_condstore (via async-imap 0.11.3), run_command* escape hatch
- `src-tauri/src/store/queries.rs` log_override, latest_override — label_overrides table exists (append-only)
- `src-tauri/src/commands/classify.rs` confirm_suggestion, override_label, dismiss_suggestion — command entry points for hooks
- `src-tauri/src/classify/batch.rs` batch_classify — batch entry point for hook

### Established Patterns
- Forward-only migrations via rusqlite_migration (M2-M11 template with preserve-rows test)
- Capability gating via capability-profile.json (Phase 27 contract)
- BODY.PEEK-only reads; UID-only addressing with UIDVALIDITY gating
- Phase 9 convergence shortcut (skip sweep when UIDs unchanged + no pending)
- MockSession for headless testing (imap/manager/session)

### Integration Points
- Phase 27 capability-profile.json CONDSTORE atom gates this phase's CONDSTORE branch
- Phase 29 IDLE watcher wakeups will run the same sync_with_borrowed (cheap CONDSTORE pass first)
- Phase 31 learning/auto-confirm consumes override_events log
- Phase 30 batch dry-run/retry uses same matcher as this phase's planner (future)

</code>

<specifics>
## Specific Ideas

- Reuse Phase 9 convergence logic (skip_sweep) as the UID range-diff baseline for CONDSTORE fallback
- CONDSTORE branch should be a Step 2b in sync_with_borrowed (after SELECT, before UID SEARCH ALL)
- capability-profile.json is in .planning/probes/ — sync reads it at startup, caches per-account capability
- override_events hook should be fire-and-forget (no await) to not block confirm/override UX

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. Phase 29 IDLE watcher and Phase 31 learning are downstream consumers.

</deferred>