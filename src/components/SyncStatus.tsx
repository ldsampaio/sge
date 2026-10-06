import { useEffect, useState, useRef } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { IconSync } from "./icons";
import type { MailboxRow } from "../types";

interface SyncStatusInfo {
  mailbox: string;
  last_sync_at: string;
  uid_validity: number;
  uid_next: number;
  message_count: number;
  /** Durable-outbox depth: read/unread toggles awaiting server acknowledgement. */
  pending_count: number;
}

type Status =
  | { kind: "idle" }
  | { kind: "syncing"; progress: string }
  | { kind: "synced"; status: SyncStatusInfo }
  | { kind: "reconnecting"; message: string }
  | { kind: "error"; message: string };

/** Poll cadence (Phase 8): the timer and the manual button share one
 *  `requestSync` entry, so refreshes never run two code paths. */
const POLL_INTERVAL_MS = 5 * 60 * 1000;
/** One automatic retry after a connection failure before surfacing an error. */
const RETRY_DELAY_MS = 5 * 1000;

/** Session-expiry shaped failures (vs. fatal sync bugs): the session may be
 *  stale, so the UI shows "reconnecting…" and retries once — sync resumes
 *  after keyring re-read + re-SELECT on the backend. */
function isConnectionError(message: string): boolean {
  return /imap connection|connection (refused|reset|failed)|timed? ?out|network|dns|econn|reconnect/i.test(
    message,
  );
}

interface BatchStartedEvent {
  BatchStarted: { range: string; server_total: number };
}
interface MessageSyncedEvent {
  MessageSynced: { uid: number; flag: string };
}
interface BatchCompletedEvent {
  BatchCompleted: { new: number; updated: number; deleted: number };
}
interface SyncCompletedEvent {
  SyncCompleted: {
    summary: {
      new: number;
      updated: number;
      unchanged: number;
      deleted: number;
      uid_validity_bump: boolean;
    };
  };
}
interface SyncErrorEvent {
  SyncError: { detail: string };
}
type SyncEvent =
  | BatchStartedEvent
  | MessageSyncedEvent
  | BatchCompletedEvent
  | SyncCompletedEvent
  | SyncErrorEvent;

function hasBatchStarted(e: SyncEvent): e is BatchStartedEvent {
  return "BatchStarted" in e;
}
function hasMessageSynced(e: SyncEvent): e is MessageSyncedEvent {
  return "MessageSynced" in e;
}
function hasBatchCompleted(e: SyncEvent): e is BatchCompletedEvent {
  return "BatchCompleted" in e;
}
function hasSyncCompleted(e: SyncEvent): e is SyncCompletedEvent {
  return "SyncCompleted" in e;
}
function hasSyncError(e: SyncEvent): e is SyncErrorEvent {
  return "SyncError" in e;
}

interface SyncStatusProps {
  onSyncComplete?: () => void;
  mailbox?: string;
  /** All known folders — a refresh syncs every one so search covers the account. */
  mailboxes?: MailboxRow[];
}

