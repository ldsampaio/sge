import React from "react";
import { invoke } from "@tauri-apps/api/core";

/** Phase 19 editor UI: add/rename/delete categories, edit keywords/rules,
 *  import/export JSON. Structural ops bump the version + migrate folders;
 *  keyword edits are in-place. Delete/merge ask for reassignment first
 *  (orphan-prompt) — the backend refuses orphaning mail. */

interface CategoryView {
  id: string;
  name: string;
  parent: string | null;
  keywords: string[];
  rule: string;
  labels: number;
}

interface TaxonomyView {
  version: number;
  name: string;
  categories: CategoryView[];
}

interface TaxonomyEditorProps {
  onClose: () => void;
}

export function TaxonomyEditor({ onClose }: TaxonomyEditorProps) {
  const [view, setView] = React.useState<TaxonomyView | null>(null);
  const [message, setMessage] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [editing, setEditing] = React.useState<CategoryView | null>(null);
  const [addingUnder, setAddingUnder] = React.useState<string | null>(null);
  const [formName, setFormName] = React.useState("");
  const [formKeywords, setFormKeywords] = React.useState("");
  const [formRule, setFormRule] = React.useState("");
  const [reassign, setReassign] = React.useState("");

  const reload = React.useCallback(async () => {
    try {
      const v = (await invoke("list_taxonomy")) as TaxonomyView;
      setView(v);
    } catch (e) {
      setMessage(`Falha ao carregar: ${e}`);
    }
  }, []);

  React.useEffect(() => {
    void reload();
  }, [reload]);

  const run = async (fn: () => Promise<unknown>, okReset = true) => {
    setBusy(true);
    setMessage(null);
    try {
      await fn();
      await reload();
      if (okReset) {
        setEditing(null);
        setAddingUnder(null);
        setFormName("");
        setFormKeywords("");
        setFormRule("");
        setReassign("");
      }
    } catch (e) {
      setMessage(`Falha: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const tops = (view?.categories ?? []).filter((c) => !c.parent);
  const childrenOf = (id: string) => (view?.categories ?? []).filter((c) => c.parent === id);

  const startEdit = (c: CategoryView) => {
    setAddingUnder(null);
    setEditing(c);
    setFormName(c.name);
    setFormKeywords(c.keywords.join(", "));
    setFormRule(c.rule);
  };

  const saveEdit = () =>
    run(async () => {
      if (!editing) return;
      if (addingUnder !== null) {
        await invoke("add_category", {
          parent: editing.parent,
          name: formName,
          keywords: formKeywords.split(",").map((k) => k.trim()).filter(Boolean),
          rule: formRule,
        });
      } else {
        if (formName !== editing.name) {
          await invoke("rename_category", { id: editing.id, newName: formName });
        }
        await invoke("update_category_keywords", {
          id: editing.id,
          keywords: formKeywords.split(",").map((k) => k.trim()).filter(Boolean),
          rule: formRule,
        });
      }
    });

  const remove = (c: CategoryView) =>
    run(async () => {
      if (!reassign) {
        setMessage("Escolha a categoria que recebe os e-mails antes de excluir.");
        return;
      }
      await invoke("delete_category", { id: c.id, reassignTo: reassign });
    });

  const importFile = async (file: File) => {
    const text = await file.text();
    await run(() => invoke("import_taxonomy", { json: text }) as Promise<unknown>);
  };

  const exportFile = async () => {
    try {
      const json = (await invoke("export_taxonomy")) as string;
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "taxonomy.json";
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setMessage(`Falha ao exportar: ${e}`);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Editar categorias"
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
        <h2 style={{ marginTop: 0 }}>
          Categorias {view && <span style={{ color: "#94a3b8", fontSize: "0.85rem" }}>v{view.version}</span>}
        </h2>
        {message && (
          <div role="status" style={{ color: "#eab308", marginBottom: 8 }}>
            {message}
          </div>
        )}
        {tops.map((t) => (
          <div key={t.id} style={{ marginBottom: 10 }}>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <strong>{t.name}</strong>
              <span style={{ color: "#94a3b8", fontSize: "0.75rem" }}>{t.labels} rótulos</span>
              <button type="button" className="btn btn-small" disabled={busy} onClick={() => startEdit(t)}>
                Editar
              </button>
              <button
                type="button"
                className="btn btn-small"
                disabled={busy}
                onClick={() => {
                  setEditing({ ...t, parent: t.id, name: "", keywords: [], rule: "" });
                  setAddingUnder(t.id);
                  setFormName("");
                  setFormKeywords("");
                  setFormRule("");
                }}
              >
                + Sub
              </button>
            </div>
            <ul style={{ listStyle: "none", paddingLeft: 16 }}>
              {childrenOf(t.id).map((c) => (
                <li key={c.id} style={{ display: "flex", gap: 8, alignItems: "center", padding: "3px 0" }}>
                  <span>{c.name}</span>
                  <span style={{ color: "#94a3b8", fontSize: "0.75rem" }}>{c.labels} rótulos</span>
                  <button type="button" className="btn btn-small" disabled={busy} onClick={() => startEdit(c)}>
                    Editar
                  </button>
                  <button
                    type="button"
                    className="btn btn-small"
                    disabled={busy}
                    onClick={() => {
                      setEditing(c);
                      setAddingUnder(null);
                      setReassign("");
                    }}
                  >
                    Excluir
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ))}
        <div style={{ display: "flex", gap: 8, marginTop: 8, flexWrap: "wrap" }}>
          <button
            type="button"
            className="btn btn-small"
            disabled={busy}
            onClick={() => {
              setEditing({ id: "", name: "", parent: null, keywords: [], rule: "", labels: 0 });
              setAddingUnder("TOP");
              setFormName("");
              setFormKeywords("");
              setFormRule("");
            }}
          >
            + Categoria
          </button>
          <label className="btn btn-small" style={{ cursor: "pointer" }}>
            Importar JSON
            <input
              type="file"
              accept=".json,application/json"
              hidden
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) void importFile(f);
              }}
            />
          </label>
          <button type="button" className="btn btn-small" onClick={() => void exportFile()}>
            Exportar
          </button>
        </div>
        {editing && (
          <div style={{ marginTop: 12, borderTop: "1px solid #1e293b", paddingTop: 12 }}>
            <h3 style={{ margin: "0 0 8px" }}>
              {addingUnder !== null ? "Nova categoria" : `Editar ${editing.name}`}
            </h3>
            <label style={{ display: "block", marginBottom: 6 }}>
              Nome
              <input
                value={formName}
                onChange={(e) => setFormName(e.target.value)}
                style={{ width: "100%" }}
              />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Palavras-chave (vírgula)
              <input
                value={formKeywords}
                onChange={(e) => setFormKeywords(e.target.value)}
                style={{ width: "100%" }}
              />
            </label>
            <label style={{ display: "block", marginBottom: 6 }}>
              Regra
              <input value={formRule} onChange={(e) => setFormRule(e.target.value)} style={{ width: "100%" }} />
            </label>
            {addingUnder === null && editing.id && (
              <label style={{ display: "block", marginBottom: 6 }}>
                Ao excluir, mover e-mails para
                <select value={reassign} onChange={(e) => setReassign(e.target.value)} style={{ width: "100%" }}>
                  <option value="">Escolha…</option>
                  {(view?.categories ?? [])
                    .filter((c) => c.id !== editing.id)
                    .map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                </select>
              </label>
            )}
            <div style={{ display: "flex", gap: 8 }}>
              <button type="button" className="btn btn-small" disabled={busy} onClick={() => void saveEdit()}>
                Salvar
              </button>
              {addingUnder === null && editing.id && (
                <button type="button" className="btn btn-small" disabled={busy} onClick={() => void remove(editing)}>
                  Excluir categoria
                </button>
              )}
              <button
                type="button"
                className="btn btn-small"
                onClick={() => {
                  setEditing(null);
                  setAddingUnder(null);
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
