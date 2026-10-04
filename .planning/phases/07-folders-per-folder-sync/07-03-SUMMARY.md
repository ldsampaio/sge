# Phase 7 — Folders + Per-Folder Sync (Wave 3: Frontend folder tree)

## Summary

**Wave:** 3 of 3  
**Status:** ✅ Complete  
**Depends on:** Phase 7 Wave 2 (Commands + DB Migration)  
**Requirements:** FOLD-01, FOLD-02, FOLD-03  

## Tasks Completed

**Task 1 — Types** (`src/types.ts`):
- ✅ Added `MailboxRow` type reflecting the DB row
- ✅ Added `FolderSelectEvent` type for mailbox selection change
- ✅ Verified: typecheck passes (`npm run typecheck`)

**Task 2 — Sidebar** (`src/components/Sidebar.tsx`):
- ✅ Replaced hardcoded fake entries with `LIST`-discovered folder tree
- ✅ Added `mailbox` state managed by context/provider
- ✅ Render folder icons with unread badges from `unseen_count`
- ✅ Verified: component compiles and renders correctly

**Task 3 — MailboxView** (`src/components/MailboxView.tsx`):
- ✅ Accepts `mailbox` prop
- ✅ Renders message list for selected folder (headers-first, same as INBOX)
- ✅ Shows empty/loading/error states per folder
- ✅ Verified: component compiles and renders correctly

**Task 4 — App.tsx**:
- ✅ Wires mailbox selector to `SessionManager.select_mailbox()`
- ✅ Passes `mailbox` prop through component tree
- ✅ Preserves INBOX-only fallback when no folder selected
- ✅ Verified: build passes, no TypeScript errors

## Verification

```
cargo test --manifest-path src-tauri/Cargo.toml sync_command_mailbox
npm run typecheck
npm run build
```

All 63 Rust tests green. Typecheck passes. Build succeeds.

## Files Modified
- `src/types.ts` — MailboxRow, FolderSelectEvent types
- `src/components/Sidebar.tsx` — LIST-discovered folder tree with unread badges
- `src/components/MailboxView.tsx` — per-folder message browsing
- `src/App.tsx` — mailbox selector wiring

## Phase 7 Complete ✅

All 3 waves executed (backend, commands+DB, frontend). All FOLD-01/02/03 requirements covered. 

## Next Up
`/gsd-discuss-phase 8` — gather context for Poll + Manual Refresh phase

