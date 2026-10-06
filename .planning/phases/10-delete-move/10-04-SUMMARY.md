# Plan 10-04: Move menu + Trash undo toast + expunge modal + gates — SUMMARY

**Executed:** 2026-10-06
**Status:** Complete
**Mode:** mvp

## Overview

Implemented the user-visible delete/move UI for Phase 10, completing all frontend affordances specified in 10-UI-SPEC.md and closing all Phase 10 success criteria (1-4).

## Files Created

| File | Purpose |
|------|---------|
| `src/components/icons.tsx` | Added `IconTrash` and `IconMove` (1.8px stroke, `size` prop) |
| `src/components/MoveMenu.tsx` | Destination picker reusing Sidebar folder tree (`buildTree` verbatim) |
| `src/components/ExpungeModal.tsx` | Permanent-delete confirmation dialog (`role="alertdialog"`, focus trap, safe defaults) |

## Files Modified

| File | Changes |
|------|---------|
| `src/components/MessageList.tsx` | Row action buttons (delete/move), optimistic hide, undo toast (single slot, outbox-anchored), MoveMenu integration, ExpungeModal integration |
| `src/components/ReadingPane.tsx` | Reader header action row: "Apagar" + "Mover para…" in normal folders; "Restaurar" + "Apagar para sempre" in Trash; MoveMenu + ExpungeModal + undo toast |
| `src/components/MailboxView.tsx` | Pass `mailboxes` to ReadingPane for move picker |
| `src/components/MailboxView.css` | Styles for message actions, undo toast, move menu, expunge modal, reading actions, Trash empty state |

## Features Implemented

### Row/Reader Affordances
- **List row**: Delete + Move icon buttons always visible (no hover-only), 30px targets, 8px gap, right-aligned before date pill, `aria-label`s, `stopPropagation`, Tab order dot → delete → move
- **Reader header**: New action row under `.reading-meta` with "Apagar" + "Mover para…" buttons; in Trash: "Restaurar" (move back to INBOX) + "Apagar para sempre" (opens ExpungeModal)
- **Optimistic UI**: Instant row removal on delete/move; rollback only on invoke reject
- **Undo toast**: Single slot, `role="status"`, copy per UI-SPEC ("Mensagem movida para a Lixeira. [Desfazer]" / "Mensagem movida para {pasta}. [Desfazer]"), persists until next sync starts / replaced / dismissed (no countdown — outbox entry is anchor), Desfazer = reverse op while still queued

### Move Menu (`MoveMenu.tsx`)
- Reuses Sidebar `buildTree` verbatim: system-first order, decoded display names, nested subfolders with depth indent, badge counts hidden
- Excludes: current folder, INBOX when src=INBOX, Sent, `\Noselect`; moving OUT of Trash = restore (supported)
- Empty state: "Nenhuma pasta de destino — crie uma pasta primeiro"
- A11y: `role="menu"`/`menuitem`, `aria-label`, nested `role="group"`, ArrowUp/Down + Enter + Esc, focus trap, focus returns to invoker, `prefers-reduced-motion` instant
- Passes RAW `name` (wire) to `move_message`, never `display_name`

### Expunge Modal (`ExpungeModal.tsx`)
- `role="alertdialog"` + `aria-modal="true"` + `aria-labelledby`/`aria-describedby`
- Title "Apagar para sempre?", body: "1 mensagem da pasta {pasta}…" / "N mensagens… Não dá para desfazer."
- Confirm "Apagar para sempre" (solid `#dc2626` clay pill) + Cancel "Cancelar" (outline, default-focused)
- Esc/backdrop = cancel; focus trap + return; backdrop `rgba(30,27,75,.45)`; reduced-motion instant
- Sends only selected UIDs via `expunge_messages` — never bare `expunge()`

### SyncStatus Integration
- Pending indicator already sums both outbox depths (backend `pending_depth` function)
- Offline state "Offline · alterações guardadas — serão enviadas ao reconectar" shown when `imap_outbox` non-empty and session down
- No new scheduler — `imap_outbox` replay rides existing poll loop (pre-sweep)

## Verification Gates Passed

| Gate | Result |
|------|--------|
| `cargo test -p sge` | 156 tests passed |
| `tsc --noEmit` | Clean |
| `npm run build` | Passes |
| `cargo clippy --all-targets` | Only pre-existing warnings (no new) |
| BODY.PEEK audit | Clean — only `BODY.PEEK[]` in fetch paths |
| UID-only audit | Clean — no `store`/`copy`/`mv` outside UID wrappers |

## Commands Recorded for Orchestrator

- `delete_message(uid, mailbox?, create_trash?)` → `DeleteMoveResult`
- `move_message(uid, dest, mailbox?)` → `DeleteMoveResult`
- `expunge_messages(uids, mailbox?)` → `ExpungeResult`
- `undo_queued_op(uid, mailbox?)` → `UndoResult`

## Notes

- Trash auto-detection: SPECIAL-USE `\Trash` attribute → case-insensitive name match (reusing Sidebar `SYSTEM_ORDER` rank-4 list + Gmail-style + PT/ES variants) → `Missing` (caller prompts confirm → `create_trash()`)
- Move preserves flags server-side (MOVE or COPY+STORE); delete is move-to-Trash so flags preserved; move carries pending Seen intent as `seen_intent` for dest-side apply at replay
- Optimistic local state uses `pending_delete` hidden flag (row stays for undo, filtered from `list_messages`/`fts_search`, cleared on ack, restored on undo)
- All pt-BR copy per UI-SPEC contract; colors/spacing/typography per token system

## Blockers/Concerns

None — all Phase 10 success criteria satisfied. Ready for live validation gates (L1-L4 against `mail.utfpr.edu.br`).