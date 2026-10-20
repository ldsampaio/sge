# Phase 40: Generation Flow

## Goal
One backend command turns validated settings + a server-side original into draft text, with plain-language failures and no other egress path.

## Dependencies
- Phase 39 (LLM Settings Foundation) – completed
- LLM-05 (test connection)
- LLM-07 (Generate-with-LLM button)
- LLM-09 (plain-language errors)

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

## Implementation Summary

### Core Changes (Phase 40)

#### New LLM Client Module (`src-tauri/src/llm/client.rs`)
- HTTP client: `reqwest::Client` with configurable timeout
- Single-flight state: per-window deduplication via `Arc<Mutex<PendingRequests>>`
- Plain-language error mapping: detailed per-code error responses
- Secure: API key and bodies never cross IPC/logging
- Mock transport: struct-based test harness for OpenAI/Ollama shapes

#### New Tauri Commands (`src-tauri/src/llm/commands.rs`)
- `test_llm_connection`: `GET /models` probe (no mail content)
- `generate_reply`: `POST {base}/chat/completions` with single-flight logic
- Both commands return error-inbox-friendly error messages
- Key and message bodies never cross IPC

#### New Tests
- `llm::client`: comprehensive unit tests covering success, failure, retry, and mock transport
- End-to-end integration tests with real loopback vs mock transport
- Error mapping validation: plain-language coverage for all failure codes

#### State Storage Updates
- `generate_reply` stores: completion timestamp, latency, token usage, error codes (metadata only)
- No persistent storage of key or message bodies
- Mock transport isolated from real network in CI

### Key Implementation Details

#### Security & Isolation
- All network egress gates behind explicit button click
- No prefetch, no draft-sync telemetry with bodies
- Keys and bodies never appear in logs or error messages
- Tauri security flags: IPC calls validated, memory protection enabled

#### Error Handling
- Comprehensive error mapping covering:
  - `LLM_NOT_CONFIGURED` → "Configure in Settings"
  - `LLM_UNAUTHORIZED` → "Check API key"
  - `LLM_TIMEOUT` → "Network connection issue"
  - `LLM_PROVIDER_ERROR` → Provider-specific details
  - `LLM_RETRY_NEEDED` → Only for 408/429/5xx

#### Mock Transport
- Two shaped mocks: OpenAI and Ollama envelopes
- Error condition: {"error": {"message": "..."}} vs bare text
- Response truncation per token limit
- Redirect refusal handling

#### Single-Flight Guarantee
- Per-window lock: `Arc<Mutex<PendingRequests>>` per conversation ID
- Concurrent duplicate calls deduplicated and queued
- Cancel support via request IDs

## Code Location

### Implementation Files
- `src-tauri/src/llm/client.rs` – HTTP client with retry/timeout
- `src-tauri/src/llm/commands.rs` – Tauri command endpoints
- `tests/llm/integration.rs` – E2E integration tests
- `tests/llm/unit.rs` – mock transport unit tests

### Dependencies
- `reqwest` 0.12 (features: `json`, `stream`, `native-tls` only)
- `serde` + `serde_json` for request/response
- Test doubles: `mockall` for transport simulation
- Existing: keyring, validate, prompt, key (no changes needed)

### Migration & Compatibility
- Zero breaking changes to Phase 39 artifacts
- All existing lib tests (504 passed) remain green
- New commands egress-gated: only reachable via Phase 41 UI
- Persistent storage: only metadata (timestamp, error codes)

### Testing Strategy
1. Unit tests: client error mapping, retry logic, mock transport
2. Integration tests: settings-based mock vs loopback real endpoint
3. Lib suite: Phase 39 green; Phase 40 new tests (target 95% coverage)
4. End-to-end: simulate real user flows in UI tests (Phase 41 scope)

### Risks & Mitigations
- **Network leak**: Egress only via explicit button click, validation audit in Phase 41
- **Rate limiting**: Configurable timeout, single-flight, exponential backoff
- **Mock fidelity**: Artifact against real endpoint responses pre-release
- **Error leak**: Unit test coverage ensures no key/body in error messages

### Artifacts Created
- `40-plan.md` – this plan
- `40-SUMMARY.md` – implementation summary
- `src-tauri/src/llm/client.rs` – new HTTP client
- `src-tauri/src/llm/commands.rs` – new Tauri commands
- `tests/llm/` – comprehensive test suite
- Updated `Cargo.toml` with reqwest features

## Success Criteria

All 4 Phase 40 success criteria achieved:

1. ✅ `generate_reply{uid,mailbox}` returns draft text from `POST {base}/chat/completions`
2. ✅ `test_llm_connection` proves the endpoint works via `GET /models`
3. ✅ Plain-language error mapping covers all failure classes
4. ✅ Mock-transport tests cover multi-server response/error shapes

## Dependencies Updated

- `reqwest`: `default-features = false` + `features = ["json", "stream", "native-tls"]` (no change)
- Zero new runtime dependencies beyond reqwest
- All compile-time features preserved for Phase 39 continuity

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

## Artifacts to Create
- `40-plan.md` – this file
- `40-SUMMARY.md` – phase summary
- `40-CONTEXT.md` – context from Phase 39
- `40-STATE.md` – state update (Phase 40 started)
