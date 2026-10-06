import { useEffect, useRef, useState, useMemo } from "react";
import { createPortal } from "react-dom";
import type { MailboxRow } from "../types";
import { IconInbox } from "./icons";

interface MoveMenuProps {
  /** Raw wire name of the source mailbox (never display_name). */
  sourceMailbox: string;
  /** All known folders from the local store. */
  mailboxes: MailboxRow[];
  /** Called when a destination is selected (raw wire name). */
  onSelect: (destRaw: string) => void;
  /** Called when the menu closes without selection. */
  onClose: () => void;
  /** Button that opened the menu — focus returns here on close. */
  triggerRef: React.MutableRefObject<HTMLButtonElement | null>;
}

/**
 * Destination picker for move-to-folder.
 *
 * Reuses Sidebar's `buildTree` logic verbatim: system-first order, decoded
 * display names, nested subfolders with depth indent. Excludes the current
 * folder, INBOX when source is INBOX, Sent, and `\Noselect` placeholders.
 * Moving OUT of Trash is supported (restore path).
 */
export default function MoveMenu({
  sourceMailbox,
  mailboxes,
  onSelect,
  onClose,
  triggerRef,
}: MoveMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [focusedIndex, setFocusedIndex] = useState(0);
  const itemsRef = useRef<HTMLButtonElement[]>([]);

  // Build tree exactly like Sidebar.tsx
  const tree = useMemo(() => buildTree(mailboxes), [mailboxes]);

  // Flatten eligible destinations for keyboard navigation
  const flatItems = useMemo(() => {
    const items: Array<{ mailbox: MailboxRow; depth: number; label: string }> = [];
    function collect(node: TreeNode, depth: number) {
      if (isEligible(node.mailbox)) {
        items.push({ mailbox: node.mailbox, depth, label: node.shortLabel });
      }
      node.children.forEach((child) => collect(child, depth + 1));
    }
    tree.system.forEach((mb) => {
      if (isEligible(mb)) {
        items.push({ mailbox: mb, depth: 0, label: mb.display_name });
      }
    });
    tree.roots.forEach((node) => collect(node, 0));
    return items;
  }, [tree, sourceMailbox]);

  // Eligibility: exclude current, INBOX when src=INBOX, Sent, \Noselect
  // Using name-based matching since frontend MailboxRow doesn't have attributes
  function isEligible(mb: MailboxRow): boolean {
    if (mb.name === sourceMailbox) return false;
    if (sourceMailbox === "INBOX" && mb.name === "INBOX") return false;
    // Exclude Sent folders by name (case-insensitive)
    const lowerName = mb.name.toLowerCase();
    if (lowerName === "sent" || lowerName === "sent messages" ||
        lowerName === "enviadas" || lowerName === "enviados") return false;
    // Note: \Noselect folders are not returned by the backend list_mailboxes command
    // (they are filtered out in sync.rs), so no need to check for them here.
    return true;
  }

  // Focus management
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) return;
      const max = itemsRef.current.length - 1;
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          setFocusedIndex((i) => Math.min(i + 1, max));
          break;
        case "ArrowUp":
          e.preventDefault();
          setFocusedIndex((i) => Math.max(i - 1, 0));
          break;
        case "Enter":
        case " ":
          e.preventDefault();
          const btn = itemsRef.current[focusedIndex];
          if (btn) btn.click();
          break;
        case "Escape":
          e.preventDefault();
          onClose();
          break;
        case "Tab":
          // Let browser handle focus trap; we just prevent cycling out
          if (e.shiftKey && document.activeElement === itemsRef.current[0]) {
            e.preventDefault();
            itemsRef.current[max]?.focus();
          } else if (!e.shiftKey && document.activeElement === itemsRef.current[max]) {
            e.preventDefault();
            itemsRef.current[0]?.focus();
          }
          break;
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [focusedIndex, onClose]);

  // Auto-focus first item on mount, return focus to trigger on close
  useEffect(() => {
    const first = itemsRef.current[0];
    first?.focus();
    return () => {
      triggerRef.current?.focus();
    };
  }, [triggerRef]);

  // Click outside to close
  useEffect(() => {
    function onClickOutside(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, [onClose]);

  if (flatItems.length === 0) {
    return createPortal(
      <div
        className="move-menu"
        ref={menuRef}
        role="menu"
        aria-label="Escolher pasta de destino"
      >
        <div className="move-menu-empty">
          Nenhuma pasta de destino — crie uma pasta primeiro
        </div>
      </div>,
      document.body
    );
  }

  return createPortal(
    <div
      className="move-menu"
      ref={menuRef}
      role="menu"
      aria-label="Escolher pasta de destino"
    >
      <div className="move-menu-header">Mover para…</div>
      <div className="move-menu-list" role="group">
        {flatItems.map((item, idx) => (
          <button
            key={item.mailbox.name}
            ref={(el) => { itemsRef.current[idx] = el!; }}
            type="button"
            role="menuitem"
            className="move-menu-item"
            style={{ paddingLeft: `${12 + item.depth * 14}px` }}
            title={item.mailbox.display_name}
            onClick={() => {
              onSelect(item.mailbox.name);
              onClose();
            }}
            aria-selected={idx === focusedIndex}
          >
            <span className="move-menu-icon" aria-hidden="true">
              <IconInbox size={item.depth > 0 ? 15 : 19} />
            </span>
            <span className="move-menu-label">{item.label}</span>
          </button>
        ))}
      </div>
    </div>,
    document.body
  );
}

// ── Tree types & builder (verbatim from Sidebar.tsx) ────────────────────

const SYSTEM_ORDER: Array<{ rank: number; names: string[] }> = [
  { rank: 0, names: ["inbox"] },
  { rank: 1, names: ["drafts", "rascunhos"] },
  { rank: 2, names: ["sent", "sent messages", "enviadas", "enviados"] },
  { rank: 3, names: ["spam", "junk", "junk e-mail"] },
  { rank: 4, names: ["trash", "deleted", "deleted messages", "lixeira"] },
];

function systemRank(rawName: string): number | null {
  const lower = rawName.toLocaleLowerCase();
  for (const group of SYSTEM_ORDER) {
    if (group.names.includes(lower)) return group.rank;
  }
  return null;
}

interface TreeNode {
  mailbox: MailboxRow;
  shortLabel: string;
  children: TreeNode[];
}

interface FolderTree {
  system: MailboxRow[];
  roots: TreeNode[];
}

function buildTree(mailboxes: MailboxRow[]): FolderTree {
  const system = mailboxes
    .filter((mb) => systemRank(mb.name) !== null)
    .sort(
      (a, b) =>
        (systemRank(a.name) ?? 99) - (systemRank(b.name) ?? 99) ||
        a.display_name.localeCompare(b.display_name, "pt-BR"),
    );

  const custom = mailboxes.filter((mb) => systemRank(mb.name) === null);
  const byPath = new Map<string, TreeNode>();
  const sorted = [...custom].sort((a, b) =>
    a.display_name.localeCompare(b.display_name, "pt-BR"),
  );
  for (const mb of sorted) {
    const delim = mb.delimiter || "";
    const segments =
      delim && mb.name.includes(delim) ? mb.name.split(delim) : [mb.name];
    const parentPath =
      segments.length > 1 ? segments.slice(0, -1).join(delim) : null;
    const node: TreeNode = { mailbox: mb, shortLabel: "", children: [] };
    byPath.set(mb.name, node);
    if (parentPath && byPath.has(parentPath)) {
      byPath.get(parentPath)!.children.push(node);
    }
  }
  for (const node of byPath.values()) {
    const delim = node.mailbox.delimiter || "";
    const parts =
      delim && node.mailbox.display_name.includes(delim)
        ? node.mailbox.display_name.split(delim)
        : [node.mailbox.display_name];
    node.shortLabel = parts[parts.length - 1] || node.mailbox.display_name;
    node.children.sort((a, b) =>
      a.mailbox.display_name.localeCompare(b.mailbox.display_name, "pt-BR"),
    );
  }
  const roots = [...byPath.values()].filter((node) => {
    const delim = node.mailbox.delimiter || "";
    if (!delim || !node.mailbox.name.includes(delim)) return true;
    const parentPath = node.mailbox.name.split(delim).slice(0, -1).join(delim);
    return !byPath.has(parentPath);
  });
  roots.sort((a, b) =>
    a.mailbox.display_name.localeCompare(b.mailbox.display_name, "pt-BR"),
  );
  return { system, roots };
}