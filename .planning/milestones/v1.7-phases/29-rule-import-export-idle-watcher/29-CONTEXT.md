# Phase 29: Rule Import/Export + IDLE Watcher - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, batch proposals accepted)

<domain>
## Phase Boundary

Users carry their smart-folder rules across machines as validated JSON and receive new INBOX mail instantly via push. Two capabilities: (1) versioned JSON rule import/export with validation (IMP-01); (2) manager-owned IdleWatcher on dedicated read-only connection for INBOX push, advisory wakeups through SyncGate, poll fallback (IDLE-01). Capability-gated by Phase 27 profile (IDLE atom, IMPORT_EXPORT atom). No UI in this phase for IDLE beyond status enum; rule import/export UI in RulesPanel.

</domain>

<decisions>
## Implementation Decisions

### Rule Import/Export (IMP-01)
- JSON schema: versioned JSON with rule_id (UUID), name, priority, conditions_json, dest_kind (folder/category), destination, scope_json, enabled, created_at, updated_at
- Validation on import: no empty patterns, no reserved names (system, auto), no duplicate names, priority range 1-999, dest_kind enum
- Conflict resolution: rule_id exists → update (version bump); name exists with different rule_id → reject 'duplicate name'; conditions reference non-existent folder → warn but allow
- Round-trip: export → import → export produces identical JSON (field-order normalized); import of exported file succeeds with version bump; invalid files (empty patterns, reserved names like 'system', 'auto') rejected with plain-language reason

### IDLE Watcher (IDLE-01)
- Dedicated connection: manager-owned `IdleWatcher` in `imap/idle.rs` — own `connect_sync` clone, SELECT INBOX + IDLE + DONE + LOGOUT read-only verbs only, 29-min re-IDLE loop, DONE on ALL exits, manager-consistent backoff
- Wakeup routing: advisory-only → `SyncWake::request_sync` → `SyncGate::try_begin` (skip-if-busy, coalesce dirty flag); poll stays fallback + multi-folder sweeper
- Status UI: honest Push/Polling/Offline status in sidebar from IdleWatcher state; kill mid-IDLE → reconnect (DONE, backoff) with no duplicate sync, no manager-session stall
- Capability gating: read IDLE atom from `capability-profile.json` (Phase 27); if false → IdleWatcher never spawns, poll-only mode

### Integration & Testing
- Test strategy: headless cargo test only. Fake-server transcripts for: rule import/export round-trip (fixpoint), IDLE DONE/reconnect/backoff, wakeup→SyncGate coalescing, poll fallback when IDLE disabled
- UI integration: status enum (Push/Polling/Offline) in sidebar from IdleWatcher state; rule import/export buttons in RulesPanel; import shows validation preview with accept/reject
- Capability gating: both features gated by capability-profile.json atoms (IDLE, IMPORT_EXPORT); if IDLE=false → IdleWatcher never spawns, poll-only; if IMPORT_EXPORT=false → rules UI disabled

### the agent's Discretion
- Exact fake-server transcript format for IDLE DONE/reconnect/backoff — planner's choice
- Rule JSON field ordering normalization for round-trip fixpoint — planner's choice
- IdleWatcher internal state machine (connecting/idle/done/reconnecting) — planner's choice
- RulesPanel import/export UX flow (modal, file picker, validation preview) — planner's choice

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/commands/sync.rs` — RulesPanel already exists with rule CRUD, `folder_rules` table, `prepare_create_wire`, `refresh_mailbox_tree`
- `src-tauri/src/store/queries.rs` — `folder_rules` CRUD, `list_mailboxes`, `ensure_mailbox`
- `src-tauri/src/imap/manager.rs` — `SessionManager` with `lease_for`, `connect_sync`, `list_mailboxes`
- `src-tauri/src/imap/session.rs` — `connect_sync`, `SyncSession` trait (extended in Phase 28)
- `src-tauri/src/sync/worker.rs` — `SyncWorker`, `SyncGate`, `SyncWake`
- `src-tauri/src/sync/worker.rs` — `MockSession` for headless testing (Phase 28 extended)
- `src-tauri/src/classify/rules.rs` — rule matching logic, `RuleMatcher`
- `src-tauri/src/commands/classify.rs` — rule CRUD commands

### Established Patterns
- Capability gating via `capability-profile.json` (Phase 27)
- Forward-only migrations with preserve-rows tests
- Fake-server transcript tripwire tests (Phase 28)
- `SyncGate` single-flight + `SyncWake` coalescing
- Manager-owned connection leasing (`SessionManager`)

### Integration Points
- Phase 28 CONDSTORE fast path → Phase 29 IDLE wakeups run cheap pass
- Phase 27 `capability-profile.json` → gates both IMPORT_EXPORT and IDLE
- `folder_rules` table → import/export source of truth
- `SyncWake` + `SyncGate` → IDLE advisory wakeup path
- RulesPanel UI → import/export UX

</code>

<specifics>
## Specific Ideas

- Reuse Phase 25 `folder_rules` schema + `rules_io` module for JSON round-trip
- IdleWatcher state machine: Connecting → Idle → Done → Reconnecting (with backoff)
- RulesPanel import: file picker → validation preview (warnings/errors) → accept/reject
- Sidebar status: enum Push | Polling | Offline from IdleWatcher state

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. Phase 30 (Batch Trust Pair) will use rule import/export + IDLE wakeups.

</deferred>