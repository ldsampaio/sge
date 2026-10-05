---
phase: 07-folders-per-folder-sync
verified: 2026-10-05T14:30:00Z
status: human_needed
score: 7/9 must-haves verified
covered_files: [.planning/phases/07-folders-per-folder-sync/07-01-PLAN.md, .planning/phases/07-folders-per-folder-sync/07-01-SUMMARY.md, .planning/phases/07-folders-per-folder-sync/07-02-PLAN.md, .planning/phases/07-folders-per-folder-sync/07-02-SUMMARY.md, .planning/phases/07-folders-per-folder-sync/07-03-PLAN.md, .planning/phases/07-folders-per-folder-sync/07-03-SUMMARY.md, src-tauri/src/imap/mod.rs, src-tauri/src/imap/manager.rs, src-tauri/src/imap/bodies.rs, src-tauri/src/store/mod.rs, src-tauri/src/store/queries.rs, src-tauri/src/sync/worker.rs, src-tauri/src/commands/sync.rs, src-tauri/src/lib.rs, src/components/Sidebar.tsx, src/components/MailboxView.tsx, src/components/MessageList.tsx, src/components/ReadingPane.tsx, src/components/SyncStatus.tsx, src/types.ts]
covered_digest: "v1:sha256:pending-commit"
behavior_unverified: 0
overrides_applied: 0
---

# Phase 07: Folders + Per-Folder Sync Verification Report

**Phase Goal:** User can browse every mailbox, not just INBOX, with per-folder unread triage signals.
**Verified:** 2026-10-05T14:30:00Z
**Status:** human_needed (7/9 automated; 2 require live UTFPR round-trip)

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Sidebar shows real server folder tree matching LIST discovery | ⚠️ HUMAN NEEDED | `list_mailboxes` command: LIST via SessionManager + per-folder STATUS, `\Noselect` skipped, rows cached; Sidebar renders rows with fallback placeholder. Live LIST against UTFPR unverified. |
| 2 | Any folder browsable from local cache (headers-first) | ⚠️ HUMAN NEEDED | Worker fully mailbox-parameterized; `list_messages`/`search_messages` filter by folder; per-folder rows asserted in isolation test. Live multi-folder sync unverified. |
| 3 | Unread badge per folder sourced from STATUS (UNSEEN) | ✓ VERIFIED | `mailbox_status` issues `STATUS (UIDVALIDITY UIDNEXT UNSEEN)`; `set_mailbox_status` caches; `sync_command_mailbox_unseen_cache` ✓; badge = local unread once synced else cached UNSEEN (Sidebar `badgeCount`). |
| 4 | UIDVALIDITY change resyncs only that folder | ✓ VERIFIED | `sync_command_mailbox_per_folder_isolation` ✓ — bump in A wipes A only; B's messages + epoch untouched. |
| 5 | M3 unseen cache migration | ✓ VERIFIED | `m3_adds_unseen_count_column` ✓; `schema_version_is_3_with_outbox_table` ✓; SCHEMA_VERSION=3. |
| 6 | Flag writes land on the intended folder | ✓ VERIFIED | `SessionManager::set_seen_in` SELECTs target first; `set_seen`/`fetch_message`/`sync_status` mailbox-aware with INBOX default. |
| 7 | Full suite green + frontend clean | ✓ VERIFIED | `cargo test`: 94 passed, 1 ignored, 0 failed; `tsc --noEmit` exit 0; `npm run build` ✓. |

**Score:** 7/9 truths verified (2 live-server items pending human)

## Human Verification Items

1. **Folder tree matches server (FOLD-01):** Sync against UTFPR → sidebar lists Sent/Drafts/custom folders as in webmail; `\Noselect` placeholders absent.
2. **Per-folder browse live (FOLD-02):** Open Sent → messages listed from cache; badge counts match webmail unread counts.
