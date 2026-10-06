# Phase 11: Folder CRUD - Context

**Gathered:** 2026-10-06
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, auto-accepted)

<domain>
## Phase Boundary

User can organize their mailbox tree — create, rename, and delete folders — and see the real server tree reflected in the sidebar. Covers FOLD-04, FOLD-05, FOLD-06. Depends on Phase 10 (verb + manager surface proven).

</domain>

<decisions>
## Implementation Decisions

### Create flow
- Create dialog: single text input for folder name + parent-folder picker reusing Sidebar buildTree (default: top level)
- Hierarchy delimiter from cached LIST delimiter; nested name joined as `parent + delimiter + leaf` in raw modified-UTF-7 wire form
- After CREATE success: re-LIST refresh sidebar automatically, select/highlight the new folder
- Create validates non-empty name, rejects delimiter-in-leaf and reserved INBOX-case variants with plain-language error

### Rename flow
- Rename via RENAME old new (same delimiter rules as create); INBOX rename blocked client-side with plain-language message
- On rename success: invalidate message selection caches pointing at old name, switch selection to new name, re-LIST sidebar; UIDs preserved (no refetch of bodies)
- System/special-use folders (Trash/Sent/Drafts by role) show rename disabled with tooltip explaining why

### Delete flow
- INBOX protected client-side and server-error mapped; `\Noselect` placeholders not deletable (filtered from delete affordance)
- Non-empty folders guarded: confirm modal shows message count and requires explicit typed or double confirmation; default path refuses delete on non-empty unless user confirms empty-then-delete is NOT done silently (refuse loudly, offer move-out guidance)
- On delete success: local cache cascade (messages, sync_state, outbox rows for that mailbox_id) cleaned; selection falls back to INBOX; sidebar re-LISTed

### Trash + roles bookkeeping
- Generalize Phase 10 trash.rs detect_trash into a roles schema: SPECIAL-USE attributes first (`\Trash`, `\Sent`, `\Drafts`), then known-name match (reuse SYSTEM_ORDER + Gmail-style variants), cached per account
- Trash Missing path keeps Phase 10 behavior: one-time CREATE behind confirmation, never silent (no twin-trash)
- NAMESPACE stays banned: delimiter comes from LIST responses only; imap-proto NAMESPACE tripwire tests extended, no NAMESPACE command ever sent

### the agent's Discretion
- Exact modal copy and placement (follow ExpungeModal tone from Phase 10)
- Whether rename/delete live in sidebar context menu vs toolbar buttons (recommend sidebar context menu + keyboard-accessible menu)

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `SessionManager::lease_for` + reconnect-retry template (manager.rs 69-103, set_seen_in 149-167) for create/rename/delete_mailbox manager methods
- `create_trash` (manager.rs 225) as the CREATE verb template; `list_mailboxes` (manager.rs 294) + `commands/sync.rs list_mailboxes` (819) for LIST refresh path with `\Noselect` skip + offline fallback
- `trash.rs detect_trash` layers (SPECIAL-USE → name match → Missing) to generalize into roles schema
- Sidebar `buildTree` + `SYSTEM_ORDER` (Sidebar.tsx) for parent picker and role name-match list
- ExpungeModal confirm pattern (ExpungeModal.tsx) for delete-folder guard modal; MoveMenu tree reuse for parent picker
- `mutf7` encode/decode helpers (imap/mutf7.rs) for raw wire names vs display names

### Established Patterns
- UID-only addressing; BODY.PEEK-only reads; Tauri commands with `Option<String>` mailbox defaulting to INBOX
- Optimistic UI + pending wash + rollback-on-reject; `{ acked, pending_count, detail }` result shape
- Store mutex held only for brief sync sections, never across `.await`; forward-only migrations with preserve-rows test
- Raw wire name passed to backend, never display_name

### Integration Points
- `SyncSession` trait (imap/mod.rs / session.rs): add create/rename/delete_mailbox verbs with try_collect drain + `SyncError::Protocol` mapping
- `mailboxes` table (store/schema.sql): add roles/delimiter/attributes bookkeeping columns via forward-only migration
- `start_sync` under manager leases (Phase 10 precondition) — folder ops reuse same session discipline, never a second connection

</code>

<specifics>
## Specific Ideas

- Parent picker reuses MoveMenu/Sidebar tree; raw modified-UTF-7 wire names on the wire, decoded display names in UI
- Rename must not leave stale selection: selection state keyed by mailbox name must migrate atomically with the re-LIST
- One live gate per roadmap hard constraint: CREATE + RENAME + DELETE verified against mail.utfpr.edu.br in sequence with cleanup (delete test folders after)

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>
