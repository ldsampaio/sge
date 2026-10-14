import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Phase 25 rules UI: list/enable-disable/edit/delete/reorder rules in
 *  options next to the taxonomy editor, plus thin + journaled bulk apply
 *  with progress and undo.
 *
 *  NOTE on arg casing: top-level command args ride camelCase (Tauri
 *  converts to snake_case); NESTED condition/destination keys are sent in
 *  snake_case verbatim so backend serde matches regardless of conversion
 *  depth. */

interface RuleConditions {
  sender_substring?: string | null;
  sender_domain?: string | null;
  subject_keyword?: string | null;
  subject_regex?: string | null;
  snippet_keyword?: string | null;
}

interface RuleView {
  id: string;
  name: string;
  priority: number;
  conditions: RuleConditions;
  destination: { kind: "folder"; display: string } | { kind: "category"; id: string };
  scope_folders: string[];
  enabled: boolean;
}

interface CategoryRef {
  id: string;
  name: string;
}

interface RuleProgress {
  run_id: number;
  done: number;
  total: number;
  moved: number;
  skipped: number;
  state: string;
}

/** Phase 29 rule portability (IMP-01): backend validation preview.
 * The frontend never validates — this renders `import_rules`
 * (commit=false) verbatim; Accept commits, Reject discards. */
interface ImportPreview {
  fileName: string;
  json: string;
  imported: number;
  updated: number;
  warnings: string[];
  errors: string[];
}

interface RulesPanelProps {
  onClose: () => void;
}

const emptyForm = () => ({
  name: "",
  sender_substring: "",
  sender_domain: "",
  subject_keyword: "",
  subject_regex: "",
  snippet_keyword: "",
  destKind: "folder" as "folder" | "category",
  destFolder: "",
  destCategory: "",
  scope: "",
  enabled: true,
});

