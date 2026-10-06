import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { IconTrash } from "./icons";

interface ExpungeModalProps {
  /** Number of messages to expunge. */
  count: number;
  /** Display name of the folder (for the message). */
  folderName: string;
  /** Raw wire names of UIDs to expunge (not used directly, passed to confirm). */
  uids: number[];
  /** Called when user confirms expunge. */
  onConfirm: (uids: number[]) => void;
  /** Called when user cancels. */
  onCancel: () => void;
  /** Whether the modal is open. */
  isOpen: boolean;
}

/**
 * Permanent-delete confirmation modal.
 *
 * - `role="alertdialog"` + `aria-modal="true"` with labelledby/describedby
 * - Title "Apagar para sempre?"
 * - Body: "1 mensagem da pasta {pasta}…" / "N mensagens da pasta {pasta}… Não dá para desfazer."
 * - Confirm "Apagar para sempre" (solid destructive `#dc2626` clay pill)
 * - Cancel "Cancelar" (outline, default-focused)
 * - Esc/backdrop = cancel; focus trap + return
 * - Backdrop `rgba(30,27,75,.45)`
 * - `prefers-reduced-motion` instant
 */
export default function ExpungeModal({
  count,
  folderName,
  uids,
  onConfirm,
  onCancel,
  isOpen,
}: ExpungeModalProps) {
  const modalRef = useRef<HTMLDivElement>(null);
  const cancelBtnRef = useRef<HTMLButtonElement>(null);
  const confirmBtnRef = useRef<HTMLButtonElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  // Focus trap
  useEffect(() => {
    if (!isOpen) return;
    previousFocusRef.current = document.activeElement as HTMLElement;
    // Focus Cancel (safe default) on open
    setTimeout(() => cancelBtnRef.current?.focus(), 0);

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
        return;
      }
      if (e.key === "Tab") {
        const focusable = modalRef.current?.querySelectorAll<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
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

  const isSingle = count === 1;
  const bodyText = isSingle
    ? `1 mensagem da pasta ${folderName} será apagada para sempre. Não dá para desfazer.`
    : `${count} mensagens da pasta ${folderName} serão apagadas para sempre. Não dá para desfazer.`;

  return createPortal(
    <div
      className="expunge-modal-backdrop"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="expunge-title"
      aria-describedby="expunge-body"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div
        className="expunge-modal"
        ref={modalRef}
        style={{
          animation: "none", // reduced-motion handled via CSS prefers-reduced-motion
        }}
      >
        <div className="expunge-modal-icon" aria-hidden="true">
          <IconTrash size={28} />
        </div>
        <h2 id="expunge-title" className="expunge-modal-title">
          Apagar para sempre?
        </h2>
        <p id="expunge-body" className="expunge-modal-body">
          {bodyText}
        </p>
        <div className="expunge-modal-actions">
          <button
            ref={cancelBtnRef}
            type="button"
            className="btn btn-outline"
            onClick={onCancel}
          >
            Cancelar
          </button>
          <button
            ref={confirmBtnRef}
            type="button"
            className="btn btn-destructive"
            onClick={() => {
              onConfirm(uids);
            }}
          >
            Apagar para sempre
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}