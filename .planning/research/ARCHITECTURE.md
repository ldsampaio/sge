# Architecture Research: LLM-Assisted Reply Drafting in SGE (Tauri v2 + React + SQLite)

**Domain:** Desktop email client — user-triggered LLM reply generation integrated into existing Tauri v2 + React + SQLite app
**Researched:** 2026-10-15
**Confidence:** HIGH

## Standard Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    React Frontend (compose windows)              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ComposeWindow │  │ LLM Settings │  │ GenerateButton       │   │
│  │(reply seed,  │  │ Panel/Menu   │  │ (explicit click only)│   │
│  │body state)   │  │ (endpoint/   │  │ loading/error/       │   │
│  └──────┬───────┘  │ model/tmpl)  │  │ insert-above-quote)  │   │
│         │          └──────┬───────┘  └──────────┬───────────┘   │
├─────────┴────────────────┴────────────────────┴───────────────┤
│                 Tauri v2 IPC boundary (invoke)                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │llm_settings  │  │generate_reply│  │ creds (keyring)      │   │
│  │get/set/test  │  │(prompt render│  │ load at call time    │   │
│  │commands      │  │+ HTTP POST)  │  │ (no caching)         │   │
│  └──────┬───────┘  └──────┬───────┘  └──────────────────────┘   │
├─────────┴────────────────┴────────────────────────────────────┤
│                    Rust Backend Services                        │
│  ┌─────────────────────────────────────────────────────┐        │
│  │  llm module: settings validate + prompt render +    │        │
│  │  reqwest client (rustls) + single-flight + timeout  │        │
│  └──────────────────────┬──────────────────────────────┘        │
├─────────────────────────┴───────────────────────────────────────┤
│                    Persistence Layer                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐    │
│  │ SQLite       │  │ OS keyring   │  │ In-memory only       │    │
│  │ llm_settings │  │ sge/llm-api  │  │ generated draft text │    │
│  │ (no secrets) │  │ key (secret) │  │ (never persisted     │    │
│  └──────────────┘  └──────────────┘  │  until user sends)   │    │
│                                       └──────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

Core verdict: **LLM HTTP call lives in the Rust backend as a new `llm` module + Tauri commands, NOT as frontend `fetch`.** Two decisive reasons: (1) the API key must never enter the WebView — frontend fetch would expose it to DevTools/DOM and require widening the CSP `connect-src` to arbitrary user URLs; (2) SGE's CSP is locked to `connect-src ipc: http://ipc.localhost` (`tauri.conf.json`), so any frontend fetch to a user-configured endpoint is blocked by default. Backend `reqwest` already exists in `Cargo.toml` (0.12, default-features off — needs a TLS feature enabled for HTTPS endpoints).

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| `src-tauri/src/llm/` (NEW) | Settings validation, prompt template rendering, OpenAI-compatible `/chat/completions` POST, timeout + single-flight, plain-language errors, never log key/body | New Rust module: `settings.rs` (validate), `prompt.rs` (render), `client.rs` (reqwest + rustls), tested with `wiremock`/`mockito` or stub transport |
| `commands::llm` (NEW) | IPC surface: `get_llm_settings`, `save_llm_settings`, `clear_llm_key`, `generate_reply`, `test_llm_connection` | Follows `commands/sync.rs` verb pattern: arg validation in Rust, `spawn_blocking` for keyring, async reqwest via `tauri::async_runtime` |
| `llm_settings` table + M23 migration (NEW) | Persist non-secret settings: base_url, model, template, temperature, max_tokens, timeout, updated_at | Forward-only `M23_LLM_SETTINGS_SQL`, `CREATE TABLE llm_settings(key TEXT PRIMARY KEY, value TEXT)` or single-row table; preserve-rows test |
| Keyring `sge/llm-api-key` entry (NEW) | Persist API key only, same pattern as `creds.rs` (`KEYRING_SERVICE="sge"`, new account id) | `KeyringStore`-style save/load/clear; loaded at call time inside `generate_reply`, never returned to frontend |
| LLM Settings UI (NEW) | Menu/panel for endpoint + model + key + template + defaults; test-connection button | New React component (e.g. `LlmSettingsPanel.tsx`) reusing settings-form idioms from `RulesPanel`/`SavedFiltersPanel` |
| Generate button in ComposeWindow (MODIFY) | Reply/reply-all only: explicit click → invoke `generate_reply` with uid+mailbox → insert result above quote → loading/error states | Modify `ComposeWindow.tsx`: new button + `generating` phase alongside `sending`/`filing`; pure string splice `generated + seed.body` |
| `lib/llm.ts` (NEW) | Frontend types + thin invoke wrappers + error mapping | Mirrors `lib/compose.ts` role: no secrets, no prompt logic (render is backend) |
| `queue_send` path (UNTOUCHED) | Generated text is ordinary body text by send time; no send-pipeline change | Explicit non-goal: proves the LLM lane composes with, not forks, the proven pipeline |

