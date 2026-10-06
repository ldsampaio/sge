import type { ReactNode } from "react";
import "./MailboxView.css";
import { IconInbox } from "./icons";
import type { MailboxRow } from "../types";

interface SidebarProps {
  selectedMailbox: string;
  mailboxes: MailboxRow[];
  onMailboxSelect: (mailbox: string) => void;
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

function systemRank(rawName: string): number | null {
  const lower = rawName.toLocaleLowerCase();
  for (const group of SYSTEM_ORDER) {
    if (group.names.includes(lower)) return group.rank;
  }
  return null;
}

interface TreeNode {
  mailbox: MailboxRow;
  /** Último segmento do nome para exibição (o caminho completo vai no title). */
  shortLabel: string;
  children: TreeNode[];
}

interface FolderTree {
  system: MailboxRow[];
  roots: TreeNode[];
}

/** Monta a árvore a partir da lista plana: sistema primeiro, resto aninhado. */
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
  // Ordena antes para montagem determinística.
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
  roots.sort((a, b) =>
    a.mailbox.display_name.localeCompare(b.mailbox.display_name, "pt-BR"),
  );
  return { system, roots };
}

export default function Sidebar({
  selectedMailbox,
  mailboxes = [],
  onMailboxSelect,
}: SidebarProps) {
  const showMailboxItems = mailboxes.length > 0;
  const tree = buildTree(mailboxes);

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
    return (
      <button
        type="button"
        className={`sidebar-item${depth > 0 ? " sidebar-subitem" : ""}`}
        style={depth > 0 ? { paddingLeft: `${12 + depth * 14}px` } : undefined}
        title={mb.display_name}
        onClick={() => onMailboxSelect(mb.name)}
        aria-current={selectedMailbox === mb.name ? "page" : undefined}
      >
        <span className="sidebar-icon" aria-hidden="true">
          <IconInbox size={depth > 0 ? 15 : 19} />
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
        <strong>Tudo em dia</strong>
        <p>
          Nenhuma mensagem nova pendente. Sincronize para buscar avisos recentes.
        </p>
      </div>
    </nav>
  );
}
