# Phase 3: Mailbox UI Shell + Search - Context

**Gathered:** 2026-10-03
**Status:** Ready for planning
**Mode:** Auto-generated (smart discuss, non-interactive)

<domain>
## Phase Boundary

Three-pane Gmail-like mailbox UI: sidebar with INBOX node, virtualized message list
(sender/subject/date, unread dot, newest-first), instant offline FTS search bar, and
a reading-pane slot (placeholder for Phase 4). Plus empty/loading/error states in every
pane. The list reads from the local SQLite store populated by Phase 2 — no IMAP round-trips
in the list path (read-only M1, BODY.PEEK only for sync).
</domain>

<decisions>
## Implementation Decisions

### Virtualization & List Performance
- Pagination in batches of 200 (matches sync worker BATCH_SIZE) with infinite scroll — no
  new npm dependency, avoids Tauri bundle complexity. The 10k-row perf smoke test already
  proves list_messages(200) < 50ms.
- DOM grows lazily; if jank at scale is measured in Phase 4, escalate to react-window.
- list_messages query ordered by date_utc DESC, uid DESC (already implemented in queries.rs).

### Message List Query
- `list_messages(mailbox: String, limit: usize)` Tauri command mirrors the frozen
  `sync_status` pattern in commands/sync.rs — thin command, blocking DB read on
  spawn_blocking thread, returns `Vec<MessageRow>`.
- `search_messages(mailbox: String, query: String)` Tauri command calls
  `queries::fts_search(conn, mailbox, query)` — BM25-ranked FTS5 results.
- Both registered in lib.rs invoke_handler alongside the existing sync commands.
- MessageRow serde fields: uid, subject, from_addr, to_addrs, date_utc, flags,
  has_attachments, preview. Frontend maps flags → is_unread via queries::is_unread.

### Search UX
- Search-as-you-type with 300ms debounce. Search term filters local SQLite FTS5 results.
  Results replace the list view. Clear search (empty string) returns to full paginated list.
- No server round-trip for search — purely offline FTS (SRCH-01 satisfied).
- Search bar is always visible in the message list pane header.

### App State Transition
- `connect_account` success (LoginForm) → App state transitions to "connected" → renders
  MailboxView instead of LoginForm+SyncStatus. SyncStatus component remains usable within
  the mailbox layout as a header/refresh widget.
- Message list is lazy-loaded on MailboxView mount (list_messages call). No auto-sync on
  transition — user presses Sync Now if needed (Phase 5 owns auto-connect).
- App.tsx holds a top-level `connected: boolean` state, set on successful connect_account.

### Reading Pane Slot
- Reading pane shows placeholder text "Select a message to read" — no message body fetching
  or rendering in Phase 3. Body fetching lands in Phase 4.
- When a message row is clicked, the reading pane shows the message's preview text as a
  non-navigable placeholder. True HTML/plaintext rendering is Phase 4.

### Styling Approach
- CSS Flexbox for the three-pane layout. New `src/components/MailboxView/MailboxView.css`.
- Extends existing App.css for shared styles (buttons, inputs, dark mode).
- Three panes: sidebar (240px) | message list (flex-grow, scrollable) | reading pane (320px).

### Empty, Loading, Error States
- Sidebar: always shows INBOX node, no state variation needed.
- Message list empty: "No messages in INBOX" + "Sync Now" hint if last_sync_at is null.
- Message list loading: skeleton rows or "Loading…" text.
- Message list error: plain-language message + Retry button (mirrors LoginForm error pattern).
- Reading pane empty: "Select a message to read" placeholder.
- Auth/TLS error on list_messages → prompt user to reconnect (return to login form).

### Claude's Discretion
- Exact pagination batch size (200), debounce delay (300ms), sidebar width (240px),
  reading pane width (320px). All are M1-appropriate defaults.
</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `MessageRow` struct in `src-tauri/src/store/queries.rs` — already serde Serialize, has all fields needed (uid, subject, from_addr, to_addrs, date_utc, flags, has_attachments, preview).
- `queries::list_messages(conn, mailbox, limit)` — already implemented, returns newest-first ordered rows.
- `queries::fts_search(conn, mailbox, query)` — already implemented, BM25-ranked FTS5 search.
- `queries::is_unread(flags_json)` — already implemented, display-only read/unread.
- `commands/sync.rs` pattern — thin Tauri command, spawn_blocking for DB read, returns typed Result.
- `AppState` holds `Arc<Mutex<Store>>` — list_messages/search_messages will use the same pattern as sync_status.

### Established Patterns
- `invoke<T>(command_name, args)` + `@tauri-apps/api/core` for frontend backend calls.
- `Channel<SyncEvent>` for streaming progress — not needed for list/search (synchronous).
- Plain-language error mapping via thiserror Display → string.
- `isTauriRuntime()` guard pattern for browser-URL plain-language message.
- React 19 hooks (useState, useEffect, useRef) — same patterns as SyncStatus.tsx.
- Dark mode via `prefers-color-scheme` in App.css.

### Integration Points
- `lib.rs` invoke_handler — add list_messages and search_messages commands.
- `commands/sync.rs` — mirror the sync_status pattern (spawn_blocking + store.conn()).
- `App.tsx` — state transition from login view to mailbox view.
- `src/components/` — new MailboxView component with sub-components.
</code_context>

<specifics>
## Specific Ideas

- Message list row: avatar dot (unread), sender bold (unread), subject, date. Gmail-like.
- Reading pane slot: gray placeholder text, not interactive (Phase 4 will replace).
- Search bar: always visible above message list, debounced input.
- SyncStatus to remain as a header widget inside MailboxView — user can re-sync from there.

</specifics>

<deferred>
## Deferred Ideas

- True message reading (HTML/plaintext rendering, sanitization) → Phase 4
- Attachment list/display → Phase 4
- Virtualized rendering (react-window) → only if pagination jank at scale
- Keyboard navigation (j/k) → post-M1 (FF-02)
- Multiple folders in sidebar → post-M1 (FF-06)
- Mark read/unread with server flag sync → post-M1 (FF-07)
</deferred>
