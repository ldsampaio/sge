---
status: passed
phase: 11
goal: User can organize their mailbox tree — create, rename, delete folders — sidebar reflects real server tree
requirements: [FOLD-04, FOLD-05, FOLD-06]
score: 4/4
live_gates: deferred (need mail.utfpr.edu.br credentials — expected, non-blocking per precedent)
review_commit: 93ad77d
verified: 2026-10-06
---

# Phase 11 Verification — Folder CRUD

Method: goal-backward. Checked each ROADMAP success criterion against
shipped code (post-review commit `93ad77d`), not just plan completion.

## Must-haves

- [x] **Create via IMAP CREATE + LIST refresh** — `create_folder` command
  (`src-tauri/src/commands/sync.rs:968`) → `SessionManager::create_mailbox_in`
  (`src-tauri/src/imap/manager.rs:228`) → `refresh_mailbox_tree`
  (`sync.rs:843`); frontend `MailboxView.tsx:103` invokes, sets
  `res.mailboxes`, selects `res.created`. Leaf-only MUTF-7 encode +
  unknown/flat-parent refusals (MJ-01/MJ-02 fixes verified in source).
  Evidence: store/extract harnesses green per summaries; `tsc` clean (re-run here).
- [x] **Rename + cache invalidation, UIDs preserved, no stale selection** —
  `rename_folder` (`sync.rs:1280`) with pure `guard_rename` (INBOX +
  system-role refusal, delimiter inference MJ-03) →
  `rename_mailbox_in` (`manager.rs:271`, UIDVALIDITY pre/post check) →
  `rename_mailbox_cache` (`sync.rs:1327`, byte-exact prefix migration MN-01) →
  re-LIST (`sync.rs:1332`); frontend migrates selection atomically to new
  name (`MailboxView.tsx:126`). Rename mode in `FolderDialog` (leaf
  pre-fill, no parent picker) + context-menu wiring (`Sidebar.tsx:266`).
- [x] **Delete + INBOX protection + non-empty guard + cache cascade** —
  `delete_folder` (`sync.rs:1363`) with `guard_delete` (INBOX/`\Noselect`
  refusal, count + typed double gate) →
  `delete_mailbox_in` (`manager.rs:300`) →
  `delete_mailbox_cache` (`sync.rs:1433`, messages + both outboxes) →
  re-LIST with INBOX fallback (`sync.rs:1437-1441`). CR-01 fix confirmed:
  frontend sends `typed_name: typedName` (`MailboxView.tsx:177`) matching
  backend binding. Delete modal (`FolderDeleteModal.tsx`) + count probe
  + raced-confirm variant handled (MN-03).
- [x] **Trash auto-detect + one-time CREATE fallback behind confirmation** —
  `resolve_roles` (`src-tauri/src/imap/roles.rs:119`: INBOX-first,
  SPECIAL-USE > names, `\Noselect`→Custom) with `detect_trash` delegating
  (`trash.rs:30`, 8 legacy tests unchanged = parity proof);
  `refresh_mailbox_tree` persists role+attributes per folder
  (`sync.rs:911`); `is_system_role` delegates to cached M8 rows
  (no second detector, T-11-08). Phase 10 CREATE-fallback behavior kept.
  M8 migration (`role`/`attributes`, preserve-rows test) green per summary.

## Cross-cutting invariants (spot-checked)

- BODY.PEEK untouched: folder verbs are name-only, no FETCH/UID paths added.
- `Refused` never retried in all three manager methods (early return pre-reconnect).
- NAMESPACE still banned: tripwire extended to new verbs (`session.rs`).
- Raw wire names on the wire; display names decoded in UI only.
- Store mutex never held across awaits post MN-02 fix.

## Deferred live items (expected gaps, non-blocking)

1. 11-01 CREATE gate: real CREATE → re-LIST → nested CREATE → DELETE cleanup.
2. 11-02 RENAME/DELETE gates: real rename (UIDVALIDITY unchanged), nested
   delete sequence, INBOX/`\Noselect` negative probes with zero verbs.
3. 11-03 Trash gate: real-LIST role resolution; Missing path must CANCEL
   at confirm, never create on the real account.

Unit + wire-identity + harness coverage (helpers 35/35, store 48/48,
tsc/eslint clean, vite green per 11-REVIEW.md; tsc re-verified clean here)
suffices for `passed` with deferred live items, per project precedent
(Phase 10 shipped the same way). Full `cargo test` remains env-blocked
(missing dbus/gtk/webkit headers — pre-existing, unrelated).
