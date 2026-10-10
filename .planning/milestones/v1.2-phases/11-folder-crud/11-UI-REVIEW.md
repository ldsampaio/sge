# Phase 11 — UI Review (Folder CRUD)

> Static 6-pillar visual/UX audit against `11-UI-SPEC.md` (INTERIM contract) + locked `11-CONTEXT.md` decisions.
> **Method:** static code review only — no runtime (headless environment, app not launched).
> **Scope:** `FolderDialog.tsx`, `FolderContextMenu.tsx`, `FolderDeleteModal.tsx`, `Sidebar.tsx` changes, `MailboxView` wiring, `MailboxView.css`.
> **Status:** advisory, non-blocking.

## Score: 8.5 / 10

| Pillar | Verdict | Notes |
|--------|---------|-------|
| 1. Layout consistency (three-pane Gmail-like) | ✅ Pass | New affordances live inside existing chrome: `Nova pasta` foot button in sidebar, context menu on sidebar items, dialogs reuse ExpungeModal skeleton + clay tokens (backdrop `rgba(30,27,75,.45)`, 20px dialog padding, Heading 20px/700, Label 14px). Delete modal reuses `expunge-modal*` classes verbatim — visually identical family. |
| 2. PT copy tone (ExpungeModal/SyncStatus match) | ✅ Pass | All user-facing copy is pt-BR and matches the UI-SPEC copy table nearly verbatim (dialog validation, tooltips, typed-confirm, offline copy mapped backend-side in `sync.rs:1091` + unit-tested). Improvement over spec: delete body resolves plural (`1 mensagem` vs `N mensagens`) instead of the `mensagem(ns)` template. |
| 3. Guard rails (INBOX / `\Noselect` / typed confirm) | ⚠️ Pass with notes | INBOX rename+delete disabled with explanatory tooltips; system-role rename disabled per role label; `\Noselect` rows get no menu at all; non-empty delete requires exact typed display-name match; confirm disabled until valid/while pending. Two gaps, both minor (F3, F4 below). |
| 4. Accessibility (keyboard, focus, roles) | ⚠️ Pass with notes | Dialog `role="dialog"` + `aria-modal` + labelledby; delete modal `role="alertdialog"` + labelledby/describedby; menu `role="menu/menuitem"` with ArrowUp/Down + Esc; focus trap + return everywhere; Cancel default-focused on delete; 14px menu items; `prefers-reduced-motion` instant on dialog + menu + backdrop. Two advisories (F1, F2). |
| 5. States (loading / error / offline) | ✅ Pass | Pending disables confirm with `Criando…/Renomeando…/Excluindo…` labels; server rejects surface inline (`role="alert"`) with backend-mapped plain language, never raw protocol; offline is loud per spec; fresh-count and typed-name races handled (modal morphs to non-empty variant / re-prompts with new name). Selection migrates (rename) or falls back to INBOX silently (delete) per spec. |
| 6. Registry safety | ✅ Pass | No new dependencies; hand-rolled clay + inline SVG (`IconFolderPlus`, `IconRename` reuse `IconTrash`). Nothing to gate. |

## Findings (all advisory, non-blocking)

- **F1 (a11y, minor): `title` tooltips on `disabled` menu items may never surface.** `FolderContextMenu.tsx:127,145` puts the explanatory tooltip on a `disabled` `<button>` — Chrome/Firefox do not fire hover events on disabled buttons, so the "INBOX é fixa / nome é fixo" explanation can be unreachable by mouse. Screen-reader users get no equivalent either (no `aria-describedby`/`aria-disabled` pattern). Consider `aria-disabled` + focusable wrapper or exposing the reason as visually-hidden describedby text. Tone/content itself matches spec.
- **F2 (a11y, minor): menu focus-return targets "previous focus", not necessarily the invoking item.** `FolderContextMenu.tsx:67` restores `document.activeElement`-at-open; right-click does not move focus, so after a mouse-opened menu focus may return somewhere other than the sidebar item (spec: "focus returns to the item"). Keyboard-opened path is fine (focus is already on the item). Consider capturing the item button ref in `Sidebar.openMenu` and restoring to it.
- **F3 (guard rails, minor): parent picker does not skip `\Noselect` placeholders.** UI-SPEC state table requires "parent picker skips them"; `FolderDialog.parentOptions` walks `buildTree` output unfiltered, and `buildTree` (`Sidebar.tsx:78`) does not filter `\Noselect` either. A `\Noselect` row can therefore be chosen as a parent. Backend delimiter logic + server CREATE would presumably reject/misplace it — worth a filter for full spec compliance.
- **F4 (states, trivial): Cancel disabled while pending in `FolderDialog`.** `FolderDialog.tsx:213` disables Cancel during the round-trip (ExpungeModal leaves Cancel enabled; Esc still cancels here via the key handler, so the user is never truly trapped). Inconsistent with the skeleton but harmless for fast ops — either re-enable Cancel or keep; no action required.
- **F5 (layout, trivial): `role="dialog"` + `aria-modal` sit on the backdrop, not the dialog box** (`FolderDialog.tsx:160`). Same pattern choice is harmless in practice (single dialog in the portal) but strictly the role belongs on the dialog container. `FolderDeleteModal` correctly puts `alertdialog` on the backdrop-as-dialog — consistent within this codebase, so leave unless a checker demands otherwise.
- **F6 (copy, trivial/positive): delimiter validation uses per-char `delim.includes(c)`.** Correct for single-char IMAP delimiters (`.`/`/`); a hypothetical multi-char delimiter would over-reject. Matches backend behavior — note only, no change.

## Checker sign-off (UI-SPEC dimensions)

- [x] Dimension 1 Copywriting: pass (verbatim spec copy + plural improvement)
- [x] Dimension 2 Visuals: pass (ExpungeModal skeleton + clay tokens reused)
- [x] Dimension 3 Color: pass (destructive reserved, never color-alone — glyph + "não pode ser desfeita" accompany)
- [x] Dimension 4 Typography: pass (Heading 20/700, Label 14, menu 14/400 exception honored)
- [x] Dimension 5 Spacing: pass (20px padding, 16px backdrop margin, 12px menu rows)
- [x] Dimension 6 Registry Safety: pass (no new deps)
- [x] Dimension 7 Inventory Provenance: hand-rolled `folder-dialog*` / `folder-menu*` classes + `IconFolderPlus`/`IconRename` in `icons.tsx`; no external provenance to record

**Approval:** advisory pass — no blocking issues. Suggested follow-ups (F1–F3) fit any future frontend touch-up; none contradict locked CONTEXT decisions.
