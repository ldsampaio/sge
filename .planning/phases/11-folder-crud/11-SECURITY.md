# Phase 11 — Security Audit (Folder CRUD)

> Threat-mitigation verification for folder CREATE / RENAME / DELETE.
> Method: static code inspection only — no live server connections.
> Note: `cargo test` could not execute in this environment (missing
> system `libdbus-1-dev` for the `keyring`/`sync-secret-service` build);
> verdicts below rest on source evidence + existing unit tests by name.

**Verdict: OPEN_THREATS** (1 low-severity defense-in-depth gap; all else secured)

---

## Per-threat evidence

| # | Threat | Status | Evidence |
|---|--------|--------|----------|
| T-11-01 | Wire-name injection (delimiter / UTF-7 / quoting) | ✅ SECURED | `validate_leaf` (`imap/mutf7.rs:135`) rejects empty, delimiter-bearing, and INBOX-variant leaves pre-wire. Only the user leaf is encoded; cached parent prefixes pass through byte-identical (`commands/sync.rs:1052-1055`, `1170-1172`; tests `prepare_create_wire_never_reencodes_parent`, `guard_rename_never_reencodes_parent_prefix`). CRLF cannot reach the wire: control chars fall into the base64 shift run (`mutf7.rs:91-97`). `"`/`\` pass through raw but async-imap 0.11 `quote!` escapes both (`client.rs:2266-2272`, `create`/`delete`/`rename` all via `quote!`). |
| T-11-02 | Destructive verbs escape intended scope (blind expunge) | ✅ SECURED | No bare `expunge()` in folder paths. Message expunge is UID-scoped `UID EXPUNGE` in ~200-UID chunks (`commands/sync.rs:616-626` → `manager.uid_expunge_in` → `session.uid_expunge`, `imap/mod.rs:624-634`). MOVE fallback refuses loudly when the unmark-others dance is unverifiable — never a blind expunge (`manager.rs:318-322`). |
| T-11-03 | INBOX protection bypass | ✅ SECURED | Triple layer: UI disables both ops for INBOX (`FolderContextMenu.tsx:52-63`); `guard_rename` (`sync.rs:1178`) and `guard_delete` (`sync.rs:1228`) refuse case-insensitively pre-wire; `delete_folder` repeats a fast refusal before any I/O (`sync.rs:1376`). `validate_leaf` reserves all INBOX case variants on any delimiter (`mutf7.rs:145`). Covered by `guard_rename_refusals_and_happy_paths`, `guard_delete_refusals_and_confirm_gates`. |
| T-11-04 | Wrong-target RENAME (hierarchy escape / stale selection) | ✅ SECURED | Pure `guard_rename` validates before any verb; unknown names refuse; `effective_delimiter` refuses ambiguous `/`+`.` wires instead of guessing (`sync.rs:1148-1162`). Verb runs under `lease_for(old)` which SELECTs the target first; single-flight mutex serializes callers (`manager.rs:69-103`). Post-rename UIDVALIDITY bump surfaces as warning, never silent accept (`manager.rs:287-292`, `sync.rs:1310-1323`). |
| T-11-05 | Rename loses messages / orphans cache (mailbox_id churn) | ✅ SECURED | `rename_mailbox_cache` keeps `mailbox_id`/`uid_validity`/messages; subtree moves via LIKE-escaped prefix UPDATEs with byte-prefix check (`queries.rs:243-283`). Case-sensitivity test pins `Pai2` ≠ child of `Pai`. |
| T-11-06 | Non-empty / parent delete without confirmation | ✅ SECURED | `guard_delete`: children check over all cached delimiters (`sync.rs:1234-1248`, `Pai2` lookalike test), fresh-STATUS message count → `NeedCount` gate → typed-name `NeedTyped` gate (`sync.rs:1249-1254`), both returning `need_*` errors with zero wire calls. Parent-with-children refuses loudly. Frontend double modal (`FolderDeleteModal.tsx`) + destructive item never default-focused. |
| T-11-07 | Offline replay resurrects deleted folders / replays stale ops | ✅ SECURED | Folder ops are online-only: no folder-op queue exists; connectivity failures map to the loud offline copy (`map_folder_error` + `is_connectivity_error`, `sync.rs:926-945`). `delete_mailbox_cache` drops both durable outboxes (`flag_outbox`, `imap_outbox`) in the same lock section (`queries.rs:291-308`), so no queued flag/move op can replay against a deleted folder. Refresh recomputes roles from LIST every time (LIST is truth, column is cache). |
| T-11-08 | `\Noselect` targeting / selection confusion | ✅ SECURED | `\Noselect` filtered at three layers: skipped in LIST refresh (`sync.rs:879`), fresh-LIST attribute check in `delete_folder` before guards (`sync.rs:1381-1392`), UI renders no menu at all (`FolderContextMenu.tsx:102`). `roles.rs:123` forces `\Noselect` → `Custom`. Every verb SELECTs its exact target via `lease_for` (create: stable INBOX lease; rename: `old`; delete: `name`) — no op runs on a stale selection. |
| T-11-09 | Credential / secret leakage in errors / logs | ✅ SECURED | `ImapError` variants carry host/username only, never passwords (`errors.rs:1-6`); `AccountConfig` Debug redacts password + `Transcript::render` scrubs occurrences ≥3 chars (`imap/mod.rs:807-923`, tests `account_debug_redacts_password`). Manager module never formats config/password (`manager.rs:15-16`). Folder error paths surface only server detail + display names. eprintln! lines log mailbox names/UIDs, never secrets (grep-verified). |

---

## Open threats

1. **(Low) No backend system-role guard on DELETE.** `guard_rename` double-guards Trash/Sent/Drafts via the single `resolve_roles` schema (`sync.rs:1184`), and the UI disables rename for system roles — but `guard_delete` checks only INBOX/unknown/children/non-empty, and the context menu enables Excluir for system-role rows (`deleteDisabled = isInbox` only, `FolderContextMenu.tsx:60`). Deleting Trash/Sent/Drafts therefore depends on UI restraint alone; a direct `delete_folder` IPC invoke succeeds. Recommend mirroring the `is_system_role` refusal into `guard_delete` (resolved from the fresh LIST in `delete_folder`, same as the `\Noselect` check), or documenting system-role delete as intentional.
   - Staleness note: both role guards read the cached M8 `role` column; pre-refresh empty roles resolve `Custom` and pass. UI-disabled states cover the common path; the backend gap above is the sharper instance.

## Explicitly out of scope (manual live gates, per 11-VALIDATION.md)

CREATE → re-LIST → select, RENAME UIDVALIDITY preservation, DELETE cascade + INBOX fallback on a real server, Trash auto-detection on real LIST — all require live IMAP credentials; not executed here.
