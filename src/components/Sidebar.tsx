import "./MailboxView.css";
import type { MessageRow } from "../types";

interface SidebarProps {
  messageCount: number;
  selectedMailbox: string;
  onMailboxSelect: (mailbox: string) => void;
}

/**
 * Sidebar with an INBOX node (M1 is INBOX-only).
 * Shows a message count badge and highlights the selected mailbox.
 */
export default function Sidebar({ messageCount, selectedMailbox, onMailboxSelect }: SidebarProps) {
  return (
    <nav className="sidebar" aria-label="Mailboxes">
      <ul className="sidebar-list">
        <li>
          <button
            type="button"
            className={`sidebar-item ${selectedMailbox === "INBOX" ? "active" : ""}`}
            onClick={() => onMailboxSelect("INBOX")}
          >
            <span className="sidebar-icon">📭</span>
            <span className="sidebar-label">INBOX</span>
            {messageCount > 0 && (
              <span className="sidebar-badge">{messageCount}</span>
            )}
          </button>
        </li>
      </ul>
    </nav>
  );
}

export type { MessageRow };
