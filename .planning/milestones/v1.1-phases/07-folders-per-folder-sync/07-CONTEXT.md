# Phase 7 — Folders + Per-Folder Sync

## Context

### Goal
User can browse every mailbox, not just INBOX, with per-folder unread triage signals.

### Requirements
- FOLD-01: User can browse the server folder tree (Sent, Drafts, custom folders) in the sidebar
- FOLD-02: User can open a folder and browse its messages from local cache
- FOLD-03: User sees unread counts per folder

### Success Criteria
1. User sees the real server folder tree (Sent, Drafts, custom folders) in the sidebar, matching LIST discovery
2. User can open any folder and browse its messages from local cache (headers-first, same fast list as INBOX)
3. User sees an unread count badge per folder sourced from STATUS (UNSEEN)
4. A UIDVALIDITY change in one folder triggers resync of only that folder — other folders' caches are untouched

### Current State
- Phase 6 complete: flag sync + durable outbox + BODY.PEEK audit + RFC 4549 tests green
- `SyncSession` trait has `select_inbox()` — hardcoded to INBOX
- `SyncWorker::sync_with_session()` is INBOX-only (hardcoded `"INBOX"` in ensure_mailbox, get_sync_state, select_inbox, set_sync_state)
- `SessionManager::lease()` SELECTs only INBOX
- `BoxedSession` impl of `SyncSession` uses `self.select("INBOX")` and `self.uid_search("ALL")`
- Schema: `mailboxes` table has name, uid_validity, uid_next, highest_modseq, last_sync_at — one row per folder already exists, just no multi-folder SELECT or LIST support
- Frontend: `Sidebar.tsx` has hardcoded fake entries (Início, Materiais, Ajuda); `MailboxView.tsx` is INBOX-only; `App.tsx` passes `mailbox` prop but `handleMailboxSelect` is a no-op
- No STATUS UNSEEN caching — unread counts computed from local flags only

### Key Architecture Decisions
- Position A (flags-first): Phase 6 established UID-only STORE + outbox + per-mailbox sync state. Phase 7 extends the same mailbox_id-keyed store to additional folders.
- LIST discovery uses async-imap's `list("", "*")` which returns `Name` structs with name, delimiter, attributes
- STATUS (UNSEEN) query uses async-imap's `status(mailbox, "(MESSAGES UIDVALIDITY UIDNEXT UNSEEN)")`
- The `mailboxes` table already supports per-folder sync_state — just need to extend queries for STATUS/unseen counts
- `sync_with_session` needs to accept a mailbox name parameter instead of hardcoding "INBOX"
- Single-flight guard (Phase 8) will wrap multi-folder sync; for M1.1 we just need to not overlap syncs on the same folder

### Files Read
- `src-tauri/src/imap/mod.rs` — SyncSession trait (257-283), BoxedSession impl (298-389)
- `src-tauri/src/imap/session.rs` — connect_sync, open_inbox
- `src-tauri/src/imap/manager.rs` — SessionManager with INBOX lease
- `src-tauri/src/sync/worker.rs` — SyncWorker.sync_with_session (148-410), replay_outbox (68-141)
- `src-tauri/src/sync/mod.rs` — SyncEvent, SyncSummary, SyncCallback
- `src-tauri/src/imap/bodies.rs` — fetch_body_message, BODY.PEEK[]
- `src-tauri/src/store/mod.rs` — Store, M2 migration
- `src-tauri/src/store/queries.rs` — ensure_mailbox, get_sync_state, set_sync_state, mailbox_id, list_messages, etc.
- `src-tauri/src/store/schema.sql` — mailboxes, messages, message_bodies, attachment_parts tables
- `src-tauri/src/commands/sync.rs` — start_sync, set_seen, sync_status, list_messages, fetch_message
- `src/components/Sidebar.tsx` — hardcoded folder list
- `src/components/MailboxView.tsx` — INBOX-only mailbox view
- `src/App.tsx` — app shell with login/sync flow
- `src/types.ts` — MessageRow type
