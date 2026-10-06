import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { MessageRow, MailboxRow } from "../types";
import {
  isUnread,
  formatRowDate,
  setSeenInFlags,
  dispatchFlagUpdate,
  FLAG_UPDATE_EVENT,
} from "../types";
import type { FlagUpdateDetail, SetSeenResult } from "../types";
import { IconInbox, IconPaperclip, IconTrash, IconMove } from "./icons";
import MoveMenu from "./MoveMenu";
import ExpungeModal from "./ExpungeModal";

export interface ListState {
  kind: "loading" | "error" | "empty" | "ready";
  message: string;
}

interface UndoToastState {
  visible: boolean;
  message: string;
  onUndo: () => void;
  isDelete: boolean; // true = delete (Trash), false = move
  destFolder?: string;
}

interface MessageListProps {
  mailbox: string;
  searchQuery: string | null;
  /** Folder rows for the search-result folder chip (raw → display lookup). */
  mailboxes?: MailboxRow[];
  onMessageSelect: (msg: MessageRow) => void;
  selectedUid: number | null;
  onStateChange?: (state: ListState) => void;
  onMessageCount?: (count: number) => void;
  refreshKey?: number;
}

interface SyncStatusInfo {
  mailbox: string;
  last_sync_at: string;
  uid_validity: number;
  uid_next: number;
  message_count: number;
  pending_count: number;
}

const PAGE_SIZE = 50;
const MAX_PAGE_BUTTONS = 7;

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

const OUTSIDE_DESKTOP_MESSAGE =
  "O SGE Edu roda na janela do app — abra com `npm run tauri dev` e use a janela do aplicativo, não o endereço do navegador.";

function avatarInitial(addr: string): string {
  const clean = addr.trim();
  if (!clean) return "?";
  const name = clean.includes("<") ? clean : clean;
  return (name.charAt(0) || "?").toUpperCase();
}

