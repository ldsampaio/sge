import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { kindFromReport } from "./ArchivePanel";
import { BatchPreviewTable, type BatchPreview } from "./BatchPreviewTable";

/** Phase 20 batch UI: explicit whole-account run with live progress,
 *  persisted report, and undo. Unconfirmed BY DESIGN (one account-level
 *  decision) — distinct from single-email always-confirmed.
 *  Phase 24 extension: archive runs (kind tag in totals_json) show an
 *  archive label + threshold echo and resume via archive_run — the shared
 *  progress/report/undo surface serves both kinds. */

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
  /** Raw totals_json (Phase 24: carries {"kind":"archive",...} for
   *  archive runs — parsed client-side for LABELING ONLY, never trusted
   *  for safety decisions). Absent on old cached payloads. */
  totals_json?: string;
  /** Phase 30: stale-drop + retry-skip counts + failed-intent count
   *  (backend truth — the retry button gates on `failed`). Absent on
   *  old cached payloads. */
  stale_dropped?: number;
  retry_skipped?: number;
  failed?: number;
}

interface RetryReport {
  run_id: number;
  state: string;
  retried: number;
  moved: number;
  skipped: number;
  failed_remaining: number;
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
  /** Run-id → totals kind ("archive" vs classify), resolved from
   *  batch_report payloads (backend truth). Interrupted archive runs
   *  resume via archive_run, classify runs via batch_classify. */
  const [runKinds, setRunKinds] = React.useState<Record<number, string>>({});
  const [runThresholds, setRunThresholds] = React.useState<Record<number, number | null>>({});
  /** Phase 30: transient dry-run plan (never persisted backend-side). */
  const [preview, setPreview] = React.useState<BatchPreview | null>(null);
  const [previewBusy, setPreviewBusy] = React.useState(false);

  const resolveKind = React.useCallback(async (id: number) => {
    try {
      const r = (await invoke("batch_report", { runId: id })) as BatchReport;
      const parsed = kindFromReport(r.totals_json, r.per_category);
      setRunKinds((prev) => (prev[id] === parsed.kind ? prev : { ...prev, [id]: parsed.kind }));
      setRunThresholds((prev) => ({ ...prev, [id]: parsed.thresholdDays }));
      return r;
    } catch {
      return null;
    }
  }, []);

  const loadRuns = React.useCallback(async () => {
    try {
      const rows = (await invoke("batch_runs_list")) as BatchRunRow[];
      setPastRuns(rows);
      // Label interrupted runs by kind so resume picks the right command.
      for (const r of rows.filter((x) => x.state === "interrupted")) {
        void resolveKind(r.id);
      }
    } catch {
      /* panel shows start-only */
    }
  }, [resolveKind]);

  React.useEffect(() => {
    void loadRuns();
  }, [loadRuns]);

