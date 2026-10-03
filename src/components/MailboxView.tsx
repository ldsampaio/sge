import { useState } from "react";
import Sidebar from "./Sidebar";
import MessageList, { type ListState } from "./MessageList";
import ReadingPane from "./ReadingPane";
import SyncStatus from "./SyncStatus";
import SearchBar from "./SearchBar";
import type { MessageRow } from "../types";
import "./MailboxView.css";

interface MailboxViewProps {
  mailbox?: string;
}

/**
 * Three-pane Gmail-like mailbox view.
 *
 * Sidebar (INBOX) | Message List (paginated, searchable) | Reading Pane (placeholder).
 * SyncStatus is embedded as a header widget for manual refresh + offline badge.
 */
export default function MailboxView({ mailbox = "INBOX" }: MailboxViewProps) {
  const [selectedMessage, setSelectedMessage] = useState<MessageRow | null>(null);
  const [searchQuery, setSearchQuery] = useState<string | null>(null);
  const [messageCount, setMessageCount] = useState(0);
  const [listState, setListState] = useState<ListState>({
    kind: "ready",
    message: "",
  });

  function handleMessageSelect(msg: MessageRow) {
    setSelectedMessage(msg);
  }

  function handleSearch(query: string) {
    setSearchQuery(query.length > 0 ? query : null);
    setSelectedMessage(null);
  }

  function handleMailboxSelect(mailbox: string) {
    void mailbox; // M1: INBOX-only — accepting but not switching
  }

  return (
    <div className="mailbox-layout">
      <header className="mailbox-header">
        <h2>SGE Mail</h2>
        <SyncStatus />
      </header>
      <div className="mailbox-panes">
        <Sidebar
          messageCount={messageCount}
          selectedMailbox={mailbox}
          onMailboxSelect={handleMailboxSelect}
        />
        <main className="message-panel">
          <div className="message-list-header">
            <SearchBar onSearch={handleSearch} disabled={listState.kind === "loading"} />
            {listState.kind === "ready" && (
              <span className="message-count">{messageCount} messages</span>
            )}
          </div>
          <MessageList
            mailbox={mailbox}
            searchQuery={searchQuery}
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
