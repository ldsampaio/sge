import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import Sidebar from "./Sidebar";
import MessageList, { type ListState } from "./MessageList";
import ReadingPane from "./ReadingPane";
import SyncStatus from "./SyncStatus";
import SearchBar from "./SearchBar";
import type { MessageRow, MailboxRow } from "../types";
import { IconCap } from "./icons";
import "./MailboxView.css";

interface MailboxViewProps {
  mailbox?: string;
}

export default function MailboxView({ mailbox = "INBOX" }: MailboxViewProps) {
  const [selectedMailbox, setSelectedMailbox] = useState(mailbox);
  const [selectedMessage, setSelectedMessage] = useState<MessageRow | null>(null);
  const [searchQuery, setSearchQuery] = useState<string | null>(null);
  const [messageCount, setMessageCount] = useState(0);
  const [mailboxes, setMailboxes] = useState<MailboxRow[]>([]);
  const [listState, setListState] = useState<ListState>({
    kind: "ready",
    message: "",
  });
  const [refreshKey, setRefreshKey] = useState(0);

  // FOLD-01: fetch mailbox list from the local store on mount.
  useEffect(() => {
    void (async () => {
      try {
        const rows = await invoke<MailboxRow[]>("list_mailboxes");
        setMailboxes(rows ?? []);
      } catch {
        // No stored mailboxes yet — sidebar falls back to the INBOX bootstrap.
        setMailboxes([]);
      }
    })();
  }, []);

  function handleMessageSelect(msg: MessageRow) {
    // Global search can surface a message from another folder — switch to
    // it so the reader fetches from the right mailbox.
    if (msg.mailbox && msg.mailbox !== selectedMailbox) {
      setSelectedMailbox(msg.mailbox);
      setRefreshKey((k) => k + 1);
    }
    setSelectedMessage(msg);
  }

  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query.length > 0 ? query : null);
    setSelectedMessage(null);
  }, []);

  function handleMailboxSelect(m: string) {
    setSelectedMailbox(m);
    setSelectedMessage(null);
    setRefreshKey((k) => k + 1);
  }

  const handleSyncComplete = useCallback(() => {
    setRefreshKey((k) => k + 1);
    // Refresh mailbox list (unread counts may have changed).
    void (async () => {
      try {
        const rows = await invoke<MailboxRow[]>("list_mailboxes");
        setMailboxes(rows ?? []);
      } catch {
        // ignore — stale counts will refresh on next sync
      }
    })();
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
        <SyncStatus key={selectedMailbox} mailbox={selectedMailbox} onSyncComplete={handleSyncComplete} />
      </header>
      <div className="mailbox-panes">
        <Sidebar
          selectedMailbox={selectedMailbox}
          mailboxes={mailboxes}
          onMailboxSelect={handleMailboxSelect}
        />
        <main className="message-panel" aria-label="Fila de leitura">
          <div className="message-list-header">
            <SearchBar
              onSearch={handleSearch}
              disabled={listState.kind === "loading"}
            />
            {listState.kind === "ready" && (
              <span className="message-count" aria-live="polite">
                {messageCount} {messageCount === 1 ? "mensagem" : "mensagens"}
              </span>
            )}
          </div>
          <MessageList
            key={selectedMailbox}
            mailbox={selectedMailbox}
            mailboxes={mailboxes}
            searchQuery={searchQuery}
            refreshKey={refreshKey}
            onMessageSelect={handleMessageSelect}
            selectedUid={selectedMessage?.uid ?? null}
            onStateChange={setListState}
            onMessageCount={setMessageCount}
          />
        </main>
        <ReadingPane key={selectedMailbox} selectedMessage={selectedMessage} mailbox={selectedMailbox} />
      </div>
    </div>
  );
}
