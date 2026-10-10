# 11-02 SUMMARY: RENAME + DELETE — verbs, cache ops, commands, live gates

**Status:** implemented, automated verification green (live gates deferred — needs real server).
**Commit:** (this plan, atomic)

## What was built

- `imap/mod.rs`: `SyncSession::rename_mailbox` / `delete_mailbox` (name-only,
  no-stream verbs); `MailboxStatus.messages` (STATUS MESSAGES datum);
  `BoxedSession` impls + STATUS item list extended.
- `imap/manager.rs`: `rename_mailbox_in` (INBOX-lease pre-STATUS, `lease_for(old)`
  + single reconnect-retry, `Refused` never retried, lease dropped before the
  post-rename `mailbox_status(new)` UIDVALIDITY check → `State` on bump) and
  `delete_mailbox_in`; `FakeSession` gains `renamed_calls`, `deleted_mailboxes`,
  `fail_rename/fail_delete`, `fail_rename_refused/fail_delete_refused`,
  `status_messages`, `bump_status_validity`.
- `store/queries.rs`: `rename_mailbox_cache` (exact + LIKE-escaped prefix
  UPDATEs, rows-touched count, id/validity/messages preserved),
  `delete_mailbox_cache` (row DELETE + both outbox drops, unknown = noop).
- `commands/sync.rs`: `rename_folder` / `delete_folder` (+ `RenameFolderResult`
  incl. UIDVALIDITY `warning`, `DeleteFolderResult` with INBOX fallback),
  pure `guard_rename` / `guard_delete` (+ `DeleteDecision`), `is_system_role`
  (Trash via single `detect_trash`, Sent/Drafts name lists with 11-03 handoff
  note), `map_folder_error` (offline/create-exists/delete-NO/probe copies),
  `has_noselect_attr`; `refresh_mailbox_tree` reuses the helper.
- `lib.rs`: both commands registered. All other `SyncSession` implementors
  updated (worker/bodies/sync_demo mocks).
- No UI in this plan (arrives 11-03); commands are callable + tested headless.

## Compile-gate deltas (vendored async-imap 0.11.3, via `cargo fetch` source)

- `rename(from, to)` / `delete(mailbox_name)` names match the plan — no delta.
- STATUS MESSAGES has NO dedicated vendored field: `parse_status` folds
  `StatusAttribute::Messages` into `Mailbox.exists`; `mailbox_status` maps
  `messages` from `.exists` (documented at the trait method + struct field).

## Verification (executed)

- Store harness (real `store/mod.rs` + `queries.rs` + `mutf7.rs` by path,
  `cargo test`): 45/45 green, incl. new subtree-rename (Pai→Novo moves
  Pai/Sub, Pai2 untouched), delete-cascade (messages + both outboxes), noop.
  The harness caught 3 real bugs pre-commit (borrow, two type errors).
- Extraction harness (real guard/mapping source + real mutf7 + real trash):
  24/24 green — guard ordering, zero-verb refusals, count/typed gates,
  per-op copies. Caught 2 wrong test expectations (fixed tests, not source).
- `rustfmt` parse: all 8 touched files OK. `npx tsc --noEmit`: clean
  (no frontend changes this plan).
- `cargo test -p sge`: still BLOCKED by environment (no dbus/gtk/webkit dev
  headers, no passwordless sudo). New manager/command `cargo` tests are
  written for a provisioned env (wire-identity, no-retry-on-Refused,
  retry-via-reconnect, UIDVALIDITY-bump `State`, messages passthrough).
- Incidental repair in this commit: 11-01's edit swallowed the
  `move_fallback_prefers_uid_move` header (body orphaned); restored + verified
  intact. Lesson recorded: anchor-integrity check after edits near tests.

## Threat-model disposition

- T-11-04 stale selection / refusal-path verbs: guards pure (zero verbs by
  construction, tested); rename returns `{old, new}` for atomic migration.
- T-11-05 subtree orphan: prefix UPDATEs per delimiter + `Pai2` non-match
  test (LIKE-escape).
- T-11-06 silent data loss: count gate + typed-name gate + server-NO mapping;
  empty-then-delete never automated. No folder-op queue invented (11-01 rule).

## Deferred (needs real server — DO NOT run without credentials)

- Live gates 11-02 task 5: RENAME `SGE-Test-<ts>` → `-b` (re-LIST new/old-gone,
  UIDVALIDITY unchanged) → nested CREATE + DELETE child + DELETE parent;
  negative probes RENAME/DELETE INBOX + DELETE `\Noselect` refused with zero
  verbs. Cleanup in reverse order on failure too.
