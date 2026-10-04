# Phase 3 — Mailbox UI Shell + Search

## Summary

Phase 3 delivered the three-pane Gmail-like mailbox UI for SGE:

- **Backend**: Added two Tauri commands — `list_messages` (paginated via OFFSET, newest-first)
  and `search_messages` (FTS5 full-text search across From/To/Subject/Preview). Both execute
  on blocking threads per the established `sync_status` pattern. All 57 Rust unit tests pass.

- **Frontend**: Created 5 new React components plus shared types and styles:
  - `MessageList` — infinite-scroll list (200 msg batches), debounced search, states (loading/empty/error/ready)
  - `Sidebar` — INBOX navigation node with message-count badge
  - `ReadingPane` — placeholder slot for future HTML rendering
  - `SearchBar` — 300ms-debounced input with clear button
  - `MailboxView` — three-pane orchestrator embedding `SyncStatus` as a header widget

- **Integration**: `App.tsx` now transitions from login form to `MailboxView` on successful
  `connect_account`. `LoginForm` now calls `save_server_config` after connect (Phase 2 bug fix
  discovered during P3 — `start_sync` requires the server config in keyring).

## Verification

| Check | Command | Result |
|-------|---------|--------|
| Rust compile | `cargo check` (src-tauri) | ✅ exit 0, no new warnings |
| Rust tests | `cargo test --lib` | ✅ 57 tests pass |
| TypeScript | `npx tsc --noEmit` | ✅ exit 0 |
| ESLint | `npx eslint src/` | ✅ 0 errors |
| Frontend build | `npx vite build` | ✅ dist/ generated |

## Security

- All message data is read from the local SQLite store only — no new IMAP calls in P3
- IMAP constraint preserved: P1–P2 used `BODY.PEEK` only; no `\Seen` set in M1
- HTML sanitization (ammonia) + iframe sandbox deferred to Phase 4 (ReadingPane is placeholder)
- Credentials remain keyring-only; no plaintext storage introduced

## Decisions

1. **OFFSET pagination** chosen over cursor-based (UID) pagination for simplicity — OFFSET
   on a fully-indexed (mailbox_id, date_utc DESC, uid DESC) table is efficient for MVP scales
2. **ReadingPane is a placeholder** — actual HTML rendering with ammonia sanitization + iframe
   sandbox is Phase 4 scope
3. **`save_server_config` call added to LoginForm** — Phase 2's `start_sync` requires it but
   LoginForm never saved it; this was a latent bug discovered when wiring up the mailbox view

## Files Added

```
src/types.ts                           # Shared MessageRow/SyncStatusInfo types
src/components/Sidebar.tsx             # INBOX navigation
src/components/MessageList.tsx         # Virtualized-by-pagination list + states
src/components/ReadingPane.tsx         # Placeholder reading pane
src/components/SearchBar.tsx           # Debounced search input
src/components/MailboxView.tsx          # Three-pane orchestrator
src/components/MailboxView.css          # Three-pane layout styles
```

## Files Modified

```
src-tauri/src/commands/sync.rs    # + list_messages, search_messages w/ offset + tests
src-tauri/src/lib.rs              # Registered new commands
src-tauri/src/store/queries.rs    # + offset param on list_messages
src/App.tsx                       # Login → MailboxView transition
src/App.css                       # + .disconnect-btn + .container position
src/components/LoginForm.tsx      # + save_server_config + onConnect callback
```
