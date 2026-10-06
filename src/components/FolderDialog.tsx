import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { buildTree, type TreeNode } from "./Sidebar";
import type { MailboxRow } from "../types";

interface FolderDialogProps {
  mode: "create";
  /** Fallback hierarchy delimiter (used when "Nível superior" is picked). */
  delimiter: string;
  /** All cached folders (raw wire names) for the parent picker. */
  parents: MailboxRow[];
  /** Raw wire names already in the tree (instant duplicate feedback). */
  existingNames: string[];
  /**
   * Raw wire parent ("" = top level) + raw leaf — encoding stays
   * backend-side, the dialog never encodes (T-11-01).
   */
  onConfirm: (wireParent: string, leaf: string) => void;
  onCancel: () => void;
  isOpen: boolean;
  /** Server/invoke rejection to show inline (cleared on the next edit). */
  serverError: string | null;
  /** True while the CREATE round-trip is in flight (confirm disabled). */
  pending: boolean;
}

/** Inline client-side validation mirroring the backend UI-SPEC copy. */
function validateLeaf(name: string, delim: string): string | null {
  const t = name.trim();
  if (!t) return "Dê um nome para a pasta.";
  if (delim && [...t].some((c) => delim.includes(c))) {
    return `O nome não pode conter '${delim}' — ele separa pastas. Crie uma pasta por vez.`;
  }
  if (t.toLowerCase() === "inbox") {
    return "INBOX é uma pasta reservada do servidor — escolha outro nome.";
  }
  return null;
}

/** Parent-picker options in buildTree order: top level, system, then depth-first. */
function parentOptions(mailboxes: MailboxRow[]): Array<{ value: string; label: string }> {
  const { system, roots } = buildTree(mailboxes);
  const out: Array<{ value: string; label: string }> = [
    { value: "", label: "Nível superior" },
  ];
  for (const mb of system) out.push({ value: mb.name, label: mb.display_name });
  const walk = (nodes: TreeNode[], depth: number) => {
    for (const n of nodes) {
      const indent = depth > 0 ? `${"  ".repeat(depth)}↳ ` : "";
      out.push({
        value: n.mailbox.name,
        label: `${indent}${n.mailbox.display_name}`,
      });
      walk(n.children, depth + 1);
    }
  };
  walk(roots, 0);
  return out;
}

/**
 * Create-folder dialog (FOLD-04, Plan 11-01).
 *
 * - `role="dialog"` + `aria-modal="true"` with labelledby
 * - Title "Nova pasta" (Heading 20px/700); 20px content padding
 * - Single input ("Nome da pasta", placeholder "Ex.: Projetos") + parent
 *   picker ("Criar dentro de", default "Nível superior")
 * - Inline plain-language errors; confirm disabled until valid/while pending
 * - Esc/backdrop = cancel; focus trap + return; initial focus on the input
 * - `prefers-reduced-motion` instant (same token as ExpungeModal)
 */
export default function FolderDialog({
  delimiter,
  parents,
  existingNames,
  onConfirm,
  onCancel,
  isOpen,
  serverError,
  pending,
}: FolderDialogProps) {
  const [name, setName] = useState("");
  const [wireParent, setWireParent] = useState("");
  const modalRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  // Reset per opening.
  useEffect(() => {
    if (isOpen) {
      setName("");
      setWireParent("");
    }
  }, [isOpen]);

  // Focus trap + Esc + return (ExpungeModal skeleton).
  useEffect(() => {
    if (!isOpen) return;
    previousFocusRef.current = document.activeElement as HTMLElement;
    setTimeout(() => inputRef.current?.focus(), 0);

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
        return;
      }
      if (e.key === "Tab") {
        const focusable = modalRef.current?.querySelectorAll<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
        );
        if (!focusable || focusable.length === 0) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.body.style.overflow = "";
      previousFocusRef.current?.focus();
    };
  }, [isOpen, onCancel]);

  if (!isOpen) return null;

  const parentRow = parents.find((m) => m.name === wireParent);
  const effectiveDelim = parentRow ? parentRow.delimiter : delimiter;
  const trimmed = name.trim();
  const clientError = validateLeaf(name, effectiveDelim);
  // Best-effort instant duplicate feedback on the raw form (non-ASCII
  // duplicates are caught backend-side after encoding — source of truth).
  const prospectiveRaw = wireParent ? `${wireParent}${effectiveDelim}${trimmed}` : trimmed;
  const duplicateError =
    !clientError && trimmed && existingNames.includes(prospectiveRaw)
      ? "Já existe uma pasta com esse nome."
      : null;
  const inlineError = clientError ?? duplicateError;
  const canConfirm = !inlineError && !pending;

  const options = parentOptions(parents);

  return createPortal(
    <div
      className="folder-dialog-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="folder-dialog-title"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="folder-dialog" ref={modalRef}>
        <h2 id="folder-dialog-title" className="folder-dialog-title">
          Nova pasta
        </h2>
        <label className="folder-dialog-label" htmlFor="folder-dialog-name">
          Nome da pasta
        </label>
        <input
          ref={inputRef}
          id="folder-dialog-name"
          className="folder-dialog-input"
          type="text"
          value={name}
          placeholder="Ex.: Projetos"
          autoComplete="off"
          onChange={(e) => setName(e.target.value)}
          aria-invalid={inlineError !== null}
          aria-describedby={inlineError ? "folder-dialog-error" : undefined}
        />
        <label className="folder-dialog-label" htmlFor="folder-dialog-parent">
          Criar dentro de
        </label>
        <select
          id="folder-dialog-parent"
          className="folder-dialog-input"
          value={wireParent}
          onChange={(e) => setWireParent(e.target.value)}
        >
          {options.map((o) => (
            <option key={o.value || "__top"} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        {(inlineError ?? (serverError && !pending ? serverError : null)) && (
          <p id="folder-dialog-error" className="folder-dialog-error" role="alert">
            {inlineError ?? serverError}
          </p>
        )}
        <div className="folder-dialog-actions">
          <button
            type="button"
            className="btn btn-outline"
            onClick={onCancel}
            disabled={pending}
          >
            Cancelar
          </button>
          <button
            type="button"
            className="btn"
            disabled={!canConfirm}
            onClick={() => {
              if (canConfirm) onConfirm(wireParent, trimmed);
            }}
          >
            {pending ? "Criando…" : "Criar pasta"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
