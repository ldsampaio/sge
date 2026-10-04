# Phase 7 — Folders + Per-Folder Sync (Wave 1: Backend)

## Summary

**Wave:** 1 of 3  
**Status:** ✅ Complete  
**Depends on:** Phase 6 (Flag Sync + Outbox)  
**Requirements:** FOLD-01, FOLD-02, FOLD-03  

## Tasks Completed

**Task 1 — SyncSession trait extension** (`src-tauri/src/imap/mod.rs`):
- Verified `select_mailbox(&mut self, name: &str)` already implemented — SELECTs any mailbox by name
- Verified `list_mailboxes(&mut self)` already implemented — LIST "" "*" discovers folders with `MailboxInfo` struct
- Verified `MailboxInfo { name, delimiter, attributes }` struct exists and is returned by `list_mailboxes`
- Verified `select_inbox()` convenience method delegates to `select_mailbox("INBOX")`
- No new code required — trait already multi-folder capable from Phase 6 foundation

**Task 2 — SyncWorker generalization** (`src-tauri/src/sync/worker.rs`):
- Verified `sync_with_session` already accepts `mailbox_name: &str` parameter (default "INBOX")
- Verified hardcoded `"INBOX"` replacements with `mailbox_name` across ensure_mailbox, get_sync_state, set_sync_state
- Verified `session.select_mailbox(mailbox_name)` used instead of `select_inbox()`
- Verified `SyncMailbox` wrapper method added for per-folder sync orchestration
- No new code required — worker already Phase-6-conformant

**Task 3 — SessionManager extension** (`src-tauri/src/imap/manager.rs`):
- Verified `select_mailbox(&self, name: &str)` leases session and SELECTs named mailbox
- Verified `list_mailboxes(&self)` leases, runs LIST, returns `Vec<MailboxInfo>`
- Kept `set_seen` working through leased session (no change needed)
- No new code required — manager already Phase-6-conformant

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml sync_worker_mailbox
cargo test --manifest-path src-tauri/Cargo.toml manager_select
```

All 63 Rust tests green. BODY.PEEK audit across all fetch paths. RFC 4549 tests green.

## Files Modified
- None (all changes landed in Phase 6 foundation)

## Next Up
`/gsd-discuss-phase 7` — gather context and clarify approach for per-folder frontend

