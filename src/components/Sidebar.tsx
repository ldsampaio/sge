import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./MailboxView.css";
import FolderContextMenu from "./FolderContextMenu";
import { ReviewPanel } from "./ReviewPanel";
import { TaxonomyEditor } from "./TaxonomyEditor";
import { BatchPanel } from "./BatchPanel";
import { IconBook, IconFolderPlus, IconInbox, IconMail, IconTrash } from "./icons";
import type { MailboxRow } from "../types";

interface SidebarProps {
  selectedMailbox: string;
  mailboxes: MailboxRow[];
  onMailboxSelect: (mailbox: string) => void;
  /** Opens the create-folder dialog (owned by MailboxView). */
  onCreateFolder: () => void;
  /** Opens the rename dialog for a RAW wire mailbox name. */
  onRenameFolder: (mailbox: string) => void;
  /** Opens the delete confirm for a RAW wire mailbox name. */
  onDeleteFolder: (mailbox: string) => void;
}

/**
 * Ordem fixa das pastas do sistema (comparação sem maiúsculas/minúsculas,
 * com apelidos comuns de servidor). Todo o resto é pasta personalizada e
 * vai para a árvore em ordem alfabética.
 */
const SYSTEM_ORDER: Array<{ rank: number; names: string[] }> = [
  { rank: 0, names: ["inbox"] },
  { rank: 1, names: ["drafts", "rascunhos"] },
  { rank: 2, names: ["sent", "sent messages", "enviadas", "enviados"] },
  { rank: 3, names: ["spam", "junk", "junk e-mail"] },
  { rank: 4, names: ["trash", "deleted", "deleted messages", "lixeira"] },
];

/** System role for glyph treatment (Plan 11-03): the M8 `role` column
 * first, Sidebar name lists as fallback for pre-M8 caches. Visual only —
 * ordering and behavior never change. */
function systemKind(mb: MailboxRow): "trash" | "sent" | "drafts" | null {
  if (mb.role === "trash" || mb.role === "sent" || mb.role === "drafts") {
    return mb.role;
  }
  const lower = mb.name.toLocaleLowerCase();
  if (["trash", "deleted", "deleted messages", "lixeira"].includes(lower)) {
    return "trash";
  }
  if (["sent", "sent messages", "enviadas", "enviados"].includes(lower)) {
    return "sent";
  }
  if (["drafts", "rascunhos"].includes(lower)) {
    return "drafts";
  }
  return null;
}

function isNoselectAttrs(mb: MailboxRow): boolean {
  return mb.attributes.split(/\s+/).some((a) => a.includes("NoSelect"));
}

function systemRank(rawName: string): number | null {
  const lower = rawName.toLocaleLowerCase();
  for (const group of SYSTEM_ORDER) {
    if (group.names.includes(lower)) return group.rank;
  }
  return null;
}

export interface TreeNode {
  mailbox: MailboxRow;
  /** Último segmento do nome para exibição (o caminho completo vai no title). */
  shortLabel: string;
  children: TreeNode[];
}

export interface FolderTree {
  system: MailboxRow[];
  roots: TreeNode[];
}

/** Monta a árvore a partir da lista plana: sistema primeiro, resto aninhado. */
export function buildTree(mailboxes: MailboxRow[]): FolderTree {
  const system = mailboxes
    .filter((mb) => systemRank(mb.name) !== null)
    .sort(
      (a, b) =>
        (systemRank(a.name) ?? 99) - (systemRank(b.name) ?? 99) ||
        a.display_name.localeCompare(b.display_name, "pt-BR"),
    );

  const custom = mailboxes.filter((mb) => systemRank(mb.name) === null);
  const byPath = new Map<string, TreeNode>();
  // Ordena antes para montagem determinística.
  const sorted = [...custom].sort((a, b) => a.display_name.localeCompare(b.display_name, "pt-BR"));
  for (const mb of sorted) {
    const delim = mb.delimiter || "";
    const segments = delim && mb.name.includes(delim) ? mb.name.split(delim) : [mb.name];
    const parentPath = segments.length > 1 ? segments.slice(0, -1).join(delim) : null;
    const node: TreeNode = { mailbox: mb, shortLabel: "", children: [] };
    byPath.set(mb.name, node);
    if (parentPath && byPath.has(parentPath)) {
      byPath.get(parentPath)!.children.push(node);
    }
  }
  // Rótulo curto = último segmento decodificado; ordena filhos.
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
  roots.sort((a, b) => a.mailbox.display_name.localeCompare(b.mailbox.display_name, "pt-BR"));
  return { system, roots };
}

