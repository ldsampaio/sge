# Stack Research

**Domain:** SGE v1.8 — user-triggered LLM reply drafting via user-configured OpenAI-compatible endpoint (Tauri v2 + Rust + React, Linux)
**Researched:** 2026-10-15
**Confidence:** HIGH (all core answers grounded in pinned `Cargo.toml` + verified docs.rs `reqwest 0.12` feature/dependency list; no new core deps required)

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| reqwest (feature widening, same line) | 0.12 (already pinned; add `features = ["json", "stream", "native-tls"]`, keep `default-features = false`) | HTTPS POST to `{base_url}/chat/completions` from Tauri commands | Already in the tree for the Phase 15 loopback health probe, so zero new HTTP crate to learn, audit, or bundle. `json` gives typed request/response via the already-pinned serde/serde_json; `native-tls` reuses the pinned native-tls 0.2 / tokio-native-tls stack (no second TLS trust store, no new OpenSSL build requirement — native-tls is already compiled); `stream` (bytes_stream over already-pinned futures-util 0.3) is added now so optional SSE streaming later needs no second Cargo change. Non-streaming `POST → .json()` ships first; streaming stays a UI decision, not a dep decision. |
| serde + serde_json | 1 (already pinned) | Chat-completions request/response shapes | No change: `#[derive(Serialize, Deserialize)]` structs for `ChatRequest { model, messages, temperature, max_tokens }` / `ChatResponse { choices[0].message.content }` with `#[serde(default)]` tolerance for provider field variance (Ollama, LM Studio, vLLM, OpenAI all speak this dialect with minor extras). |
| keyring | 3 + `sync-secret-service` (already pinned) | LLM API key storage | Reuse the exact Phase 1/5 `KeyringStore` pattern with a new service/account pair (e.g. service `sge-llm`, account `openai-compatible`). Same consent model as mail creds (saved only from the settings screen), same Linux Secret Service backend, zero new secret-handling code paths. The key is read keyring-side inside `generate_reply` and never returned to the frontend. |
| rusqlite + rusqlite_migration | 0.37 bundled / 2.x (already pinned) | Non-secret LLM settings persistence | Same M20+ migration pattern: new single-row `llm_settings` table (`base_url`, `model`, `prompt_template`, `temperature`, `max_tokens`, `timeout_secs`, `updated_at`). No `tauri-plugin-store` (would be a second persistence mechanism for one row) and no plaintext config file (settings live next to mail data in the existing DB). |
| tokio `time` | 1.x `rt, sync, time` (already pinned) | Per-generation timeout | `tokio::time::timeout` around the reqwest call enforces the user-configured `timeout_secs`; `Client::timeout` as backstop. No new dep. |
| zeroize + thiserror | 1 / 2 (already pinned) | In-memory key hygiene + typed LLM errors | `zeroize::Zeroizing<String>` for the key between keyring read and request build; a `LlmError` enum (NotConfigured / NoKey / Timeout / HttpStatus / Transport / BadResponse / InvalidUrl) mapped to plain-language frontend strings. No new dep. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| (none new) | — | — | v1.8 needs no new crates. Everything above is a feature-flag or table addition to already-pinned deps. |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| Mock chat-completions server in tests (`tokio` + `std::net::TcpListener` or `wiremock` as dev-dep only if planners insist) | Deterministic tests for prompt rendering, Bearer header, timeout, non-2xx mapping, key-never-in-error | Prefer a ~30-line hand-rolled mock on loopback (same style as the Phase 15 probe) over adding `wiremock`/`mockito` to the tree; mock transport in CI per PROJECT.md, live endpoint only with a user-provided key. |
| `cargo tree -i native-tls / rustls` | Verify no second TLS stack enters the graph after the reqwest feature change | Must show reqwest → hyper-tls → native-tls only; if any `rustls`/`ring` crate appears, a feature flag is wrong. |
| Existing `live-*.sh` harness pattern | One manual live gate with a real key (Ollama loopback or user endpoint) | Deferred-with-reason like all prior live gates; never commit a key. |

## Installation

