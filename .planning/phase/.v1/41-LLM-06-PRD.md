# SGE — LLM-06: One-click Ollama localhost preset (Phase 41 deliverable)

**Status:** Implemented and unit-tested; awaiting Phase 41 human UAT against a configured endpoint (Ollama or any OpenAI-compatible).

## Context

LLM-05 (settings UI for any OpenAI-compatible endpoint + test-connection) is already implemented. LLM-06 extends the settings menu with a one-click preset that fills default values for local-server use: `base_url = http://localhost:11434/v1` and model names common in the Ollama Hub (`qwen2.5`, `llama3`, `mistral`, etc.).  

The generic handler (LLM-07) already supports any endpoint, so this feature is purely convenience: users who want to run an inference server locally don't have to memorize port numbers or endpoints.

## UI

A new row in the Settings → LLM section with:

- Title: "Ollama localhost"
- Controls: none (button only)
- Action: writes `{"base_url": "http://localhost:11434/v1", "model": prompt} → write_to_settings()` and updates the display to show the saved values. A reset-to-default button clears it back to a blank row.

**Wiring code exists:** the settings panel (`LlmSettingsPanel.tsx`) already conditionally renders rows based on whether `OllamaPreset` is enabled in config (a new const). That flag was added by commit 50c517b; this PRD documents the feature for UAT.

## Acceptance Criteria

- **AC-01** Row labelled "Ollama localhost" appears and lets the user pick a model name from a short preset list (`qwen2.5`, `llama3:8b`, `mistral`, `dolphin-mistral`).
- **AC-02** Clicking it writes to the keyring-backed config file (never logs the URL or model), then reloads the settings view.
- **AC-03** A "Reset" button clears the row back to the default OpenAI-compatible endpoint form.
- **AC-04** Test-connection uses the newly saved URL + model name and reports success/failure in plain language ("Conexão OK", "401: chave inválida").

## Privacy Guardrail (LLM-11)

The generation call remains the sole chokepoint; this UI change touches nothing except settings persistence. No bodies leave except on Generate click. Errors are plain-language, no key exposure.  

**Regression check:** unit tests confirm `generate_reply` signature and endpoint-params remained unchanged after adding the presets (the feature is orthogonal).

## Test Cases

| # | Description | Expected |
|---|---|---|
| 41-06-01 | User clicks Ollama preset; settings view refreshes with `http://localhost:11434/v1` + selected model | PASS (UI shows values) |
| 41-06-02 | Click Test → endpoint is unreachable | "Connection timed out" (no stack trace, no credentials in UI) |
| 41-06-03 | Ollama returns 401 for that model name | "Chave inválida. Verifique se o modelo está disponível e que você forneceu uma chave." |

## References

- `.planning/PROJECT.md` Phase 41 section
- `.planning/v1.8-research/privacy.md` guardrail implementation
- LLM-07: generic OpenAI-compatible handler (commit 3925918 + follow-ups)

---

*Author:* GSD session 2026-10-18  
*Related commits:* dc7f8c7, 50c517b  
*Next:* UAT + Phase 41 completion