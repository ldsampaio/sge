# Phase 39: LLM Settings Foundation - Context

**Gathered:** 2026-10-15
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, user-accepted per area)

<domain>
## Phase Boundary

Validated LLM settings persist securely — non-secrets in SQLite, API key in the OS keyring — with a fenced prompt renderer, independent of any UI. Covers LLM-01..04 (base URL + model, keyring key, prompt template, generation defaults). Phase 40 owns the generation client + single-egress IPC chokepoint; Phase 41 owns the settings UI + Generate button. Per user decision this phase ALSO ships the Tauri settings commands (wired/egress-gated in Phase 40/41), diverging from the roadmap's "no IPC" note.

</domain>

<decisions>
## Implementation Decisions

### Settings Store Shape
- M23 single-row `llm_settings` table via forward migration (non-secrets only); every prior migration stays untouched
- Sane research defaults on fresh install (OpenAI-class endpoint + model default, temperature 0.7, max tokens 500, timeout 60s; empty key; template with `{sender}`/`{subject}`/`{body}`)
- API key in a SEPARATE new keyring entry (e.g. `sge-llm-api-key`) reusing Phase 5 `creds.rs` helpers — never packed with mail creds
- Key presence exposed as presence-only boolean; the key itself is never returned over IPC, never logged
- Reset-to-default restores the prompt template text only; URL/model/key/defaults the user set are kept

### Validation + Security Boundaries
- Base-URL policy: https-only with http-loopback exception (`localhost`/`127.0.0.1`/`[::1]` for Ollama-class servers); anything else rejected at save time with a plain-language reason
- Clamp temperature + max tokens (+ URL/model presence) at save time; TIMEOUT left unclamped per user call ("clamp all ranges but timeout") — planner decides timeout handling (validate-only, no upper clamp)
- Empty model rejected; each rejection names the field and the allowed range
- Validation errors are field-named plain-language reasons; metadata-only — key and mail bodies never appear in errors or logs
- Tauri settings commands ship IN THIS PHASE per user decision ("Commands now") — planner must reconcile with the roadmap's no-IPC note and the Phase 40 single-egress chokepoint (commands present but generation egress stays gated until Phase 40)

### Prompt Renderer Contract
- Strict placeholder set: exactly `{sender}`, `{subject}`, `{body}` substituted; unknown `{tokens}` left literal so typos stay visible
- Body wrapped in an explicit data fence; invisible HTML stripped pre-render (`hidden-div` class, `display:none`) so injected instructions become inert text
- Body truncation ~8k chars pre-render with a `[truncated …]` disclosure marker so the model knows input was cut
- Phase 39 verification is Rust unit tests only (validation, fencing, truncation, keyring presence-bool) — no live endpoint calls in this phase; `cargo tree` gate (no `rustls`/`ring`) applies after reqwest feature widening to `json`/`stream`/`native-tls`

### the agent's Discretion
- Timeout handling shape (validate-only vs soft guidance) — user exempted timeout from clamping, details at planner's discretion
- Exact M23 DDL column names/types, keyring account string, default template wording — follow codebase conventions (`store/mod.rs` M-pattern, `creds.rs` entry pattern)

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/creds.rs` — `KeyringStore` (service `sge`), `SavedCredentials` JSON-blob pattern, `KEYRING_SERVER_CFG` separate-entry precedent, `spawn_blocking` hard rule for keyring calls
- `src-tauri/src/store/mod.rs` — `M::up(M{N}_SQL)` forward-migration chain (next free: M23; `SCHEMA_VERSION` const is stale at 14 — planner to confirm/correct), `Store::open_in_memory` for unit tests
- `src-tauri/src/store/schema.sql` — v1 baseline (untouched by forward migrations)
- `src-tauri/src/lib.rs` — existing `#[tauri::command]` + `generate_handler!` registration pattern for the new settings commands
- `src-tauri/Cargo.toml` — reqwest 0.12 `default-features = false` (currently no `json`/`stream`/`native-tls`); widen features per roadmap, keep native-tls-only

### Established Patterns
- Single-writer SQLite invariant (`Arc<Mutex<Store>>` at Tauri State layer; all SQL in `queries.rs`)
- Forward-only migrations, baseline text never edited
- Keyring calls inside `spawn_blocking`, never on a runtime thread; `MemoryStore` for tests where Secret Service is absent
- Plain-language error enums via `thiserror` (see `CredsError`, `StoreError`)

### Integration Points
- New `llm` backend lane module (roadmap constraint) + settings commands registered in `lib.rs`
- Phase 40 consumes: validated-settings read path + prompt render for `generate_reply` / `test_llm_connection`
- Phase 41 consumes: settings-section UI + template reset + Ollama preset over these commands

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches within the decisions above.

</specifics>

<deferred>
## Deferred Ideas

- Streaming generation output — deferred to v1.8.x (REQUIREMENTS.md trigger: latency complaints)
- Per-draft instruction override / multi-prompt library — deferred to v2+
- Timeout clamp values — user exempted timeout from clamping in this phase; revisit only if Phase 40 needs a bound for the cancel/single-flight design

</deferred>
