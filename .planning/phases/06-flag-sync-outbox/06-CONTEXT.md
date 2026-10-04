# Phase 6: Flag Sync + Outbox - Context

**Gathered:** 2026-10-04
**Status:** Ready for planning
**Mode:** Auto (user requested no prompts — recommended answers auto-accepted)

<domain>
## Phase Boundary

Phase 6 ends the read-only era: the user can mark messages read/unread with the Seen
flag synced to the IMAP server (FLAG-01), including toggles made offline that queue
durably and replay on reconnect (FLAG-02). Reconcile + durable outbox ship with the
first STORE. SessionManager ownership + extended SyncSession trait land here as the
foundation later phases build on. No folders, no poll timer, no backfill in this phase.

</domain>

<decisions>
## Implementation Decisions

### Flag toggle UX (auto-accepted)
- Auto-mark read when a message is opened in the reader (Thunderbird-style Seen-on-open), plus explicit toggle for mark-unread
- Toggle affordance: clickable unread dot in message list rows + mark read/unread button in reader header
- Optimistic UI: toggle applies instantly locally, per-message pending state until server acknowledges
- Pending indicator: subtle pending style on the row + aggregate state in SyncStatus; never blocks browsing

### Offline queue and conflicts (auto-accepted)
- Automatic replay on reconnect (no manual flush button)
- Pending-wins reconcile: unacknowledged local toggles beat server FLAGS diffs on merge
- Durable SQLite outbox table with RFC 4549 playback rules (drop op when message missing, drop folder queue on UIDVALIDITY change)
- Failed ops surface in sync status text and retry on the next sync naturally

### Sync integrity and scope (auto-accepted)
- UID-only STORE addressing (`uid_store` + `±FLAGS.SILENT (\Seen)`); never sequence numbers
- BODY.PEEK audit over every fetch path (imap/headers.rs, imap/bodies.rs, sync/bodies.rs, commands/sync.rs fetch_message) — no path may set \Seen as a side effect
- SessionManager owns the single session; mailbox-scoped leases from day one (INBOX is the `mailbox="INBOX"` case)
- Scope guard: no delete/expunge/move, no \Flagged keywords, no bulk multi-select — read/unread triage only

### the agent's Discretion
- Exact outbox table shape and replay batching; poll-interval-independent reconnect detection details are deferred to Phase 8 — Phase 6 only needs "replay on next successful sync/session open"

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `SyncSession` trait (imap/mod.rs:253) — extend with `set_seen`; `connect_sync` (imap/session.rs:755) per-command connections retire toward manager
- `messages.flags` JSON column + `is_unread`/`parse_flags` (store/queries.rs:28-37) — D-flags comment marks the read-only rule this phase lifts
- `mailboxes` row already holds per-folder sync state (`uid_validity`, `uid_next`, `highest_modseq`, `last_sync_at` — schema.sql:21-24)
- Commands `fetch_message`, `list_messages`, `start_sync`, `cancel_sync` (commands/sync.rs); frontend `invoke` sites in MessageList/ReadingPane/SyncStatus
- Unread dot UI exists (`message-row unread/read`, `unread-dot` — MessageList.tsx:200-213); `isUnread(msg.flags)` helper in frontend

### Established Patterns
- Single-session SyncEngine ownership; `AppState` holds `Arc<Mutex<Store>>`; `Channel<SyncEvent>` for progress streaming
- BODY.PEEK-only fetches across headers/bodies paths; UIDVALIDITY-guarded incremental sync; OFFSET pagination for lists
- Frontend polls `sync_status`; Tauri commands return `Result<_, String>`

### Integration Points
- New `set_seen` Tauri command invoked from MessageList dot + ReadingPane header button
- Outbox replay hooks into session open / post-sync completion in sync/worker.rs
- Reconcile merges pending ops against server FLAGS during incremental sync write-back

</code>

<specifics>
## Specific Ideas

No specific requirements — auto mode with recommended defaults. Refer to ROADMAP phase description and success criteria.

</specifics>

<deferred>
## Deferred Ideas

- Seen-on-open delay tuning (mark after N seconds of reading vs instant) — ship instant, tune later if needed
- \Flagged/star support — later milestone, not triage-minimal
- Session idle-timeout empirical test (1-hour-open) — Phase 8 poll concern, noted in research gaps

</deferred>