## Recommended Project Structure

```
src-tauri/src/
├── llm/                  # NEW — LLM lane (mirrors classify/ layout)
│   ├── mod.rs            # re-exports, error type LlmError (plain-language)
│   ├── settings.rs       # validation: https/http-loopback-only, model non-empty,
│   │                     # ranges for temperature/max_tokens/timeout
│   ├── prompt.rs         # template render: {sender}/{subject}/{body} substitution
│   └── client.rs         # reqwest POST to {base}/chat/completions, timeout,
│                         # single-flight gate, response extraction
├── commands/
│   ├── mod.rs            # MODIFY — add `pub mod llm;`
│   ├── llm.rs            # NEW — 5 commands (get/save/generate/test/clear)
│   ├── sync.rs           # UNTOUCHED (queue_send/send_pending/fetch_message reused)
│   └── classify.rs       # UNTOUCHED (reference pattern only)
├── store/
│   ├── mod.rs            # MODIFY — append M23 migration, bump version const
│   └── queries.rs        # MODIFY — add llm_settings get/set fns (no raw SQL elsewhere)
├── creds.rs              # MODIFY — add KEYRING_LLM_KEY account id + save/load/clear fns
└── lib.rs                # MODIFY — register 5 commands in generate_handler!

src/
├── components/
│   ├── ComposeWindow.tsx # MODIFY — Generate button + generating state + insert-above-quote
│   └── LlmSettingsPanel.tsx  # NEW — settings form (endpoint/model/key/template/defaults)
└── lib/
    ├── compose.ts        # UNTOUCHED (buildReplySeed/quoteBody reused as-is)
    └── llm.ts            # NEW — invoke wrappers + TS types for settings/generate
```

### Structure Rationale

- **`llm/` mirrors `classify/`:** the codebase already has a proven lane-module convention (`classify/worker.rs` + `ClassifyGate`, `sidecar.rs` supervisor). Reviewers navigate by analogy; plan-check expects it.
- **Settings split keyring/SQLite is a security invariant, not a preference:** `schema.sql` header states "No secrets in any table (threat T-02-02)". The API key in SQLite would violate a documented threat model. Non-secret settings in SQLite follow the `classify_settings` / `ui_state` key-value precedent (M14/M18).
- **Prompt rendering in backend:** keeps template semantics testable in Rust unit tests (no WebView needed), keeps the original body out of extra IPC round-trips (backend already holds `fetch_message` data path), and lets the privacy guard ("nothing leaves except on click") be enforced at one choke point — `generate_reply`.
- **ComposeWindow modified, not forked:** reply/reply-all seeding (`buildReplySeed`, `quoteBody`) is correct and covered; the LLM feature is a body-text prefix insertion. A second editor component would duplicate the send/attachment/filing logic (669 lines) and rot.

## Architectural Patterns

### Pattern 1: Backend-owned secret + network (Tauri CSP/CORS rationale)

**What:** All third-party HTTP with secrets goes through Rust (`reqwest`) exposed as Tauri commands; the WebView never sees keys or makes cross-origin calls.
**When to use:** Always in this app — CSP `connect-src` is `ipc:`-only and must stay that way.
**Trade-offs:** Slightly more Rust code vs. trivial `fetch`; gains: key never in DOM/DevTools, no CSP widening per user URL, timeout/abort controlled, errors sanitized before crossing IPC.