  React.useEffect(() => {
    const unlisten = listen<BatchProgressEvent>("batch-progress", (e) => {
      setProgress(e.payload);
      // Resolve the run kind once so archive progress shows its label +
      // threshold echo while running (report arrives at the end).
      if (runKinds[e.payload.run_id] === undefined) {
        void resolveKind(e.payload.run_id);
      }
      if (e.payload.state === "done" || e.payload.state === "interrupted") {
        setBusy(false);
        void loadReport(e.payload.run_id);
      }
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [resolveKind, runKinds]);

  const loadReport = async (id: number) => {
    try {
      const r = (await invoke("batch_report", { runId: id })) as BatchReport;
      setReport(r);
      const parsed = kindFromReport(r.totals_json, r.per_category);
      setRunKinds((prev) => ({ ...prev, [id]: parsed.kind }));
      setRunThresholds((prev) => ({ ...prev, [id]: parsed.thresholdDays }));
    } catch (e) {
      setMessage(`Falha no relatório: ${e}`);
    }
  };

  const kindOf = (id: number | null): string => (id === null ? "classify" : runKinds[id] ?? "classify");

  /** Phase 30: dry-run preview (transient — confirm re-collects fresh,
   *  never replays these rows). */
  const showPreview = async () => {
    setPreviewBusy(true);
    setMessage(null);
    try {
      const p = (await invoke("preview_batch", { scope: null })) as BatchPreview;
      setPreview(p);
    } catch (e) {
      setMessage(`Falha na pré-visualização: ${e}`);
    } finally {
      setPreviewBusy(false);
    }
  };

  /** Phase 30: retry only the failed items of the displayed run. */
  const retry = async () => {
    if (report === null) return;
    setBusy(true);
    setMessage(null);
    try {
      const r = (await invoke("retry_failed", { runId: report.run_id })) as RetryReport;
      if (r.state === "already-running") {
        setMessage("Uma repetição já está em andamento — aguarde.");
      } else {
        setMessage(`${r.moved} recuperados · ${r.skipped} ignorados · restam ${r.failed_remaining}.`);
      }
      await loadReport(report.run_id);
      await loadRuns();
    } catch (e) {
      setMessage(`Falha ao repetir: ${e}`);
    } finally {
      setBusy(false);
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
    setPreview(null);
    try {
      if (resume !== undefined && kindOf(resume) === "archive") {
        // Interrupted ARCHIVE run: resume on the archive path (the
        // classifier path would misfile archive candidates).
        const id = (await invoke("archive_run", { resumeRun: resume })) as number;
        setRunId(id);
      } else {
        const id = (await invoke("batch_classify", { scope: null, resumeRun: resume ?? null })) as number;
        setRunId(id);
      }
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
    const archiveUndo = kindOf(report?.run_id ?? runId) === "archive";
    if (!window.confirm(archiveUndo ? "Desfazer o arquivamento e restaurar os originais?" : "Desfazer a reorganização e restaurar os originais?")) return;
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
            <button type="button" className="btn" disabled={previewBusy} onClick={() => void showPreview()}>
              Pré-visualizar
            </button>
            <button type="button" className="btn" style={{ marginLeft: 8 }} onClick={() => void start()}>
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
                Retomar #{r.id}{kindOf(r.id) === "archive" ? " (arquivo)" : ""}
              </button>
            ))}
          </>
        )}
        {(busy || progress) && (
          <div style={{ margin: "12px 0" }}>
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.85rem" }}>
              <span>
                {(progress?.run_id ?? runId) !== null && kindOf(progress?.run_id ?? runId) === "archive" ? (
                  <>
                    Arquivo · mais de {runThresholds[progress?.run_id ?? runId ?? -1] ?? "?"} dias ·{" "}
                  </>
                ) : null}
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
            <h3>Relatório{kindOf(report.run_id) === "archive" ? " de arquivo" : ""}</h3>
            {kindOf(report.run_id) === "archive" && runThresholds[report.run_id] != null && (
              <p style={{ color: "#94a3b8", fontSize: "0.8rem" }}>
                Arquivo · mais de {runThresholds[report.run_id]} dias
              </p>
            )}
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
              {(report.stale_dropped ?? 0) > 0 && ` · ${report.stale_dropped} ignorados (mudaram)`}
              {(report.retry_skipped ?? 0) > 0 && ` · ${report.retry_skipped} ignorados na repetição`}
            </p>
            {(report.state === "done" || report.state === "interrupted") &&
              (report.failed ?? 0) > 0 && (
                <button
                  type="button"
                  className="btn btn-small"
                  disabled={busy}
                  style={{ marginRight: 8 }}
                  onClick={() => void retry()}
                >
                  Repetir {report.failed} falhas
                </button>
              )}
            {report.state === "done" && (
              <button type="button" className="btn btn-small" onClick={() => void undo()}>
                {kindOf(report.run_id) === "archive" ? "Desfazer arquivamento" : "Desfazer reorganização"}
              </button>
            )}
          </div>
        )}
        {preview && !progress && !busy && (
          <BatchPreviewTable
            preview={preview}
            confirming={busy}
            confirmLabel="Executar de verdade"
            onConfirm={() => void start()}
          />
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