```bash
# The ONLY Cargo change for v1.8 — widen reqwest features on the pinned 0.12 line:
# reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "native-tls"] }
cargo update -p reqwest --precise 0.12.28  # optional: lock the verified patch

# Everything else is already in src-tauri/Cargo.toml — DO NOT re-add:
# serde / serde_json 1 (chat-completions shapes)
# keyring 3 + sync-secret-service (API-key entry, new service/account pair)
# rusqlite 0.37 bundled + rusqlite_migration 2.x (llm_settings table)
# tokio 1 rt/sync/time (generation timeout) / futures 0.3 (streaming later)
# zeroize 1 (key hygiene) / thiserror 2 (LlmError) / chrono 0.4 (updated_at)
# lettre 0.11 / async-imap 0.11 (untouched — generated text flows into the existing queue_send path)
```

```bash
# Frontend: no new npm packages. Generation is a Tauri invoke; insertion is plaintext.
```

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| Raw reqwest chat-completions POST (~60 lines) | `async-openai` crate | Only if v1.8 later needs assistants/vector-store/file-search APIs. For one `chat/completions` call against arbitrary base URLs, async-openai adds version churn and its own re-exported reqwest that can fight the pinned 0.12 line — all cost, no benefit now. |
| reqwest `native-tls` feature | reqwest `rustls-tls-*` features | If OpenSSL dev headers ever become a build problem on a target distro. Today native-tls 0.2 is already compiled into the bundle (IMAP + SMTP), so rustls would add a second TLS stack and trust store for one POST — never for v1.8. |
| Non-streaming JSON response first | SSE streaming from day one | Only if UAT shows generation latency hurts UX. Non-streaming keeps the compose-window change to "button → spinner → paste above quote"; `stream` feature is pre-enabled so streaming is a follow-up plan, not a Cargo change. |
| Hand-rolled `{sender}/{subject}/{body}` substitution | `handlebars` / `minijinja` / `tera` | Never for v1.8: three placeholders via chained `.replace()` with a missing-placeholder guard is ~10 lines and snapshot-testable. A template engine is justified only if templates grow conditionals/loops. |
| SQLite `llm_settings` table | `tauri-plugin-store` JSON file | If settings ever need cross-machine sync or non-DB portability. Today the DB is the established settings home; a second store doubles backup/wipe semantics for one row. |
| Hand-rolled loopback mock | `wiremock` / `mockito` dev-deps | If the mock grows beyond one endpoint (auth-failure + timeout + truncation cases are still trivially hand-rolled). Keep the tree lean until then. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `async-openai`, `openai-api-rs`, `rig`, LangChain-style orchestration crates | Typed wrappers around the one endpoint v1.8 calls; each pins its own HTTP/TLS versions and update cadence, risking lockfile fights with the pinned reqwest 0.12 / native-tls 0.2 stack | Raw reqwest POST with local serde structs |
| reqwest 0.13 bump inside v1.8 | 0.13.x exists (latest 0.13.5, 2026-09-08) but buys nothing v1.8 needs and re-opens the transitive-dependency surface (hyper/http tower versions) mid-milestone | Stay on pinned 0.12; schedule 0.13 as its own migration if ever wanted |
| `eventsource-stream` / `reqwest-eventsource` / SSE crates now | Streaming is explicitly optional and deferred; adding an SSE crate before the non-streaming path ships is speculative complexity | `bytes_stream` (pre-enabled `stream` feature) + ~40-line manual `data:` line parser later, only if needed |
| Template-engine crates (`handlebars`, `minijinja`, `tera`) | Three-placeholder substitution does not need a grammar, and engines invite logic-in-templates that complicates the privacy story | Chained `.replace()` + unit tests over `{sender}/{subject}/{body}` |
| Markdown renderers (`pulldown-cmark`, react-markdown) for generated text | Model output pasted as rendered markdown is an injection surface (links, images, HTML passthrough) in a mail composer; also a new dep on both sides | Insert generated text as plaintext above the quoted original (matches "user reviews before sending"); revisit rendering only with a sanitizer (ammonia already pinned) |
| `tauri-plugin-store` / plaintext config file for settings | Second persistence mechanism / key-adjacent secrets on disk in cleartext | SQLite row for non-secrets + keyring entry for the key (existing Phase 5 pattern) |
| Frontend-held API key (key in React state / localStorage) | Key crosses the IPC boundary, lands in devtools-accessible memory and possibly disk; violates the keyring-only constraint | Key lives keyring-side; frontend calls `generate_reply(message_id)` and receives only generated text |
| `http://` remote base URLs | Mail content would leave the machine in cleartext on top of leaving it at all | Enforce https-only except explicit `http://localhost|127.0.0.1` loopback exception (covers Ollama/LM Studio default); reject everything else with a plain-language error |

