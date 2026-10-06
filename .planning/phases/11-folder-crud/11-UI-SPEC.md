---
phase: "11"
slug: "folder-crud"
status: draft
shadcn_initialized: false
preset: none
created: "2026-10-06"
---

# Phase 11 — UI Design Contract

> Visual and interaction contract for frontend phases.
> **INTERIM — orchestrator-authored:** the `gsd-ui-phase` skill is not installed in this runtime, so this contract was derived directly from locked `11-CONTEXT.md` decisions + the Phase 10 UI-SPEC tokens (referenced, not redefined). A full `/gsd-ui-phase 11` pass may refine copy; no locked decision may be contradicted without a CONTEXT amendment.

---

## Design System

Tokens reused verbatim from Phase 10 UI-SPEC (`../10-delete-move/10-UI-SPEC.md`): Nunito + Baloo 2; 4 sizes / 2 weights (400 + 700 only); spacing multiples of 4 (`--space-1/2/4/5/6/8/10`); colors `--color-background #f4f5fd`, `--color-card #ffffff`, `--color-primary #4f46e5`, `--color-destructive #dc2626`, backdrop `rgba(30, 27, 75, 0.45)`, `--color-ring` 4px focus ring; hand-rolled clay style, no component library; icons hand-rolled inline SVG (`src/components/icons.tsx`) — this phase adds `IconFolderPlus`, `IconRename` (pencil), reuses `IconTrash`.

New components in this phase (all hand-rolled, clay style per `MailboxView.css`):

- `FolderDialog` — create/rename dialog: single text input + parent picker (create only) reusing Sidebar `buildTree`; NOT drag-and-drop (locked CONTEXT decision)
- `FolderContextMenu` — sidebar folder context menu: Renomear / Excluir (apagar pasta); keyboard-accessible menu (`role="menu"`)
- `FolderDeleteModal` — delete-folder confirmation copying the `ExpungeModal` skeleton (portal, alertdialog, focus-Cancel-default, Esc/backdrop-cancel, focus trap + return)

---

## Spacing / Typography / Color

Per Phase 10 contract, no changes. Additions:

- Dialog content padding 20px (`--space-5`); modal-to-backdrop margin minimum 16px (`--space-4`).
- Dialog title uses Heading 20px/700; input + buttons use Label 14px (buttons 700).
- Context-menu items use Label 14px/400 with 12px (`--space-3`) row padding (menu-item exception, same as move-menu).
- Disabled menu items (INBOX rename, system-folder rename, `\Noselect` delete) render muted (`--color-muted-foreground`) with a tooltip explaining why — never hidden without explanation (locked CONTEXT: "rename disabled with tooltip explaining why").
- Destructive reserved additions for this phase: FolderDeleteModal confirm button (solid `#dc2626`); "Excluir pasta" menu item text in destructive color; never color-alone (trash glyph + "para sempre" text accompany).

---

## Copywriting Contract

App language is pt-BR. All copy below is pt-BR:

| Element | Copy |
|---------|------|
| Sidebar new-folder affordance | "Nova pasta" (button at sidebar foot, folder-plus icon; `aria-label` "Criar nova pasta") |
| Context menu rename | "Renomear" (`aria-label` "Renomear pasta {nome}") |
| Context menu delete | "Excluir pasta" (`aria-label` "Excluir pasta {nome}") |
| Create dialog title | "Nova pasta" |
| Create dialog input | Label "Nome da pasta", placeholder "Ex.: Projetos"; parent picker label "Criar dentro de" (default "Nível superior") |
| Create validation: empty | "Dê um nome para a pasta." |
| Create validation: delimiter | "O nome não pode conter '{delim}' — ele separa pastas. Crie uma pasta por vez." |
| Create validation: INBOX variant | "INBOX é uma pasta reservada do servidor — escolha outro nome." |
| Create validation: exists | "Já existe uma pasta com esse nome." |
| Rename dialog title | "Renomear {nome}" (input pre-filled with leaf segment only) |
| Rename INBOX blocked | "A INBOX não pode ser renomeada — ela é fixa do servidor." |
| Rename system folder disabled tooltip | "{papel} do sistema — o nome é fixo." (e.g. "Lixeira do sistema — o nome é fixo.") |
| Delete modal title (empty folder) | "Excluir pasta {nome}?" |
| Delete modal body (empty) | "A pasta {nome} será excluída. Essa ação não pode ser desfeita." |
| Delete modal title (non-empty) | "Excluir pasta com mensagens?" |
| Delete modal body (non-empty) | "{nome} tem N mensagem(ns). Para excluir, confirme digitando o nome da pasta — as mensagens serão apagadas junto. Mova-as antes se quiser guardá-las." |
| Delete modal typed confirm | Input label "Digite {nome} para confirmar"; confirm stays disabled until exact match |
| Delete modal confirm | "Excluir pasta" (destructive solid) |
| Delete modal cancel | "Cancelar" (secondary outline; default-focused on open) |
| Delete INBOX blocked | "A INBOX não pode ser excluída — ela é fixa do servidor." |
| Delete with children refused | "A pasta {nome} tem subpastas — exclua ou mova as subpastas primeiro." |
| Delete server refusal (non-empty) | "O servidor não permitiu excluir {nome}: {detail}." |
| Offline folder op | "Sem conexão — pastas só podem ser alteradas online. Tente de novo ao reconectar." |
| Trash missing confirm (reuse) | Existing `need_trash_confirm` path unchanged |
| Selection fallback notice | Silent (selection simply lands on INBOX — no toast; the sidebar highlight is the signal) |