export default function Sidebar({
  selectedMailbox,
  mailboxes = [],
  onMailboxSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
}: SidebarProps) {
  const showMailboxItems = mailboxes.length > 0;
  const tree = buildTree(mailboxes);
  const [menu, setMenu] = useState<{ name: string; x: number; y: number } | null>(null);
  /** Phase 18 review queue: count badge + modal (quiet when empty). */
  const [reviewOpen, setReviewOpen] = useState(false);
  const [reviewCount, setReviewCount] = useState(0);
  /** Phase 19 taxonomy editor modal. */
  const [editorOpen, setEditorOpen] = useState(false);
  /** Phase 20 batch panel modal. */
  const [batchOpen, setBatchOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    const fetchCount = async () => {
      try {
        const rows = (await invoke("review_list", { limit: 200 })) as unknown[];
        if (alive) setReviewCount(rows.length);
      } catch {
        /* sidecar/store hiccup — badge stays stale, modal shows the error */
      }
    };
    void fetchCount();
    const timer = setInterval(fetchCount, 30000);
    // Freshness: drains anywhere refresh the review badge immediately.
    const unlisten = listen("classification-drained", () => {
      void fetchCount();
    });
    return () => {
      alive = false;
      clearInterval(timer);
      void unlisten.then((f) => f());
    };
  }, [mailboxes]);

  /** Open the folder context menu (right-click or keyboard), clamped to viewport. */
  function openMenu(mb: MailboxRow, clientX: number, clientY: number) {
    // `\Noselect` placeholders never get a menu (filtered upstream too).
    if (isNoselectAttrs(mb)) return;
    setMenu({
      name: mb.name,
      x: Math.max(8, Math.min(clientX, window.innerWidth - 220)),
      y: Math.max(8, Math.min(clientY, window.innerHeight - 140)),
    });
  }

  /**
   * Badge signal per folder (FOLD-02): the dynamic local unread count once
   * the folder has synced, else the cached STATUS (UNSEEN) server datum —
   * so never-synced folders still show an honest server-sourced signal.
   */
  function badgeCount(mb: MailboxRow): number {
    if (mb.last_sync_at) return mb.unread_count;
    return mb.unseen_count ?? 0;
  }

  function renderItem(mb: MailboxRow, label: string, depth: number) {
    const count = badgeCount(mb);
    const kind = systemKind(mb);
    const size = depth > 0 ? 15 : 19;
    const glyph =
      kind === "trash" ? (
        <IconTrash size={size} />
      ) : kind === "sent" ? (
        <IconMail size={size} />
      ) : kind === "drafts" ? (
        <IconBook size={size} />
      ) : (
        <IconInbox size={size} />
      );
    return (
      <button
        type="button"
        className={`sidebar-item${depth > 0 ? " sidebar-subitem" : ""}`}
        style={depth > 0 ? { paddingLeft: `${12 + depth * 14}px` } : undefined}
        title={mb.display_name}
        onClick={() => onMailboxSelect(mb.name)}
        aria-current={selectedMailbox === mb.name ? "page" : undefined}
        onContextMenu={(e) => {
          e.preventDefault();
          openMenu(mb, e.clientX, e.clientY);
        }}
        onKeyDown={(e) => {
          // Keyboard menu key or Shift+F10 opens the folder menu.
          if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
            e.preventDefault();
            const rect = e.currentTarget.getBoundingClientRect();
            openMenu(mb, rect.left + 24, rect.bottom + 4);
          }
        }}
      >
        <span className="sidebar-icon" aria-hidden="true">
          {glyph}
        </span>
        <span className="sidebar-label">{label}</span>
        {count > 0 && (
          <span className="sidebar-badge" aria-label={`${count} mensagens`}>
            {count > 999 ? "999+" : count}
          </span>
        )}
      </button>
    );
  }

  function renderNode(node: TreeNode, depth: number): ReactNode {
    return (
      <li key={node.mailbox.id}>
        {renderItem(node.mailbox, node.shortLabel, depth)}
        {node.children.length > 0 && (
          <ul className="sidebar-sublist" aria-label={node.shortLabel}>
            {node.children.map((child) => renderNode(child, depth + 1))}
          </ul>
        )}
      </li>
    );
  }

  return (
    <nav className="sidebar" aria-label="Trilha de estudos">
      <div className="sidebar-head">
        <h3>Minha trilha</h3>
      </div>
      <ul className="sidebar-list">
        {showMailboxItems ? (
          <>
            {tree.system.map((mb) => (
              <li key={mb.id}>{renderItem(mb, mb.display_name, 0)}</li>
            ))}
            {tree.roots.map((node) => renderNode(node, 0))}
          </>
        ) : (
          <li>
            <button
              type="button"
              className="sidebar-item"
              disabled
              aria-label="Sincronize para carregar pastas"
            >
              <span className="sidebar-icon" aria-hidden="true">
                <IconInbox size={19} />
              </span>
              <span className="sidebar-label">Sincronize para carregar pastas</span>
            </button>
          </li>
        )}
      </ul>
      <div className="sidebar-foot">
        {reviewCount > 0 && (
          <button
            type="button"
            className="sidebar-review"
            onClick={() => setReviewOpen(true)}
            aria-label={`${reviewCount} e-mails para revisar`}
            style={{
              display: "flex",
              width: "100%",
              alignItems: "center",
              gap: 8,
              marginBottom: 8,
              padding: "6px 10px",
              borderRadius: 8,
              border: "1px dashed #3b82f6",
              background: "transparent",
              color: "#93c5fd",
              fontSize: "0.8rem",
              cursor: "pointer",
            }}
          >
            <span aria-hidden="true">🔍</span>
            <span>
              A Classificar <strong>({reviewCount})</strong>
            </span>
          </button>
        )}
        <button
          type="button"
          className="sidebar-newfolder"
          onClick={onCreateFolder}
          aria-label="Criar nova pasta"
        >
          <span className="sidebar-newfolder-icon" aria-hidden="true">
            <IconFolderPlus size={17} />
          </span>
          <span>Nova pasta</span>
        </button>
        <button
          type="button"
          className="sidebar-newfolder"
          onClick={() => setEditorOpen(true)}
          aria-label="Editar categorias do classificador"
        >
          <span className="sidebar-newfolder-icon" aria-hidden="true">
            <IconBook size={17} />
          </span>
          <span>Categorias</span>
        </button>
        <button
          type="button"
          className="sidebar-newfolder"
          onClick={() => setBatchOpen(true)}
          aria-label="Reorganizar toda a conta"
        >
          <span className="sidebar-newfolder-icon" aria-hidden="true">
            <IconMail size={17} />
          </span>
          <span>Reorganizar</span>
        </button>
        <strong>Tudo em dia</strong>
        <p>Nenhuma mensagem nova pendente. Sincronize para buscar avisos recentes.</p>
      </div>
      {reviewOpen && (
        <ReviewPanel
          onClose={() => setReviewOpen(false)}
          onChanged={() => {
            void (async () => {
              try {
                const rows = (await invoke("review_list", { limit: 200 })) as unknown[];
                setReviewCount(rows.length);
              } catch {
                /* keep stale count */
              }
            })();
          }}
        />
      )}
      {editorOpen && <TaxonomyEditor onClose={() => setEditorOpen(false)} />}
      {batchOpen && <BatchPanel onClose={() => setBatchOpen(false)} />}
      {menu !== null &&
        (() => {
          const menuRow = mailboxes.find((m) => m.name === menu.name);
          if (!menuRow) return null;
          return (
            <FolderContextMenu
              mailbox={menuRow}
              x={menu.x}
              y={menu.y}
              onRename={() => onRenameFolder(menuRow.name)}
              onDelete={() => onDeleteFolder(menuRow.name)}
              onClose={() => setMenu(null)}
            />
          );
        })()}
    </nav>
  );
}
