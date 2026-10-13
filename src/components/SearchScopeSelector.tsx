import { useEffect, useRef, useState } from "react";
import type { MailboxRow } from "../types";

/** Folder scope for search: whole account or a narrowed folder set. */
export type SearchScope = { kind: "all" } | { kind: "folders"; names: string[] };

/** Scope → `search_scoped` IPC shape (`null` = whole account). */
export function scopeToFolders(scope: SearchScope): string[] | null {
  return scope.kind === "all" ? null : scope.names;
}

/** Human pt-BR label for the results header + selector button. */
export function scopeLabel(
  scope: SearchScope,
  mailboxes: MailboxRow[],
  current: string,
): string {
  if (scope.kind === "all") return "todas as pastas";
  const display = (raw: string) =>
    mailboxes.find((m) => m.name === raw)?.display_name ?? raw;
  if (scope.names.length === 1) {
    return scope.names[0] === current
      ? `pasta atual (${display(current)})`
      : display(scope.names[0]);
  }
  return `${scope.names.length} pastas`;
}

interface SearchScopeSelectorProps {
  mailboxes: MailboxRow[];
  currentMailbox: string;
  value: SearchScope;
  onChange: (scope: SearchScope) => void;
  disabled?: boolean;
}

/**
 * Folder-scope picker attached to the search bar (D-ui-surface): whole
 * account (default, matching the "Buscar em todas as pastas" direction),
 * current folder, or a multi-select folder set. Checkbox dropdown inside
 * the existing design system (`move-menu*` classes, no new route/pane).
 */
export default function SearchScopeSelector({
  mailboxes,
  currentMailbox,
  value,
  onChange,
  disabled,
}: SearchScopeSelectorProps) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<string[]>([]);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  // Opening the menu seeds the checkbox draft from the live scope.
  function handleToggle() {
    if (!open) {
      setDraft(value.kind === "folders" ? [...value.names] : []);
    }
    setOpen((o) => !o);
  }

  // Outside click / Escape closes (MoveMenu pattern, inline).
  useEffect(() => {
    if (!open) return;
    function onPointer(e: PointerEvent) {
      if (
        menuRef.current &&
        !menuRef.current.contains(e.target as Node) &&
        triggerRef.current &&
        !triggerRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      }
    }
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open ]);

  function toggleFolder(raw: string) {
    setDraft((prev) =>
      prev.includes(raw) ? prev.filter((n) => n !== raw) : [...prev, raw],
    );
  }

  function applyDraft() {
    onChange(draft.length === 0 ? { kind: "all" } : { kind: "folders", names: draft });
    setOpen(false);
  }

  const currentDisplay =
    mailboxes.find((m) => m.name === currentMailbox)?.display_name ?? currentMailbox;

  return (
    <span className="search-scope">
      <button
        ref={triggerRef}
        type="button"
        className="btn btn-ghost"
        onClick={handleToggle}
        disabled={disabled}
        aria-haspopup="true"
        aria-expanded={open}
        aria-label="Escopo da busca"
        title={`Buscar em: ${scopeLabel(value, mailboxes, currentMailbox)}`}
      >
        {scopeLabel(value, mailboxes, currentMailbox)} ▾
      </button>
      {open && (
        <div ref={menuRef} className="move-menu" role="dialog" aria-label="Escopo da busca">
          <div className="move-menu-header">Buscar em…</div>
          <div className="move-menu-list" role="group">
            <button
              type="button"
              className="move-menu-item"
              aria-selected={value.kind === "all"}
              onClick={() => {
                onChange({ kind: "all" });
                setOpen(false);
              }}
            >
              <span className="move-menu-label">Todas as pastas</span>
            </button>
            <button
              type="button"
              className="move-menu-item"
              aria-selected={
                value.kind === "folders" &&
                value.names.length === 1 &&
                value.names[0] === currentMailbox
              }
              onClick={() => {
                onChange({ kind: "folders", names: [currentMailbox] });
                setOpen(false);
              }}
            >
              <span className="move-menu-label">Pasta atual ({currentDisplay})</span>
            </button>
            {mailboxes.map((mb) => (
              <label key={mb.name} className="move-menu-item">
                <input
                  type="checkbox"
                  checked={draft.includes(mb.name)}
                  onChange={() => toggleFolder(mb.name)}
                />
                <span className="move-menu-label">{mb.display_name}</span>
              </label>
            ))}
            <button type="button" className="move-menu-item" onClick={applyDraft}>
              <span className="move-menu-label">
                {draft.length === 0 ? "Limpar seleção (todas)" : `Aplicar (${draft.length})`}
              </span>
            </button>
          </div>
        </div>
      )}
    </span>
  );
}
