import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { MessageRow } from "../types";
import { isUnread, humanizeDate } from "../types";

export interface ListState {
  kind: "loading" | "error" | "empty" | "ready";
  message: string;
}

interface MessageListProps {
  mailbox: string;
  searchQuery: string | null;
  onMessageSelect: (msg: MessageRow) => void;
  selectedUid: number | null;
  onStateChange?: (state: ListState) => void;
  onMessageCount?: (count: number) => void;
}

const PAGE_SIZE = 200;

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

const OUTSIDE_DESKTOP_MESSAGE =
  "SGE is running outside its desktop window — launch with `npm run tauri dev` and use the app window, not the browser URL.";

/**
 * Virtualized-by-pagination message list with infinite scroll.
 *
 * Fetches 200 messages at a time from the local SQLite store via
 * `list_messages` (or `search_messages` when searchQuery is set).
 * Pagination through OFFSET keeps the DOM small even at 10k+ messages.
 */
export default function MessageList({
  mailbox,
  searchQuery,
  onMessageSelect,
  selectedUid,
  onStateChange,
  onMessageCount,
}: MessageListProps) {
  const [messages, setMessages] = useState<MessageRow[]>([]);
  const [loaded, setLoaded] = useState(0);
  const [hasMore, setHasMore] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reportState = useCallback(
    (state: ListState) => {
      onStateChange?.(state);
    },
    [onStateChange],
  );

  const fetchBatch = useCallback(
    async (offset: number, replace: boolean) => {
      if (!isTauriRuntime()) {
        reportState({ kind: "error", message: OUTSIDE_DESKTOP_MESSAGE });
        return;
      }

      setLoading(true);
      setError(null);
      if (replace) reportState({ kind: "loading", message: "Loading messages…" });

      try {
        let result: MessageRow[];
        if (searchQuery) {
          result = await invoke("search_messages", {
            mailbox,
            query: searchQuery,
          });
        } else {
          result = await invoke("list_messages", {
            mailbox,
            limit: PAGE_SIZE,
            offset,
          });
        }

        if (replace) {
          setMessages(result);
          onMessageCount?.(result.length);
        } else {
          setMessages((prev) => [...prev, ...result]);
          onMessageCount?.(result.length);
        }
        setLoaded(replace ? result.length : loaded + result.length);
        setHasMore(result.length === PAGE_SIZE);
        reportState({ kind: "ready", message: "" });
      } catch (err) {
        const msg = String(err);
        setError(msg);
        setMessages([]);
        reportState({ kind: "error", message: msg });
      } finally {
        setLoading(false);
      }
    },
    [mailbox, searchQuery, loaded, reportState, onMessageCount],
  );

  // Initial load + search query change → replace full list.
  useEffect(() => {
    setMessages([]);
    setLoaded(0);
    setHasMore(true);
    void fetchBatch(0, true);
  }, [mailbox, searchQuery]);

  // Infinite scroll: load next batch when near bottom.
  const handleScroll = useCallback(
    (e: React.UIEvent<HTMLDivElement>) => {
      if (searchQuery) return; // no pagination in search mode
      const { scrollTop, scrollHeight, clientHeight } = e.currentTarget;
      if (scrollTop + clientHeight > scrollHeight - 200) {
        if (hasMore && !loading) {
          void fetchBatch(loaded, false);
        }
      }
    },
    [hasMore, loading, loaded, fetchBatch, searchQuery],
  );

  // --- Render states ---

  if (!isTauriRuntime()) {
    return (
      <div className="message-list">
        <p role="alert">{OUTSIDE_DESKTOP_MESSAGE}</p>
      </div>
    );
  }

  if (error && messages.length === 0) {
    const isAuthError =
      error.includes("auth") ||
      error.includes("TLS") ||
      error.includes("IMAP");
    return (
      <div className="message-list">
        <div className="list-error">
          <p role="alert">Failed to load messages: {error}</p>
          <button type="button" onClick={() => void fetchBatch(0, true)}>
            Retry
          </button>
          {isAuthError && (
            <p>
              Authentication or connection failed — please reconnect from the
              login screen.
            </p>
          )}
        </div>
      </div>
    );
  }

  if (messages.length === 0 && !loading && !searchQuery) {
    return (
      <div className="message-list">
        <div className="list-empty">
          <p>📭 No messages in INBOX</p>
          <p>
            Connect to your server and sync to see messages here. Use the Sync
            Now button above.
          </p>
        </div>
      </div>
    );
  }

  if (messages.length === 0 && searchQuery && !loading) {
    return (
      <div className="message-list">
        <div className="list-empty">
          <p>No results for “{searchQuery}”</p>
        </div>
      </div>
    );
  }

  return (
    <div className="message-list" onScroll={handleScroll}>
      {messages.map((msg) => {
        const unread = isUnread(msg.flags);
        const selected = selectedUid === msg.uid;
        return (
          <button
            type="button"
            key={msg.uid}
            className={`message-row ${unread ? "unread" : "read"} ${selected ? "selected" : ""}`}
            onClick={() => onMessageSelect(msg)}
          >
            <span className={`unread-dot ${unread ? "unread" : "read"}`} aria-label={unread ? "Unread" : "Read"} />
            <div className="message-sender">{msg.from_addr}</div>
            <div className="message-subject">{msg.subject || "(no subject)"}</div>
            <div className="message-date">{humanizeDate(msg.date_utc)}</div>
            {msg.has_attachments && (
              <span className="attachment-icon" title="Has attachments">
                📎
              </span>
            )}
          </button>
        );
      })}
      {loading && hasMore && (
        <div className="list-loading">
          <p>Loading more…</p>
        </div>
      )}
      {!hasMore && !loading && messages.length > 0 && (
        <div className="list-end">
          <p>End of messages</p>
        </div>
      )}
    </div>
  );
}
