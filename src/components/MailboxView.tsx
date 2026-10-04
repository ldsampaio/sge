import { useState, useCallback } from "react";
import Sidebar from "./Sidebar";
import MessageList, { type ListState } from "./MessageList";
import ReadingPane from "./ReadingPane";
import SyncStatus from "./SyncStatus";
import SearchBar from "./SearchBar";
import type { MessageRow } from "../types";
import { IconCap } from "./icons";
import "./MailboxView.css";

interface MailboxViewProps {
  mailbox?: string;
}

export default function MailboxView({ mailbox = "INBOX" }: MailboxViewProps) {
  const [selectedMessage, setSelectedMessage] = useState<MessageRow | null>(null);
  const [searchQuery, setSearchQuery] = useState<string | null>(null);
  const [messageCount, setMessageCount] = useState(0);
  const [listState, setListState] = useState<ListState>({
    kind: "ready",
    message: "",
  });
  const [refreshKey, setRefreshKey] = useState(0);

  function handleMessageSelect(msg: MessageRow) {
    setSelectedMessage(msg);
  }

  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query.length > 0 ? query : null);
    setSelectedMessage(null);
  }, []);

  function handleMailboxSelect(m: string) {
    void m;
  }

  const handleSyncComplete = useCallback(() => {
    setRefreshKey((k) => k + 1);
  }, []);

  return (
    <div className="mailbox-layout">
      <header className="mailbox-header">
        <div className="mailbox-titleblock">
          <span className="mailbox-avatar" aria-hidden="true">
            <IconCap size={24} />
          </span>
          <div>
            <h2>Sala de aula · Caixa de entrada</h2>
            <p className="mailbox-subtitle">
              Avisos e materiais da escola, organizados para estudar offline
            </p>
          </div>
        </div>
        <SyncStatus onSyncComplete={handleSyncComplete} />
      </header>
      <div className="mailbox-panes">
        <Sidebar
          messageCount={messageCount}
          selectedMailbox={mailbox}
          onMailboxSelect={handleMailboxSelect}
        />
        <main className="message-panel" aria-label="Fila de leitura">
          <div className="message-list-header">
            <SearchBar onSearch={handleSearch} disabled={listState.kind === "loading"} />
            {listState.kind === "ready" && (
              <span className="message-count" aria-live="polite">
                {messageCount} {messageCount === 1 ? "mensagem" : "mensagens"}
              </span>
            )}
          </div>
          <MessageList
            mailbox={mailbox}
            searchQuery={searchQuery}
            refreshKey={refreshKey}
            onMessageSelect={handleMessageSelect}
            selectedUid={selectedMessage?.uid ?? null}
            onStateChange={setListState}
            onMessageCount={setMessageCount}
          />
        </main>
        <ReadingPane selectedMessage={selectedMessage} />
      </div>
    </div>
  );
}
