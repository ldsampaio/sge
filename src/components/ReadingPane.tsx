import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { MessageRow, MessageView, SetSeenResult, MailboxRow } from "../types";
import { isUnread, dispatchFlagUpdate } from "../types";
import { IconBook, IconDownload, IconMail, IconTrash, IconMove } from "./icons";
import MoveMenu from "./MoveMenu";
import ExpungeModal from "./ExpungeModal";
import { SuggestionBar } from "./ClassifyBadges";
import "./MailboxView.css";

/** Suggestion detail for the reader bar (mirrors the Rust struct). */
interface SuggestionDetail {
  message_id: number;
  primary_id: string;
  primary_name: string;
  child_name: string | null;
  secondary_name: string | null;
  confidence: number;
  needs_review: boolean;
  stale: boolean;
  dismissed: boolean;
  dest_display: string;
  justification: string;
  tops: Array<[string, string]>;
}

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

interface ReadingPaneProps {
  selectedMessage: MessageRow | null;
  mailbox?: string;
  /** All known folders for the move menu. */
  mailboxes?: MailboxRow[];
}

export default function ReadingPane({ selectedMessage, mailbox = "INBOX", mailboxes = [] }: ReadingPaneProps) {
  const [message, setMessage] = useState<MessageView | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [savingPart, setSavingPart] = useState<string | null>(null);
  /** Read/unread of the open message; drives the header toggle's target label. */
  const [localUnread, setLocalUnread] = useState(false);

  // Move menu state
  const [moveMenu, setMoveMenu] = useState<{
    open: boolean;
    uid: number | null;
    triggerRef: React.MutableRefObject<HTMLButtonElement | null>;
  }>({ open: false, uid: null, triggerRef: { current: null } });
  /** Phase 18 suggestion bar state (reader-scoped, per selected message). */
  const [suggestion, setSuggestion] = useState<SuggestionDetail | null>(null);
  const [suggestBusy, setSuggestBusy] = useState(false);
  const [suggestMsg, setSuggestMsg] = useState<string | null>(null);

  useEffect(() => {
    setSuggestion(null);
    setSuggestMsg(null);
    if (!selectedMessage) return;
    void (async () => {
      try {
        const d = (await invoke("suggestion_for_uid", {
          mailbox,
          uid: selectedMessage.uid,
        })) as SuggestionDetail | null;
        setSuggestion(d && !d.dismissed ? d : null);
      } catch {
        setSuggestion(null);
      }
    })();
  }, [selectedMessage, mailbox]);

  const confirmSuggestion = async (allowNest: boolean) => {
    if (!suggestion) return;
    setSuggestBusy(true);
    setSuggestMsg(null);
    try {
      const r = (await invoke("confirm_suggestion", {
        messageId: suggestion.message_id,
        allowNest,
      })) as { detail: string };
      setSuggestMsg(r.detail);
      setSuggestion(null);
    } catch (e) {
      const s = String(e);
      if (s.includes("COLLISION")) {
        if (window.confirm("Já existe uma pasta Auto. Arquivar dentro dela?")) {
          await confirmSuggestion(true);
        }
      } else {
        setSuggestMsg(`Falha ao arquivar: ${e}`);
      }
    } finally {
      setSuggestBusy(false);
    }
  };

  const correctSuggestion = async (toId: string) => {
    if (!suggestion) return;
    setSuggestBusy(true);
    setSuggestMsg(null);
    try {
      const r = (await invoke("override_label", {
        messageId: suggestion.message_id,
        toCategory: toId,
      })) as { detail: string };
      setSuggestMsg(r.detail);
      setSuggestion(null);
    } catch (e) {
      setSuggestMsg(`Falha ao corrigir: ${e}`);
    } finally {
      setSuggestBusy(false);
    }
  };

  const dismissSuggestion = async () => {
    if (!suggestion) return;
    try {
      await invoke("dismiss_suggestion", { messageId: suggestion.message_id });
      setSuggestion(null);
    } catch (e) {
      setSuggestMsg(`Falha ao dispensar: ${e}`);
    }
  };

  /** Manual classify (unlabeled or excluded mail, stale re-do). Wires the
   *  `classify_message_uid` command the audit found orphaned. */
  const classifyNow = async () => {
    if (!selectedMessage) return;
    setSuggestBusy(true);
    setSuggestMsg(null);
    try {
      await invoke("classify_message_uid", { mailbox, uid: selectedMessage.uid });
      const d = (await invoke("suggestion_for_uid", {
        mailbox,
        uid: selectedMessage.uid,
      })) as SuggestionDetail | null;
      setSuggestion(d && !d.dismissed ? d : null);
      if (!d) setSuggestMsg("Sem confiança suficiente — foi para A Classificar.");
    } catch (e) {
      setSuggestMsg(`Falha ao classificar: ${e}`);
    } finally {
      setSuggestBusy(false);
    }
  };
  // Expunge modal state
  const [expungeModal, setExpungeModal] = useState<{
    open: boolean;
    uids: number[];
    folderName: string;
    triggerRef: React.MutableRefObject<HTMLButtonElement | null>;
  }>({ open: false, uids: [], folderName: "", triggerRef: { current: null } });
  // Undo toast state
  const [undoToast, setUndoToast] = useState<{
    visible: boolean;
    message: string;
    onUndo: () => void;
  }>({ visible: false, message: "", onUndo: () => {} });

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

    invoke<MessageView>("fetch_message", { uid, mailbox })
      .then(async (result) => {
        if (stale) return;
        setMessage(result);
        setStatus("ready");
        // Thunderbird-style Seen-on-open: opening an unread message marks it
        // read optimistically — no toast, browsing never blocked.
        if (isUnread(flags)) {
          setLocalUnread(false);
          try {
            const r = await invoke<SetSeenResult>("set_seen", { uid, seen: true, mailbox });
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
  }, [selectedMessage, mailbox]);

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
      const r = await invoke<SetSeenResult>("set_seen", { uid, seen: targetSeen, mailbox });
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
          mailbox,
        });
      }
    } catch {
      // Dialog cancelled or error — silently ignore.
    } finally {
      setSavingPart(null);
    }
  }

  /** Show undo toast for delete/move operations. */
  function showUndoToast(message: string, onUndo: () => void) {
    setUndoToast({ visible: true, message, onUndo });
  }

  function hideUndoToast() {
    setUndoToast({ visible: false, message: "", onUndo: () => {} });
  }

  /** Handle delete from reader: invoke delete_message. */
  const handleDelete = async () => {
    if (!selectedMessage || !isTauriRuntime()) return;
    const uid = selectedMessage.uid;
    try {
      await invoke<{ acked: boolean; pending_count: number; detail: string }>(
        "delete_message",
        { uid, mailbox }
      );
      showUndoToast(
        `Mensagem movida para a Lixeira.`,
        () => handleUndoDelete(uid)
      );
    } catch (err) {
      const msg = String(err);
      if (msg.startsWith("need_trash_confirm:")) {
        setErrorMsg(msg.replace("need_trash_confirm: ", ""));
      } else {
        setErrorMsg(`Não foi possível apagar: ${msg}`);
      }
    }
  };

  /** Handle move from reader: open move menu. */
  const handleMove = (triggerRef: React.MutableRefObject<HTMLButtonElement | null>) => {
    if (!selectedMessage) return;
    setMoveMenu({ open: true, uid: selectedMessage.uid, triggerRef });
  };

  const closeMoveMenu = () => {
    setMoveMenu({ open: false, uid: null, triggerRef: { current: null } });
  };

  const onMoveSelect = async (destRaw: string) => {
    if (!selectedMessage) return;
    const uid = selectedMessage.uid;
    closeMoveMenu();
    try {
      await invoke<{ acked: boolean; pending_count: number; detail: string }>(
        "move_message",
        { uid, dest: destRaw, mailbox }
      );
      const folderLabel = mailboxes.find((mb) => mb.name === destRaw)?.display_name ?? destRaw;
      showUndoToast(
        `Mensagem movida para ${folderLabel}.`,
        () => handleUndoMove(uid, destRaw)
      );
    } catch (err) {
      const msg = String(err);
      setErrorMsg(`Não foi possível mover: ${msg}`);
      const folderLabel = mailboxes.find((mb) => mb.name === destRaw)?.display_name ?? destRaw;
      showUndoToast(
        `Não foi possível mover para ${folderLabel}: ${msg}`,
        () => handleMove(moveMenu.triggerRef)
      );
    }
  };

  const handleUndoDelete = async (uid: number) => {
    try {
      await invoke<{ restored: boolean; detail: string }>("undo_queued_op", { uid, mailbox });
      hideUndoToast();
    } catch (err) {
      const msg = String(err);
      setErrorMsg(`Não foi possível desfazer: ${msg}`);
      hideUndoToast();
    }
  };

  const handleUndoMove = async (uid: number, _destRaw: string) => {
    try {
      await invoke<{ restored: boolean; detail: string }>("undo_queued_op", { uid, mailbox });
      hideUndoToast();
    } catch (err) {
      const msg = String(err);
      setErrorMsg(`Não foi possível desfazer: ${msg}`);
      hideUndoToast();
    }
  };

  /** Open expunge modal (Trash only). */
  const openExpungeModal = (triggerRef: React.MutableRefObject<HTMLButtonElement | null>) => {
    if (!selectedMessage) return;
    setExpungeModal({
      open: true,
      uids: [selectedMessage.uid],
      folderName: mailboxes.find((mb) => mb.name === mailbox)?.display_name ?? mailbox,
      triggerRef,
    });
  };

  const closeExpungeModal = () => {
    setExpungeModal({ open: false, uids: [], folderName: "", triggerRef: { current: null } });
  };

  const onExpungeConfirm = async (uids: number[]) => {
    try {
      await invoke<{ removed: number; acked: boolean; detail: string }>(
        "expunge_messages",
        { uids, mailbox }
      );
      closeExpungeModal();
    } catch (err) {
      const msg = String(err);
      setErrorMsg(`Não foi possível apagar para sempre: ${msg}`);
      closeExpungeModal();
    }
  };

  /** Handle restore from Trash: move back to INBOX (or origin if known). */
  const handleRestore = async () => {
    if (!selectedMessage) return;
    const uid = selectedMessage.uid;
    const destRaw = "INBOX"; // Default restore destination
    try {
      await invoke<{ acked: boolean; pending_count: number; detail: string }>(
        "move_message",
        { uid, dest: destRaw, mailbox }
      );
      const folderLabel = mailboxes.find((mb) => mb.name === destRaw)?.display_name ?? destRaw;
      showUndoToast(
        `Mensagem movida para ${folderLabel}.`,
        () => handleUndoMove(uid, destRaw)
      );
    } catch (err) {
      const msg = String(err);
      setErrorMsg(`Não foi possível restaurar: ${msg}`);
    }
  };

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
        {/* Phase 18: confirm-gated suggestion bar (backend-enforced gate). */}
        {suggestion && !suggestion.stale && (
          <SuggestionBar
            destDisplay={suggestion.dest_display}
            confidence={suggestion.confidence}
            needsReview={suggestion.needs_review}
            justification={suggestion.justification}
            tops={suggestion.tops.map(([id, name]) => ({ id, name }))}
            busy={suggestBusy}
            message={suggestMsg}
            onConfirm={() => void confirmSuggestion(false)}
            onCorrect={(toId) => void correctSuggestion(toId)}
            onDismiss={() => void dismissSuggestion()}
          />
        )}
        {suggestion && suggestion.stale && (
          <div role="group" aria-label="Sugestão desatualizada" style={{ margin: "8px 0", fontSize: "0.82rem" }}>
            <span style={{ color: "#eab308" }}>
              A taxonomia mudou desde esta sugestão —{" "}
            </span>
            <button type="button" className="btn btn-small" disabled={suggestBusy} onClick={() => void classifyNow()}>
              Reclassificar
            </button>
          </div>
        )}
        {!suggestion && selectedMessage && (
          <div style={{ margin: "8px 0", fontSize: "0.82rem" }}>
            <button type="button" className="btn btn-small" disabled={suggestBusy} onClick={() => void classifyNow()}>
              Classificar agora
            </button>
            {suggestMsg && (
              <span role="status" style={{ marginLeft: 8, color: "#94a3b8" }}>
                {suggestMsg}
              </span>
            )}
          </div>
        )}
        {suggestion && !suggestion.stale && suggestMsg && (
          <div role="status" style={{ color: "#4ade80", fontSize: "0.82rem" }}>
            {suggestMsg}
          </div>
        )}
        {/* Action row: delete/move or restore/expunge when in Trash */}
        <div className="reading-actions" role="group" aria-label="Ações da mensagem">
          {mailbox.toLowerCase() === "trash" || mailbox.toLowerCase() === "lixeira" ? (
            <>
              <button
                type="button"
                className="btn btn-restore"
                onClick={() => void handleRestore()}
                title="Restaurar para a Caixa de entrada"
              >
                <IconMove size={16} /> Restaurar
              </button>
              <button
                type="button"
                ref={(el) => { expungeModal.triggerRef.current = el; }}
                className="btn btn-destructive"
                onClick={() => openExpungeModal(expungeModal.triggerRef)}
                title="Apagar para sempre"
              >
                <IconTrash size={16} /> Apagar para sempre
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                className="btn"
                onClick={() => void handleDelete()}
                title="Apagar — vai para a Lixeira"
              >
                <IconTrash size={16} /> Apagar
              </button>
              <button
                type="button"
                ref={(el) => { moveMenu.triggerRef.current = el; }}
                className="btn"
                onClick={() => handleMove(moveMenu.triggerRef)}
                title="Mover para outra pasta"
              >
                <IconMove size={16} /> Mover para…
              </button>
            </>
          )}
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

      {/* Undo Toast */}
      {undoToast.visible && (
        <div className="undo-toast" role="status" aria-live="polite">
          <span>{undoToast.message}</span>
          <button
            type="button"
            className="btn btn-undo"
            onClick={() => {
              undoToast.onUndo();
              hideUndoToast();
            }}
          >
            Desfazer
          </button>
        </div>
      )}

      {/* Move Menu */}
      {moveMenu.open && moveMenu.uid !== null && (
        <MoveMenu
          sourceMailbox={mailbox}
          mailboxes={mailboxes}
          onSelect={onMoveSelect}
          onClose={closeMoveMenu}
          triggerRef={moveMenu.triggerRef}
        />
      )}

      {/* Expunge Modal */}
      {expungeModal.open && (
        <ExpungeModal
          count={expungeModal.uids.length}
          folderName={expungeModal.folderName}
          uids={expungeModal.uids}
          onConfirm={onExpungeConfirm}
          onCancel={closeExpungeModal}
          isOpen={expungeModal.open}
        />
      )}
    </aside>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
