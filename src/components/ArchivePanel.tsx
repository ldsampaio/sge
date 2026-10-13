import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MailboxRow } from "../types";

/** Phase 24 archive UI: rule editor (threshold + folder scope) with manual
 *  "Arquivar agora" trigger, live progress, persisted report, and undo.
 *  MANUAL ONLY — no repeating timer, no mount-effect auto-run, no
 *  background polling invokes archive_run anywhere in this file
 *  (scheduler guard: this file must contain no timer/scheduler calls —
 *  the SUMMARY records the exact grep). The backend is the
 *  enforcer (7–730 days, non-empty scope); this UI only echoes reasons.
 *  Settings persist in SQLite via get/set_archive_settings — never
 *  localStorage — so the rule survives restart. */

interface ArchiveSettings {
  threshold_days: number;
  included_folders: string[];
}

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
  totals_json: string;
}

interface ArchivePanelProps {
  onClose: () => void;
}

/** Backend truth for run labeling: parses the totals_json kind tag
 *  (`{"kind":"archive","threshold_days":N}`) written by archive_run.
 *  Never a client-side flag — the backend record decides. */
export function parseArchiveKind(totalsJson: string): { kind: string; thresholdDays: number | null } {
  try {
    const v = JSON.parse(totalsJson) as { kind?: unknown; threshold_days?: unknown };
    return {
      kind: typeof v.kind === "string" ? v.kind : "classify",
      thresholdDays: typeof v.threshold_days === "number" ? v.threshold_days : null,
    };
  } catch {
    return { kind: "classify", thresholdDays: null };
  }
}

/** Run kind from a report payload: backend totals_json kind first
 *  (authoritative); per-category `archive` label id as fallback for runs
 *  whose totals were overwritten by the shared undo (`{}` — verbatim
 *  Phase-20 behavior, see 24-01). Labeling only, never safety. */
export function kindFromReport(
  totalsJson: string | undefined,
  perCategory: Array<[string, number]> | undefined,
): { kind: string; thresholdDays: number | null } {
  const parsed = parseArchiveKind(totalsJson ?? "");
  if (parsed.kind === "archive") return parsed;
  if ((perCategory ?? []).some(([cat]) => cat === "archive")) {
    return { kind: "archive", thresholdDays: parsed.thresholdDays };
  }
  return parsed;
}

/** Structural always-excluded folders (mirrors the backend selector —
 *  UI restraint never the only guard; the backend refuses these even if
 *  forged via IPC). Returns the one-line trust rationale or null. */
export function archiveExclusionReason(mb: MailboxRow): string | null {
  const names = [mb.display_name, mb.name];
  if (names.includes("A Classificar")) {
    return "Fica sempre visível para revisão — arquivar a esconderia.";
  }
  if (names.includes("Sent") || mb.role === "sent") {
    return "Enviados nunca entram no arquivo automático.";
  }
  if (names.includes("Drafts") || mb.role === "drafts") {
    return "Rascunhos nunca entram no arquivo automático.";
  }
  if (
    names.includes("Auto") ||
    names.includes("Auto/Archive") ||
    names.includes("Auto.Archive") ||
    names.some((n) => n.startsWith("Auto/Archive/") || n.startsWith("Auto.Archive."))
  ) {
    return "O próprio arquivo nunca é arquivado de novo.";
  }
  return null;
}

