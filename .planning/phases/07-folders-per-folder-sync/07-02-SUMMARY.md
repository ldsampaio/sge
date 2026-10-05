# Phase 7 — Folders + Per-Folder Sync (Wave 2: Commands + DB Migration)

## Summary

**Wave:** 2 of 3
**Status:** ⚠️ PARTIAL — corrected 2026-10-05 (prior revision claimed complete; audit found Task 1 and parts of Task 3 unimplemented)
**Depends on:** Phase 7 Wave 1 (Backend: SyncSession + SyncWorker multi-folder)
**Requirements:** FOLD-01, FOLD-02, FOLD-03

## Tasks — Actual State

**Task 1 — DB Migration M3** (`src-tauri/src/store/mod.rs`): ❌ NOT DONE
- `SCHEMA_VERSION` is still 2; no `unseen_count` column exists; `m3_adds_unseen_count_column` was never written.
- Gap-closure decision (2026-10-05): DROP M3. Unread counts are computed dynamically from `messages.flags` JSON (`list_mailboxes`, `count_unread`), which stays consistent across flag toggles without a cache-invalidation path. No migration needed.

**Task 2 — Query functions** (`src-tauri/src/store/queries.rs`): ✅ DONE
- `list_mailboxes(conn) -> Vec<MailboxRow>` present; `MailboxRow { id, name, uid_validity, uid_next, last_sync_at, unread_count }` present (field named `unread_count`, computed dynamically — supersedes planned `unseen_count` column).
- `count_unread(conn, mailbox)` present.
- ❌ `set_mailbox_status` NOT written — open item for gap closure (STATUS UNSEEN caching).

**Task 3 — Tauri commands** (`src-tauri/src/commands/sync.rs`): ⚠️ PARTIAL
- ✅ `start_sync` generalized with `mailbox: String` param, passed to `sync_with_session`.
- ❌ `list_folders`/`list_mailboxes` command NOT written and NOT registered in `lib.rs` `invoke_handler!` — but the frontend (`MailboxView.tsx`) already calls `invoke("list_mailboxes")`, so the folder tree is broken at runtime. Open item for gap closure (command + registration).
- ❌ `sync_status` / `list_messages` / `fetch_message` mailbox generalization — unverified, open item for gap closure.

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml sync_command_mailbox   # MISSING — never written
cargo test --manifest-path src-tauri/Cargo.toml m3                     # DROPPED with M3 decision
```

Suite at audit: 90 passed, 0 failed (no folder-gate tests exist).

## Files Modified (real)

- `src-tauri/src/store/queries.rs` — MailboxRow, list_mailboxes, count_unread
- `src-tauri/src/commands/sync.rs` — start_sync mailbox param

## Open Items → Gap Closure

1. `list_mailboxes` Tauri command + `invoke_handler!` registration (runtime break).
2. `set_mailbox_status` + STATUS UNSEEN wiring (FOLD-02 criterion literal).
3. Mailbox params on sync_status / list_messages / fetch_message (verify or add).
4. Per-folder UIDVALIDITY isolation test (FOLD-03 criterion 4).
5. `sync_command_mailbox` gate test.
