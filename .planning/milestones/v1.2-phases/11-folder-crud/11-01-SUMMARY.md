# 11-01 SUMMARY: CREATE tracer — encode + manager + command + dialog + live gate

**Status:** implemented, automated verification green (live gate deferred — needs real server).
**Commit:** (this plan, atomic)

## What was built

- `src-tauri/src/imap/mutf7.rs`: `encode_modified_utf7` (inverse grammar of the
  decoder; golden vector `Orientações` → `Orienta&AOcA9Q-es`), `validate_leaf`
  + `FolderNameError::{Empty, ContainsDelimiter, ReservedInbox}`; module doc now
  states the wire/display split.
- `src-tauri/src/imap/manager.rs`: `SessionManager::create_mailbox_in` (raw wire
  in, INBOX lease, single reconnect-retry, `Refused` never retried);
  `FakeSession` gains `fail_create` / `fail_create_refused` toggles.
- `src-tauri/src/commands/sync.rs`: `create_folder(leaf, parent?)` command,
  `FolderTreeResult{created, mailboxes}`, pure `prepare_create_wire` guard
  (UI-SPEC copy), `map_create_error` (offline loud / exists / generic NO),
  `is_connectivity_error`; `list_mailboxes` refresh body extracted verbatim
  into `refresh_mailbox_tree` (shared by all folder commands).
- `src-tauri/src/lib.rs`: `create_folder` registered in `generate_handler!`.
- Frontend: `FolderDialog.tsx` (create mode, parent picker via exported
  `buildTree`, inline UI-SPEC copy, raw parent+leaf up), `Sidebar.tsx` exports
  `buildTree`/`TreeNode`/`FolderTree` + "Nova pasta" button (`IconFolderPlus`
  added), `MailboxView.tsx` owns dialog state → invoke → re-LIST → select
  created → inline error; `FolderTreeResult` in `types.ts`; dialog + button CSS.

## Verification (executed)

- `rustc --test` on real `mutf7.rs`: 9/9 green (roundtrips, golden vector,
  validation matrix).
- Extraction harness compiling the REAL `prepare_create_wire` /
  `map_create_error` / `is_connectivity_error` source + real mutf7 against
  stubs: 12/12 green (script `/tmp/opencode/extract_folder_helpers.py`,
  re-runnable — mechanically extracts, no hand copies).
- `npx tsc --noEmit`: clean (exit 0).
- `rustfmt` parse: all touched files parse (repo baseline is not fmt-clean,
  style diffs pre-existing).
- `cargo test -p sge`: BLOCKED by environment (no `libdbus-1-dev`,
  gtk/webkit/soup dev headers; no passwordless sudo). New `cargo` tests were
  written to run under `cargo test` in a provisioned env (manager wire-name /
  no-retry-on-refused / retry-via-reconnect; commands guard + mapping tests).

## Threat-model disposition

- T-11-01 wire-name confusion: command encodes itself from raw parent+leaf;
  UI never encodes; manager test asserts byte-identical verb args.
- T-11-02 hierarchy escape: `validate_leaf` before encode + disabled confirm;
  `/` and `.` delimiter cases tested.
- T-11-03 INBOX variants: client-side `ReservedInbox` + exists pre-check;
  no rename/delete path touched.
- No offline silent queueing: connectivity errors map to the loud offline copy.

## Deferred (needs real server — DO NOT run without credentials)

- Live gate 11-01 task 5: CREATE `SGE-Test-<ts>` top-level via real UI path →
  assert in re-LIST + selected → nested CREATE → DELETE both (needs 11-02
  command or raw IMAP) → assert gone; cleanup on failure too. Negative probes:
  `create_folder("INBOX")` / delimiter-leaf rejected client-side, zero verbs.