**Example:**
```rust
// commands/llm.rs — key loaded at call time, never returned to frontend
#[tauri::command]
async fn generate_reply(
    state: tauri::State<'_, AppState>,
    uid: u32,
    mailbox: String,
) -> Result<GenerateReplyResult, String> {
    let settings = load_llm_settings(&state)?;          // SQLite, no secrets
    let api_key = load_llm_key_blocking().await?;        // keyring, spawn_blocking
    let original = fetch_original_for_prompt(&state, uid, &mailbox)?; // BODY.PEEK path
    let prompt = crate::llm::prompt::render(&settings.template, &original)?; // backend render
    crate::llm::client::complete(&settings, &api_key, &prompt).await
        .map_err(|e| e.to_string()) // plain-language, key/body never interpolated
}
```

### Pattern 2: Single-flight generation gate (reuse SyncGate/ClassifyGate idiom)

**What:** An `LlmGate` (atomic bool / tokio mutex) refuses a second concurrent `generate_reply` with a friendly "already generating" error instead of stacking requests.
**When to use:** Per compose window; prevents double-click storms and spend duplication against metered endpoints.
**Trade-offs:** Minimal code (copy `ClassifyGate`); alternative (request abort) adds cancellation plumbing v1.8 does not need.

### Pattern 3: Settings-as-key-value + validate-in-Rust

**What:** Non-secret LLM settings stored like `classify_settings(key,value)`; every save re-validated backend-side (URL scheme/host, model non-empty, numeric ranges). Frontend validation is advisory only — direct `invoke` must also fail clean.
**When to use:** All settings writes, mirroring the destructive-folder-guards precedent ("enforced backend-side, not UI-only").
**Trade-offs:** Duplicated range constants FE/BE; gains: IPC boundary is untrusted by design.

**Example:**
```rust
// llm/settings.rs
pub struct LlmSettings {
    pub base_url: String,   // https://... or http://127.0.0.1|localhost only
    pub model: String,       // non-empty, trim-checked
    pub template: String,    // must contain {body}; {sender}/{subject} optional
    pub temperature: f32,    // 0.0..=2.0
    pub max_tokens: u32,     // 1..=8192 (or provider-sane cap)
    pub timeout_secs: u64,   // 5..=180
}
```

## Data Flow

### Generate Flow (the v1.8 critical path)

```
User clicks "Generate with LLM" (reply window ONLY)
    ↓
Frontend: setGenerating(true); invoke("generate_reply", { uid, mailbox })
    ↓ (IPC — no key, no body text crosses TO backend; backend re-reads both)
Backend generate_reply:
  load_llm_settings (SQLite) → validate present+complete
    ↓ (missing → "configure LLM settings first" plain-language error)
  load API key (keyring, spawn_blocking) → missing → same guidance error
    ↓
  fetch original (BODY.PEEK via existing fetch_message path — never sets \Seen)
    ↓
  render prompt: template.replace({sender},{subject},{body})
    ↓ (truncate body to sane cap, e.g. 8-12k chars, note truncation — cost/latency guard)
  reqwest POST {base_url}/chat/completions { model, messages:[{role:user,content:prompt}],
    temperature, max_tokens } with timeout + LlmGate
    ↓
  extract choices[0].message.content → trim → empty → "empty response" error
    ↓
Frontend receives { generated_text }:
  setBody(generated_text + "\n" + seed.body)   // insert ABOVE quoted original
  setGenerating(false); user reviews/edits → normal Send path (queue_send, untouched)
```

### Settings Flow

```
Settings menu → LlmSettingsPanel edits non-secrets + key field
    ↓
invoke("save_llm_settings", { base_url, model, template, temperature, max_tokens, timeout })
  → backend validates → SQLite upsert (no secret column exists — structural guarantee)
invoke key save separately (or same command split internally: settings→SQLite, key→keyring)
    ↓
"Test connection" → invoke("test_llm_connection") → GET {base}/models or minimal
  chat completion with 1 max_token → success/fail plain-language, key never echoed
```

### State Management

