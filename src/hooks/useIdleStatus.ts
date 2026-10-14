import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Phase 29 IDLE push status (IDLE-01 UI surface).
 *
 * Polls the backend `idle_status` command and maps the watcher snapshot
 * to an honest Push/Polling/Offline label. The backend is authoritative
 * (capability gate included) — this hook only renders what it reports.
 */

export interface IdleStatusInfo {
  status: string;
  detail: string;
  last_wakeup: string | null;
  reconnect_attempt: number;
}

export interface PushView {
  /** Push (green) | Polling (amber) | Offline (gray). */
  label: "Push" | "Polling" | "Offline";
  color: string;
  tooltip: string;
  reconnectAttempt: number;
}

/** Pure mapping (unit-testable without Tauri): backend status string →
 * sidebar view. Unknown strings degrade to Polling — never a phantom
 * Push, and never an alarming Offline for a transient unknown. */
export function mapIdleStatus(s: IdleStatusInfo): PushView {
  if (s.status === "Push") {
    return {
      label: "Push",
      color: "#22c55e",
      tooltip: s.detail || "aguardando e-mail em tempo real",
      reconnectAttempt: 0,
    };
  }
  if (s.status === "Offline") {
    return {
      label: "Offline",
      color: "#64748b",
      tooltip: s.detail || "observador parado",
      reconnectAttempt: 0,
    };
  }
  // "Polling" and "Unavailable" both render as Polling; reconnecting
  // carries the attempt count in the tooltip.
  const tooltip =
    s.reconnect_attempt > 0
      ? `Reconectando (tentativa ${s.reconnect_attempt})…`
      : s.detail || "verificação automática";
  return {
    label: "Polling",
    color: s.reconnect_attempt > 0 ? "#f59e0b" : "#60a5fa",
    tooltip,
    reconnectAttempt: s.reconnect_attempt,
  };
}

const POLL_MS = 5000;
const RECONNECT_POLL_MS = 1000;

/** Poll `idle_status`; start the watcher once on mount (best-effort —
 * refusal means poll-only mode) and stop it on unmount. Forwards the
 * backend `idle-wakeup` event as a window `sge-idle-wakeup` CustomEvent
 * so the sync entry (SyncStatus) can run one gate-enforced pass. */
export function useIdleStatus() {
  const [view, setView] = useState<PushView>({
    label: "Polling",
    color: "#60a5fa",
    tooltip: "verificação automática",
    reconnectAttempt: 0,
  });
  const viewRef = useRef(view);
  viewRef.current = view;

  const fetch = useCallback(async () => {
    try {
      const s = (await invoke("idle_status")) as IdleStatusInfo;
      setView(mapIdleStatus(s));
    } catch {
      /* backend hiccup — keep the last honest state */
    }
  }, []);

  useEffect(() => {
    let alive = true;
    // Best-effort start: refusal (no credentials yet, or IDLE-gated
    // server) is normal — the UI stays in Polling.
    void invoke("idle_start")
      .catch(() => undefined)
      .then(() => {
        if (alive) void fetch();
      });
    void fetch();
    const timer = window.setInterval(() => {
      if (!alive) return;
      // Backoff awareness: poll faster while reconnecting.
      void fetch();
    }, POLL_MS);
    const fast = window.setInterval(() => {
      if (alive && viewRef.current.reconnectAttempt > 0) void fetch();
    }, RECONNECT_POLL_MS);
    const unlisten = listen("idle-wakeup", () => {
      window.dispatchEvent(new CustomEvent("sge-idle-wakeup"));
      if (alive) void fetch();
    });
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.clearInterval(fast);
      void unlisten.then((f) => f());
      // Fire-and-forget stop; abort on unmount, handle account switch
      // by the next mount's idle_start ("already running" short-circuits
      // only within one backend session slot).
      void invoke("idle_stop").catch(() => undefined);
    };
  }, [fetch]);

  return view;
}
