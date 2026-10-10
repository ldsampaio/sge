# Phase 11: Folder CRUD — Research

**Researched:** 2026-10-06
**Method:** inline orchestrator research (no `gsd-phase-researcher` agent available in this runtime; codebase evidence cited per finding)
**Inputs:** `11-CONTEXT.md`, REQUIREMENTS.md (FOLD-04/05/06), `10-PATTERNS.md`, live source at `src-tauri/src/`, `src/`

---

## 1. Transport: CREATE / RENAME / DELETE verbs (async-imap 0.11)

- `SyncSession::create_mailbox` already exists (Phase 10, `imap/mod.rs` 347 + impl 662-671): `self.create(&name_owned).await`, error shape `SyncError::Protocol("CREATE {name}: {e}")`. **No response stream to drain** — unlike `set_seen`, CREATE returns no per-message stream, so no `try_collect` is needed. Same will hold for RENAME/DELETE.
- async-imap 0.11 `Session` mirrors the sync `imap` crate surface: `create`, `delete(mailbox)`, `rename(from, to)` all exist as `async fn`s returning `Result`. No vendored source on disk (no cargo registry in this environment) — **Plan 11-02 task 1 carries an explicit compile gate** (`cargo test -p sge imap::`) so a name mismatch (`rename` vs alternatives) fails fast at compile time, not at review.
- New trait verbs follow the `set_seen` template minus the drain: `fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>>` and `fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>>`, errors `SyncError::Protocol("RENAME {old} -> {new}: {e}")` / `("DELETE {name}: {e}")`.
- Rule inherited from Phase 10: never parse untagged extended responses; RENAME/DELETE have none to parse. NAMESPACE stays banned — tripwire test at `imap/session.rs` 693-719 asserts the vendored parser still rejects NAMESPACE; Plan 11-03 extends it to RENAME/DELETE-adjacent parser coverage (no new commands sent).

## 2. RFC 9051 semantics the plans must encode

| Op | Server behavior | Plan consequence |
|---|---|---|
| `CREATE INBOX` (any case) | Fails (INBOX always exists) | Client-side reject of INBOX-case variants with plain-language error (CONTEXT locked) + map server NO |
| `CREATE existing` | `NO` already exists | Map to "já existe" guidance; after success re-LIST and select new folder |
| `CREATE` with bad hierarchy | `NO` (bad separator, `\Noselect` parent) | Validate leaf has no delimiter client-side; parent from picker is always a real folder |
| `RENAME INBOX x` | Special case: server COPIES INBOX content, keeps INBOX | **Blocked client-side** (CONTEXT locked) — never sent |
| `RENAME a b` where b exists | `NO` | Map to plain-language error, no local state touched |
| `RENAME` UID/UIDVALIDITY | UIDs preserved; UIDVALIDITY SHOULD be preserved (Dovecot/Cyrus preserve) | Keep local `mailbox_id` row, UPDATE name only; **still re-verify UIDVALIDITY post-rename** via STATUS and treat a bump like any epoch change (cheap insurance, one STATUS call) |
| `RENAME parent` with inferiors | Server renames whole subtree (Dovecot/Cyrus) | Local cache must UPDATE all rows with prefix `old + delimiter`, not just the exact row |
| `DELETE INBOX` | Always fails | Blocked client-side AND server error mapped (defense in depth, CONTEXT locked) |
| `DELETE` with child mailboxes | Fails on most servers | Refuse client-side when LIST shows children (offer rename/move guidance); never rely on server alone |
| `DELETE` non-empty (no children) | Allowed by RFC; some servers (Exchange-compat, some Cyrus configs) refuse | Guard with STATUS MESSAGES count + confirm modal, then ATTEMPT and map NO to plain-language error — no silent empty-then-delete, ever |

## 3. Modified-UTF-7 ENCODE is new work