```
ComposeWindow body: React useState (existing) — generated text is a setBody splice.
Generating flag: new `phase` member ("generating") alongside idle/sending/filing,
  or separate boolean; disables Generate + Send while in flight (no queue interleaving).
Settings form: local component state, saved via invoke; no global store needed
  (single-account, single-window concern — matches existing per-window compose state).
Backend LlmGate: ephemeral in-memory (NOT persisted); crash-safe by construction.
```

### Key Data Flows

1. **Settings persistence:** UI → `save_llm_settings` → validate → SQLite (`llm_settings`) + keyring (`sge/llm-api-key`). Read-back via `get_llm_settings` returns everything EXCEPT the key (returns `has_key: bool`).
2. **Generation:** click → `generate_reply{uid,mailbox}` → settings+key load → BODY.PEEK original → render → POST → `{generated_text}` → splice above quote.
3. **Failure:** every backend error maps to plain-language string with no key/body interpolation (follow `SidecarError`/`CredsError` precedent); frontend shows in existing `notice` slot.

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| v1.8 (single user, reply-only) | Current design is complete. No queue, no history, no streaming needed. |
| Later (templates per folder, history) | Add `llm_history` table (prompt hash + response + ts, NO bodies unless user opts in); still backend-owned. |
| Much later (streaming, multi-provider) | `client.rs` trait (`LlmProvider`) + SSE via `tauri::async_runtime` + Channel events; provider adapters behind it. Do NOT build now. |

### Scaling Priorities

1. **First bottleneck:** User-configured endpoint latency (remote APIs 5-30 s). Fix: timeout default 60 s + progress state + retry button. Already in design.
2. **Second bottleneck:** Large thread bodies blowing context/cost. Fix: body truncation cap at render time with a UI note ("original truncated to N chars"). Cheap, do in v1.8.

## Anti-Patterns

### Anti-Pattern 1: Frontend `fetch` to the LLM endpoint

**What people do:** Call the OpenAI-compatible endpoint directly from React (key in component state, `fetch(base_url)`).
**Why it's wrong:** Key lands in WebView memory/DevTools; requires `connect-src` CSP widening to `https://*` (destroys the offline-default posture and opens exfiltration surface); CORS preflight failures vary per provider; every provider quirk must be handled in JS without the Rust test harness.
**Do this instead:** Backend `generate_reply` command; frontend only sees `{generated_text}`.

### Anti-Pattern 2: Storing the API key in SQLite or config file

**What people do:** Add an `api_key` column to `llm_settings` for convenience.
**Why it's wrong:** Violates schema.sql T-02-02 ("No secrets in any table"); SQLite file is backed up/copied casually; plaintext at rest. Also breaks the keyring-mocked test pattern.
**Do this instead:** Keyring entry `sge/llm-api-key` via `creds.rs` pattern; SQLite holds `has_key` implicitly (keyring load `Some/None`).

### Anti-Pattern 3: Auto/background generation or send-path coupling

**What people do:** Hook generation into sync arrival, or auto-send the generated text.
**Why it's wrong:** Violates the v1.8 privacy constraint (content leaves ONLY on click) and scope guard (no auto-send); couples the experimental lane to the exactly-once send state machine (`sent_unfiled`/`uncertain`), risking duplicates.
**Do this instead:** Generate button only in reply windows; generated text becomes ordinary `body` state; `queue_send` untouched.

### Anti-Pattern 4: New editor component for LLM replies

**What people do:** Build `LlmComposeWindow.tsx` duplicating ComposeWindow.
**Why it's wrong:** Duplicates send/attachment/filing/quote logic; two editors rot independently; threading-header bugs double.
**Do this instead:** ~40-line modification to `ComposeWindow.tsx` (button + generating state + splice).

### Anti-Pattern 5: Prompt rendering in frontend

