import React from "react";
import { invoke } from "@tauri-apps/api/core";

/** Phase 17 minimal status surface (full badges land in Phase 18).
 *  Shows the classifier state beside SyncStatus: pending depth while the
 *  behind-sync queue drains, a warming note while the sidecar boots, and
 *  nothing when idle+empty (keeps the login screen quiet). */

interface ClassifyStatusResult {
  sidecar: string;
  pending: number;
  excluded: string[];
}

const STATE_COPY: Record<string, string> = {
  running: "classificando",
  starting: "aquecendo o classificador…",
  down: "classificador indisponível",
  stopped: "classificador parado",
};

export function ClassifyStatusWired() {
  const [status, setStatus] = React.useState<ClassifyStatusResult | null>(null);

  React.useEffect(() => {
    let alive = true;
    const fetch = async () => {
      try {
        const result = (await invoke("classify_status")) as ClassifyStatusResult;
        if (alive) setStatus(result);
      } catch (e) {
        console.error("[SGE] classify_status invoke failed:", e);
      }
    };
    fetch();
    const timer = setInterval(fetch, 15000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  if (!status) return null;
  const pending = status.pending ?? 0;
  // Idle + empty + running = nothing to say (quiet by default).
  if (status.sidecar === "running" && pending === 0) return null;
  const copy =
    pending > 0
      ? `${pending} mensagem${pending === 1 ? "" : "ns"} na fila de classificação`
      : (STATE_COPY[status.sidecar] ?? status.sidecar);

  return (
    <div
      className="classify-status"
      aria-label="estado da classificação"
      role="status"
      style={{
        display: "inline-flex",
        alignItems: "center",
        marginLeft: 8,
        color: pending > 0 ? "#38bdf8" : "#94a3b8",
        fontSize: "0.75rem",
        fontWeight: 500,
      }}
    >
      {copy}
    </div>
  );
}
