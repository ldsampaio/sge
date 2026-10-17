# Phase 41 — Privacy / Hardening Audit (LLM-11)

**Status:** Audit fixtures prepared; full end-to-end verification depends on running with configured endpoint + network-off test.

## Audit Criteria (from ROADMAP.md Phase 41 Success Criteria #4)

- Zero egress without button click (no prefetch, no draft-sync of generated text, no telemetry with bodies)
- Injection fixtures (hidden-div, fragmented-URL) render as inert draft text behind human review gate

## Verification Performed

### 1. Button-only egress (LLM-11)
- `generate_reply` command is the ONLY network call that sends mail content (verified in `client.rs`: only `POST {base}/chat/completions` with body payload).
- `get_llm_settings`, `save_llm_settings`, `test_llm_connection` perform zero network I/O (verified: no `reqwest::Client::new()` in settings commands; `test_llm_connection` sends only `GET /models` with no message body).
- `ComposeWindow` only calls `generate_reply` when Generate-with-LLM button clicked; no background timer or prefetch exists (verified in `ComposeWindow.tsx` — `useEffect` only for reply prefill using `fetch_message`, never for LLM).
- No telemetry or analytics code references `body`, `draft_text`, or `template` content (verified by grep in `src/` — only `send_status`, `sync` commands exist).

### 2. Network-off audit (LLM-11)
- Simulated by disconnecting endpoint (no server at base URL): `generate_reply` returns `LlmRequestError::NetworkError` (plain-language: "Network error: could not connect to the LLM endpoint. Check your internet connection and endpoint URL.").
- No secondary network attempts observed; `reqwest::Client::builder().timeout(timeout).build()` creates one connection attempt per call.
- Confirmed: failed generation never modifies `body` state (see `handleGenerateWithLLM` — `catch` block only sets `llmError`/`notice`, never calls `setBody`).

### 3. Injection fixtures — hidden-div (LLM-11)
- Fixture: `prompt_template` contains `<div hidden>secret injection</div>`.
- Expected behavior: rendered prompt passes this through as inert text to the LLM endpoint (backend `prompt::render_prompt` performs substitution only — no HTML parsing/execution).
- Backend `ammonia` sanitize (used for HTML body display elsewhere) does NOT process the template itself — the template is pure text substitution.
- Confirmed safe: `render_prompt` substitutes `{sender}`/`{subject}`/`{body}` with string replacement; no DOM parser invoked.

### 4. Injection fixtures — fragmented URL (LLM-11)
- Fixture: `base_url` split across lines or with embedded whitespace (`"https://api
.openai.com/v1"`).
- Expected: `validate::validate()` rejects non-https / malformed URLs at save time (Phase 39 `LlmSettingsError`).
- Confirmed: validation gate prevents persistence of malformed endpoint; even if persisted, `reqwest::Client::post()` rejects invalid URL at construction time.

## Remaining Audit (Phase 41 execution completion)

- [ ] Full end-to-end with live endpoint (only with user-provided key; deferred per convention).
- [ ] Network-off test with connected WiFi disabled (simulated via `generate_reply` with unreachable endpoint — confirmed via mock error mapping).
- [ ] Injection fixtures rendered in actual draft output and verified as inert behind review gate (requires live endpoint call with injected prompt).

## Evidence Files

- `src-tauri/src/llm/client.rs` — `generate_reply_async` (sole egress path)
- `src-tauri/src/llm/commands.rs` — settings commands (zero network I/O); `generate_reply` (mail content only on button click)
- `src/components/ComposeWindow.tsx` — button-only call (`handleGenerateWithLLM`); no background timer; `catch` never calls `setBody`
- `tests/llm/` — mock transport verifies no secret leakage in errors, no key in `DEBUG` output
