---
status: findings-fixed
phase: 11
files_reviewed: 24
findings:
  critical: 1
  major: 3
  minor: 3
  info: 4
total: 11
fixes_committed:
  - fix(11-review): typed_name arg, leaf-only encode, parent/delimiter guards
tests:
  helpers_harness: 35 passed
  store_harness: 48 passed
  tsc: clean
  eslint: clean
  vite_build_node20: green
---

# Code Review — Phase 11 (Folder CRUD)

Scope: commits `48c68b1` (11-01 CREATE), `aa60ad7` (11-02 RENAME/DELETE),
`16025b4` (11-03 roles/menu/modals). Reviewed the full
`48c68b1^..16025b4` diff (backend + frontend). No live server connections
attempted; verification via extraction/store harnesses, `tsc`, `eslint`,
`vite build` (node 20).

## Critical

### CR-01 — `delete_folder` typed-confirm arg never arrived (non-empty delete impossible)
`src/components/MailboxView.tsx` invoked `delete_folder` with
`typedName`, but the Tauri command binds `typed_name: Option<String>`
(`src-tauri/src/commands/sync.rs:1297`). Tauri matches arg keys exactly,
so the backend always saw `None` → every confirmed non-empty delete
returned `need_typed_confirm:{display}` and the folder could never be
deleted. **Fixed:** invoke now sends `typed_name: typedName`.
(`create_folder` `{leaf, parent}` and `rename_folder` `{old, new_leaf}`
were checked and already match.)

## Major

### MJ-01 — CREATE/RENAME double-encoded non-ASCII parent wire names
`prepare_create_wire` and `guard_rename` ran `encode_modified_utf7` over
the whole `parent + delimiter + leaf` string, but the parent is already a
RAW wire name. Any `&…-` shift in the parent (`Caf&AOk-`) was corrupted
(`&` → `&-`), issuing the verb against a wrong/non-existent name.
**Fixed:** only the user-typed leaf is encoded; the cached wire parent is
joined byte-identical. Regression tests:
`prepare_create_wire_never_reencodes_parent`,
`guard_rename_never_reencodes_parent_prefix`.

### MJ-02 — `create_folder` silently mis-joined unknown/flat parents
An unknown parent (stale picker selection) fell through to `""`
delimiter, producing a garbage concatenated top-level name
(`"Pai" + "Filho"` → `"PaiFilho"`) that was CREATEd on the server.
A known parent with an empty delimiter (flat namespace) had the same
join defect. **Fixed:** unknown parent refuses with the refresh copy
before any verb call; parent + empty delimiter refuses with
"não aceita subpastas" before any verb call.

### MJ-03 — Rename could silently promote nested folders to top level
With a pre-M6 cached row (empty `delimiter`) and a hierarchical wire
name (`Pai/Sub`), `guard_rename` skipped the parent prefix and renamed
to a top-level name. **Fixed:** `effective_delimiter()` infers the
unambiguous single-kind delimiter (`/` xor `.`); ambiguous wires carrying
both refuse with the refresh copy. The command reuses the same helper for
the cache migration, so guard and migration can never disagree on the
prefix. Regression test: `guard_rename_infers_missing_delimiter`.

## Minor

### MN-01 — `rename_mailbox_cache` LIKE prefix is ASCII case-insensitive
SQLite `LIKE` folds ASCII case, so renaming `Pai` could also have moved a
case-differing `PAI/X` subtree. **Fixed:** coarse LIKE pre-filter kept,
Rust-side `starts_with(old_prefix)` decides (byte-exact). Regression
test: `rename_mailbox_cache_prefix_match_is_case_sensitive`.

### MN-02 — `refresh_mailbox_tree` held the store `MutexGuard` across STATUS awaits
The lock spanned one network round-trip per folder, stalling all other
store readers for the whole refresh. **Fixed:** probes collected
lock-free, then a single brief locked write section. No behavior change
(same writes, same order, same error copy).

