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
          Sem importação/exportação (a portabilidade é o JSON da taxonomia).
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
      </div>
    </div>
  );
}