- `imap/mutf7.rs` is **decode-only** and says so explicitly: "This module only decodes; encoding is never needed (the client never creates folders in v1.x)." Phase 11 invalidates that comment.
- Plan 11-01 adds `encode_modified_utf7`: printable ASCII (except `&`) passes through (so hierarchy delimiters `/`/`.` are never encoded); `&` → `&-`; non-ASCII runs → UTF-16BE → modified base64 (`,` for `/`, no padding). Roundtrip property tests: `decode(encode(x)) == x` for ASCII, `&`, Latin-1 (`Lixeira & Cia`), CJK; delimiter-in-leaf rejected BEFORE encode by the validation helper (encode never sees a delimiter decision).
- Wire rule (CONTEXT locked): raw encoded names on the wire (`CREATE`/`RENAME`/`DELETE`/`SELECT`), decoded `display_name` in UI. `MailboxInfo.name` stays raw.

## 4. Delimiter + NAMESPACE ban

- Delimiter comes ONLY from LIST responses, persisted per mailbox by M6 (`set_mailbox_delimiter`, `store/mod.rs` 182-184). Nested name join is `parent + delimiter + leaf` using the PARENT's cached delimiter; top-level create uses no delimiter.
- NAMESPACE is never sent; tripwire at `session.rs` 693-719 extended in Plan 11-03.

## 5. `mailboxes` bookkeeping (M8)