export default function SyncStatus({ onSyncComplete, mailbox = "INBOX", mailboxes = [] }: SyncStatusProps) {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const channelRef = useRef<Channel<SyncEvent> | null>(null);
  const syncingRef = useRef(false);
  const retryTimerRef = useRef<number | null>(null);
  /** Live mirror — the poll interval closure always sees current folders. */
  const mailboxesRef = useRef<MailboxRow[]>(mailboxes);
  mailboxesRef.current = mailboxes;
  const mailboxRef = useRef(mailbox);
  mailboxRef.current = mailbox;
  /** Last known outbox depth, so a replay failure can report N. */
  const [lastPending, setLastPending] = useState(0);

  async function pollStatus() {
    try {
      const s: SyncStatusInfo = await invoke("sync_status", { mailbox: mailboxRef.current });
      setLastPending(s.pending_count);
      setStatus({ kind: "synced", status: s });
    } catch {
      setStatus({ kind: "idle" });
    }
  }

  async function startSync() {
    await requestSync("manual");
  }

  /**
   * The ONE sync entry (Phase 8): poll timer, manual button, and the
   * reconnect retry all funnel here. Every refresh syncs ALL known folders
   * sequentially through the same `start_sync` path (backend single-flight
   * guard serializes), so account-wide search always has fresh local data.
   */
  async function requestSync(source: "manual" | "poll" | "retry") {
    if (syncingRef.current) return;
    syncingRef.current = true;

    const known = mailboxesRef.current;
    const targets =
      known.length > 0 ? known.map((mb) => mb.name) : [mailboxRef.current];
    const labelOf = (raw: string) =>
      known.find((mb) => mb.name === raw)?.display_name ?? raw;
    setStatus({
      kind: "syncing",
      progress:
        source === "poll"
          ? `Verificação automática… (${targets.length} pastas)`
          : targets.length > 1
            ? `Preparando a sala… (1/${targets.length})`
            : "Preparando a sala…",
    });

    const channel = new Channel<SyncEvent>();
    channelRef.current = channel;

    channel.onmessage = (event: SyncEvent) => {
      if (hasBatchStarted(event)) {
        setStatus({ kind: "syncing", progress: `Buscando ${event.BatchStarted.range}…` });
      } else if (hasMessageSynced(event)) {
        setStatus({ kind: "syncing", progress: `Aviso ${event.MessageSynced.uid}…` });
      } else if (hasBatchCompleted(event)) {
        const { new: n, updated, deleted } = event.BatchCompleted;
        const parts = [`+${n} novos`];
        if (updated) parts.push(`${updated} atualizados`);
        if (deleted) parts.push(`${deleted} removidos`);
        setStatus({ kind: "syncing", progress: parts.join(" · ") });
      } else if (hasSyncCompleted(event)) {
        const { new: n, updated, unchanged, deleted } = event.SyncCompleted.summary;
        const parts = [`${n} novos`];
        if (updated) parts.push(`${updated} atualizados`);
        if (unchanged) parts.push(`${unchanged} já lidos`);
        if (deleted) parts.push(`${deleted} removidos`);
        setStatus({ kind: "syncing", progress: `Pronto: ${parts.join(" · ")}` });
        void pollStatus().then(() => onSyncComplete?.());
      } else if (hasSyncError(event)) {
        setStatus({ kind: "error", message: event.SyncError.detail });
      }
    };

    try {
      for (let i = 0; i < targets.length; i++) {
        const target = targets[i];
        if (targets.length > 1) {
          setStatus({
            kind: "syncing",
            progress: `Buscando ${labelOf(target)}… (${i + 1}/${targets.length})`,
          });
        }
        await invoke("start_sync", { onEvent: channel, mailbox: target });
      }
      await pollStatus();
      onSyncComplete?.();
    } catch (err) {
      const message = String(err);
      if (source !== "retry" && isConnectionError(message)) {
        // Honest "reconnecting…" (not a fatal disconnect): the session may
        // have expired — retry once; the backend re-reads the keyring and
        // re-SELECTs before the worker proceeds.
        setStatus({ kind: "reconnecting", message });
        syncingRef.current = false;
        channelRef.current = null;
        retryTimerRef.current = window.setTimeout(() => {
          void requestSync("retry");
        }, RETRY_DELAY_MS);
        return;
      }
      setStatus({ kind: "error", message });
    } finally {
      syncingRef.current = false;
      channelRef.current = null;
    }
  }

  async function cancelSync() {
    try {
      await invoke("cancel_sync");
    } catch {
      // Best-effort cancel
    }
    setStatus({ kind: "idle" });
  }

  useEffect(() => {
    void pollStatus();
    // Poll timer (Phase 8): fires through the same `requestSync` entry as
    // the manual button; the backend single-flight guard skips the tick
    // when a pass is already running, so polls never overlap.
    const timer = window.setInterval(() => {
      if (!syncingRef.current) {
        void requestSync("poll");
      }
    }, POLL_INTERVAL_MS);
    return () => {
      window.clearInterval(timer);
      if (retryTimerRef.current !== null) {
        window.clearTimeout(retryTimerRef.current);
      }
    };
  }, []);

  return (
    <div className="sync-status" aria-label="Sincronização da sala">
      <div className="sync-status-main">
        <h2>Sincronização da caixa</h2>
        {status.kind === "idle" && <p>Ainda não sincronizado — entre e busque.</p>}

        {status.kind === "synced" && (
          <>
            <p>
              {status.status.last_sync_at
                ? `Em dia · última busca ${status.status.last_sync_at}`
                : "Offline · nunca sincronizado"}
            </p>
            {status.status.pending_count > 0 && !status.status.last_sync_at && (
              <p className="sub">
                Offline · alterações guardadas — serão enviadas ao reconectar
              </p>
            )}
            <p className="sub">
              {status.status.message_count}{" "}
              {status.status.message_count === 1 ? "aviso guardado" : "avisos guardados"} para
              estudar offline
            </p>
            {status.status.pending_count > 0 && (
              <p className="sub" role="status">
                {status.status.pending_count === 1
                  ? "1 alteração aguardando envio"
                  : `${status.status.pending_count} alterações aguardando envio`}
              </p>
            )}
          </>
        )}

        {status.kind === "syncing" && (
          <>
            <p role="status">{status.progress}</p>
            <div className="sync-progress" aria-hidden="true">
              <span />
            </div>
          </>
        )}

        {status.kind === "reconnecting" && (
          <>
            <p role="status">Reconectando… tentando buscar de novo.</p>
            <div className="sync-progress" aria-hidden="true">
              <span />
            </div>
            <p className="sub">A sessão pode ter expirado — nada foi apagado.</p>
          </>
        )}

        {status.kind === "error" && (
          <>
            <p role="alert">
              <span
                title={`Falha na busca: ${status.message}`}
                style={{
                  display: "block",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                Falha na busca: {status.message}
              </span>
            </p>
            {lastPending > 0 && (
              <p className="sub">
                {lastPending === 1
                  ? "Não foi possível enviar 1 alteração de leitura — tentando de novo na próxima busca."
                  : `Não foi possível enviar ${lastPending} alterações de leitura — tentando de novo na próxima busca.`}
              </p>
            )}
          </>
        )}
      </div>

      {status.kind === "syncing" ? (
        <button type="button" onClick={() => void cancelSync()}>
          Pausar
        </button>
      ) : (
        <button type="button" onClick={() => void startSync()}>
          <IconSync size={15} />
          Buscar mensagens
        </button>
      )}
      {status.kind === "error" && (
        <button type="button" onClick={() => void startSync()}>
          Tentar de novo
        </button>
      )}
    </div>
  );
}