**What people do:** Substitute `{body}` in JS and send the full prompt over IPC.
**Why it's wrong:** Mail body crosses IPC unnecessarily (larger attack surface for logging); template semantics untestable in Rust suite; two render implementations drift.
**Do this instead:** Send `{uid, mailbox}`; backend fetches original + renders (it already owns both).

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| OpenAI-compatible endpoint (`/chat/completions`) | Backend `reqwest` POST, Bearer key, JSON; `rustls` TLS (enable `rustls-tls` on reqwest 0.12 — currently `default-features=false` with no TLS, loopback-HTTP only) | Base URL normalize: strip trailing `/`, reject non-http(s), refuse remote `http://` (allow only loopback http for Ollama/LM Studio); validate `/models` on Test. Timeouts 5–180 s. |
| OS keyring (Secret Service) | `creds.rs` new entry, `spawn_blocking` (STACK hard rule) | Same as mail creds; `MemoryStore` equivalent for tests; key never logged — audit error strings for interpolation. |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| ComposeWindow ↔ `generate_reply` | `invoke` with `{uid, mailbox}` → `{generated_text}` | No key/body over IPC to backend; body returns only. Reuses `fetch_message` BODY.PEEK path — `\Seen` invariant preserved. |
| `commands::llm` ↔ `store::queries` | `get/set_llm_settings` fns; single-writer `Arc<Mutex<Store>>` | Follow M14/M18 key-value precedent; forward-only M23; preserve-rows test. |
| `commands::llm` ↔ `creds.rs` | `save/load/clear_llm_key` via `KeyringStore` pattern | Load at call time inside command; never cache key in `AppState` (memory-only session holds MAIL password only). |
| Generate lane ↔ send pipeline | None (deliberate) — generated text is `body` state | `queue_send`/`send_pending`/`send_row_state` untouched; threading headers from `buildReplySeed` untouched. |
| Settings UI ↔ main window | Same-window panel/menu (no new Tauri window needed) | Compose windows are separate OS windows (`compose-*` capability); settings live in main window — no cross-window events. |

## Suggested Build Order (dependency-respecting)

1. **Settings store (backend):** M23 migration + `queries` get/set + `creds.rs` key entry + unit tests (preserve-rows, roundtrip, no-secret-in-SQLite assertion). Unblocks everything.
2. **`llm` module (backend):** `settings.rs` validate + `prompt.rs` render + `client.rs` reqwest (enable TLS feature) with mock-transport tests. No IPC yet — pure logic, CI-safe.
3. **IPC surface:** `commands/llm.rs` (5 commands) + `lib.rs` registration. Test with mock endpoint; `test_llm_connection` first for manual verification.
4. **Settings UI:** `LlmSettingsPanel.tsx` + `lib/llm.ts` wrappers + menu entry. Manual UAT: save → restart → values persist, key absent from SQLite (query the db file directly).
5. **Compose integration (last):** Generate button + generating state + insert-above-quote splice in `ComposeWindow.tsx`, reply/reply-all only. UAT with mock + (user-keyed) live endpoint.

## Sources

- SGE codebase (HIGH): `src-tauri/src/lib.rs` (command registry, AppState), `src-tauri/src/creds.rs` (keyring pattern), `src-tauri/src/store/mod.rs` (M2–M22 migration precedent), `src-tauri/src/store/schema.sql` (T-02-02 no-secrets invariant), `src-tauri/Cargo.toml` (reqwest 0.12 default-features=false — TLS gap), `src-tauri/tauri.conf.json` (CSP `connect-src ipc:`), `src-tauri/src/sidecar.rs` + `commands/classify.rs` (lane-module + gate precedent), `src/components/ComposeWindow.tsx` + `src/lib/compose.ts` (reply seed/quote/threading to reuse).
- Tauri v2 docs (MEDIUM, architectural knowledge): frontend `fetch` to arbitrary hosts requires CSP `connect-src` widening + `capabilities` http allowlists; backend HTTP avoids both — standard Tauri guidance for secret-bearing calls.
- OpenAI Chat Completions API shape (MEDIUM, stable public API): `POST {base}/chat/completions` with `{model, messages, temperature, max_tokens}` → `choices[0].message.content`; shared by Ollama/LM Studio/OpenAI-compatible servers.

---
*Architecture research for: v1.8 LLM-assisted replies (Tauri v2 + React + SQLite integration)*
*Researched: 2026-10-15*
