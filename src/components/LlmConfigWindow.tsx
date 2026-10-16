// Phase 41: LLM Configuration Window (LLM-06 compliance)
// Standalone window for LLM endpoint configuration.
// Supports multiple models: list, edit, add new, delete.
// API key is stored locally in OS keyring and used to access the LLM.
// Provider presets: Groq, OpenRouter, Custom/Ollama.
// Dynamic model dropdown: fetches available models from provider API.

import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export interface LlmSettingsView {
  id?: number;
  provider: string;
  base_url: string;
  model: string;
  temperature: number;
  max_tokens: number;
  timeout_secs: number;
  prompt_template: string;
  has_api_key: boolean;
}

export interface LlmModelEntry {
  id?: number;
  provider: "groq" | "openrouter" | "custom";
  base_url: string;
  model: string;
  temperature: number;
  max_tokens: number;
  timeout_secs: number;
  prompt_template: string;
  has_api_key: boolean;
}

const PROVIDER_PRESETS: Record<string, { base_url: string; model: string; description: string }> = {
  groq: {
    base_url: "https://api.groq.com/openai/v1",
    model: "llama-3.1-8b-instant",
    description: "Groq — inferência ultra-rápida (llama-3.1, mixtral, gemma)",
  },
  openrouter: {
    base_url: "https://openrouter.ai/api/v1",
    model: "openai/gpt-4o-mini",
    description: "OpenRouter — centenas de modelos via uma única chave",
  },
  custom: {
    base_url: "http://localhost:11434/v1",
    model: "llama3",
    description: "Ollama local ou outro endpoint compatível OpenAI",
  },
};

const PRIMARY_BG   = "#0f62fe";
const SECONDARY_BG = "#d2d2d2";
const DANGER_BG    = "#d93025";
const OK_BG        = "#137333";

