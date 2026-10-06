# Phase 11: Folder CRUD — Pattern Map

**Mapped:** 2026-10-06
**Method:** inline orchestrator mapping (no `gsd-pattern-mapper` agent available in this runtime). Every file below reuses an exact analog from `../10-delete-move/10-PATTERNS.md` — section references are to that file.
**Files analyzed:** 9 (5 backend groups + 3 UI + 1 shared)
**Analogs found:** 9 / 9

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `src-tauri/src/imap/mutf7.rs` (+ `encode_modified_utf7`) | service (codec) | pure function | `decode_modified_utf7` same file (inverse operation, same shift grammar) | exact |
| `src-tauri/src/imap/mod.rs` (`rename_mailbox`/`delete_mailbox` verbs) | service (transport) | request-response | §1 `set_seen` impl (435-450) minus the drain; `create_mailbox` (662-671) for no-stream verbs | exact |
| `src-tauri/src/imap/manager.rs` (`create/rename/delete_mailbox_in`) | service (session owner) | request-response | §2 `set_seen_in` reconnect-retry (111-128) + `create_trash` (225-237) | exact |
| `src-tauri/src/imap/roles.rs` (new; generalizes `trash.rs`) | service (resolver) | pure function | `imap/trash.rs detect_trash` layers (kept as delegating wrapper) | exact |
| `src-tauri/src/store/mod.rs` + `queries.rs` (M8 + cache ops) | model/store | CRUD | §4 M2/M7 migration + outbox query templates; `drop_outbox_for_mailbox` (937) + `drop_imap_outbox_for_mailbox` (1103) | exact |
| `src-tauri/src/commands/sync.rs` (`create/rename/delete_folder`) | controller (Tauri command) | request-response | §5 `set_seen` skeleton (179-276) + `delete_message` confirm-gated CREATE flow (361-480); `list_mailboxes` refresh (819-...) | exact |
| `src/components/FolderDialog.tsx` (new) | component | request-response | `MoveMenu.tsx` tree reuse + `ExpungeModal.tsx` dialog skeleton (portal, focus trap, Esc-cancel) | role-match |
| `src/components/Sidebar.tsx` (context menu + Nova pasta) | component | request-response | §8 `buildTree` (46-95) + `renderItem` + `SYSTEM_ORDER` (17-23); `SyncStatus.tsx` copy tone | exact |
| `src/components/FolderDeleteModal.tsx` (new) | component | request-response | `ExpungeModal.tsx` full skeleton (alertdialog, focus-Cancel-default, typed-confirm extension) | exact |

## Pattern Assignments

### 1. `mutf7.rs` — `encode_modified_utf7` (codec, pure)

**Analog:** `decode_modified_utf7` same file (shift grammar `&<b64>-`, `&-` literal, `,` for `/`).
**Rules:** ASCII passthrough except `&`; non-ASCII runs → UTF-16BE → modified base64, strip `=` padding; operate on the leaf AFTER validation rejects delimiter/empty/INBOX (encoder never makes hierarchy decisions). Roundtrip tests mirror decode tests. Update the module doc comment that claims "encoding is never needed".

### 2. `imap/mod.rs` — `rename_mailbox` / `delete_mailbox` (transport)

**Analog:** 10-PATTERNS §1 + in-tree `create_mailbox` impl (no-stream verb shape).
```rust
fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>> {
    Box::pin(async move {
        self.rename(&old_owned, &new_owned).await
            .map_err(|e| SyncError::Protocol(format!("RENAME {old} -> {new}: {e}")))?;
        Ok(())
    })
}
```
**Rules:** name-only verbs (no UID args, no UID-set formatting); raw wire names in, never display names; confirm vendored method names (`rename`/`delete`) at compile gate; MockSession arms (`renamed_calls`, `deleted_mailboxes`, `fail_rename`, `fail_delete`) in `manager.rs` tests module (772-781 neighbor).

### 3. `manager.rs` — `create_mailbox_in` / `rename_mailbox_in` / `delete_mailbox_in` (service)

