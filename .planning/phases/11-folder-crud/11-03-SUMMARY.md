# 11-03 SUMMARY: Roles schema + sidebar UI + guards + tripwires

**Status:** implemented, automated verification green (live Trash gate deferred — needs real server).
**Commit:** (this plan, atomic)

## What was built

- `store/mod.rs` + `queries.rs`: M8 (`role`/`attributes` TEXT NOT NULL
  DEFAULT '', SCHEMA_VERSION 7 → 8) + `set_mailbox_role` upsert +
  `MailboxRow.role/attributes` + extended `list_mailboxes` SELECT +
  M8 preserve-rows test (v7 rows + delimiters survive, new cols defaulted).
- `imap/roles.rs` (new): `Role::{Inbox,Trash,Sent,Drafts,Custom}` +
  `as_str` wire values + `resolve_roles` (INBOX-first, then SPECIAL-USE
  attributes beating names across roles, then full-name, then leaf;
  `\Noselect` always Custom; first hit wins Trash > Sent > Drafts).
- `imap/trash.rs`: `detect_trash` is now a delegating wrapper over
  `resolve_roles` (attr-sourced Trash preferred, then first Trash entry);
  TRASH name lists MOVED to roles.rs (not copied); all 8 tests UNCHANGED.
- `commands/sync.rs`: `is_system_role` delegates to `resolve_roles` over
  cached M8 rows (no local lists — T-11-08); `refresh_mailbox_tree` persists
  role + space-joined attributes per folder on every LIST (T-11-07).
- `imap/session.rs`: NAMESPACE tripwire extended with the
  `folder_verbs_send_no_banned_command` source scan (mod.rs + manager.rs,
  comment lines excluded, needle built-not-written so the grep gate stays
  confined to session.rs).
- Frontend: `FolderContextMenu.tsx` (menu, arrows/Enter/Esc, INBOX +
  system-role disabled states with tooltips, `\Noselect` filtered),
  `FolderDeleteModal.tsx` (ExpungeModal skeleton, empty vs non-empty +
  typed-name gate), `FolderDialog` rename mode (leaf pre-fill, no parent
  picker), `Sidebar` right-click/Shift+F10 + keyboard menu + role glyphs
  (Trash/Sent/Drafts) with unchanged ordering, `MailboxView` rename
  (atomic selection migration) + delete (count probe → modal → INBOX
  fallback, raced `need_delete_confirm` bumps the modal variant);
  `MailboxRow.role/attributes` + `RenameFolderResult`/`DeleteFolderResult`
  in `types.ts`; menu CSS.

## Verification (executed)

- Store harness (real module by path, `cargo test`): 47/47 green, incl.
  M8 preserve-rows, `set_mailbox_role` upsert roundtrip, version-8 asserts.
- Extraction harness (real guards + real mutf7/trash/roles): 32/32 green —
  all 8 trash tests UNCHANGED pass against the delegating wrapper (parity
  proof), 8 new roles tests (attr-wins, Gmail-style, leaf, noselect,
  INBOX-never-Trash), refactored `is_system_role` covered via guard tests.
- Tripwire simulation (same predicate as the new test): zero non-comment
  `namespace` lines in mod.rs/manager.rs; `grep -rin namespace imap/`
  returns session.rs (tripwire) + pre-existing Phase-1 ban notes in
  probe.rs and mod.rs docs (baseline, untouched).
- `npx tsc --noEmit`: clean. `npx prettier --write` applied to touched TS;
  new `roles.rs` rustfmt-clean; all 12 touched Rust files parse.
- `cargo test -p sge`: still BLOCKED by environment (no dbus/gtk/webkit dev
  headers, no passwordless sudo). New tests are written for a provisioned
  env (roles unit tests, M8 tests, tripwire scan).
- Frontend build: `npx vite build` GREEN (36 modules, dist emitted) under
  local node 20.19.5 (system node is 18; vite 8 + eslint 10 require 20+).

## Threat-model disposition

- T-11-07 stale roles: column rewritten on every LIST refresh; guards read
  cached M8 rows (attributes included, so SPECIAL-USE wins server-true).
- T-11-08 second detector: TRASH lists MOVED (grep: single definition in
  roles.rs); `is_system_role` delegates; Sent/Drafts 11-02 local lists
  removed in this plan.
- T-11-09 destructive-default UI: Cancel default-focus, typed-confirm gate,
  INBOX/`\Noselect` never offered; red never color-alone (glyph + copy).
- Residual nuance (documented, no test impact): multi-candidate Trash
  priority is attr-first then input-order (old code was attr → full-name →
  leaf across mailboxes); all 8 legacy fixtures resolve identically.

## Deferred (needs real server — DO NOT run without credentials)

- Live Trash gate (task 4): resolve roles on the real LIST, log detected
  Trash + Sent + Drafts; assert no CREATE fires when Trash Found; if
  Missing, assert the UI confirm path triggers — do NOT confirm on the real
  account, cancel and record. Plus 11-01/11-02 gates per their summaries.