export function ArchivePanel({ onClose }: ArchivePanelProps) {
  const [settings, setSettings] = React.useState<ArchiveSettings | null>(null);
  const [folders, setFolders] = React.useState<MailboxRow[]>([]);
  const [thresholdText, setThresholdText] = React.useState("90");
  const [checked, setChecked] = React.useState<string[]>([]);
  const [message, setMessage] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [runId, setRunId] = React.useState<number | null>(null);
  const [progress, setProgress] = React.useState<BatchProgressEvent | null>(null);
  const [report, setReport] = React.useState<BatchReport | null>(null);
  const [reportKind, setReportKind] = React.useState("archive");

  const loadSettings = React.useCallback(async () => {
    try {
      const s = (await invoke("get_archive_settings")) as ArchiveSettings;
      setSettings(s);
      setThresholdText(String(s.threshold_days));
      setChecked(s.included_folders);
    } catch (e) {
      setMessage(`Falha ao carregar a regra: ${e}`);
    }
  }, []);

  const loadFolders = React.useCallback(async () => {
    try {
      const rows = (await invoke("list_mailboxes")) as MailboxRow[];
      setFolders(rows);
    } catch {
      /* checklist shows scope-only */
    }
  }, []);

  React.useEffect(() => {
    void loadSettings();
    void loadFolders();
  }, [loadSettings, loadFolders]);

  const loadReport = React.useCallback(async (id: number) => {
    try {
      const r = (await invoke("batch_report", { runId: id })) as BatchReport;
      setReport(r);
      setReportKind(kindFromReport(r.totals_json, r.per_category).kind);
    } catch (e) {
      setMessage(`Falha no relatório: ${e}`);
    }
  }, []);

  React.useEffect(() => {
    const unlisten = listen<BatchProgressEvent>("batch-progress", (e) => {
      if (runId !== null && e.payload.run_id !== runId) return;
      setProgress(e.payload);
      if (e.payload.state === "done" || e.payload.state === "interrupted") {
        setBusy(false);
        void loadReport(e.payload.run_id);
      }
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [runId, loadReport]);

  const save = async () => {
    const days = Number.parseInt(thresholdText, 10);
    if (!Number.isFinite(days) || days < 7 || days > 730) {
      setMessage("O prazo precisa estar entre 7 e 730 dias.");
      return;
    }
    setBusy(true);
    setMessage(null);
    try {
      const s = (await invoke("set_archive_settings", {
        thresholdDays: days,
        includedFolders: checked,
      })) as ArchiveSettings;
      setSettings(s);
      setThresholdText(String(s.threshold_days));
      setChecked(s.included_folders);
      setMessage("Regra salva — vale para os próximos arquivamentos.");
    } catch (e) {
      setMessage(`Falha ao salvar: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const runNow = async (resume?: number) => {
    if (busy) return; // single-flight: double-click never double-invokes (T-24-06)
    const days = settings?.threshold_days ?? 90;
    if (resume === undefined) {
      if (!window.confirm(`Arquivar e-mails com mais de ${days} dias? Dá para desfazer depois.`)) return;
    }
    setBusy(true);
    setMessage(null);
    setReport(null);
    setProgress(null);
    try {
      const id = (await invoke("archive_run", { resumeRun: resume ?? null })) as number;
      setRunId(id);
      setReportKind("archive");
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
    if (!window.confirm("Desfazer o arquivamento e restaurar os originais?")) return;
    try {
      const n = (await invoke("undo_batch", { runId })) as number;
      setMessage(`${n} e-mails restaurados.`);
      await loadReport(runId);
    } catch (e) {
      setMessage(`Falha ao desfazer: ${e}`);
    }
  };

  const toggle = (display: string) => {
    setChecked((prev) => (prev.includes(display) ? prev.filter((f) => f !== display) : [...prev, display]));
  };

  const pct = progress && progress.total > 0 ? Math.round((progress.done / progress.total) * 100) : 0;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Arquivo automático"
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
          minWidth: 480,
          maxWidth: 640,
          maxHeight: "85vh",
          overflow: "auto",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ marginTop: 0 }}>Arquivo automático</h2>
        <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
          Guarda e-mails antigos em <code>Auto/Archive/ano/categoria</code> — só move, nunca apaga.
          O arquivo só roda quando você toca no botão: sem agendamento, sem segundo plano.
        </p>
        {message && (
          <div role="status" style={{ color: "#eab308", marginBottom: 8 }}>
            {message}
          </div>
        )}
        <div style={{ marginTop: 12, borderTop: "1px solid #1e293b", paddingTop: 12 }}>
          <h3 style={{ margin: "0 0 8px" }}>Regra de idade</h3>
          <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
            Arquivar e-mails com mais de
            <input
              value={thresholdText}
              onChange={(e) => setThresholdText(e.target.value)}
              inputMode="numeric"
              aria-label="Prazo em dias"
              style={{ width: 80 }}
            />
            dias
          </label>
          <p style={{ color: "#94a3b8", fontSize: "0.8rem" }}>
            Padrão 90 dias · mínimo 7 · máximo 730. Vale para os próximos arquivamentos.
          </p>
        </div>
        <div style={{ marginTop: 12, borderTop: "1px solid #1e293b", paddingTop: 12 }}>
          <h3 style={{ margin: "0 0 4px" }}>Pastas no arquivo</h3>
          <ul style={{ listStyle: "none", padding: 0 }}>
            {folders.map((f) => {
              const reason = archiveExclusionReason(f);
              const isChecked = checked.includes(f.display_name);
              return (
                <li
                  key={f.name}
                  style={{ display: "flex", gap: 8, alignItems: "center", padding: "3px 0" }}
                >
                  <input
                    type="checkbox"
                    checked={reason ? false : isChecked}
                    disabled={reason !== null || busy}
                    onChange={() => toggle(f.display_name)}
                    aria-label={f.display_name}
                  />
                  <span style={{ flex: 1, color: reason ? "#64748b" : undefined }}>{f.display_name}</span>
                  {reason && <span style={{ color: "#64748b", fontSize: "0.75rem" }}>{reason}</span>}
                </li>
              );
            })}
            {folders.length === 0 && (
              <li style={{ color: "#94a3b8", fontSize: "0.8rem" }}>
                Atualize as pastas para montar a lista — o escopo salvo continua valendo.
              </li>
            )}
          </ul>
          <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
            <button type="button" className="btn btn-small" disabled={busy} onClick={() => void save()}>
              Salvar regra
            </button>
            <button
              type="button"
              className="btn btn-small"
              disabled={busy}
              onClick={() => void runNow()}
            >
              Arquivar agora
            </button>
          </div>
        </div>
        {(busy || progress) && (
          <div style={{ margin: "12px 0", borderTop: "1px solid #1e293b", paddingTop: 12 }}>
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: "0.85rem" }}>
              <span>
                Arquivo · mais de {settings?.threshold_days ?? 90} dias · {progress?.done ?? 0} de{" "}
                {progress?.total ?? "?"} · {progress?.moved ?? 0} movidos
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
          <div style={{ marginTop: 12, borderTop: "1px solid #1e293b", paddingTop: 12 }}>
            <h3>
              Relatório {reportKind === "archive" ? "de arquivo" : "de reorganização"}
            </h3>
            <ul style={{ listStyle: "none", padding: 0 }}>
              {report.per_category.map(([cat, n]) => (
                <li key={cat}>
                  {cat}: <strong>{n}</strong>
                </li>
              ))}
            </ul>
            <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
              {report.per_category.reduce((a, [, n]) => a + n, 0)} arquivados · estado {report.state}
              {report.undone > 0 && ` · ${report.undone} desfeitos`}
            </p>
            {report.state === "interrupted" && runId !== null && (
              <button
                type="button"
                className="btn btn-small"
                disabled={busy}
                onClick={() => void runNow(runId)}
              >
                Retomar #{runId}
              </button>
            )}
            {(report.state === "done" || report.state === "undone") && (
              <button type="button" className="btn btn-small" onClick={() => void undo()}>
                Desfazer arquivamento
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
