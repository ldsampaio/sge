import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { MessageRow, MessageView } from "../types";
import "./MailboxView.css";

interface ReadingPaneProps {
  selectedMessage: MessageRow | null;
}

/**
 * Reading pane (Phase 4): renders the full message body in a sandboxed
 * iframe (HTML) or `<pre>` (plain text), with an attachment list that
 * saves via a native file-picker → `save_attachment` Tauri command.
 *
 * Security model:
 *  - HTML is ammonia-sanitized server-side (bodies::sanitize_html).
 *  - The iframe `sandbox` attribute grants NO permissions — no scripts,
 *    no same-origin, no forms, no toplevel navigation.
 *  - `referrerPolicy="no-referrer"` prevents referrer leakage to remote
 *    resources inside the iframe.
 *  - Attachment save paths are reduced to basename server-side (D-attachments).
 */
export default function ReadingPane({ selectedMessage }: ReadingPaneProps) {
  const [message, setMessage] = useState<MessageView | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [savingPart, setSavingPart] = useState<string | null>(null);

  useEffect(() => {
    if (!selectedMessage) {
      setMessage(null);
      setStatus("idle");
      return;
    }

    const uid = selectedMessage.uid;
    setStatus("loading");
    setErrorMsg("");
    setMessage(null);

    invoke<MessageView>("fetch_message", { uid })
      .then((result) => {
        setMessage(result);
        setStatus("ready");
      })
      .catch((err: { message: string }) => {
        setErrorMsg(err.message || "Failed to fetch message");
        setStatus("error");
      });
  }, [selectedMessage]);

  async function handleSaveAttachment(part_number: string, name: string) {
    setSavingPart(part_number);
    try {
      const chosen = await save({
        title: "Save attachment",
        defaultPath: name,
      });
      if (chosen) {
        await invoke("save_attachment", {
          uid: selectedMessage?.uid,
          part_number,
          file_path: chosen,
        });
      }
    } catch {
      // Dialog cancelled or error — silently ignore.
    } finally {
      setSavingPart(null);
    }
  }

  if (!selectedMessage) {
    return (
      <aside className="reading-pane" aria-label="Reading pane">
        <p className="reading-placeholder">Select a message to read</p>
      </aside>
    );
  }

  if (status === "loading") {
    return (
      <aside className="reading-pane" aria-label="Reading pane">
        <p className="reading-placeholder">Loading message…</p>
      </aside>
    );
  }

  if (status === "error") {
    return (
      <aside className="reading-pane" aria-label="Reading pane">
        <p className="reading-error">{errorMsg}</p>
      </aside>
    );
  }

  if (!message) {
    return (
      <aside className="reading-pane" aria-label="Reading pane">
        <p className="reading-placeholder">No content</p>
      </aside>
    );
  }

  return (
    <aside className="reading-pane" aria-label="Reading pane">
      <header className="reading-header">
        <h2 className="reading-subject">
          {message.subject || <em>(no subject)</em>}
        </h2>
        <div className="reading-meta">
          <span>From: {message.from_addr}</span>
          {message.to_addrs.length > 0 && (
            <span>To: {message.to_addrs.join(", ")}</span>
          )}
          <time dateTime={message.date_utc}>
            {new Date(message.date_utc).toLocaleString()}
          </time>
        </div>
      </header>

      {message.has_attachments && message.attachments.length > 0 && (
        <div className="reading-attachments">
          {message.attachments.map((att) => (
            <button
              key={att.part_number}
              className="attachment-btn"
              onClick={() => handleSaveAttachment(att.part_number, att.name)}
              disabled={savingPart === att.part_number}
            >
              📎 {att.name} · {formatBytes(att.size)}
              {savingPart === att.part_number && " ↻"}
            </button>
          ))}
        </div>
      )}

      <div className="reading-body">
        {message.html ? (
          <iframe
            sandbox=""
            srcDoc={message.html}
            title={message.subject || "Message"}
            className="reading-iframe"
            referrerPolicy="no-referrer"
          />
        ) : message.text ? (
          <pre className="reading-text">{message.text}</pre>
        ) : (
          <p className="reading-placeholder">No readable content</p>
        )}
      </div>
    </aside>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