- Current table (`store/schema.sql` 18-25 + M6): `id, name UNIQUE, uid_validity, uid_next, highest_modseq, last_sync_at, delimiter`. `sync_state` is folded into the row; `messages.mailbox_id` has `ON DELETE CASCADE`.
- M8 adds two columns, forward-only: `role TEXT NOT NULL DEFAULT ''` (resolved role: `trash|sent|drafts|inbox|custom`) and `attributes TEXT NOT NULL DEFAULT ''` (space-joined LIST attributes for `\Noselect`/`\Noinferiors`/SPECIAL-USE checks). Preserve-rows test mirrors `m2_upgrades_v1_database_forward_preserving_rows` (`store/mod.rs` ~470-511); `SCHEMA_VERSION` 7 → 8.
- Rename cache op: `UPDATE mailboxes SET name = ? WHERE name = ?` + `UPDATE ... SET name = replace(...) WHERE name LIKE 'old/delim%'` per child delimiter (children share the parent's delimiter). Keep `uid_validity`, `id`, rows untouched.
- Delete cache op: `DELETE FROM mailboxes WHERE name = ?` (messages cascade via FK) + `drop_outbox_for_mailbox` + `drop_imap_outbox_for_mailbox` (`queries.rs` 937, 1103 already exist) in the same lock section. Children must already be gone (client-side children guard in §2), assert zero remaining `LIKE` rows after delete in tests.

## 6. Roles schema (generalizes Phase 10 `trash.rs`)

- `detect_trash` layers (SPECIAL-USE → full-name → leaf-name, `\Noselect` skipped) are correct and tested (8 tests). Plan 11-03 adds `imap/roles.rs` with `resolve_roles(mailboxes) -> Vec<(wire_name, Role)>` covering Trash/Sent/Drafts via the same layer order; **`detect_trash` delegates to it** (thin wrapper kept for the `delete_message` call site) so there is never a second detector to drift — this is the assumption-delta answer (§10).
- Name-match lists reuse Sidebar `SYSTEM_ORDER` ranks 1/2/4 (`drafts/rascunhos`, `sent/sent messages/enviadas/enviados`, `trash/deleted/deleted messages/lixeira`) + the Gmail-style/`[Gmail]/` variants already in `trash.rs`.
- Trash Missing path unchanged: `need_trash_confirm:` error → user confirms → one-time `create_trash()` (never silent, no twin-trash). `AppState::trash_cache` (`lib.rs` 40-53) stays; role resolution itself is recomputed on every LIST refresh and persisted to the M8 `role` column (cache survives restarts, recompute keeps it honest).

## 7. STATUS MESSAGES for the non-empty guard

- `MailboxStatus` (`imap/mod.rs` 366-370) has `uid_validity/uid_next/unseen` — no message count. The delete guard needs `STATUS <mb> (MESSAGES)`.
- Decision: extend the existing `mailbox_status` STATUS item list with `MESSAGES` and add `messages: u32` to the struct. All construction sites updated in the same task (compiler-guided: `BoxedSession` impl ~708, `MockSession` in `manager.rs` tests, any probe fixtures). No second STATUS round-trip, no new verb. `mailbox_status` is also called per folder in `list_mailboxes` refresh — the extra datum is free.

## 8. Commands + UI surface

- Command skeleton: `set_seen`/`delete_message` (`commands/sync.rs` 179-276, 361-480) — `mailbox: Option<String>` → INBOX default, `load_account_config` → `manager_for` → `spawn_blocking`+`block_on`, lock never across `.await`, result `{ acked, pending_count, detail }`. Folder ops need no outbox (mailbox tree ops are online-only: fail loudly when offline — no queue exists for CREATE/RENAME/DELETE and inventing one is out of scope).
- `list_mailboxes` command (819-...) already skips `\Noselect`, does STATUS per folder, and has the offline-cached fallback — folder ops reuse its refresh path verbatim (call `list_mailboxes` logic post-mutation, then return the fresh tree).
- Selection state lives in `MailboxView` (`selectedMailbox` useState, line 17) passed to `Sidebar`/`MessageList`/`ReadingPane`. Rename: `setSelectedMailbox(new)` in the SAME handler tick as the re-LIST (atomic from the user's view — CONTEXT locked "no stale selection"). Delete: `setSelectedMailbox("INBOX")`.
- `buildTree` (`Sidebar.tsx` 46-95) is reused for the create parent-picker (export it) and for the context-menu tree. `SYSTEM_ORDER` stays the name-match source.
- `ExpungeModal.tsx` is the exact modal template: portal, `role="alertdialog"`, focus-Cancel-default, Esc/backdrop-cancel, focus trap + return, backdrop `rgba(30,27,75,.45)`, destructive `#dc2626` clay pill, pt-BR copy. The folder-delete modal copies this skeleton with folder copy ("Apagar pasta {nome}?" + count line + double confirmation for non-empty).

## 9. Assumption-delta checkpoint (advisory — FIRED, answered here)

- Detector fired on pluralization cue "fallback" (roadmap criterion 4: Trash CREATE fallback). Question: does generalizing `detect_trash` into a roles schema create a second source of truth for "where is Trash"?
- Answer: NO second detector — `detect_trash` becomes a delegating wrapper over `resolve_roles` (same layers, one code path, existing 8 tests keep passing unchanged). `delete_message`'s call site is untouched. Recorded so the planner does not invent a parallel `find_trash_v2`.

## 10. Live-gate plan (mail.utfpr.edu.br, per CONTEXT locked decision)

One live gate per roadmap hard constraint, sequenced with cleanup, never touching INBOX or real user folders:

1. CREATE `SGE-Test-<ts>` top-level → assert in re-LIST → (gate 1: criterion 1).
2. RENAME `SGE-Test-<ts>` → `SGE-Test-<ts>-b` → assert selection migrated + re-LIST shows new, old gone → (gate 2: criterion 2).
3. CREATE nested `SGE-Test-<ts>-b/Sub` → DELETE `Sub` → DELETE parent → assert both gone from re-LIST → (gate 3: criterion 3).
4. Trash auto-detection probe: resolve roles on the real LIST, log detected Trash (no CREATE unless Missing + confirmed) → (gate 4: criterion 4).
5. Negative probes (no state change): CREATE INBOX-variant rejected client-side; RENAME INBOX blocked client-side; DELETE INBOX blocked client-side.

Manual-only (needs real credentials + network): recorded in VALIDATION.md manual table.

---

## Validation Architecture

- **Quick (per task commit):** `cargo test -p sge <module>::` for the module touched (`imap::`, `store::`, `commands::`), plus `cargo clippy --all-targets` before wave close. Frontend tasks: `tsc --noEmit` + `vite build` smoke where wired.
- **Full (per wave):** `cargo test -p sge` (whole backend suite stays green) + `tsc --noEmit`. Estimated < 180s (executor measures on first wave and records actual).
- **Live gates (§10):** manual-only, sequenced in Plan 11-02/11-03 verify steps with cleanup; each gate names its cleanup so a failed gate never leaves `SGE-Test-*` folders behind (delete in reverse order on failure too).
- **Sampling rule:** no 3 consecutive tasks without an automated verify; every `<automated>` command in plans carries a `<fails_when>` sibling (stated failing direction).
