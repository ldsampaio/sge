import { useEffect, useState, useRef } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";

interface SyncStatus {
  mailbox: string;
  last_sync_at: string;
  uid_validity: number;
  uid_next: number;
  message_count: number;
}

type Status =
  | { kind: "idle" }
  | { kind: "syncing"; progress: string }
  | { kind: "synced"; status: SyncStatus }
  | { kind: "error"; message: string };

// Shape of events streamed from the backend SyncEvent enum.
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
  SyncCompleted: { summary: { new: number; updated: number; unchanged: number; deleted: number; uid_validity_bump: boolean } };
}
interface SyncErrorEvent {
  SyncError: { detail: string };
}
type SyncEvent = BatchStartedEvent | MessageSyncedEvent | BatchCompletedEvent | SyncCompletedEvent | SyncErrorEvent;

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

export default function SyncStatus() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const channelRef = useRef<Channel<SyncEvent> | null>(null);

  // Poll sync_status on mount and after each sync completes
  async function pollStatus() {
    try {
      const s: SyncStatus = await invoke("sync_status");
      setStatus({ kind: "synced", status: s });
    } catch {
      setStatus({ kind: "idle" });
    }
  }

  useEffect(() => {
    void pollStatus();
  }, []);

  async function startSync() {
    setStatus({ kind: "syncing", progress: "Starting sync…" });

    // Create a Channel for real-time progress events from the backend.
    const channel = new Channel<SyncEvent>();
    channelRef.current = channel;

    channel.onmessage = (event: SyncEvent) => {
      if (hasBatchStarted(event)) {
        setStatus({ kind: "syncing", progress: `Fetching ${event.BatchStarted.range}…` });
      } else if (hasMessageSynced(event)) {
        setStatus({ kind: "syncing", progress: `Message ${event.MessageSynced.uid}…` });
      } else if (hasBatchCompleted(event)) {
        const { new: n, updated, deleted } = event.BatchCompleted;
        const parts = [`+${n} new`];
        if (updated) parts.push(`${updated} updated`);
        if (deleted) parts.push(`${deleted} deleted`);
        setStatus({ kind: "syncing", progress: parts.join(", ") });
      } else if (hasSyncCompleted(event)) {
        const { new: n, updated, unchanged, deleted } = event.SyncCompleted.summary;
        const parts = [`${n} new`];
        if (updated) parts.push(`${updated} updated`);
        if (unchanged) parts.push(`${unchanged} unchanged`);
        if (deleted) parts.push(`${deleted} deleted`);
        setStatus({ kind: "syncing", progress: `Done: ${parts.join(", ")}` });
        void pollStatus();
      } else if (hasSyncError(event)) {
        setStatus({ kind: "error", message: event.SyncError.detail });
      }
    };

    try {
      await invoke("start_sync", { onEvent: channel });
      // Final status poll after sync completes
      await pollStatus();
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
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

  return (
    <div>
      <h2>Sync Status</h2>
      {status.kind === "idle" && <p>Not connected — log in first.</p>}

      {status.kind === "synced" && (
        <div>
          <p>
            {status.status.last_sync_at
              ? `Up to date — last synced ${status.status.last_sync_at}`
              : "Offline — never synced"}
          </p>
          <p>
            INBOX: {status.status.message_count} messages, UID next {status.status.uid_next}
          </p>
        </div>
      )}

      {status.kind === "syncing" && (
        <div>
          <p>{status.progress}</p>
          <button type="button" onClick={() => void cancelSync()}>
            Cancel
          </button>
        </div>
      )}

      {status.kind === "error" && (
        <div>
          <p role="alert">Sync error: {status.message}</p>
          <button type="button" onClick={() => void startSync()}>
            Retry
          </button>
        </div>
      )}

      <button
        type="button"
        onClick={() => void startSync()}
        disabled={status.kind === "syncing"}
      >
        {status.kind === "syncing" ? "Syncing…" : "Sync Now"}
      </button>
    </div>
  );
}