export default function LlmConfigWindow() {
  const [models, setModels] = useState<LlmModelEntry[]>([]);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [isAdding, setIsAdding] = useState(false);
  const [newModel, setNewModel] = useState<LlmModelEntry>({
    id: undefined,
    provider: "custom",
    base_url: "http://localhost:11434/v1",
    model: "llama3",
    temperature: 0.7,
    max_tokens: 500,
    timeout_secs: 30,
    prompt_template: "Reply to {sender}\nSubject: {subject}\nBody: {body}",
    has_api_key: false,
  });
  const [apiKeyValue, setApiKeyValue] = useState("");
  const [availableModels, setAvailableModels] = useState<string[]>([]);
  const [isFetchingModels, setIsFetchingModels] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [testStatus, setTestStatus] = useState<"idle" | "testing" | "ok" | "fail">("idle");
  const [testMsg, setTestMsg] = useState("");

  // ---- Load existing settings ------------------------------------------------
  useEffect(() => {
    void (async () => {
      try {
        const v = (await invoke("get_llm_models")) as LlmSettingsView[];
        setModels(
          v.map((m) => ({
            id: m.id,
            provider: m.provider as "groq" | "openrouter" | "custom",
            base_url: m.base_url,
            model: m.model,
            temperature: m.temperature,
            max_tokens: m.max_tokens,
            timeout_secs: m.timeout_secs,
            prompt_template: m.prompt_template,
            has_api_key: m.has_api_key,
          }))
        );
      } catch {
        setModels([]);
      }
    })();
  }, []);

  // ---- Actions ----------------------------------------------------------------
  const startAdd = () => {
    setEditingId(null);
    setIsAdding(true);
    setApiKeyValue("");
    setAvailableModels([]);
    setFetchError(null);
    setNewModel({
      id: undefined,
      provider: "custom",
      base_url: "http://localhost:11434/v1",
      model: "llama3",
      temperature: 0.7,
      max_tokens: 500,
      timeout_secs: 30,
      prompt_template: "Reply to {sender}\nSubject: {subject}\nBody: {body}",
      has_api_key: false,
    });
  };

  const cancelAddEdit = () => {
    setIsAdding(false);
    setEditingId(null);
    setApiKeyValue("");
    setAvailableModels([]);
    setFetchError(null);
  };

  const startEdit = (id: number) => {
    const found = models.find((m) => m.id === id);
    if (!found) return;
    setEditingId(id);
    setIsAdding(false);
    setApiKeyValue("");
    setAvailableModels([]);
    setFetchError(null);
    setNewModel({ ...found });
  };

  const onProviderChange = (provider: string) => {
    const preset = PROVIDER_PRESETS[provider];
    if (!preset) return;
    setNewModel((prev) => ({
      ...prev,
      provider: provider as "groq" | "openrouter" | "custom",
      base_url: preset.base_url,
      model: preset.model,
    }));
    setAvailableModels([]);
    setFetchError(null);
  };

  // Fetch available models from provider API
  const fetchAvailableModels = async () => {
    if (!newModel.base_url || !apiKeyValue) {
      setFetchError("Informe a Base URL e a Chave de API antes de buscar modelos.");
      return;
    }
    setIsFetchingModels(true);
    setFetchError(null);
    try {
      const modelsList = (await invoke("fetch_llm_models", {
        baseUrl: newModel.base_url,
        apiKey: apiKeyValue,
        timeoutSecs: newModel.timeout_secs,
      })) as string[];
      setAvailableModels(modelsList);
      if (modelsList.length > 0 && !newModel.model) {
        setNewModel({ ...newModel, model: modelsList[0] });
      }
    } catch (e: any) {
      setFetchError("Erro ao buscar modelos: " + (e?.message || String(e)));
      setAvailableModels([]);
    } finally {
      setIsFetchingModels(false);
    }
  };

  // Auto-fetch when provider changes and API key is present
  useEffect(() => {
    if (apiKeyValue && newModel.base_url) {
      void fetchAvailableModels();
    }
  }, [newModel.provider, apiKeyValue, newModel.base_url]);

  // Salvar modelo (configurações + chave de API gravada localmente no keyring)
  const saveModel = async () => {
    const input: any = {
      id: editingId ?? null,
      provider: newModel.provider,
      base_url: newModel.base_url,
      model: newModel.model,
      temperature: newModel.temperature,
      max_tokens: newModel.max_tokens,
      timeout_secs: newModel.timeout_secs,
      prompt_template: newModel.prompt_template,
      api_key: apiKeyValue || null,
    };
    try {
      const saved = (await invoke("save_llm_model", { input })) as LlmSettingsView[];
      setModels(
        saved.map((m) => ({
          id: m.id,
          provider: m.provider as "groq" | "openrouter" | "custom",
          base_url: m.base_url,
          model: m.model,
          temperature: m.temperature,
          max_tokens: m.max_tokens,
          timeout_secs: m.timeout_secs,
          prompt_template: m.prompt_template,
          has_api_key: m.has_api_key,
        }))
      );
      setTestMsg("Modelo e chave salvos localmente no keyring.");
    } catch (e) {
      setTestMsg("Erro ao salvar: " + String(e));
    }
    setEditingId(null);
    setIsAdding(false);
    setApiKeyValue("");
    setAvailableModels([]);
    setFetchError(null);
  };

  const removeModel = async (id: number) => {
    try {
      const saved = (await invoke("delete_llm_model", { id })) as LlmSettingsView[];
      setModels(
        saved.map((m) => ({
          id: m.id,
          provider: m.provider as "groq" | "openrouter" | "custom",
          base_url: m.base_url,
          model: m.model,
          temperature: m.temperature,
          max_tokens: m.max_tokens,
          timeout_secs: m.timeout_secs,
          prompt_template: m.prompt_template,
          has_api_key: m.has_api_key,
        }))
      );
    } catch (e) {
      setTestMsg("Erro ao remover: " + String(e));
    }
  };

  const runTest = async () => {
    if (models.length === 0) {
      setTestStatus("fail");
      setTestMsg("Nenhum modelo configurado.");
      return;
    }
    setTestStatus("testing");
    setTestMsg("Testando conexão...");
    try {
      const result = (await invoke("test_llm_connection")) as {
        success: boolean;
        error?: string;
        models_count: number;
      };
      if (result.success) {
        setTestStatus("ok");
        setTestMsg(`Conectado — ${result.models_count} modelo(s) disponível(ais).`);
      } else {
        setTestStatus("fail");
        setTestMsg(result.error ?? "Falha na conexão.");
      }
    } catch (e: any) {
      setTestStatus("fail");
      setTestMsg("Erro de conexão: " + (e?.message || String(e)));
    }
  };

  const applyOllamaPreset = () => {
    setApiKeyValue("");
    setAvailableModels([]);
    setFetchError(null);
    setNewModel((prev) => ({
      ...prev,
      provider: "custom",
      base_url: "http://localhost:11434/v1",
      model: "llama3",
      temperature: 0.7,
      max_tokens: 500,
      timeout_secs: 30,
      prompt_template: "Reply to {sender}\nSubject: {subject}\nBody: {body}",
      has_api_key: false,
    }));
  };

  const resetTemplate = async () => {
    try {
      const v = (await invoke("reset_llm_template")) as LlmSettingsView[];
      setApiKeyValue("");
      setAvailableModels([]);
      setFetchError(null);
      if (v.length > 0) {
        const m = v[0];
        setNewModel({
          id: m.id,
          provider: m.provider as "groq" | "openrouter" | "custom",
          base_url: m.base_url,
          model: m.model,
          temperature: m.temperature,
          max_tokens: m.max_tokens,
          timeout_secs: m.timeout_secs,
          prompt_template: m.prompt_template,
          has_api_key: m.has_api_key,
        });
        if (editingId !== null) {
          saveModel();
        }
      }
    } catch (e) {
      setTestMsg("Reset template failed: " + String(e));
    }
  };

  // ---- Input styling ----------------------------------------------------------
  const inputStyle = {
    width: "100%",
    padding: "7px",
    border: "1px solid #888",
    borderRadius: 4,
    backgroundColor: "#fff",
    color: "#000",
    fontSize: 13,
  };

  const labelStyle = {
    display: "block",
    marginBottom: 4,
    fontWeight: "bold",
    fontSize: 13,
    color: "#333",
  };

  // ---- Render helpers ---------------------------------------------------------
  const providerLabel = (p: string) => {
    switch (p) {
      case "groq": return "Groq";
      case "openrouter": return "OpenRouter";
      case "custom": return "Custom / Ollama";
      default: return p;
    }
  };

  const modelCard = (m: LlmModelEntry) => (
    <div key={m.id} style={{ border: "1px solid #ccc", borderRadius: 6, padding: 12, marginBottom: 10 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
        <div style={{ fontWeight: "bold", fontSize: 16 }}>{m.model}</div>
        <div style={{ fontSize: 11, color: "#666", backgroundColor: "#eef", padding: "2px 6px", borderRadius: 3 }}>
          {providerLabel(m.provider)}
        </div>
      </div>
      <div style={{ fontSize: 12, marginBottom: 4, color: "#333" }}>
        <span style={{ fontWeight: "bold" }}>Base URL:</span> {m.base_url}
      </div>
      <div style={{ fontSize: 12, marginBottom: 4, color: "#333" }}>
        <span style={{ fontWeight: "bold" }}>Temp:</span> {m.temperature} &nbsp;
        <span style={{ fontWeight: "bold" }}>Max tokens:</span> {m.max_tokens} &nbsp;
        <span style={{ fontWeight: "bold" }}>Timeout:</span> {m.timeout_secs}s
      </div>
      <div style={{ fontSize: 12, marginBottom: 4, color: "#333" }}>
        <span style={{ fontWeight: "bold" }}>Template:</span> {m.prompt_template}
      </div>
      <div style={{ fontSize: 12, marginBottom: 8, color: "#333" }}>
        <span style={{ fontWeight: "bold" }}>Chave:</span>{m.has_api_key ? " •••• (configurada no keyring)" : " (não configurada)"}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <button
          onClick={() => startEdit(m.id!)}
          style={{ padding: "6px 14px", fontSize: 13, backgroundColor: SECONDARY_BG, color: "#000", border: "none", borderRadius: 4, cursor: "pointer" }}
        >
          Editar
        </button>
        <button
          onClick={() => removeModel(m.id!)}
          style={{ padding: "6px 14px", fontSize: 13, backgroundColor: DANGER_BG, color: "#fff", border: "none", borderRadius: 4, cursor: "pointer" }}
        >
          Remover
        </button>
      </div>
    </div>
  );

  // ---- Form -------------------------------------------------------------------
  const modelForm = () => (
    <div style={{ border: "1px solid #ccc", borderRadius: 6, padding: 14, marginTop: 12, backgroundColor: "#fafafa" }}>
      <h4 style={{ marginTop: 0 }}>{editingId !== null ? "Editar modelo" : "Incluir novo modelo"}</h4>

      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>Provedor</label>
        <select
          value={newModel.provider}
          onChange={(e) => onProviderChange(e.target.value)}
          style={{ ...inputStyle }}
        >
          <option value="groq">Groq (api.groq.com/openai/v1)</option>
          <option value="openrouter">OpenRouter (openrouter.ai/api/v1)</option>
          <option value="custom">Custom / Ollama local</option>
        </select>
        <p style={{ marginTop: 4, fontSize: 11, color: "#666" }}>
          {PROVIDER_PRESETS[newModel.provider].description}
        </p>
      </div>

      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>Base URL</label>
        <input
          type="text"
          autoComplete="off"
          value={newModel.base_url}
          onChange={(e) => setNewModel({ ...newModel, base_url: e.target.value })}
          placeholder="https://api.groq.com/openai/v1"
          style={inputStyle}
        />
      </div>

      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>Modelo</label>
        <div style={{ display: "flex", gap: 8, alignItems: "flex-start" }}>
          <select
            value={newModel.model}
            onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
            style={{ flex: 1, ...inputStyle }}
          >
            {availableModels.length > 0 ? (
              <>
                <option value="">-- selecionar da lista --</option>
                {availableModels.map((m) => (
                  <option key={m} value={m}>{m}</option>
                ))}
              </>
            ) : (
              <option value={newModel.model}>{newModel.model}</option>
            )}
          </select>
          <button
            onClick={fetchAvailableModels}
            disabled={isFetchingModels || !apiKeyValue || !newModel.base_url}
            style={{
              padding: "7px 14px",
              fontSize: 13,
              backgroundColor: isFetchingModels ? "#999" : PRIMARY_BG,
              color: "#fff",
              border: "none",
              borderRadius: 4,
              cursor: isFetchingModels || !apiKeyValue || !newModel.base_url ? "not-allowed" : "pointer",
            }}
          >
            {isFetchingModels ? "Buscando..." : "Buscar modelos"}
          </button>
        </div>
        {fetchError && <p style={{ marginTop: 4, fontSize: 11, color: DANGER_BG }}>{fetchError}</p>}
        {availableModels.length > 0 && (
          <p style={{ marginTop: 4, fontSize: 11, color: "#666" }}>
            {availableModels.length} modelo(s) disponível(is) neste endpoint.
          </p>
        )}
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 8, marginBottom: 12 }}>
        <div>
          <label style={labelStyle}>Temperatura</label>
          <input
            type="number"
            value={newModel.temperature}
            step="0.1"
            onChange={(e) => setNewModel({ ...newModel, temperature: parseFloat(e.target.value) || 0 })}
            style={inputStyle}
          />
        </div>
        <div>
          <label style={labelStyle}>Max tokens</label>
          <input
            type="number"
            value={newModel.max_tokens}
            step="1"
            onChange={(e) => setNewModel({ ...newModel, max_tokens: parseInt(e.target.value) || 0 })}
            style={inputStyle}
          />
        </div>
        <div>
          <label style={labelStyle}>Timeout (s)</label>
          <input
            type="number"
            value={newModel.timeout_secs}
            step="1"
            onChange={(e) => setNewModel({ ...newModel, timeout_secs: parseInt(e.target.value) || 0 })}
            style={inputStyle}
          />
        </div>
      </div>

      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>Template de prompt</label>
        <input
          type="text"
          autoComplete="off"
          value={newModel.prompt_template}
          onChange={(e) => setNewModel({ ...newModel, prompt_template: e.target.value })}
          placeholder="Reply to {sender}\nSubject: {subject}\nBody: {body}"
          style={{ ...inputStyle, minHeight: 40 }}
        />
      </div>

      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>Chave de API (gravada localmente no keyring)</label>
        <input
          type="password"
          autoComplete="off"
          value={apiKeyValue}
          onChange={(e) => {
            setApiKeyValue(e.target.value);
            setNewModel({ ...newModel, has_api_key: e.target.value.length > 0 });
          }}
          placeholder="gsk_... ou sk-or-..."
          style={inputStyle}
        />
        <p style={{ marginTop: 4, fontSize: 12, color: "#666" }}>
          A chave é armazenada de forma segura no keyring do SO e usada para acessar o LLM.
        </p>
      </div>

      <div style={{ marginBottom: 12 }}>
        <label style={labelStyle}>Possui chave de API configurada</label>
        <select
          value={newModel.has_api_key ? "1" : "0"}
          onChange={(e) => setNewModel({ ...newModel, has_api_key: e.target.value === "1" })}
          style={{ ...inputStyle }}
        >
          <option value="0">Não</option>
          <option value="1">Sim</option>
        </select>
      </div>

      <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
        <button
          onClick={saveModel}
          style={{ padding: "8px 18px", fontSize: 14, backgroundColor: PRIMARY_BG, color: "#fff", border: "none", borderRadius: 4, cursor: "pointer" }}
        >
          Salvar modelo e chave
        </button>
        <button
          onClick={cancelAddEdit}
          style={{ padding: "8px 18px", fontSize: 14, backgroundColor: SECONDARY_BG, color: "#000", border: "none", borderRadius: 4, cursor: "pointer" }}
        >
          Cancelar
        </button>
      </div>
    </div>
  );

  // ---- Preset row -------------------------------------------------------------
  const presetRow = () => (
    <div style={{ marginTop: 10, display: "flex", gap: 8 }}>
      <button
        onClick={applyOllamaPreset}
        style={{ padding: "6px 14px", fontSize: 13, backgroundColor: PRIMARY_BG, color: "#fff", border: "none", borderRadius: 4, cursor: "pointer" }}
      >
        Preset Ollama (localhost)
      </button>
      <button
        onClick={resetTemplate}
        style={{ padding: "6px 14px", fontSize: 13, backgroundColor: SECONDARY_BG, color: "#000", border: "none", borderRadius: 4, cursor: "pointer" }}
      >
        Resetar template
      </button>
    </div>
  );

  // ---- Render -----------------------------------------------------------------
  return (
    <div style={{ padding: 20, border: "1px solid #ccc", borderRadius: 8, maxWidth: 900, margin: "auto", marginTop: 20, fontFamily: "sans-serif", backgroundColor: "#fff" }}>
      <h2 style={{ marginTop: 0, color: "#1a1a1a" }}>Configuração do Endpoint LLM</h2>
      <p style={{ color: "#555", marginBottom: 20 }}>Gerencie os modelos LLM usados para geração de respostas.</p>

      {/* Models list */}
      <div style={{ marginBottom: 20 }}>
        <h3 style={{ marginBottom: 8, color: "#1a1a1a" }}>Modelos configurados</h3>
        {models.length > 0 ? (
          <>
            {models.map(modelCard)}
            <div style={{ marginTop: 12 }}>
              <button
                onClick={startAdd}
                style={{ padding: "10px 22px", fontSize: 15, backgroundColor: PRIMARY_BG, color: "#fff", border: "none", borderRadius: 6, cursor: "pointer", boxShadow: "0 2px 4px rgba(0,0,0,0.15)" }}
              >
                ➕ Incluir novo modelo
              </button>
            </div>
          </>
        ) : (
          <>
            <p style={{ color: "#777", fontStyle: "italic", marginBottom: 12 }}>Nenhum modelo configurado ainda.</p>
            <button
              onClick={startAdd}
              style={{ padding: "10px 22px", fontSize: 15, backgroundColor: PRIMARY_BG, color: "#fff", border: "none", borderRadius: 6, cursor: "pointer", boxShadow: "0 2px 4px rgba(0,0,0,0.15)" }}
            >
              ➕ Incluir novo modelo
            </button>
          </>
        )}
      </div>

      {/* Add / Edit form */}
      {(isAdding || editingId !== null) && modelForm()}

      {!isAdding && editingId === null && models.length === 0 && presetRow()}

      {/* Test connection */}
      <div style={{ marginTop: 20, padding: 12, backgroundColor: "#f1f1f1", borderRadius: 6 }}>
        <h4 style={{ marginTop: 0, marginBottom: 8, color: "#1a1a1a" }}>Testar conexão</h4>
        <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
          <button
            onClick={runTest}
            disabled={testStatus === "testing"}
            style={{ padding: "6px 16px", fontSize: 14, backgroundColor: OK_BG, color: "#fff", border: "none", borderRadius: 4, cursor: "pointer" }}
          >
            {testStatus === "testing" ? "Testando..." : "Testar conexão"}
          </button>
        </div>
        {testStatus === "ok" && <p style={{ color: OK_BG, margin: 0 }}>{testMsg}</p>}
        {testStatus === "fail" && <p style={{ color: DANGER_BG, margin: 0 }}>{testMsg}</p>}
      </div>

      {/* Close */}
      <div style={{ marginTop: 24, textAlign: "right" }}>
        <button
          onClick={() => getCurrentWindow().close()}
          style={{ padding: "8px 20px", fontSize: 14, backgroundColor: SECONDARY_BG, color: "#000", border: "1px solid #666", borderRadius: 4, cursor: "pointer" }}
        >
          Fechar janela
        </button>
      </div>
    </div>
  );
}