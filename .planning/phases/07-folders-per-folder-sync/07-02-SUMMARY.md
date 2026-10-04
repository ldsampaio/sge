# Phase 7 — Folders + Per-Folder Sync (Wave 2: Commands + DB Migration)

## Summary

**Wave:** 2 of 3  
**Status:** ✅ Complete  
**Depends on:** Phase 7 Wave 1 (Backend: SyncSession + SyncWorker multi-folder)  
**Requirements:** FOLD-01, FOLD-02, FOLD-03  

## Tasks Completed

**Task 1 — DB Migration M3** (`src-tauri/src/store/mod.rs`):
- ✅ Added `unseen_count INTEGER DEFAULT 0` column to `mailboxes` table
- ✅ Updated `SCHEMA_VERSION` to 3
- ✅ Wrote migration test: `m3_adds_unseen_count_column`
- ✅ Verified: `cargo test --manifest-path src-tauri/Cargo.toml m3` passes

**Task 2 — Query functions** (`src-tauri/src/store/queries.rs`):
- ✅ Added `list_mailboxes(conn) -> Vec<MailboxRow>` — returns all folders with unread counts
- ✅ Added `MailboxRow` struct: `{ id, name, uid_validity, uid_next, unseen_count, last_sync_at }`
- ✅ Added `set_mailbox_status(conn, name, uid_validity, uid_next, unseen_count)` — upsert STATUS data
- ✅ Added `count_unread(conn, mailbox_id) -> i64` — count messages with flags not containing \Seen
- ✅ Verified: all query functions compile and return correct types

**Task 3 — Tauri commands** (`src-tauri/src/commands/sync.rs`):
- ✅ Added `list_folders(state) -> Result<Vec<MailboxRow>, String>` — calls SessionManager.list_mailboxes + STATUS, caches in DB
- ✅ Generalized `start_sync(state, mailbox: String, on_event)` — accepts mailbox name, SELECTs that folder
- ✅ Generalized `sync_status(state, mailbox: String)` — looks up sync state for named mailbox
- ✅ Generalized `list_messages(state, mailbox: String, offset, limit)` — filters by named mailbox
- ✅ Generalized `fetch_message(state, uid, mailbox: String)` — SELECTs named mailbox before fetch
- ✅ Verified: `cargo test --manifest-path src-tauri/Cargo.toml sync_command_mailbox` passes

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml m3
cargo test --manifest-path src-tauri/Cargo.toml sync_command_mailbox
```

All 63 Rust tests green throughout. No runtime errors.

## Files Modified
- `src-tauri/src/store/mod.rs` — M3 migration + SCHEMA_VERSION 3
- `src-tauri/src/store/queries.rs` — list_mailboxes, MailboxRow, set_mailbox_status, count_unread
- `src-tauri/src/commands/sync.rs` — list_folders, generalized start_sync/sync_status/list_messages/fetch_message

## Next Up
`/gsd-discuss-phase 7` — (already completed in Wave 1); or proceed to Wave 3 frontend

