import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { IconTrash } from "./icons";

interface FolderDeleteModalProps {
  /** Decoded display name of the folder (for copy + `title`). */
  displayName: string;
  /** Locally probed message count (0 = empty variant). */
  count: number;
  /** Inline backend error (e.g. a raced `need_delete_confirm` is handled by
   * the parent by bumping `count`; anything else shows here). */
  serverError: string | null;
  /** True while the DELETE round-trip is in flight. */
  pending: boolean;
  /** Called with the typed name (non-empty) or null (empty). */
  onConfirm: (typedName: string | null) => void;
  onCancel: () => void;
  isOpen: boolean;
}

/**
 * Delete-folder confirmation (FOLD-06, Plan 11-03).
 *
 * Copies the `ExpungeModal` skeleton (portal, `alertdialog`,
 * focus-Cancel-default, Esc/backdrop-cancel, trap + return) with folder
 * copy per UI-SPEC:
 *
 * - Empty: "Excluir pasta {nome}?" + "A pasta {nome} será excluída…"
 * - Non-empty: "Excluir pasta com mensagens?" + "{nome} tem N mensagem(ns)…
 *   Mova-as antes se quiser guardá-las." + typed-name input (confirm stays
 *   disabled until the exact display-name match)
 * - Confirm "Excluir pasta" (solid destructive); Cancel "Cancelar"
 *   (secondary outline, default-focused)
 */
export default function FolderDeleteModal({
  displayName,
  count,
  serverError,
  pending,
  onConfirm,
  onCancel,
  isOpen,
}: FolderDeleteModalProps) {
  const [typed, setTyped] = useState("");
  const modalRef = useRef<HTMLDivElement>(null);
  const cancelBtnRef = useRef<HTMLButtonElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  // Reset per opening.
  useEffect(() => {
    if (isOpen) setTyped("");
  }, [isOpen, displayName]);

  // Focus trap (Cancel = safe default) + Esc + return.
  useEffect(() => {
    if (!isOpen) return;
    previousFocusRef.current = document.activeElement as HTMLElement;
    setTimeout(() => cancelBtnRef.current?.focus(), 0);

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

  const nonEmpty = count > 0;
  const title = nonEmpty ? "Excluir pasta com mensagens?" : `Excluir pasta ${displayName}?`;
  const body = nonEmpty
    ? `${displayName} tem ${count} ${count === 1 ? "mensagem" : "mensagens"}. Para excluir, confirme digitando o nome da pasta — as mensagens serão apagadas junto. Mova-as antes se quiser guardá-las.`
    : `A pasta ${displayName} será excluída. Essa ação não pode ser desfeita.`;
  const canConfirm = !pending && (!nonEmpty || typed === displayName);

  return createPortal(
    <div
      className="expunge-modal-backdrop"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="folder-delete-title"
      aria-describedby="folder-delete-body"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="expunge-modal" ref={modalRef} style={{ animation: "none" }}>
        <div className="expunge-modal-icon" aria-hidden="true">
          <IconTrash size={28} />
        </div>
        <h2 id="folder-delete-title" className="expunge-modal-title" title={displayName}>
          {title}
        </h2>
        <p id="folder-delete-body" className="expunge-modal-body">
          {body}
        </p>
        {nonEmpty && (
          <>
            <label className="folder-dialog-label" htmlFor="folder-delete-typed">
              {`Digite ${displayName} para confirmar`}
            </label>
            <input
              id="folder-delete-typed"
              className="folder-dialog-input"
              type="text"
              value={typed}
              autoComplete="off"
              onChange={(e) => setTyped(e.target.value)}
              aria-invalid={typed !== displayName}
            />
          </>
        )}
        {serverError && !pending && (
          <p className="folder-dialog-error" role="alert">
            {serverError}
          </p>
        )}
        <div className="expunge-modal-actions">
          <button
            ref={cancelBtnRef}
            type="button"
            className="btn btn-outline"
            onClick={onCancel}
            disabled={pending}
          >
            Cancelar
          </button>
          <button
            type="button"
            className="btn btn-destructive"
            disabled={!canConfirm}
            onClick={() => {
              if (canConfirm) onConfirm(nonEmpty ? typed : null);
            }}
          >
            {pending ? "Excluindo…" : "Excluir pasta"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
