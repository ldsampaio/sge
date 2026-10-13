import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type SearchSort = "rank" | "date";

/** Saved filter row as returned by the `list_filters` command. */
export interface SavedFilterRow {
  id: number;
  name: string;
  query: string;
  folders: string[] | null;
  sort: string;
  created_at: string;
  updated_at: string;
}

interface SavedFiltersPanelProps {
  /** Live search state (for "save current"). Empty query = nothing to save. */
  currentQuery: string;
  currentFolders: string[] | null;
  currentSort: SearchSort;
  /** Apply a filter: parent restores query + scope + sort + active id. */
  onApply: (filter: SavedFilterRow) => void;
  onClose: () => void;
}

function invokeErrorCopy(e: unknown): string {
  return typeof e === "string"
    ? e
    : e instanceof Error
      ? e.message
      : "Não foi possível concluir a operação.";
}

/**
 * Saved-filter management as a small dropdown off the search bar
 * (D-ui-surface — NOT a separate options page). Save-current, apply,
 * rename, delete call the plan-02 filter commands; backend
 * plain-language validation errors surface verbatim (T-23-10 — the panel
 * never invents success). Delete is immediate with no confirm (filters
 * hold no mail).
 */
export default function SavedFiltersPanel({
  currentQuery,
  currentFolders,
  currentSort,
  onApply,
  onClose,
}: SavedFiltersPanelProps) {
  const [filters, setFilters] = useState<SavedFilterRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [saving, setSaving] = useState(false);
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [pending, setPending] = useState(false);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const rows = await invoke<SavedFilterRow[]>("list_filters");
      setFilters(rows ?? []);
    } catch (e) {
      setError(invokeErrorCopy(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function handleSaveCurrent() {
    setSaving(true);
    setError(null);
    try {
      await invoke("save_filter", {
        name: newName,
        query: currentQuery,
        folders: currentFolders,
        sort: currentSort,
      });
      setNewName("");
      await refresh();
    } catch (e) {
      // Backend validation copy, verbatim (T-23-10).
      setError(invokeErrorCopy(e));
    } finally {
      setSaving(false);
    }
  }

  async function handleRename(id: number) {
    setPending(true);
    setError(null);
    try {
      await invoke("rename_filter", { id, name: renameValue });
      setRenamingId(null);
      setRenameValue("");
      await refresh();
    } catch (e) {
      setError(invokeErrorCopy(e));
    } finally {
      setPending(false);
    }
  }

  async function handleDelete(id: number) {
    setPending(true);
    setError(null);
    try {
      await invoke("delete_filter", { id });
      await refresh();
    } catch (e) {
      setError(invokeErrorCopy(e));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="move-menu" role="dialog" aria-label="Filtros salvos">
      <div className="move-menu-header">Filtros salvos</div>
      {error && (
        <p className="reading-error" role="alert">
          {error}
        </p>
      )}
      {loading ? (
        <p className="pager-info" role="status">
          Carregando filtros…
        </p>
      ) : filters.length === 0 ? (
        <div className="move-menu-list">
          <p className="pager-info">Nenhum filtro salvo ainda.</p>
          <p className="pager-info">Salve a busca atual para repetir depois.</p>
        </div>
      ) : (
        <div className="move-menu-list" role="group">
          {filters.map((f) => (
            <div key={f.id} className="move-menu-item">
              {renamingId === f.id ? (
                <>
                  <input
                    type="text"
                    className="search-input"
                    value={renameValue}
                    maxLength={80}
                    onChange={(e) => setRenameValue(e.currentTarget.value)}
                    aria-label="Novo nome do filtro"
                    disabled={pending}
                  />
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={() => void handleRename(f.id)}
                    disabled={pending || renameValue.trim().length === 0}
                  >
                    Salvar
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={() => setRenamingId(null)}
                    disabled={pending}
                  >
                    Cancelar
                  </button>
                </>
              ) : (
                <>
                  <button
                    type="button"
                    className="move-menu-label"
                    title={`Aplicar: ${f.query || "(tudo)"}`}
                    onClick={() => {
                      onApply(f);
                      onClose();
                    }}
                    style={{ background: "none", border: "none", cursor: "pointer" }}
                  >
                    {f.name}
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={() => {
                      setError(null);
                      setRenamingId(f.id);
                      setRenameValue(f.name);
                    }}
                    disabled={pending}
                    aria-label={`Renomear ${f.name}`}
                  >
                    Renomear
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={() => void handleDelete(f.id)}
                    disabled={pending}
                    aria-label={`Apagar ${f.name}`}
                  >
                    Apagar
                  </button>
                </>
              )}
            </div>
          ))}
        </div>
      )}
      <div className="move-menu-list">
        <input
          type="text"
          className="search-input"
          placeholder="Nome do filtro…"
          value={newName}
          maxLength={80}
          onChange={(e) => setNewName(e.currentTarget.value)}
          aria-label="Nome do novo filtro"
          disabled={saving}
        />
        <button
          type="button"
          className="btn"
          onClick={() => void handleSaveCurrent()}
          disabled={saving || newName.trim().length === 0 || currentQuery.trim().length === 0}
          title={
            currentQuery.trim().length === 0
              ? "Busque algo antes de salvar"
              : "Salvar a busca atual como filtro"
          }
        >
          Salvar busca atual
        </button>
      </div>
    </div>
  );
}