### MN-03 — Raw `need_typed_confirm:{display}` shown to users on typed race
`handleDeleteConfirm` only parsed `need_delete_confirm`. **Fixed:** the
typed race now surfaces "O nome da pasta mudou — digite {display} para
confirmar." (Reachable only on a genuine rename race now that CR-01 is
fixed, since the modal already enforces an exact client-side match.)

## Informational (no change — verified safe or by design)

- **IN-01 — Trash parity deviations are strictly safer.** `detect_trash`
  now delegates to `resolve_roles`; the 8 legacy `trash.rs` tests pass
  UNCHANGED (name lists MOVED, not copied — no second detector). Two
  uncovered edge inputs change outcome: INBOX carrying `\Trash` no longer
  resolves Trash (old code returned INBOX as Trash — a wrong-target
  SELECT), and a `Trash`-named folder carrying `\Sent` resolves Sent
  (attribute-first schema). Both directions reduce destructive-op risk.
- **IN-02 — System-role delete stays allowed by design.** UI-SPEC only
  requires rename-disabled for system roles and delete-disabled for
  INBOX/`\Noselect`; backend matches spec (`guard_rename` double-guards
  Trash/Sent/Drafts, `guard_delete` refuses INBOX/`\Noselect`/children).
  Server NO on DELETE maps to the exact UI-SPEC refusal copy.
- **IN-03 — DELETE/RENAME lease SELECTs the target first.** Harmless on
  Dovecot-class servers; noted because a strict server could balk at
  DELETE of the selected mailbox. Left as-is (no live gate available to
  validate an INBOX-lease change); `SELECT` never sets `\Seen`.
- **IN-04 — `is_connectivity_error` substring matching** could classify a
  server NO containing "connect…" (e.g. `[UNAVAILABLE] connection
  limit`) as offline. Fail direction is safe (loud retry copy, no verb
  retried, no queue); accepted.

## Verified clean (no findings)

- **BODY.PEEK compliance:** no FETCH paths added; CREATE/RENAME/DELETE are
  name-only verbs with no UID args and no streams to drain.
- **Refused never retried:** all three manager methods return `Refused`
  before the reconnect path; covered by fake-session tests.
- **Stream/error hygiene:** `SyncError::Protocol` carries op + server
  text only; passwords are `Zeroizing`, redacted from `Debug`, and
  scrubbed from transcripts (`imap/mod.rs` `transcript_redacts_secrets`).
- **NAMESPACE:** tripwire test extended to the new verbs; no banned
  command issued (grep-clean by construction).
- **UID-only addressing / destructive-op safety:** rename preserves
  `mailbox_id`/`uid_validity`/messages (prefix migration, no refetch);
  delete requires count + typed double-confirm with zero verbs on every
  refusal path; stale-selection migration returns `{old, new}` and the UI
  migrates atomically with INBOX fallback on delete.
- **display_name staleness (checked, not a bug):** `display_name` is
  computed as `decode(name)` in `list_mailboxes`, so the name-only cache
  migration cannot leave stale display text.
- **`\Noselect` spelling (checked, not a bug):** all checks use the
  mapper's canonical `\NoSelect` spelling consistently (LIST →
  `name_attribute_to_string` → M8 cache → guards → frontend rows).
- **Result shapes:** `FolderTreeResult`/`RenameFolderResult`/
  `DeleteFolderResult` field names match frontend consumption exactly
  (post CR-01 fix).

## Verification after fixes

- Extraction harness (real `sync.rs` guards + real mutf7/trash/roles):
  **35/35 green** (32 pre-existing + 3 new regression tests).
- Store harness (real `store/mod.rs` + `queries.rs` via `cargo test`):
  **48/48 green** (incl. new case-sensitivity test + M8 migration tests).
- `tsc --noEmit`: clean. `eslint` on touched components: clean.
- `vite build` green under node 20 (system node 18 cannot build —
  pre-existing environment limitation, unrelated to this change).
- `cargo test` (full Tauri crate) still blocked by missing system
  headers (dbus/gtk/webkit) — pre-existing, unchanged by this review.
- Live-server gates remain deferred (need `mail.utfpr.edu.br`
  credentials) — unchanged from phase summaries.