## Stack Patterns by Variant

**If the user points at a local server (Ollama / LM Studio default `http://localhost:11434/v1`):**
- Allow the loopback-http exception, empty-key path (no `Authorization` header when no key stored), and longer default timeout (local model load can take 30–60 s on first token).
- Because local inference is the privacy-best configuration and must be first-class, not a grudging fallback.

**If the user points at a hosted OpenAI-compatible endpoint (`https://…`):**
- Require a keyring-stored key; send `Authorization: Bearer <key>`; enforce `timeout_secs` default ~30 s; map 401/403 to "check your API key" and 429 to "rate limited, try again later" — never surface raw bodies or the key.
- Because hosted calls exfiltrate mail content by design and every failure message must stay key-free and body-free.

**If streaming is later requested (post-v1.8 or late-v1.8 UAT):**
- Use the pre-enabled `stream` feature: `Response::bytes_stream` → split `data:` lines → `{"choices":[{"delta":{"content":…}}]}` → forward chunks via a Tauri `Channel`/event to the compose window; `[DONE]` terminates.
- Because no Cargo change is needed, streaming stays a UI-plan delta, not a stack delta.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| reqwest 0.12 (`json`, `stream`, `native-tls`, no default features) | serde_json 1 (pinned), futures-util 0.3 (pinned), native-tls 0.2 + tokio-native-tls 0.3 (pinned transitive) | All three features resolve to crates already in the lockfile — verify with `cargo tree`; `ring`/`rustls` must NOT appear |
| reqwest 0.12 | tokio 1 `rt` (Tauri `async_runtime`) | reqwest 0.12 runs on Tokio; Tauri v2's async runtime is Tokio-based, so `#[tauri::command(async)]` can `.await` reqwest directly — no `spawn_blocking`, unlike the io-managesieve pattern from v1.5 research |
| keyring 3 new `sge-llm` entry | sync-secret-service backend (pinned) + existing `KeyringStore` helper | Same collection as mail creds; delete-key path must remove the entry (settings "clear key" button) so stale keys don't linger |
| `llm_settings` migration (M-next) | rusqlite_migration 2.x + existing migrations chain | Single-row table with `CHECK` on ranges (temperature 0–2, max_tokens > 0, timeout 5–300); base_url/model/template `NOT NULL`; never store the key here — add a code comment + a test asserting no `api_key` column |

## Tauri Integration Points (for roadmap)

- New commands: `get_llm_settings` (returns non-secret row only), `save_llm_settings` (validates https-or-loopback URL, model non-empty, template contains `{body}`), `save_llm_api_key` / `delete_llm_api_key` (keyring write/delete), `generate_reply(message_id)` — reads settings + key server-side, renders template from the SQLite message (sender/subject/body-plaintext), POSTs, returns `{ text }` or a plain-language error. Frontend never sees the key or base URL secret material.
- `generate_reply` is called ONLY from the Reply/Reply-All compose-window button (v1.8 scope guard); result text is prepended above the existing quoted original and flows out through the untouched `queue_send`/lettre pipeline — async-imap and lettre are explicitly out of scope for this milestone.
- Privacy/logging contract (new code, existing ethos): no `log`/println of key, prompt body, or response body — log only `model`, `status_code`, `latency_ms`, and error kind.

## Sources

- `src-tauri/Cargo.toml` (pinned: reqwest 0.12 default-features=false, serde/serde_json 1, keyring 3 + sync-secret-service, rusqlite 0.37 bundled, tokio rt/sync/time, zeroize, thiserror, lettre 0.11 untouched) — HIGH, project ground truth
- docs.rs `reqwest 0.12` crate page (0.12.28 verified; 0.13.5 latest as of 2026-09-08; optional deps confirm `json`→serde_json, `stream`→futures-util/wasm-streams, `native-tls`→native-tls 0.2/hyper-tls/tokio-native-tls) — HIGH
- `src-tauri/src/lib.rs` + `live_gate_*.rs` grep (KeyringStore save/load pattern, keyring-first auth, never-log-secrets ethos) — HIGH, existing integration surface
- `.planning/PROJECT.md` v1.8 scope (OpenAI-compatible base URL + model + key in keyring, single `{sender}/{subject}/{body}` template, user-triggered only, mock transport in CI) — HIGH

---
*Stack research for: SGE v1.8 LLM-assisted replies (OpenAI-compatible chat completions)*
*Researched: 2026-10-15*
