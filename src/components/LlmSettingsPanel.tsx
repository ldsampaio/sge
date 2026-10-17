// Phase 41 — LLM Settings Panel (LLM-06 compliance)
// One-click Ollama localhost preset, masked key (••••), reset-to-default
// template, test-connection passes before first generation.

import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface LlmSettingsView {
  base_url: string;
  model: string;
  temperature: number;
  max_tokens: number;
  timeout_secs: number;
  prompt_template: string;
  has_api_key: boolean;
}

export default function LlmSettingsPanel() {
  const [view, setView] = useState<LlmSettingsView | null>(null);
  const [keyInput, setKeyInput] = useState("");
  const [testStatus, setTestStatus] = useState<"idle" | "testing" | "ok" | "fail">("idle");
  const [testMsg, setTestMsg] = useState("");

  useEffect(() => {
    void (async () => {
      try {
        const v = (await invoke("get_llm_settings")) as LlmSettingsView;
        setView(v);
      } catch {
        setView(null);
      }
    })();
  }, []);

  const applyPreset = async () => {
    const preset = {
      base_url: "http://localhost:11434/v1",
      model: "llama3",
      temperature: 0.7,
      max_tokens: 500,
      timeout_secs: 30,
      prompt_template: "Reply to {sender}\nSubject: {subject}\nBody: {body}",
    };
    try {
      await invoke("save_llm_settings", { input: { ...preset, api_key: null } });
      setView({ ...preset, has_api_key: view?.has_api_key ?? false } as LlmSettingsView);
    } catch (e) {
      setTestMsg("Failed to apply preset: " + String(e));
    }
  };

  const runTest = async () => {
    setTestStatus("testing");
    setTestMsg("");
    try {
      const result = (await invoke("test_llm_connection")) as {
        success: boolean;
        error?: string;
        models_count: number;
      };
      if (result.success) {
        setTestStatus("ok");
        setTestMsg(`Connected — ${result.models_count} model(s) available.`);
      } else {
        setTestStatus("fail");
        setTestMsg(result.error ?? "Connection failed.");
      }
    } catch (e: any) {
      setTestStatus("fail");
      setTestMsg("Connection error: " + (e?.message || String(e)));
    }
  };

  const resetTemplate = async () => {
    try {
      const v = (await invoke("reset_llm_template")) as LlmSettingsView;
      setView(v);
    } catch (e) {
      setTestMsg("Reset failed: " + String(e));
    }
  };

  const saveKey = async () => {
    if (!keyInput.trim()) return;
    try {
      await invoke("save_llm_settings", {
        input: {
          base_url: view?.base_url ?? "https://api.openai.com/v1",
          model: view?.model ?? "gpt-4o-mini",
          temperature: view?.temperature ?? 0.7,
          max_tokens: view?.max_tokens ?? 500,
          timeout_secs: view?.timeout_secs ?? 30,
          prompt_template: view?.prompt_template ?? "",
          api_key: keyInput.trim(),
        },
      });
      setKeyInput("");
      const v = (await invoke("get_llm_settings")) as LlmSettingsView;
      setView(v);
    } catch (e) {
      setTestMsg("Key save failed: " + String(e));
    }
  };

  const clearKey = async () => {
    try {
      await invoke("clear_llm_api_key");
      const v = (await invoke("get_llm_settings")) as LlmSettingsView;
      setView(v);
    } catch (e) {
      setTestMsg("Clear failed: " + String(e));
    }
  };

  return (
    <section aria-label="LLM Settings" style={{ padding: 16, border: "1px solid #ddd", borderRadius: 8, maxWidth: 640 }}>
      <h3>LLM — Configuração do Endpoint</h3>

      {view && (
        <div style={{ marginBottom: 12 }}>
          <p><strong>Base URL:</strong> {view.base_url}</p>
          <p><strong>Modelo:</strong> {view.model}</p>
          <p><strong>Temperatura:</strong> {view.temperature}</p>
          <p><strong>Max tokens:</strong> {view.max_tokens}</p>
          <p><strong>Timeout:</strong> {view.timeout_secs}s</p>
          <p><strong>Chave:</strong> {view.has_api_key ? "•••• (configurada)" : "(não configurada)"}</p>
        </div>
      )}

      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
        <button onClick={applyPreset}>Preset Ollama (localhost)</button>
        <button onClick={resetTemplate}>Resetar template</button>
        <button onClick={runTest} disabled={testStatus === "testing"}>
          {testStatus === "testing" ? "Testando..." : "Testar conexão"}
        </button>
      </div>

      {testStatus === "ok" && <p style={{ color: "green" }}>{testMsg}</p>}
      {testStatus === "fail" && <p role="alert" style={{ color: "firebrick" }}>{testMsg}</p>}

      <div style={{ marginTop: 8 }}>
        <label>
          Chave de API (mascarada no armazenamento — nunca exibida):
          <input
            type="password"
            value={keyInput}
            onChange={(e) => setKeyInput(e.target.value)}
            placeholder="sk-..."
            style={{ marginLeft: 4, width: 320 }}
          />
        </label>
        <button onClick={saveKey} style={{ marginLeft: 4 }}>Salvar chave</button>
        <button onClick={clearKey} style={{ marginLeft: 4 }}>Limpar chave</button>
      </div>

      <div style={{ marginTop: 8, fontSize: 12, color: "#666" }}>
        Nenhum conteúdo de e-mail é enviado até clicar em "Gerar com LLM" (LLM-11: eixo de saída apenas por botão).
      </div>
    </section>
  );
}