---

## UI Considerations

Applicable state considerations resolved: 7 covered, 1 backstop, 0 unresolved.

| Category | Element(s) | Status | Resolution / Reason |
|----------|------------|--------|---------------------|
| populated | sidebar tree after ops | ✅ covered | After CREATE/RENAME/DELETE: re-LIST refresh; CREATE selects+highlights the new folder; RENAME migrates highlight to the new name in the same tick; DELETE falls back to INBOX |
| populated | context menu placement | ✅ covered | Right-click (and keyboard menu key / Shift+F10 where available) on any sidebar item opens `FolderContextMenu`; Esc closes, focus returns to the item |
| partial | dialog validation | ✅ covered | Inline plain-language errors under the input (copy table above); confirm disabled until valid; never a bare server NO without mapping |
| zero-one-many | delete count phrasing | ✅ covered | Empty folder: no count line; non-empty: "{nome} tem N mensagem(ns)…" with typed-name double confirmation |
| empty | `\Noselect` placeholders | ✅ covered | No delete affordance at all (filtered, CONTEXT locked); no rename affordance; parent picker skips them |
| long-text | folder names in copy | ✅ covered | `{nome}` uses decoded display names, ellipsis at one line, full name in `title` (matches sidebar pattern) |
| loading | CREATE/RENAME/DELETE round-trip | 🧪 backstop | Ops are fast local-then-remote; dialog shows pending-disabled confirm while awaiting; held-out visual check that no spinner skeleton appears |

---

## Accessibility

| Requirement | Contract |
|-------------|----------|
| Dialog semantics | `role="dialog"` + `aria-modal="true"` + `aria-labelledby` (title); input labelled; Esc/backdrop = cancel |
| Delete modal semantics | `role="alertdialog"` + `aria-modal="true"` + labelledby/describedby (copies ExpungeModal) |
| Focus | Focus trap while open; initial focus on input (create/rename) or "Cancelar" (delete, safe default); focus returns to invoking control on close |
| Context menu semantics | `role="menu"` + `role="menuitem"`; ArrowUp/Down moves, Enter selects, Esc closes; focus returns to the sidebar item |
| Destructive distinction | Never color alone: trash glyph + "para sempre"/"não pode ser desfeita" text accompany red styling |
| Reduced motion | Dialog/menu/modal transitions disabled under `prefers-reduced-motion: reduce` (instant appear) |
| Focus visibility | Existing `--color-ring` 4px focus ring on all new interactive elements |

---

## Registry Safety

| Registry | Blocks Used | Safety Gate |
|----------|-------------|-------------|
| shadcn official | none | not required |
| third-party | none (hand-rolled, no new dependencies) | not required |

---

## Checker Sign-Off

- [ ] Dimension 1 Copywriting: pending
- [ ] Dimension 2 Visuals: pending
- [ ] Dimension 3 Color: pending
- [ ] Dimension 4 Typography: pending
- [ ] Dimension 5 Spacing: pending
- [ ] Dimension 6 Registry Safety: pending
- [ ] Dimension 7 Inventory Provenance: pending

**Approval:** pending checker review (interim — see header note)
