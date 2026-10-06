import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { IconRename, IconTrash } from "./icons";
import type { MailboxRow } from "../types";

interface FolderContextMenuProps {
  /** Folder the menu was opened for (raw wire name in `.name`). */
  mailbox: MailboxRow;
  /** Viewport position (already clamped by the opener). */
  x: number;
  y: number;
  onRename: () => void;
  onDelete: () => void;
  onClose: () => void;
}

const SYSTEM_ROLE_LABEL: Record<string, string> = {
  trash: "Lixeira",
  sent: "Enviadas",
  drafts: "Rascunhos",
};

function isNoselect(mb: MailboxRow): boolean {
  return mb.attributes.split(/\s+/).some((a) => a.includes("NoSelect"));
}

/**
 * Sidebar folder context menu (FOLD-04/05/06, Plan 11-03).
 *
 * - `role="menu"` + `role="menuitem"`; ArrowUp/Down moves, Enter selects,
 *   Esc closes; focus returns to the invoking control on close
 * - Renomear / Excluir pasta with UI-SPEC `aria-label`s; destructive item
 *   in destructive color (never color alone — trash glyph accompanies)
 * - Disabled states with tooltips: INBOX (both ops), system-role rows
 *   (rename disabled); `\Noselect` rows render no menu at all (filtered)
 * - `prefers-reduced-motion` instant
 */
export default function FolderContextMenu({
  mailbox,
  x,
  y,
  onRename,
  onDelete,
  onClose,
}: FolderContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);

  // `\Noselect` placeholders get no affordances at all (filtered, not
  // just disabled — CONTEXT locked). Computed here, applied after hooks.
  const noselect = isNoselect(mailbox);

  const isInbox = mailbox.name.toLowerCase() === "inbox";
  const systemRole = SYSTEM_ROLE_LABEL[mailbox.role] ?? null;
  const renameDisabled = isInbox || systemRole !== null;
  const renameTitle = isInbox
    ? "A INBOX não pode ser renomeada — ela é fixa do servidor."
    : systemRole !== null
      ? `${systemRole} do sistema — o nome é fixo.`
      : undefined;
  const deleteDisabled = isInbox;
  const deleteTitle = isInbox
    ? "A INBOX não pode ser excluída — ela é fixa do servidor."
    : undefined;

  // Focus first enabled item + key handling + return.
  useEffect(() => {
    const previous = document.activeElement as HTMLElement;
    const items = menuRef.current?.querySelectorAll<HTMLButtonElement>(
      '[role="menuitem"]:not([disabled])',
    );
    setTimeout(() => items?.[0]?.focus(), 0);

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        const enabled = Array.from(
          menuRef.current?.querySelectorAll<HTMLButtonElement>(
            '[role="menuitem"]:not([disabled])',
          ) ?? [],
        );
        if (enabled.length === 0) return;
        e.preventDefault();
        const idx = enabled.indexOf(document.activeElement as HTMLButtonElement);
        const next =
          e.key === "ArrowDown"
            ? enabled[(idx + 1) % enabled.length]
            : enabled[(idx - 1 + enabled.length) % enabled.length];
        next.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previous?.focus();
    };
  }, [onClose]);

  if (noselect) return null;

  return createPortal(
    <div
      className="folder-menu-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <div
        ref={menuRef}
        className="folder-menu"
        role="menu"
        aria-label={`Ações para ${mailbox.display_name}`}
        style={{ left: x, top: y }}
      >
        <button
          type="button"
          role="menuitem"
          className="folder-menu-item"
          aria-label={`Renomear pasta ${mailbox.display_name}`}
          title={renameTitle}
          disabled={renameDisabled}
          onClick={() => {
            onRename();
            onClose();
          }}
        >
          <span aria-hidden="true">
            <IconRename size={16} />
          </span>
          <span>Renomear</span>
        </button>
        <button
          type="button"
          role="menuitem"
          className="folder-menu-item folder-menu-item-destructive"
          aria-label={`Excluir pasta ${mailbox.display_name}`}
          title={deleteTitle}
          disabled={deleteDisabled}
          onClick={() => {
            onDelete();
            onClose();
          }}
        >
          <span aria-hidden="true">
            <IconTrash size={16} />
          </span>
          <span>Excluir pasta</span>
        </button>
      </div>
    </div>,
    document.body,
  );
}