export function RulesPanel({ onClose }: RulesPanelProps) {
  const [rules, setRules] = React.useState<RuleView[] | null>(null);
  const [categories, setCategories] = React.useState<CategoryRef[]>([]);
  const [message, setMessage] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [editingId, setEditingId] = React.useState<string | null>(null);
  const [creating, setCreating] = React.useState(false);
  const [form, setForm] = React.useState(emptyForm());
  const [progress, setProgress] = React.useState<RuleProgress | null>(null);
  const [lastRun, setLastRun] = React.useState<number | null>(null);
  const [thinResult, setThinResult] = React.useState<string | null>(null);
  /** Phase 29: import validation preview (null = no import in flight). */
  const [sieveText, setSieveText] = React.useState<string | null>(null);
  const [sieveWarnings, setSieveWarnings] = React.useState<number | null>(null);
  const [exportBusy, setExportBusy] = React.useState(false);
  /** Phase 31 Export-Sieve (SIEVE-01): preview + copy + Blob download. */
  const exportSieve = async () => {
    setExportBusy(true);
    try {
      const res = (await invoke("export_sieve_text")) as { text: string; warnings_count: number };
      setSieveText(res.text);
      setSieveWarnings(res.warnings_count);
    } catch (e) {
      setMessage(`Falha export Sieve: ${e}`);
    } finally {
      setExportBusy(false);
    }
  };
  const copySieve = () => {
    if (!sieveText) return;
    navigator.clipboard.writeText(sieveText).catch(() => {});
  };
  const downloadSieve = () => {
    if (!sieveText) return;
    const blob = new Blob([sieveText], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url; a.download = "rules.sieve"; a.click(); URL.revokeObjectURL(url);
  };
  const [preview, setPreview] = React.useState<ImportPreview | null>(null);
  const [portBusy, setPortBusy] = React.useState(false);
  const fileRef = React.useRef<HTMLInputElement | null>(null);

  const reload = React.useCallback(async () => {
    try {
      const list = (await invoke("list_rules")) as RuleView[];
      setRules(list);
    } catch (e) {
      setMessage(`Falha ao carregar regras: ${e}`);
    }
    try {
      const tax = (await invoke("list_taxonomy")) as {
        categories: { id: string; name: string }[];
      };
      setCategories(tax.categories.map((c) => ({ id: c.id, name: c.name })));
    } catch {
      /* destination select degrades to free text */
    }
  }, []);

  React.useEffect(() => {
    void reload();
  }, [reload]);

  React.useEffect(() => {
    const unlisten = listen<RuleProgress>("rule-progress", (e) => {
      setProgress(e.payload);
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  const set = (k: keyof ReturnType<typeof emptyForm>, v: string | boolean) =>
    setForm((f) => ({ ...f, [k]: v }));

  const conditionsOf = (): RuleConditions => ({
    sender_substring: form.sender_substring.trim() || null,
    sender_domain: form.sender_domain.trim() || null,
    subject_keyword: form.subject_keyword.trim() || null,
    subject_regex: form.subject_regex.trim() || null,
    snippet_keyword: form.snippet_keyword.trim() || null,
  });

  const save = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const destination =
        form.destKind === "folder"
          ? { kind: "folder", display: form.destFolder.trim() }
          : { kind: "category", id: form.destCategory };
      const scopeFolders = form.scope
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      if (editingId) {
        await invoke("update_rule", {
          id: editingId,
          name: form.name,
          conditions: conditionsOf(),
          destination,
          scopeFolders,
          enabled: form.enabled,
        });
      } else {
        await invoke("create_rule", {
          name: form.name,
          conditions: conditionsOf(),
          destination,
          scopeFolders,
        });
      }
      setCreating(false);
      setEditingId(null);
      setForm(emptyForm());
      await reload();
    } catch (e) {
      // Backend validation speaks plain language — surface verbatim.
      setMessage(`${e}`);
    } finally {
      setBusy(false);
    }
  };

  const startEdit = (r: RuleView) => {
    setCreating(true);
    setEditingId(r.id);
    setForm({
      name: r.name,
      sender_substring: r.conditions.sender_substring ?? "",
      sender_domain: r.conditions.sender_domain ?? "",
      subject_keyword: r.conditions.subject_keyword ?? "",
      subject_regex: r.conditions.subject_regex ?? "",
      snippet_keyword: r.conditions.snippet_keyword ?? "",
      destKind: r.destination.kind,
      destFolder: r.destination.kind === "folder" ? r.destination.display : "",
      destCategory: r.destination.kind === "category" ? r.destination.id : "",
      scope: (r.scope_folders ?? []).join(", "),
      enabled: r.enabled,
    });
  };

  const remove = async (r: RuleView) => {
    // Immediate, no confirm — rules hold no mail.
    try {
      await invoke("delete_rule", { id: r.id });
      await reload();
    } catch (e) {
      setMessage(`Falha ao excluir: ${e}`);
    }
  };

  const toggle = async (r: RuleView) => {
    try {
      await invoke("set_rule_enabled", { id: r.id, enabled: !r.enabled });
      await reload();
    } catch (e) {
      setMessage(`Falha ao alternar: ${e}`);
    }
  };

  const move = async (index: number, dir: -1 | 1) => {
    const list = rules ?? [];
    const j = index + dir;
    if (j < 0 || j >= list.length) return;
    // Swap explicit integer priorities (lower wins; ties by creation).
    const a = list[index];
    const b = list[j];
    try {
      await invoke("set_rule_priority", { id: a.id, priority: b.priority });
      await invoke("set_rule_priority", { id: b.id, priority: a.priority });
      await reload();
    } catch (e) {
      setMessage(`Falha ao reordenar: ${e}`);
    }
  };

  const thinApply = async () => {
    setBusy(true);
    setThinResult(null);
    try {
      const r = (await invoke("apply_rules", { mailbox: null })) as {
        evaluated: number;
        hits: number;
      };
      setThinResult(`${r.hits} sugestão(ões) em ${r.evaluated} e-mails — confirme em A Classificar.`);
    } catch (e) {
      setMessage(`Falha ao aplicar: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const bulkApply = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const id = (await invoke("apply_rules_bulk", { mailbox: null })) as number;
      setLastRun(id);
      setProgress({ run_id: id, done: 0, total: 0, moved: 0, skipped: 0, state: "running" });
    } catch (e) {
      setMessage(`Falha no modo em lote: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const undo = async () => {
    if (lastRun === null) return;
    try {
      const n = (await invoke("undo_batch", { runId: lastRun })) as number;
      setMessage(`${n} movido(s) restaurado(s).`);
      setLastRun(null);
      setProgress(null);
    } catch (e) {
      setMessage(`Falha ao desfazer: ${e}`);
    }
  };

  const destLabel = (r: RuleView) =>
    r.destination.kind === "folder" ? r.destination.display : `categoria ${r.destination.id}`;

  /** Phase 29: export all rules as versioned JSON download. */
  const doExport = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const json = (await invoke("export_rules")) as string;
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      const stamp = new Date().toISOString().replace(/[:.]/g, "-");
      a.href = url;
      a.download = `regras-${stamp}.json`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
      const count = (rules ?? []).length;
      setMessage(`${count} regra(s) exportada(s).`);
    } catch (e) {
      setMessage(`Falha ao exportar: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  /** Phase 29: file picked → backend dry-run validation → preview modal.
   * WR-02: reject absurd files before touching the backend (rules are
   * metadata-only — 1 MiB is generous; mirrors MAX_IMPORT_BYTES). */
  const MAX_IMPORT_BYTES = 1 << 20; // 1 MiB
  const onFilePicked = async (file: File) => {
    setPortBusy(true);
    setMessage(null);
    try {
      if (file.size > MAX_IMPORT_BYTES) {
        setMessage("Arquivo grande demais para importar (máx. 1 MiB).");
        return;
      }
      const text = await file.text();
      // May throw on malformed JSON — surface as a preview-level error.
      let result: Omit<ImportPreview, "fileName" | "json">;
      try {
        result = (await invoke("import_rules", { json: text, commit: false })) as Omit<
          ImportPreview,
          "fileName" | "json"
        >;
      } catch (e) {
        setPreview({
          fileName: file.name,
          json: text,
          imported: 0,
          updated: 0,
          warnings: [],
          errors: [`arquivo inválido: ${e}`],
        });
        return;
      }
      setPreview({ fileName: file.name, json: text, ...result });
    } finally {
      setPortBusy(false);
      if (fileRef.current) fileRef.current.value = "";
    }
  };

  /** Phase 29: Accept commits the valid subset (invalid entries are skipped
   * with per-item reasons — WR-04 partial-import contract); Reject discards
   * everything. Accept is enabled whenever at least one rule is committable. */
  const acceptImport = async () => {
    if (!preview || preview.imported + preview.updated === 0) return;
    setPortBusy(true);
    try {
      const result = (await invoke("import_rules", {
        json: preview.json,
        commit: true,
      })) as Omit<ImportPreview, "fileName" | "json">;
      setMessage(
        `${result.imported} importada(s), ${result.updated} atualizada(s)` +
          (result.warnings.length > 0 ? `, ${result.warnings.length} aviso(s).` : "."),
      );
      setPreview(null);
      await reload();
    } catch (e) {
      setMessage(`Falha ao importar: ${e}`);
    } finally {
      setPortBusy(false);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Regras de arquivamento"
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
          maxWidth: 720,
          maxHeight: "85vh",
          overflow: "auto",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ marginTop: 0 }}>Regras de arquivamento</h2>
        <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
          Destinos determinísticos avaliados após cada sync — nada se move sem confirmar.
          Leve suas regras entre máquinas: exporte o JSON versionado e importe com
          pré-visualização de validação.
        </p>
        {message && (
          <div role="status" style={{ color: "#eab308", marginBottom: 8 }}>
            {message}
          </div>
        )}
        {rules === null ? (
          <p>Carregando…</p>
        ) : rules.length === 0 ? (
          <p style={{ fontSize: "0.85rem" }}>Nenhuma regra. Crie a primeira abaixo.</p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0 }}>
            {rules.map((r, i) => (
              <li
                key={r.id}
                style={{
                  padding: "8px 0",
                  borderTop: "1px solid #1e293b",
                  opacity: r.enabled ? 1 : 0.55,
                }}
              >
                <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
                  <span style={{ flex: 1, minWidth: 160 }}>
                    <strong>{r.name}</strong>
                    <span style={{ color: "#94a3b8", fontSize: "0.8rem" }}>
                      {" "}→ {destLabel(r)} {!r.enabled && "(desativada)"}
                    </span>
                  </span>
                  <button type="button" className="btn btn-small" disabled={busy} onClick={() => void toggle(r)}>
                    {r.enabled ? "Desativar" : "Ativar"}
                  </button>
                  <button type="button" className="btn btn-small" disabled={busy || i === 0} onClick={() => void move(i, -1)} aria-label="Subir prioridade">
                    ↑
                  </button>
                  <button
                    type="button"
                    className="btn btn-small"
                    disabled={busy || i === (rules?.length ?? 0) - 1}
                    onClick={() => void move(i, 1)}
                    aria-label="Descer prioridade"
                  >
                    ↓
                  </button>
                  <button type="button" className="btn btn-small" disabled={busy} onClick={() => startEdit(r)}>
                    Editar
                  </button>
                  <button type="button" className="btn btn-small" disabled={busy} onClick={() => void remove(r)}>
                    Excluir
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
        <div style={{ display: "flex", gap: 8, marginTop: 8, flexWrap: "wrap" }}>
          <button
            type="button"
            className="btn btn-small"
            disabled={busy}
            onClick={() => {
              setCreating(true);
              setEditingId(null);
              setForm(emptyForm());
            }}
          >
            + Regra
          </button>
          <button type="button" className="btn btn-small" disabled={busy} onClick={() => void thinApply()}>
            Aplicar agora
          </button>
          <button type="button" className="btn btn-small" disabled={busy} onClick={() => void bulkApply()}>
            Aplicar em tudo (com diário)
          </button>
          {lastRun !== null && (
            <button type="button" className="btn btn-small" disabled={busy} onClick={() => void undo()}>
              Desfazer lote
            </button>
          )}
          {/* Phase 29 portability: export downloads versioned JSON;
              import opens a picker → backend dry-run → preview modal. */}
          <button type="button" className="btn btn-small" disabled={busy} onClick={() => void doExport()}>
            Exportar regras
          </button>
          <button
            type="button"
            className="btn btn-small"
            disabled={busy || portBusy}
            onClick={() => fileRef.current?.click()}
          >
            Importar regras
          </button>
          <input
            ref={fileRef}
            type="file"
            accept=".json,application/json"
            style={{ display: "none" }}
            aria-label="Escolher arquivo JSON de regras"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void onFilePicked(f);
            }}
          />
        </div>
        {/* Phase 31 Export-Sieve (SIEVE-01) section */}
        <div style={{ marginTop: 8, borderTop: "1px dashed #334", paddingTop: 6 }}>
          <b>Exportar Sieve (.sieve)</b> &nbsp;
          <button type="button" className="btn btn-small" disabled={exportBusy} onClick={() => void exportSieve()}>Preview/.sieve</button>
          {sieveText !== null && (
            <>
              <button type="button" className="btn btn-small" onClick={copySieve}>Copiar</button>
              <button type="button" className="btn btn-small" onClick={downloadSieve}>Baixar .sieve</button>
              <span style={{ marginLeft: 6, fontSize: "0.85rem", color: "#c7d2fe" }}>
                avisos={sieveWarnings ?? "-"}
              </span>
            </>
          )}
          {sieveText && (
            <pre style={{ fontSize: "0.78rem", background: "#0b1220", maxHeight: 160, overflow: "auto", marginTop: 4, whiteSpace: "pre-wrap", wordBreak: "break-word" }}>
              {sieveText}
            </pre>
          )}
          <p style={{ fontSize: "0.75rem", color: "#8896ab", margin: 2 }}>
            Preview filtrado; download via Blob (sem escrita no backend). Sem novos deps.
          </p>
        </div>
        {thinResult && (
          <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>{thinResult}</p>
        )}
        {progress && (
          <p style={{ color: "#94a3b8", fontSize: "0.85rem" }} role="status">
            Lote #{progress.run_id}: {progress.done}/{progress.total} · {progress.moved} movidos ·{" "}
            {progress.skipped} ignorados · {progress.state}
          </p>
        )}
        {creating && (
          <div style={{ marginTop: 12, borderTop: "1px solid #1e293b", paddingTop: 12 }}>
            <h3 style={{ margin: "0 0 8px" }}>{editingId ? "Editar regra" : "Nova regra"}</h3>
            <label style={{ display: "block", marginBottom: 6 }}>
              Nome
              <input value={form.name} onChange={(e) => set("name", e.target.value)} style={{ width: "100%" }} />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Remetente contém
              <input
                value={form.sender_substring}
                onChange={(e) => set("sender_substring", e.target.value)}
                style={{ width: "100%" }}
              />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Domínio do remetente
              <input
                value={form.sender_domain}
                onChange={(e) => set("sender_domain", e.target.value)}
                placeholder="exemplo.com"
                style={{ width: "100%" }}
              />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Assunto contém
              <input
                value={form.subject_keyword}
                onChange={(e) => set("subject_keyword", e.target.value)}
                style={{ width: "100%" }}
              />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Assunto (expressão regular)
              <input
                value={form.subject_regex}
                onChange={(e) => set("subject_regex", e.target.value)}
                placeholder="^Fatura \d{4}$"
                style={{ width: "100%" }}
              />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Trecho do e-mail contém
              <input
                value={form.snippet_keyword}
                onChange={(e) => set("snippet_keyword", e.target.value)}
                style={{ width: "100%" }}
              />
            </label>
            <div style={{ display: "flex", gap: 8, marginBottom: 6 }}>
              <label>
                <input
                  type="radio"
                  checked={form.destKind === "folder"}
                  onChange={() => set("destKind", "folder")}
                />{" "}
                Pasta
              </label>
              <label>
                <input
                  type="radio"
                  checked={form.destKind === "category"}
                  onChange={() => set("destKind", "category")}
                />{" "}
                Categoria
              </label>
            </div>
            {form.destKind === "folder" ? (
              <label style={{ display: "block", marginBottom: 6 }}>
                Pasta de destino
                <input
                  value={form.destFolder}
                  onChange={(e) => set("destFolder", e.target.value)}
                  placeholder="Arquivo"
                  style={{ width: "100%" }}
                />
              </label>
            ) : (
              <label style={{ display: "block", marginBottom: 6 }}>
                Categoria de destino
                <select
                  value={form.destCategory}
                  onChange={(e) => set("destCategory", e.target.value)}
                  style={{ width: "100%" }}
                >
                  <option value="">Escolha…</option>
                  {categories.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <label style={{ display: "block", marginBottom: 6 }}>
              Escopo (pastas, vírgula — vazio = todas)
              <input
                value={form.scope}
                onChange={(e) => set("scope", e.target.value)}
                placeholder="INBOX"
                style={{ width: "100%" }}
              />
            </label>
            {editingId && (
              <label style={{ display: "block", marginBottom: 6 }}>
                <input
                  type="checkbox"
                  checked={form.enabled}
                  onChange={(e) => setForm((f) => ({ ...f, enabled: e.target.checked }))}
                />{" "}
                Ativada
              </label>
            )}
            <div style={{ display: "flex", gap: 8 }}>
              <button type="button" className="btn btn-small" disabled={busy} onClick={() => void save()}>
                Salvar
              </button>
              <button
                type="button"
                className="btn btn-small"
                onClick={() => {
                  setCreating(false);
                  setEditingId(null);
                }}
              >
                Cancelar
              </button>
            </div>
          </div>
        )}
        <div style={{ marginTop: 12, textAlign: "right" }}>
          <button type="button" className="btn" onClick={onClose}>
            Fechar
          </button>
        </div>
        {/* Phase 29 import validation preview: backend ImportResult
            verbatim — errors red, warnings amber, Accept commits the valid
            subset (invalid entries skipped with reasons — WR-04), Reject
            discards all. Accept needs ≥1 committable rule. */}
        {preview && (
          <div
            role="dialog"
            aria-modal="true"
            aria-label="Pré-visualização da importação"
            style={{
              position: "fixed",
              inset: 0,
              background: "rgba(0,0,0,0.6)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              zIndex: 70,
            }}
            onClick={() => setPreview(null)}
          >
            <div
              style={{
                background: "var(--color-bg, #0f172a)",
                borderRadius: 12,
                padding: 20,
                minWidth: 420,
                maxWidth: 600,
                maxHeight: "80vh",
                overflow: "auto",
              }}
              onClick={(e) => e.stopPropagation()}
            >
              <h3 style={{ marginTop: 0 }}>Importar de {preview.fileName}</h3>
              <p style={{ color: "#94a3b8", fontSize: "0.85rem" }} role="status">
                {preview.imported} nova(s), {preview.updated} atualizada(s)
                {preview.warnings.length > 0 && `, ${preview.warnings.length} aviso(s)`}
                {preview.errors.length > 0 && `, ${preview.errors.length} erro(s)`}.
              </p>
              {preview.errors.length > 0 && (
                <div style={{ marginBottom: 8 }}>
                  <strong style={{ color: "#f87171" }}>Erros (estas regras serão ignoradas)</strong>
                  <ul style={{ color: "#fca5a5", fontSize: "0.85rem" }}>
                    {preview.errors.map((e, i) => (
                      <li key={i}>{e}</li>
                    ))}
                  </ul>
                </div>
              )}
              {preview.warnings.length > 0 && (
                <div style={{ marginBottom: 8 }}>
                  <strong style={{ color: "#fbbf24" }}>Avisos (importa mesmo assim)</strong>
                  <ul style={{ color: "#fcd34d", fontSize: "0.85rem" }}>
                    {preview.warnings.map((w, i) => (
                      <li key={i}>{w}</li>
                    ))}
                  </ul>
                </div>
              )}
              {preview.errors.length === 0 && preview.imported === 0 && preview.updated === 0 && (
                <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
                  Nada a importar — o arquivo não traz regras novas.
                </p>
              )}
              <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
                <button
                  type="button"
                  className="btn btn-small"
                  disabled={portBusy || preview.imported + preview.updated === 0}
                  title={
                    preview.imported + preview.updated === 0
                      ? "Nada válido para importar"
                      : preview.errors.length > 0
                        ? `Confirmar importação das ${preview.imported + preview.updated} válida(s) (regras com erro serão ignoradas)`
                        : "Confirmar importação"
                  }
                  onClick={() => void acceptImport()}
                >
                  Aceitar
                </button>
                <button
                  type="button"
                  className="btn btn-small"
                  disabled={portBusy}
                  onClick={() => setPreview(null)}
                >
                  Rejeitar
                </button>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
# Sieve-managed badge (31-03; was 31-01 extension point)
// skip-check pre-matcher: managed rules never score locally
