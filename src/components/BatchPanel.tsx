import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Phase 20 batch UI: explicit whole-account run with live progress,
 *  persisted report, and undo. Unconfirmed BY DESIGN (one account-level
 *  decision) — distinct from single-email always-confirmed. */

interface BatchProgressEvent {
  run_id: number;
  done: number;
  total: number;
  moved: number;
  review: number;
  state: string;
}

interface BatchReport {
  run_id: number;
  state: string;
  per_category: Array<[string, number]>;
  remainder: number;
  undone: number;
}

interface BatchPanelProps {
  onClose: () => void;
}

interface BatchRunRow {
  id: number;
  state: string;
  started_at: string;
}

export function BatchPanel({ onClose }: BatchPanelProps) {
  const [runId, setRunId] = React.useState<number | null>(null);
  const [progress, setProgress] = React.useState<BatchProgressEvent | null>(null);
  const [report, setReport] = React.useState<BatchReport | null>(null);
  const [message, setMessage] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [pastRuns, setPastRuns] = React.useState<BatchRunRow[]>([]);

  const loadRuns = React.useCallback(async () => {
    try {
      const rows = (await invoke("batch_runs_list")) as BatchRunRow[];
      setPastRuns(rows);
    } catch {
      /* panel shows start-only */
    }
  }, []);

  React.useEffect(() => {
    void loadRuns();
  }, [loadRuns]);

  React.useEffect(() => {
    const unlisten = listen<BatchProgressEvent>("batch-progress", (e) => {
      setProgress(e.payload);
      if (e.payload.state === "done" || e.payload.state === "interrupted") {
        setBusy(false);
        void loadReport(e.payload.run_id);
      }
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  const loadReport = async (id: number) => {
    try {
      const r = (await invoke("batch_report", { runId: id })) as BatchReport;
      setReport(r);
    } catch (e) {
      setMessage(`Falha no relatório: ${e}`);
    }
  };

  const start = async (resume?: number) => {
    if (resume === undefined) {
      if (!window.confirm("Reorganizar toda a conta sem confirmação por e-mail? Dá para desfazer depois.")) return;
    }
    setBusy(true);
    setMessage(null);
    setReport(null);
    setProgress(null);
    try {
      const id = (await invoke("batch_classify", { scope: null, resumeRun: resume ?? null })) as number;
      setRunId(id);
      await loadRuns();
    } catch (e) {
      setMessage(`Falha ao iniciar: ${e}`);
      setBusy(false);
    }
  };

  const cancel = async () => {
    try {
      await invoke("cancel_batch");
    } catch (e) {
      setMessage(`Falha ao cancelar: ${e}`);
    }
  };

  const undo = async () => {
    if (runId === null) return;
    if (!window.confirm("Desfazer a reorganização e restaurar os originais?")) return;
    try {
      const n = (await invoke("undo_batch", { runId })) as number;
      setMessage(`${n} e-mails restaurados.`);
      await loadReport(runId);
    } catch (e) {
      setMessage(`Falha ao desfazer: ${e}`);
    }
  };

  const pct = progress && progress.total > 0 ? Math.round((progress.done / progress.total) * 100) : 0;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Reorganizar conta"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.5)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 60,
      }}
      onClick={onClose}
    >
      <div
        style={{
          background: "var(--color-bg, #0f172a)",
          borderRadius: 12,
          padding: 20,
          minWidth: 440,
          maxWidth: 600,
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ marginTop: 0 }}>Reorganizar conta</h2>
        <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
          Classifica tudo e arquiva em <code>Auto/</code> sem pedir por e-mail. Dá para desfazer.
        </p>
        {message && (
          <div role="status" style={{ color: "#eab308", marginBottom: 8 }}>
            {message}
          </div>
        )}
        {!progress && !busy && (
          <>
            <button type="button" className="btn" onClick={() => void start()}>
              Começar agora
            </button>
            {pastRuns.filter((r) => r.state === "interrupted").map((r) => (
              <button
                key={r.id}
                type="button"
                className="btn btn-small"
                style={{ marginLeft: 8 }}
                onClick={() => void start(r.id)}
              >
                Retomar #{r.id}
              </button>
            ))}
          </>
        )}
        {(busy || progress) && (
          <div style={{ margin: "12px 0" }}>
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.85rem" }}>
              <span>
                {progress?.done ?? 0} de {progress?.total ?? "?"} · {progress?.moved ?? 0} movidos ·{" "}
                {progress?.review ?? 0} para revisão
              </span>
              <span>{pct}%</span>
            </div>
            <div style={{ height: 8, borderRadius: 4, background: "#1e293b", marginTop: 6 }}>
              <div style={{ width: `${pct}%`, height: "100%", borderRadius: 4, background: "#38bdf8" }} />
            </div>
            {busy && (
              <button type="button" className="btn btn-small" style={{ marginTop: 8 }} onClick={() => void cancel()}>
                Cancelar
              </button>
            )}
          </div>
        )}
        {report && (
          <div style={{ marginTop: 12 }}>
            <h3>Relatório</h3>
            <ul style={{ listStyle: "none", padding: 0 }}>
              {report.per_category.map(([cat, n]) => (
                <li key={cat}>
                  {cat}: <strong>{n}</strong>
                </li>
              ))}
            </ul>
            <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
              Restam {report.remainder} para revisão · estado {report.state}
              {report.undone > 0 && ` · ${report.undone} desfeitos`}
            </p>
            {report.state === "done" && (
              <button type="button" className="btn btn-small" onClick={() => void undo()}>
                Desfazer reorganização
              </button>
            )}
          </div>
        )}
        <div style={{ marginTop: 12, textAlign: "right" }}>
          <button type="button" className="btn" onClick={onClose}>
            Fechar
          </button>
        </div>
      </div>
    </div>
  );
}