**Analog:** 10-PATTERNS §2 (`set_seen_in` reconnect-retry verbatim; `create_trash` INBOX-lease shape).
**Rules:** `lease_for("INBOX")` for CREATE (no folder needs selecting); `lease_for(old)` for RENAME, `lease_for(name)` for DELETE (keeps SELECT state sane); single reconnect-retry; `SyncError::Refused` (validation) never retried; post-rename STATUS UIDVALIDITY check inside `rename_mailbox_in` (one call, bump → `SyncError::State`, caller treats like epoch change).

### 4. `roles.rs` — `resolve_roles` (resolver, pure)

**Analog:** `trash.rs` `detect_trash` (layers + `\Noselect` skip + tests stay green unchanged).
**Rules:** `resolve_roles(&[MailboxInfo]) -> Vec<(String /*wire*/, Role)>`, Role ∈ {Inbox, Trash, Sent, Drafts, Custom}; layer order SPECIAL-USE → full-name → leaf, first hit wins per mailbox; `detect_trash` delegates (finds Trash entry or Missing). Name lists reuse `SYSTEM_ORDER` ranks + `TRASH_FULL/LEAF_NAMES` (moved, not copied).

### 5. `store` — M8 + `rename_mailbox_cache` / `delete_mailbox_cache` (model)

**Analog:** 10-PATTERNS §4 (M2 DDL template, preserve-rows test, `ON CONFLICT` upserts).
**Rules:** `SCHEMA_VERSION` 7 → 8; `ALTER TABLE mailboxes ADD COLUMN role ...; ADD COLUMN attributes ...;` forward-only; rename = exact UPDATE + prefix UPDATEs for subtree children (per-child-delimiter LIKE); delete = row DELETE (FK cascades messages) + both outbox drops in the same lock section; assert no orphan `LIKE` rows post-op in tests.

### 6. `commands/sync.rs` — `create_folder` / `rename_folder` / `delete_folder` (controller)

**Analog:** 10-PATTERNS §5 (`set_seen` skeleton; `delete_message` confirm-gated flow; `list_mailboxes` refresh + offline fallback).
**Rules:** online-only (offline → loud error, no queue); validation order: INBOX/`\Noselect`/children/non-empty guards BEFORE any wire verb; post-mutation reuse the `list_mailboxes` refresh body and return the fresh tree + affected name(s); STATUS MESSAGES (extended `mailbox_status`, RESEARCH §7) feeds the non-empty guard.

### 7-9. UI — `FolderDialog`, `Sidebar` menu, `FolderDeleteModal` (components)

**Analog:** 10-PATTERNS §7/§8 + `ExpungeModal.tsx` + `MoveMenu.tsx` + `MailboxView.tsx` selection state.
**Rules:** export `buildTree` from Sidebar for the parent picker; raw `name` to commands, `display_name` shown; `MailboxView.setSelectedMailbox` migrates on rename / falls back to INBOX on delete in the same tick as re-LIST; modal copies ExpungeModal skeleton + typed-name confirm for non-empty; pt-BR copy per 11-UI-SPEC.

## Shared Patterns (all inherited, no deltas)

Tauri command shape · single-flight (`SyncGate::try_begin` for folder ops too) · error handling (`Refused` = no retry) · offline-first store discipline (lock never across `.await`) · UID-only/BODY.PEEK untouched (this phase issues zero message verbs).

## No Analog Found

| File | Role | Reason |
|---|---|---|
| `encode_modified_utf7` | codec (encoder) | Decode-only module; encoder is new but the shift grammar + test style transfer exactly |
| Typed-name double confirmation | component behavior | No typed-confirm precedent (ExpungeModal is single-click); planner specifies exact-match enable rule per UI-SPEC copy table |

## Metadata

**Analog search scope:** `src-tauri/src/imap/`, `src-tauri/src/sync/`, `src-tauri/src/store/`, `src-tauri/src/commands/`, `src/components/`, `src/`
**Pattern extraction date:** 2026-10-06
