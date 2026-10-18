# SGE v1.8 Requirements — LLM-Assisted Replies

Milestone: v1.8 LLM-Assisted Replies (brownfield; continues from Phase 38, v1.7 compose UX complete).
Research: `.planning/research/SUMMARY.md` (+ STACK/FEATURES/ARCHITECTURE/PITFALLS.md), confidence HIGH.

Constraints (carried from `.planning/PROJECT.md` / research):
- BODY.PEEK-only on all read paths; UID-only addressing; UIDVALIDITY-first gating; SessionManager single-session invariant.
- Secrets at rest in OS keyring only (keyring 3 + sync-secret-service) — mail creds and LLM API key alike; never plaintext, never logged, never returned over IPC to the frontend.
- Mail content leaves the machine ONLY on explicit Generate-button click; offline default preserved; no auto-send, no background generation.
- Zero new core dependencies (reqwest 0.12 feature widening to `json`/`stream`/`native-tls` only; `cargo tree` must show no `rustls`/`ring`).
- Reply/Reply-All windows only (no new-message/forward generation); single account; Linux only.
- Live endpoint calls only with user-provided key; CI uses mock transport (hand-rolled loopback mock), live gate deferred-with-reason per project convention.

## Requirements (v1.8 — scoped)

> **Status legend (2026-10-18):** `[x] implemented` = code exists with unit/mock-transport tests — **not** validated against a live endpoint. `[ ] pending` = not built.
>
> ⚠️ **No v1.8 requirement is validated.** The LLM lane has never been called against a real endpoint with a real key, and until the 2026-10-18 repair (`dc7f8c7`) LLM-07/LLM-08 were unreachable in practice: Phase 38 left the Reply window inert, so there was no surface to generate from or insert into. Treat every `[x]` below as "built and offline-tested", nothing stronger.

### Settings (LLM configuration menu)

- [x] **LLM-01**: User can configure LLM base URL (any OpenAI-compatible endpoint) + model name in a settings menu section
- [x] **LLM-02**: User can save an API key that is stored in the OS keyring (new service/account entry, masked `••••` in UI), never in SQLite/logs/IPC responses
- [x] **LLM-03**: User can edit the single reply prompt template with `{sender}`/`{subject}`/`{body}` placeholders, with a sane default template and reset-to-default
- [x] **LLM-04**: User can set generation defaults (temperature / max tokens / timeout) with sane defaults and clamped ranges on save
- [x] **LLM-05**: User can test the configured endpoint from settings (test-connection call) before first generation
- [ ] **LLM-06**: User can one-click fill an Ollama localhost preset (`http://localhost:11434/v1` + model) for local-server use

### Generation (Reply/Reply-All windows)

- [x] **LLM-07**: User can click a Generate-with-LLM button in Reply/Reply-All windows (next to Enviar) that calls `POST {base}/chat/completions` (non-streaming) with loading state and cancel
- [ ] **LLM-08**: Generated text is inserted above the quoted original in the compose body, fully editable; threading headers (`In-Reply-To`/`References`) and recipients are untouched; send stays manual via existing `queue_send`
- [x] **LLM-09**: Generation failures surface as plain-language errors (unconfigured → settings link; 401 → wrong/revoked key; timeout/network → connectivity; provider 4xx/5xx → provider text) with retry; key and bodies never appear in errors or logs
- [ ] **LLM-10**: User can regenerate (re-call the same prompt) when the first draft misses
- [ ] **LLM-11**: Nothing leaves the machine except on button click (no prefetch, no draft-sync of generated text, no telemetry with bodies) — preserved by single-chokepoint backend command + closing audit

> **LLM-07/LLM-08 caveat:** the generation *call* is built (LLM-07), and insert-above-quote exists in `ComposeWindow`, but LLM-08 also requires threading headers and recipients to be untouched and `queue_send` to work — which was false until `dc7f8c7`. LLM-08 stays `[ ]` until a real reply is observed threading correctly. **Related repair not in this list:** the same commit made reply/threading/attachments real, which no LLM requirement covered.

## Future Requirements (deferred)

- Streaming output (token-by-token into the body) — deferred to v1.8.x, trigger: latency complaints on long generations
- Per-draft instruction override — deferred, trigger: users ask to adjust tone per reply
- Multi-prompt library (saved prompt templates) — deferred to v2+, contradicts single-template scope
- Generation in forward / new-message windows — deferred, needs its own context-spec
- Voice matching from sent mail — deferred to v2+, PII expansion conflicts with minimal-context posture

## Out of Scope (explicit exclusions with reasoning)

- Auto-send of generated replies — sends wrong-tone/wrong-fact mail under the user's name; destroys trust; violates the milestone's core promise
- Background / pre-generation on message open — sends incoming mail to a third party without consent; burns tokens; contradicts offline-by-default posture
- Full-mailbox / thread-history context — unbounded prompts (cost, latency), more PII per call, needs retrieval machinery; single-message context only
- Per-provider native SDKs (Anthropic/Gemini/bedrock) — N× auth/format/error maintenance; one OpenAI-compatible shape covers OpenAI, Ollama, LM Studio, OpenRouter, LiteLLM
- Plaintext API-key storage (SQLite/config) — credential theft on disk read; violates security constraint
- Logging prompts/responses for debugging — mail bodies + key material in logs; log only status codes, latency, token counts, redacted error classes

## Traceability

| Requirement | Phase | Plan | Status |
|-------------|-------|------|--------|
| LLM-01 | Phase 39 | — | implemented (unvalidated) |
| LLM-02 | Phase 39 | — | implemented (unvalidated) |
| LLM-03 | Phase 39 | — | implemented (unvalidated) |
| LLM-04 | Phase 39 | — | implemented (unvalidated) |
| LLM-05 | Phase 40 | — | implemented (unvalidated) |
| LLM-06 | Phase 41 | — | pending |
| LLM-07 | Phase 40 | — | implemented (unvalidated) |
| LLM-08 | Phase 41 | — | pending — blocked on real threading headers, delivered by `dc7f8c7`, not yet UAT'd |
| LLM-09 | Phase 40 | — | implemented (unvalidated) |
| LLM-10 | Phase 41 | — | pending |
| LLM-11 | Phase 41 | — | pending |
