import { useEffect, useState, useRef } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { IconSync } from "./icons";

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
  | { kind: "error"; message: string };

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
}

export default function SyncStatus({ onSyncComplete, mailbox = "INBOX" }: SyncStatusProps) {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const channelRef = useRef<Channel<SyncEvent> | null>(null);
  const syncingRef = useRef(false);
  /** Last known outbox depth, so a replay failure can report N. */
  const [lastPending, setLastPending] = useState(0);

  async function pollStatus() {
    try {
      const s: SyncStatusInfo = await invoke("sync_status", { mailbox });
      setLastPending(s.pending_count);
      setStatus({ kind: "synced", status: s });
    } catch {
      setStatus({ kind: "idle" });
    }
  }

  async function startSync() {
    if (syncingRef.current) return;
    syncingRef.current = true;
    setStatus({ kind: "syncing", progress: "Preparando a sala…" });

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
      await invoke("start_sync", { onEvent: channel, mailbox });
      await pollStatus();
      onSyncComplete?.();
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
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
