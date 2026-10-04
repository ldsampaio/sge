import "./MailboxView.css";
import { IconHome, IconInbox, IconBook, IconHelp } from "./icons";

interface SidebarProps {
  messageCount: number;
  unreadCount?: number;
  selectedMailbox: string;
  onMailboxSelect: (mailbox: string) => void;
}

export default function Sidebar({
  messageCount,
  unreadCount = 0,
  selectedMailbox,
  onMailboxSelect,
}: SidebarProps) {
  return (
    <nav className="sidebar" aria-label="Trilha de estudos">
      <div className="sidebar-head">
        <h3>Minha trilha</h3>
      </div>
      <ul className="sidebar-list">
        <li>
          <button
            type="button"
            className={`sidebar-item ${selectedMailbox === "INBOX" ? "active" : ""}`}
            onClick={() => onMailboxSelect("INBOX")}
            aria-current={selectedMailbox === "INBOX" ? "page" : undefined}
          >
            <span className="sidebar-icon" aria-hidden="true">
              <IconInbox size={19} />
            </span>
            <span className="sidebar-label">Caixa de entrada</span>
            {messageCount > 0 && (
              <span className="sidebar-badge" aria-label={`${messageCount} mensagens`}>
                {messageCount > 999 ? "999+" : messageCount}
              </span>
            )}
          </button>
        </li>
        <li>
          <button
            type="button"
            className="sidebar-item"
            onClick={() => onMailboxSelect("INBOX")}
            title="Em breve: resumo do dia"
          >
            <span className="sidebar-icon" aria-hidden="true">
              <IconHome size={19} />
            </span>
            <span className="sidebar-label">Início</span>
          </button>
        </li>
        <li>
          <button
            type="button"
            className="sidebar-item"
            onClick={() => onMailboxSelect("INBOX")}
            title="Em breve: só mensagens com anexo"
          >
            <span className="sidebar-icon" aria-hidden="true">
              <IconBook size={19} />
            </span>
            <span className="sidebar-label">Materiais</span>
          </button>
        </li>
        <li>
          <button
            type="button"
            className="sidebar-item"
            onClick={() => onMailboxSelect("INBOX")}
            title="Em breve: ajuda de estudos"
          >
            <span className="sidebar-icon" aria-hidden="true">
              <IconHelp size={19} />
            </span>
            <span className="sidebar-label">Ajuda</span>
          </button>
        </li>
      </ul>
      <div className="sidebar-foot">
        <strong>{unreadCount > 0 ? `${unreadCount} para ler` : "Tudo em dia"}</strong>
        <p>
          {unreadCount > 0
            ? "Mensagens novas ficam com a borda laranja. Leia no seu ritmo — nada é apagado do servidor."
            : "Nenhuma mensagem nova pendente. Sincronize para buscar avisos recentes."}
        </p>
      </div>
    </nav>
  );
}
