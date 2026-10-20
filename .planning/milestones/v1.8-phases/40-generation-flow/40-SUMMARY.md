# Phase 40: Generation Flow - Summary

## Phase Overview
**Phase 40: Generation Flow**
- **Goal**: One backend command turns validated settings + a server-side original into draft text, with plain-language failures and no other egress path.
- **Dependencies**: Phase 39 (LLM Settings Foundation) – completed
- **Requirements**: LLM-05, LLM-07, LLM-09

## Success Criteria
1. `generate_reply{uid,mailbox}` returns draft text from `POST {base}/chat/completions` (non-streaming, configurable timeout, cancel, per-window single-flight) without any body crossing IPC to the frontend.
2. `test_llm_connection` proves the endpoint works via `GET /models` (no mail content sent) before first generation.
3. Every failure class maps to a plain-language error (unconfigured → settings link; 401 → wrong/revoked key; timeout/network → connectivity; provider 4xx/5xx → provider text) with retry; key and bodies never in errors or logs (metadata-only: status, latency, token usage).
4. Mock-transport tests cover multi-server response/error shapes (OpenAI/Ollama envelopes, {"error"} vs bare, empty/length-stop as errors, redirect refused, truncation-on-wire).

## Plans

### 1. Test Connection (LLM-05)
- Endpoint: `GET /models` (no mail content)
- Verify successful response before any generation
- Store test result for regression

### 2. Generate Button (LLM-07)
- Endpoint: `POST {base}/chat/completions`
- Non-streaming, configurable timeout, cancel support
- Per-window single-flight (no concurrent calls)
- Loading state + cancel UI

### 3. Plain-Language Errors (LLM-09)
- Unconfigured → link to settings
- 401 → wrong/revoked API key
- Timeout/Network → connectivity message
- Provider 4xx/5xx → provider-specific text
- Retry only on 408/429/5xx
- Key and bodies never appear in error messages or logs

### 4. Mock Transport Tests
- Multi-server response/error shapes
- OpenAI/Ollama envelopes
- {"error"} vs bare error formats
- Empty/`length`-stop as errors
- Redirect refused
- Truncation-on-wire

## Implementation Notes
- Backend-owned `llm` lane module + Tauri commands
- No new core dependencies (reqwest 0.12 feature widening to `json`/`stream`/`native-tls` only)
- Secrets at rest in OS keyring only (keyring 3 + sync-secret-service)
- Mail content leaves machine ONLY on explicit Generate-button click
- No auto-send, no background generation
- Reply/Reply-All windows only (single account, Linux only)
- Live endpoint calls only with user-provided key; CI uses mock transport
- Zero new core deps

## Risks & Mitigations
- **Risk**: Frontend IPC leaks body content
  **Mitigation**: Ensure all error paths use metadata-only (status, latency, token usage) and never include body or key
- **Risk**: Rate limiting / throttling
  **Mitigation**: Configurable timeout, per-window single-flight, exponential backoff
- **Risk**: Secret exposure in logs
  **Mitigation**: Audit all error paths; ensure key never appears in logs
- **Risk**: Test/CI environment differences
  **Mitigation**: Use mock transport for CI; real endpoint only in staging

## Artifacts
- `40-plan.md` – phase plan
- `40-SUMMARY.md` – this summary
- `40-CONTEXT.md` – context from Phase 39
- `40-STATE.md` – state update (Phase 40 started)