export default function MessageList({
  mailbox,
  searchQuery,
  mailboxes = [],
  onMessageSelect,
  selectedUid,
  onStateChange,
  onMessageCount,
  refreshKey,
}: MessageListProps) {
  const [messages, setMessages] = useState<MessageRow[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Uids with an optimistic toggle awaiting server acknowledgement (pending wash). */
  const [pendingUids, setPendingUids] = useState<Set<number>>(new Set());
  const searchCache = useRef<{ query: string; rows: MessageRow[] } | null>(null);
  const countRef = useRef(onMessageCount);
  countRef.current = onMessageCount;

  // Refs for action buttons (move menu trigger)
  const actionBtnRefs = useRef<Map<number, HTMLButtonElement>>(new Map());

  // Undo toast state (single slot, replaced by newer ops)
  const [undoToast, setUndoToast] = useState<UndoToastState>({
    visible: false,
    message: "",
    onUndo: () => {},
    isDelete: true,
  });
  // Move menu state
  const [moveMenu, setMoveMenu] = useState<{
    open: boolean;
    uid: number | null;
    triggerRef: React.MutableRefObject<HTMLButtonElement | null>;
  }>({ open: false, uid: null, triggerRef: { current: null } });
  // Expunge modal state
  const [expungeModal, setExpungeModal] = useState<{
    open: boolean;
    uids: number[];
    folderName: string;
    triggerRef: React.MutableRefObject<HTMLButtonElement | null>;
  }>({ open: false, uids: [], folderName: "", triggerRef: { current: null } });

  const reportState = useCallback(
    (state: ListState) => {
      onStateChange?.(state);
    },
    [onStateChange],
  );

  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const safePage = Math.min(page, totalPages - 1);

  const loadPage = useCallback(
    async (p: number) => {
      if (!isTauriRuntime()) {
        reportState({ kind: "error", message: OUTSIDE_DESKTOP_MESSAGE });
        return;
      }

      setLoading(true);
      setError(null);
      reportState({ kind: "loading", message: "Carregando avisos…" });

      try {
        if (searchQuery) {
          if (!searchCache.current || searchCache.current.query !== searchQuery) {
            const rows: MessageRow[] = await invoke("search_messages", {
              mailbox: null,
              query: searchQuery,
            });
            searchCache.current = { query: searchQuery, rows };
          }
          const all = searchCache.current.rows;
          setMessages(all.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE));
          setTotal(all.length);
          countRef.current?.(all.length);
        } else {
          const [rows, status] = await Promise.all([
            invoke<MessageRow[]>("list_messages", {
              mailbox,
              limit: PAGE_SIZE,
              offset: p * PAGE_SIZE,
            }),
            invoke<SyncStatusInfo>("sync_status", { mailbox }),
          ]);
          setMessages(rows);
          setTotal(status.message_count);
          countRef.current?.(status.message_count);
          // Outbox drained (server acknowledged everything) → clear the wash.
          if (status.pending_count === 0) {
            setPendingUids(new Set());
          }
        }
        reportState({ kind: "ready", message: "" });
      } catch (err) {
        const msg = String(err);
        setError(msg);
        setMessages([]);
        setTotal(0);
        reportState({ kind: "error", message: msg });
      } finally {
        setLoading(false);
      }
    },
    [mailbox, searchQuery, reportState],
  );

  /** Raw folder name → display name for the search-result chip. */
  function folderLabel(raw: string): string {
    return mailboxes.find((mb) => mb.name === raw)?.display_name ?? raw;
  }

  /** Show undo toast for delete/move operations. */
  function showUndoToast(
    isDelete: boolean,
    count: number,
    onUndo: () => void,
    destFolder?: string
  ) {
    const folderLabelDisplay = destFolder
      ? mailboxes.find((mb) => mb.name === destFolder)?.display_name ?? destFolder
      : "Lixeira";
    const msg = isDelete
      ? count === 1
        ? `Mensagem movida para a ${folderLabelDisplay}.`
        : `${count} mensagens movidas para a ${folderLabelDisplay}.`
      : count === 1
      ? `Mensagem movida para ${folderLabelDisplay}.`
      : `${count} mensagens movidas para ${folderLabelDisplay}.`;
    setUndoToast({
      visible: true,
      message: msg,
      onUndo,
      isDelete,
      destFolder,
    });
    // Toast persists until next sync / replaced / dismissed — no auto-dismiss timer
    // (outbox entry is the anchor per CONTEXT.md decision)
  }

  /** Hide undo toast. */
  function hideUndoToast() {
    setUndoToast((prev) => ({ ...prev, visible: false }));
  }

  /** Handle delete: optimistic hide + invoke delete_message. */
  const handleDelete = useCallback(
    async (uid: number) => {
      if (!isTauriRuntime()) {
        reportState({ kind: "error", message: OUTSIDE_DESKTOP_MESSAGE });
        return;
      }
      // Optimistic hide: remove from list immediately
      const previousMessages = messages;
      setMessages((prev) => prev.filter((m) => m.uid !== uid));
      try {
        const result = await invoke<{ acked: boolean; pending_count: number; detail: string }>(
          "delete_message",
          { uid, mailbox }
        );
        if (result.acked) {
          showUndoToast(true, 1, () => handleUndoDelete(uid));
        } else {
          // Queued — show undo with "will retry on next sync"
          showUndoToast(true, 1, () => handleUndoDelete(uid));
        }
      } catch (err) {
        // Invoke rejected: rollback optimistic hide
        setMessages(previousMessages);
        const msg = String(err);
        if (msg.startsWith("need_trash_confirm:")) {
          // Special case: no Trash folder, need user confirm to create
          setError(msg.replace("need_trash_confirm: ", ""));
        } else {
          setError(`Não foi possível apagar: ${msg}`);
        }
        reportState({ kind: "error", message: msg });
      }
    },
    [mailbox, messages, mailboxes, reportState]
  );

  /** Handle move: optimistic hide + invoke move_message. */
  const handleMove = useCallback(
    async (uid: number, destRaw: string) => {
      if (!isTauriRuntime()) {
        reportState({ kind: "error", message: OUTSIDE_DESKTOP_MESSAGE });
        return;
      }
      const previousMessages = messages;
      setMessages((prev) => prev.filter((m) => m.uid !== uid));
      try {
        const result = await invoke<{ acked: boolean; pending_count: number; detail: string }>(
          "move_message",
          { uid, dest: destRaw, mailbox }
        );
        if (result.acked) {
          showUndoToast(false, 1, () => handleUndoMove(uid, destRaw), destRaw);
        } else {
          showUndoToast(false, 1, () => handleUndoMove(uid, destRaw), destRaw);
        }
      } catch (err) {
        setMessages(previousMessages);
        const msg = String(err);
        setError(`Não foi possível mover: ${msg}`);
        // Show retry toast
        setUndoToast({
          visible: true,
          message: `Não foi possível mover: ${msg}`,
          onUndo: () => handleMove(uid, destRaw),
          isDelete: false,
          destFolder: destRaw,
        });
        reportState({ kind: "error", message: msg });
      }
    },
    [mailbox, messages, mailboxes, reportState]
  );

  /** Undo delete: invoke undo_queued_op + restore row. */
  const handleUndoDelete = useCallback(
    async (uid: number) => {
      try {
        const result = await invoke<{ restored: boolean; detail: string }>("undo_queued_op", {
          uid,
          mailbox,
        });
        if (result.restored) {
          // Restore the message row (it was hidden optimistically)
          // The row will reappear on next loadPage since pending_delete is cleared
          // For immediate feedback, we could re-fetch, but the toast dismisses
          // and the row is restored server-side. We'll just reload.
          void loadPage(safePage);
        }
        hideUndoToast();
      } catch (err) {
        const msg = String(err);
        setError(`Não foi possível desfazer: ${msg}`);
        hideUndoToast();
      }
    },
    [mailbox, loadPage, safePage]
  );

  /** Undo move: invoke undo_queued_op + restore row. */
  const handleUndoMove = useCallback(
    async (uid: number, _destRaw: string) => {
      try {
        const result = await invoke<{ restored: boolean; detail: string }>("undo_queued_op", {
          uid,
          mailbox,
        });
        if (result.restored) {
          void loadPage(safePage);
        }
        hideUndoToast();
      } catch (err) {
        const msg = String(err);
        setError(`Não foi possível desfazer: ${msg}`);
        hideUndoToast();
      }
    },
    [mailbox, loadPage, safePage]
  );

  /** Open move menu for a UID. */
  const openMoveMenu = useCallback(
    (uid: number, triggerRef: React.MutableRefObject<HTMLButtonElement | null>) => {
      setMoveMenu({ open: true, uid, triggerRef });
    },
    []
  );

  /** Close move menu. */
  const closeMoveMenu = useCallback(() => {
    setMoveMenu({ open: false, uid: null, triggerRef: { current: null } });
  }, []);

  /** Handle move menu selection. */
  const onMoveSelect = useCallback(
    (destRaw: string) => {
      if (moveMenu.uid !== null) {
        handleMove(moveMenu.uid, destRaw);
      }
      closeMoveMenu();
    },
    [moveMenu.uid, handleMove, closeMoveMenu]
  );

  /** Close expunge modal. */
  const closeExpungeModal = useCallback(() => {
    setExpungeModal({ open: false, uids: [], folderName: "", triggerRef: { current: null } });
  }, []);

  /** Confirm expunge. */
  const onExpungeConfirm = useCallback(
    async (uids: number[]) => {
      try {
        const result = await invoke<{ removed: number; acked: boolean; detail: string }>(
          "expunge_messages",
          { uids, mailbox }
        );
        if (result.acked) {
          // Rows already hidden, will be removed from local store on next sync
          void loadPage(safePage);
        }
        closeExpungeModal();
      } catch (err) {
        const msg = String(err);
        setError(`Não foi possível apagar para sempre: ${msg}`);
        closeExpungeModal();
      }
    },
    [mailbox, loadPage, safePage, closeExpungeModal]
  );

  /**
   * Optimistic read/unread toggle (FLAG-01/FLAG-02): flips the row to the
   * target state instantly, calls set_seen, and keeps the accent-soft wash
   * until the server acknowledges. Rolls back only if the invoke rejects.
   * Browsing is never blocked by the pending state.
   */
  const toggleFlag = useCallback(
    async (uid: number, targetSeen: boolean, previousFlags: string) => {
      if (!isTauriRuntime()) {
        reportState({ kind: "error", message: OUTSIDE_DESKTOP_MESSAGE });
        return;
      }
      setMessages((prev) =>
        prev.map((m) =>
          m.uid === uid ? { ...m, flags: setSeenInFlags(m.flags, targetSeen) } : m,
        ),
      );
      setPendingUids((prev) => new Set(prev).add(uid));
      try {
        const result = await invoke<SetSeenResult>("set_seen", { uid, seen: targetSeen, mailbox });
        dispatchFlagUpdate({
          uid: result.uid,
          seen: result.seen,
          acked: result.acked,
          pending_count: result.pending_count,
          detail: result.detail,
          origin: "list",
        });
        if (result.acked) {
          setPendingUids((prev) => {
            const next = new Set(prev);
            next.delete(uid);
            return next;
          });
        }
      } catch (err) {
        // Invoke rejected: the store write likely never happened — roll back
        // the optimistic flip and surface via the established error path.
        setMessages((prev) =>
          prev.map((m) => (m.uid === uid ? { ...m, flags: previousFlags } : m)),
        );
        setPendingUids((prev) => {
          const next = new Set(prev);
          next.delete(uid);
          return next;
        });
        const msg = String(err);
        dispatchFlagUpdate({
          uid,
          seen: targetSeen,
          acked: false,
          pending_count: -1, // error sentinel: listeners must not apply as row state
          detail: msg,
          origin: "list",
        });
        setError(msg);
        reportState({ kind: "error", message: msg });
      }
    },
    [mailbox, reportState],
  );

  // Keep rows in sync with toggles made from the reader pane (and re-apply
  // our own dispatches idempotently). -1 pending_count = error notice: no
  // row-state change is applied.
  useEffect(() => {
    function onFlagUpdate(e: Event) {
      const detail = (e as CustomEvent<FlagUpdateDetail>).detail;
      if (!detail || detail.pending_count === -1) return;
      setMessages((prev) =>
        prev.map((m) =>
          m.uid === detail.uid
            ? { ...m, flags: setSeenInFlags(m.flags, detail.seen) }
            : m,
        ),
      );
      if (detail.acked) {
        setPendingUids((prev) => {
          const next = new Set(prev);
          next.delete(detail.uid);
          return next;
        });
      } else {
        setPendingUids((prev) => new Set(prev).add(detail.uid));
      }
    }
    window.addEventListener(FLAG_UPDATE_EVENT, onFlagUpdate);
    return () => window.removeEventListener(FLAG_UPDATE_EVENT, onFlagUpdate);
  }, []);

  useEffect(() => {
    searchCache.current = null;
    setPage(0);
    void loadPage(0);
  }, [mailbox, searchQuery, refreshKey]);

  useEffect(() => {
    void loadPage(safePage);
  }, [safePage]);

  function goTo(p: number) {
    setPage(Math.max(0, Math.min(totalPages - 1, p)));
  }

  const rangeStart = total === 0 ? 0 : safePage * PAGE_SIZE + 1;
  const rangeEnd = Math.min(total, (safePage + 1) * PAGE_SIZE);

  if (!isTauriRuntime()) {
    return (
      <div className="message-list">
        <p role="alert">{OUTSIDE_DESKTOP_MESSAGE}</p>
      </div>
    );
  }

  if (error && messages.length === 0) {
    return (
      <div className="message-list">
        <div className="list-error">
          <p role="alert">Não foi possível carregar os avisos: {error}</p>
          <button type="button" className="btn" onClick={() => void loadPage(safePage)}>
            Tentar de novo
          </button>
        </div>
      </div>
    );
  }

  if (!loading && messages.length === 0 && !searchQuery) {
    return (
      <div className="message-list">
        <div className="list-empty">
          <span className="list-empty-ill" aria-hidden="true">
            <IconInbox size={30} />
          </span>
          <p>Sua fila de leitura está vazia</p>
          <p>
            Busque os avisos com o botão “Buscar avisos” acima. Depois da primeira busca, tudo fica
            guardado para estudar offline.
          </p>
        </div>
      </div>
    );
  }

  if (!loading && messages.length === 0 && searchQuery) {
    return (
      <div className="message-list">
        <div className="list-empty">
          <span className="list-empty-ill" aria-hidden="true">
            <IconInbox size={30} />
          </span>
          <p>Nada encontrado para “{searchQuery}”</p>
          <p>Tente outro remetente ou palavra do assunto.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="message-list-wrap">
      <div className="message-list" role="listbox" aria-label="Avisos da sala">
        {messages.map((msg) => {
          const unread = isUnread(msg.flags);
          const selected = selectedUid === msg.uid;
          const pending = pendingUids.has(msg.uid);
          // Callback ref to capture the move button DOM element
          const setMoveBtnRef = (el: HTMLButtonElement | null) => {
            if (el) actionBtnRefs.current.set(msg.uid, el);
            else actionBtnRefs.current.delete(msg.uid);
          };
          // Create a stable ref object for this message's move button
          const moveBtnRef = { current: actionBtnRefs.current.get(msg.uid) ?? null } as React.MutableRefObject<HTMLButtonElement | null>;
          return (
            <button
              type="button"
              key={msg.uid}
              role="option"
              aria-selected={selected}
              className={`message-row ${unread ? "unread" : "read"} ${selected ? "selected" : ""}`}
              style={pending ? { backgroundColor: "var(--color-accent-soft)" } : undefined}
              onClick={() => onMessageSelect(msg)}
            >
              <span
                className={`unread-dot ${unread ? "unread" : "read"}`}
                role="button"
                tabIndex={0}
                aria-label={unread ? "Não lido" : "Lido"}
                onClick={(e) => {
                  e.stopPropagation();
                  void toggleFlag(msg.uid, unread, msg.flags);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    e.stopPropagation();
                    void toggleFlag(msg.uid, unread, msg.flags);
                  }
                }}
              />
              <span className="message-sender-cell">
                <span className="message-avatar" aria-hidden="true">
                  {avatarInitial(msg.from_addr)}
                </span>
                <span className="message-sender" title={msg.from_addr}>
                  {msg.from_addr}
                </span>
              </span>
              <span className="message-subject-cell">
                <span style={{ minWidth: 0, flex: 1 }}>
                  <span className="message-subject">{msg.subject || "(sem assunto)"}</span>
                  {msg.preview && (
                    <span className="message-preview">{msg.preview.slice(0, 90)}</span>
                  )}
                </span>
                {msg.has_attachments && (
                  <span className="attachment-icon" title="Tem material em anexo">
                    <IconPaperclip size={15} />
                  </span>
                )}
              </span>
              <span className="message-date">{formatRowDate(msg.date_utc)}</span>
              <div className="message-actions" role="group" aria-label="Ações da mensagem">
                <button
                  type="button"
                  className="message-action-btn message-delete-btn"
                  aria-label="Apagar mensagem"
                  title="Apagar — vai para a Lixeira"
                  onClick={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    handleDelete(msg.uid);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      e.stopPropagation();
                      handleDelete(msg.uid);
                    }
                  }}
                >
                  <IconTrash size={18} />
                </button>
                <button
                  ref={setMoveBtnRef}
                  type="button"
                  className="message-action-btn message-move-btn"
                  aria-label="Mover mensagem para…"
                  title="Mover mensagem para…"
                  onClick={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    openMoveMenu(msg.uid, moveBtnRef);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      e.stopPropagation();
                      openMoveMenu(msg.uid, moveBtnRef);
                    }
                  }}
                >
                  <IconMove size={18} />
                </button>
              </div>
              {searchQuery && msg.mailbox !== mailbox && (
                <span
                  className="message-folder-chip"
                  title={`Pasta: ${folderLabel(msg.mailbox)}`}
                >
                  {folderLabel(msg.mailbox)}
                </span>
              )}
            </button>
          );
        })}
        {loading && (
          <div className="list-loading">
            <p role="status">Carregando avisos…</p>
          </div>
        )}
      </div>
      {totalPages > 1 && (
        <nav className="pager" aria-label="Paginação dos avisos">
          <button
            type="button"
            className="pager-btn"
            disabled={safePage === 0 || loading}
            onClick={() => goTo(safePage - 1)}
            aria-label="Página anterior"
          >
            ← Anterior
          </button>
          {pageNumbers(safePage, totalPages).map((n, i) =>
            n === "…" ? (
              <span key={`gap-${i}`} className="pager-gap" aria-hidden="true">
                …
              </span>
            ) : (
              <button
                key={n}
                type="button"
                className={`pager-btn pager-num ${n === safePage ? "active" : ""}`}
                disabled={loading}
                onClick={() => goTo(n)}
                aria-label={`Página ${n + 1}`}
                aria-current={n === safePage ? "page" : undefined}
              >
                {n + 1}
              </button>
            ),
          )}
          <button
            type="button"
            className="pager-btn"
            disabled={safePage >= totalPages - 1 || loading}
            onClick={() => goTo(safePage + 1)}
            aria-label="Próxima página"
          >
            Próxima →
          </button>
        </nav>
      )}
      <p className="pager-info" aria-live="polite">
        Mostrando {rangeStart}–{rangeEnd} de {total}
      </p>

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
    </div>
  );
}

function pageNumbers(current: number, totalPages: number): (number | "…")[] {
  if (totalPages <= MAX_PAGE_BUTTONS) {
    return Array.from({ length: totalPages }, (_, i) => i);
  }
  const pages = new Set<number>([0, totalPages - 1, current - 1, current, current + 1]);
  const sorted = [...pages].filter((p) => p >= 0 && p < totalPages).sort((a, b) => a - b);
  const out: (number | "…")[] = [];
  for (let i = 0; i < sorted.length; i++) {
    if (i > 0 && sorted[i] - sorted[i - 1] > 1) out.push("…");
    out.push(sorted[i]);
  }
  return out;
}
