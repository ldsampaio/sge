import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { MessageRow, MessageView, SetSeenResult } from "../types";
import { isUnread, dispatchFlagUpdate } from "../types";
import { IconBook, IconDownload, IconMail } from "./icons";
import "./MailboxView.css";

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

interface ReadingPaneProps {
  selectedMessage: MessageRow | null;
}

export default function ReadingPane({ selectedMessage }: ReadingPaneProps) {
  const [message, setMessage] = useState<MessageView | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [savingPart, setSavingPart] = useState<string | null>(null);
  /** Read/unread of the open message; drives the header toggle's target label. */
  const [localUnread, setLocalUnread] = useState(false);

  useEffect(() => {
    if (!selectedMessage) {
      setMessage(null);
      setStatus("idle");
      return;
    }

    const uid = selectedMessage.uid;
    const flags = selectedMessage.flags;
    let stale = false;
    setStatus("loading");
    setErrorMsg("");
    setMessage(null);
    setLocalUnread(isUnread(flags));

    invoke<MessageView>("fetch_message", { uid })
      .then(async (result) => {
        if (stale) return;
        setMessage(result);
        setStatus("ready");
        // Thunderbird-style Seen-on-open: opening an unread message marks it
        // read optimistically — no toast, browsing never blocked.
        if (isUnread(flags)) {
          setLocalUnread(false);
          try {
            const r = await invoke<SetSeenResult>("set_seen", { uid, seen: true });
            if (stale) return;
            dispatchFlagUpdate({
              uid: r.uid,
              seen: r.seen,
              acked: r.acked,
              pending_count: r.pending_count,
              detail: r.detail,
              origin: "reader",
            });
          } catch (err) {
            if (stale) return;
            // Invoke rejected: keep the row unread, no row-state notice.
            setLocalUnread(true);
            dispatchFlagUpdate({
              uid,
              seen: true,
              acked: false,
              pending_count: -1,
              detail: String(err),
              origin: "reader",
            });
          }
        }
      })
      .catch((err: { message: string }) => {
        if (stale) return;
        setErrorMsg(err.message || "Não foi possível abrir o aviso");
        setStatus("error");
      });

    return () => {
      stale = true;
    };
  }, [selectedMessage]);

  /**
   * Explicit mark read/unread from the header. Optimistic flip of the local
   * state plus the same set_seen invoke + rollback shape as the list toggle.
   */
  async function handleToggleSeen() {
    if (!selectedMessage || !isTauriRuntime()) return;
    const uid = selectedMessage.uid;
    const targetSeen = localUnread; // unread -> read, read -> unread
    setLocalUnread(!targetSeen);
    try {
      const r = await invoke<SetSeenResult>("set_seen", { uid, seen: targetSeen });
      dispatchFlagUpdate({
        uid: r.uid,
        seen: r.seen,
        acked: r.acked,
        pending_count: r.pending_count,
        detail: r.detail,
        origin: "reader",
      });
    } catch (err) {
      setLocalUnread(targetSeen);
      dispatchFlagUpdate({
        uid,
        seen: targetSeen,
        acked: false,
        pending_count: -1,
        detail: String(err),
        origin: "reader",
      });
    }
  }

  async function handleSaveAttachment(part_number: string, name: string) {
    setSavingPart(part_number);
    try {
      const chosen = await save({
        title: "Guardar material",
        defaultPath: name,
      });
      if (chosen) {
        await invoke("save_attachment", {
          uid: selectedMessage?.uid,
          partNumber: part_number,
          filePath: chosen,
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
      <aside className="reading-pane flex-center" aria-label="Leitura">
        <p className="reading-placeholder">
          <span className="list-empty-ill" aria-hidden="true">
            <IconBook size={28} />
          </span>
          Escolha um aviso na fila para ler aqui
        </p>
      </aside>
    );
  }

  if (status === "loading") {
    return (
      <aside className="reading-pane flex-center" aria-label="Leitura">
        <p className="reading-placeholder" role="status">
          Abrindo o aviso…
        </p>
      </aside>
    );
  }

  if (status === "error") {
    return (
      <aside className="reading-pane" aria-label="Leitura">
        <p className="reading-error" role="alert">
          {errorMsg}
        </p>
      </aside>
    );
  }

  if (!message) {
    return (
      <aside className="reading-pane flex-center" aria-label="Leitura">
        <p className="reading-placeholder">Sem conteúdo para mostrar</p>
      </aside>
    );
  }

  return (
    <aside className="reading-pane" aria-label="Leitura do aviso">
      <header className="reading-header">
        <span className="reading-kicker">E-mail</span>
        <h2 className="reading-subject">{message.subject || <em>(sem assunto)</em>}</h2>
        <div className="reading-meta">
          <span>
            <strong>De:</strong> {message.from_addr}
          </span>
          {message.to_addrs.length > 0 && (
            <span>
              <strong>Para:</strong> {message.to_addrs.join(", ")}
            </span>
          )}
          <time dateTime={message.date_utc}>
            {new Date(message.date_utc).toLocaleString("pt-BR")}
          </time>
        </div>
        <button
          type="button"
          className="btn"
          onClick={() => void handleToggleSeen()}
        >
          {localUnread ? "Marcar como lido" : "Marcar como não lido"}
        </button>
      </header>

      {message.has_attachments && message.attachments.length > 0 && (
        <div className="reading-attachments">
          <span className="reading-attachments-title">
            Materiais desta aula · {message.attachments.length}
          </span>
          {message.attachments.map((att) => (
            <button
              key={att.part_number}
              className="attachment-btn"
              onClick={() => handleSaveAttachment(att.part_number, att.name)}
              disabled={savingPart === att.part_number}
            >
              <IconDownload size={14} />
              {att.name} · {formatBytes(att.size)}
              {savingPart === att.part_number && " …"}
            </button>
          ))}
        </div>
      )}

      <div className="reading-body">
        {message.html ? (
          <iframe
            sandbox=""
            srcDoc={message.html}
            title={message.subject || "Aviso"}
            className="reading-iframe"
            referrerPolicy="no-referrer"
          />
        ) : message.text ? (
          <pre className="reading-text">{message.text}</pre>
        ) : (
          <p className="reading-placeholder">
            <span className="list-empty-ill" aria-hidden="true">
              <IconMail size={26} />
            </span>
            Este aviso não tem texto legível
          </p>
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
