import "./MailboxView.css";
import { IconInbox } from "./icons";
import type { MailboxRow } from "../types";

interface SidebarProps {
  selectedMailbox: string;
  mailboxes: MailboxRow[];
  onMailboxSelect: (mailbox: string) => void;
}

export default function Sidebar({
  selectedMailbox,
  mailboxes = [],
  onMailboxSelect,
}: SidebarProps) {
  const showMailboxItems = mailboxes.length > 0;

  /**
   * Badge signal per folder (FOLD-02): the dynamic local unread count once
   * the folder has synced, else the cached STATUS (UNSEEN) server datum —
   * so never-synced folders still show an honest server-sourced signal.
   */
  function badgeCount(mb: MailboxRow): number {
    if (mb.last_sync_at) return mb.unread_count;
    return mb.unseen_count ?? 0;
  }

  return (
    <nav className="sidebar" aria-label="Trilha de estudos">
      <div className="sidebar-head">
        <h3>Minha trilha</h3>
      </div>
      <ul className="sidebar-list">
        {showMailboxItems
          ? mailboxes.map((mb) => (
              <li key={mb.id}>
                <button
                  type="button"
                  className={`sidebar-item ${selectedMailbox === mb.name ? "active" : ""}`}
                  onClick={() => onMailboxSelect(mb.name)}
                  aria-current={selectedMailbox === mb.name ? "page" : undefined}
                >
                  <span className="sidebar-icon" aria-hidden="true">
                    <IconInbox size={19} />
                  </span>
                  <span className="sidebar-label">{mb.name}</span>
                  {badgeCount(mb) > 0 && (
                    <span
                      className="sidebar-badge"
                      aria-label={`${badgeCount(mb)} mensagens`}
                    >
                      {badgeCount(mb) > 999 ? "999+" : badgeCount(mb)}
                    </span>
                  )}
                </button>
              </li>
            ))
          : (
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
