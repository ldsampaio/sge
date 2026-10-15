# Phase 39: LLM Settings Foundation - Research

**Researched:** 2026-10-15
**Domain:** Rust backend — SQLite settings store + OS keyring secret + prompt renderer + Tauri IPC commands (no network egress)
**Confidence:** HIGH

## Summary

Phase 39 builds the settings foundation for v1.8 LLM replies: a validated `llm_settings` single-row SQLite table (M23 forward migration), a separate OS-keyring entry for the API key, a fenced prompt renderer, and Tauri settings commands — with **zero network egress** (no `reqwest::Client` construction, no `.send()`; generation lands in Phase 40). All building blocks already exist in-repo: `creds.rs` separate-entry precedent (`KEYRING_SERVER_CFG`), `classify_settings` key-value precedent, `M12` single-row `CHECK (id = 1)` precedent, `regex` + `ammonia 4` already pinned, and `reqwest 0.12.28` in the lockfile with `default-features = false`.

The two load-bearing verifications were done against local sources this session: (1) `reqwest 0.12.28`'s `native-tls` feature resolves to `hyper-tls + native-tls-crate + tokio-native-tls` only — no `rustls`/`ring` anywhere in that feature closure; (2) `reqwest::Url` is an unconditional re-export (`pub use url::Url`), so base-URL parsing needs zero new dependencies. The critical API pitfall found: `ammonia::clean_text` is an HTML **escaper** (like `htmlspecialchars`), not a tag stripper — the renderer must use `Builder` with an empty tag set instead.

**Primary recommendation:** New `src-tauri/src/llm/` lane module (`settings.rs` types + `validate.rs` + `prompt.rs` renderer + keyring key helpers reusing `creds.rs` patterns), M23 single-row table seeded with defaults in-migration, `reqwest::Url`-based base-URL policy, regex hidden-element pre-strip + empty-tag ammonia flatten for the renderer, Tauri commands returning `has_api_key: bool` only.

## User Constraints (from CONTEXT.md)

### Locked Decisions
- M23 single-row `llm_settings` table via forward migration (non-secrets only); every prior migration stays untouched
- Sane research defaults on fresh install (OpenAI-class endpoint + model default, temperature 0.7, max tokens 500, timeout 60s; empty key; template with `{sender}`/`{subject}`/`{body}`)
- API key in a SEPARATE new keyring entry (e.g. `sge-llm-api-key`) reusing Phase 5 `creds.rs` helpers — never packed with mail creds
- Key presence exposed as presence-only boolean; the key itself is never returned over IPC, never logged
- Reset-to-default restores the prompt template text only; URL/model/key/defaults the user set are kept
- Base-URL policy: https-only with http-loopback exception (`localhost`/`127.0.0.1`/`[::1]` for Ollama-class servers); anything else rejected at save time with a plain-language reason
- Clamp temperature + max tokens (+ URL/model presence) at save time; TIMEOUT left unclamped per user call ("clamp all ranges but timeout") — planner decides timeout handling (validate-only, no upper clamp)
- Empty model rejected; each rejection names the field and the allowed range
- Validation errors are field-named plain-language reasons; metadata-only — key and mail bodies never appear in errors or logs
- Tauri settings commands ship IN THIS PHASE per user decision ("Commands now") — planner must reconcile with the roadmap's no-IPC note and the Phase 40 single-egress chokepoint (commands present but generation egress stays gated until Phase 40)
- Strict placeholder set: exactly `{sender}`, `{subject}`, `{body}` substituted; unknown `{tokens}` left literal so typos stay visible
- Body wrapped in an explicit data fence; invisible HTML stripped pre-render (`hidden-div` class, `display:none`) so injected instructions become inert text
- Body truncation ~8k chars pre-render with a `[truncated …]` disclosure marker so the model knows input was cut
- Phase 39 verification is Rust unit tests only (validation, fencing, truncation, keyring presence-bool) — no live endpoint calls in this phase; `cargo tree` gate (no `rustls`/`ring`) applies after reqwest feature widening to `json`/`stream`/`native-tls`

### the agent's Discretion
- Timeout handling shape (validate-only vs soft guidance) — user exempted timeout from clamping, details at planner's discretion
- Exact M23 DDL column names/types, keyring account string, default template wording — follow codebase conventions (`store/mod.rs` M-pattern, `creds.rs` entry pattern)

### Deferred Ideas (OUT OF SCOPE)
- Streaming generation output — deferred to v1.8.x (REQUIREMENTS.md trigger: latency complaints)
- Per-draft instruction override / multi-prompt library — deferred to v2+
- Timeout clamp values — user exempted timeout from clamping in this phase; revisit only if Phase 40 needs a bound for the cancel/single-flight design

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| LLM-01 | Configure LLM base URL + model name in settings | M23 table columns + `reqwest::Url` https/loopback validation policy + `get/save` commands |
| LLM-02 | API key in OS keyring separate entry, never in SQLite/logs/IPC | `entry_for`-pattern keyring helper (`sge-llm-api-key`), presence-bool payload, spawn_blocking rule |
| LLM-03 | Editable single prompt template `{sender}`/`{subject}`/`{body}` + default + reset | Renderer contract (exact-3 substitution, fence, hidden-strip, ~8k truncation) + reset-template-only command |
| LLM-04 | Generation defaults (temp/max-tokens/timeout) with sane defaults + clamped ranges | Save-time clamp table (temp/max-tokens; timeout validate-only), `thiserror` field-named errors |

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Settings validation | Backend (Rust `llm/` lane) | — | Never trust frontend strings; same posture as `connect_account` host validation in `lib.rs` |
| Non-secret persistence | Backend (SQLite M23) | — | Single-writer invariant (`Arc<Mutex<Store>>`, SQL in `queries.rs`) |
| API-key custody | OS keyring (Secret Service) | — | Secrets never touch SQLite/files/logs/IPC responses per REQUIREMENTS.md |
| Prompt rendering | Backend (pure function) | — | Phase 40 calls it server-side; keeps mail bodies out of the frontend |
| Settings CRUD IPC | Backend Tauri commands | — | Commands present but perform zero network I/O; egress stays gated until Phase 40 |

## Standard Stack

### Core (no new packages — widen + reuse only)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| reqwest (features `json`, `stream`, `native-tls`, `default-features = false`) | 0.12.28 (locked) | `reqwest::Url` for base-URL parse/validate; `Client` later in Phase 40 | Already pinned; `url = "2.4"` is a non-optional reqwest dep so `Url` costs zero new crates [VERIFIED: reqwest 0.12.28 Cargo.toml + local registry src/lib.rs:280 `pub use url::Url;`] |
| rusqlite + rusqlite_migration | 0.37 bundled / 2.x (pinned) | M23 `llm_settings` table | Established M-pattern; `Store::open_in_memory` for unit tests [VERIFIED: src-tauri/src/store/mod.rs:79-108] |
| keyring + sync-secret-service | 3 (pinned) | Separate `sge-llm-api-key` entry | `entry_for` + `KEYRING_SERVER_CFG` separate-entry precedent [VERIFIED: src-tauri/src/creds.rs:83,125-129] |
| ammonia | 4.2.1 (locked) | Flatten body HTML to inert plain text | Already dep; `Builder::tags()` setter confirmed [VERIFIED: local ammonia-4 source `pub fn tags`] |
| regex | 1.x (pinned) | Hidden-element pre-strip (`display:none`, `hidden-div`) | Already dep (Phase 16 pin) |
| thiserror | 2 (pinned) | `LlmSettingsError` field-named plain-language errors | `CredsError`/`StoreError` precedent [VERIFIED: src-tauri/src/creds.rs:20-28] |

**Cargo.toml change (only dependency change in this phase):**
```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "native-tls"] }
```
Feature-closure audit from the authoritative manifest [CITED: https://github.com/seanmonstar/reqwest/blob/v0.12.28/Cargo.toml]: `native-tls = ["default-tls"]` → `default-tls = ["dep:hyper-tls", "dep:native-tls-crate", "__tls", "dep:tokio-native-tls"]` — pulls **no** `rustls`/`ring` (all `rustls-tls-*` features are separate and stay off). `json` adds only `serde_json`; `stream` adds `futures-util` + `tokio-util` + `tokio/fs`. With `default-features = false`, `http2`/`charset`/`system-proxy` stay off (note: `http2` is the only default feature that references `hyper-rustls`, and only as an optional `?` dep — off here).

**Post-change gate:** `cargo tree -i rustls -i ring` must print nothing (expect `error: nothing to print`); `cargo tree | grep -E 'hyper-tls|native-tls|openssl'` must show the native chain (`native-tls 0.2.18` + `openssl 0.10.81` already in lock [VERIFIED: src-tauri/Cargo.lock]).

### Supporting
| Library | Purpose | When to Use |
|---------|---------|-------------|
| `imap::is_loopback` (`src-tauri/src/imap/mod.rs:80-83`) | Loopback allowlist precedent (`localhost`, `127.0.0.1`, `::1`, bracket-tolerant) | Reuse the exact semantics for the http-loopback exception (extract to shared helper or mirror narrowly) |
| `classify_settings` get/set (`store/queries.rs:2369-2381`) | Key-value settings read/write precedent | Reference only — M23 uses a dedicated single-row table per locked decision |
| `MemoryStore` (`creds.rs:169-179`) | Keyring mock where Secret Service is absent | Unit tests: one `MemoryStore` instance per secret slot |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `reqwest::Url` re-export | Direct `url = "2"` dependency | Direct dep violates the "zero new core dependencies" letter (even though `url 2.5.8` is already in lock); re-export is free. Fallback only if re-export path breaks. |
| Regex hidden-element strip | Full DOM pass with html5ever | html5ever is ammonia-transitive, not direct; adding it as direct dep breaks zero-new-deps. Regex scoped to the two CONTEXT-named markers is proportionate; document fragility. |
| Seed defaults in M23 SQL | Lazy-seed on first read in `queries.rs` | In-migration `INSERT` gives fresh-install defaults atomically with schema; lazy-seed splits default definitions across two places. Prefer in-migration seed. |

## Package Legitimacy Audit

No new packages installed in this phase. `reqwest` widening only enables already-vetted optional features of the pinned `0.12.28` (locked, MIT/Apache, established crate). `url 2.5.8`, `native-tls 0.2.18`, `openssl 0.10.81` already present in `src-tauri/Cargo.lock` [VERIFIED: Cargo.lock grep].

| Package | Registry | Verdict | Disposition |
|---------|----------|---------|-------------|
| reqwest 0.12.28 (feature widening) | crates.io (locked) | OK | Approved — feature closure audited above, no rustls/ring |

**Packages removed due to SLOP:** none. **Flagged SUS:** none.

## Architecture Patterns

### System Architecture Diagram

```
Settings UI (Phase 41, not this phase)
        │ Tauri IPC (serde JSON, secrets never in responses)
        ▼
┌─ Tauri commands (THIS PHASE, zero network I/O) ──────────────┐
│ get_llm_settings → LlmSettingsView { …, has_api_key: bool }  │
│ save_llm_settings(input incl. optional api_key)              │
│   ├─ validate() → field-named LlmSettingsError on failure    │
│   ├─ SQLite write (non-secrets) via store Mutex              │
│   └─ keyring write (api_key only) via spawn_blocking         │
│ reset_llm_template / clear_llm_api_key                       │
└──────────────────────────────────────────────────────────────┘
        │                    │
        ▼                    ▼
  SQLite M23            OS keyring (service `sge`)
  llm_settings          ├─ sge-default-account (mail, untouched)
  (single row,          ├─ sge-server-cfg (precedent, untouched)
   id=1, no secrets)    └─ sge-llm-api-key (NEW, plain string)

Phase 40 consumes: validated-settings read path + render_prompt() → single-egress generate_reply
```

### Recommended Project Structure
```
src-tauri/src/
├── llm/
│   ├── mod.rs          # lane root: LlmSettings struct, DEFAULT_* consts, DEFAULT_TEMPLATE
│   ├── validate.rs     # LlmSettingsError (thiserror) + validate() policy
│   ├── prompt.rs       # render_prompt(): hidden-strip → flatten → truncate → fence → substitute
│   └── key.rs          # OPTIONAL thin wrapper: llm keyring entry_for helpers (or fold into creds.rs)
├── store/
│   ├── mod.rs          # + M23_SQL const, register M::up(M23_SQL); fix stale SCHEMA_VERSION
│   └── queries.rs      # get_llm_settings/upsert_llm_settings (all SQL lives here)
└── lib.rs              # + 3–4 #[tauri::command] fns + generate_handler! entries
```

### Pattern 1: Single-row settings table with in-migration seed
**What:** `CREATE TABLE llm_settings (id INTEGER PRIMARY KEY CHECK (id = 1), …)` + `INSERT INTO llm_settings (id, …) VALUES (1, …defaults…)` in the same M23 SQL string. Upserts use `INSERT … ON CONFLICT(id) DO UPDATE`.
**When to use:** Exactly this phase (locked decision).
**Precedent:** `taxonomy (id INTEGER PRIMARY KEY CHECK (id = 1), …)` in M12 [VERIFIED: src-tauri/src/store/mod.rs:377-384]. Note `classify_settings` instead uses key-value rows — M23 deliberately does NOT follow that; it follows the `taxonomy` single-row shape.
**Example:**
```rust
// Source: store/mod.rs M12 pattern (taxonomy), adapted
const M23_LLM_SETTINGS_SQL: &str = concat!(
    "CREATE TABLE llm_settings (",
    "  id             INTEGER PRIMARY KEY CHECK (id = 1),",
    "  base_url       TEXT NOT NULL DEFAULT '',",
    "  model          TEXT NOT NULL DEFAULT '',",
    "  temperature    REAL NOT NULL DEFAULT 0.7,",
    "  max_tokens     INTEGER NOT NULL DEFAULT 500,",
    "  timeout_secs   INTEGER NOT NULL DEFAULT 60,",
    "  prompt_template TEXT NOT NULL DEFAULT '',",
    "  updated_at     TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
    "INSERT INTO llm_settings (id, base_url, model, temperature, max_tokens, timeout_secs, prompt_template)",
    " VALUES (1, 'https://api.openai.com/v1', '<default-model>', 0.7, 500, 60, '<default-template>');",
);
```

### Pattern 2: Base-URL validation via reqwest::Url
**What:** `reqwest::Url::parse(trimmed)` → scheme must be `https`, OR `http` with loopback host (`localhost`/`127.0.0.1`/`[::1]` per `imap::is_loopback` semantics). Normalize stored value: trimmed, trailing `/` stripped (so Phase 40 can safely append `/chat/completions`).
**When to use:** Save-time validation of LLM-01.
**Example:**
```rust
// Source: reqwest 0.12.28 src/lib.rs:280 `pub use url::Url;` (unconditional re-export)
let url = reqwest::Url::parse(base_url.trim())
    .map_err(|_| LlmSettingsError::InvalidBaseUrl("base URL must look like https://…".into()))?;
match url.scheme() {
    "https" => {}
    "http" if is_loopback_host(url.host_str().unwrap_or("")) => {}
    _ => return Err(LlmSettingsError::InvalidBaseUrl(
        "base URL must use https, except http://localhost for local servers".into())),
}
```

### Pattern 3: Prompt renderer pipeline (pure function, unit-tested)
**What:** `render_prompt(template, sender, subject, body_html) -> String`: (1) regex-strip hidden elements **with their text content**; (2) flatten remaining HTML to plain text with `ammonia::Builder` + **empty tag set** (disallowed tags are stripped, text kept; `script`/`style` are in ammonia's `clean_content_tags` so their content is dropped [VERIFIED: ammonia-4 source line 389]); (3) char-boundary truncate to ~8000 chars + `\n[truncated …]` marker; (4) substitute **only** `{sender}`/`{subject}`/`{body}` via three literal `.replace()` calls (unknown `{tokens}` pass through untouched by construction); (5) wrap body in an explicit fence naming it untrusted data.
**When to use:** Every render; Phase 40 calls this before building the chat-completions payload.
**Example:**
```rust
// Sources: ammonia 4.2 docs (Builder::tags setter); regex crate (already pinned)
const BODY_CHAR_LIMIT: usize = 8000; // [ASSUMED] value — CONTEXT says "~8k"
fn strip_hidden(html: &str) -> String {
    // Case-insensitive; covers CONTEXT-named markers: hidden-div class + display:none.
    // Fragile by nature (regex over HTML) — scoped narrowly on purpose; see Pitfalls.
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(
        r"(?is)<([a-z][a-z0-9]*)[^>]*(?:display\s*:\s*none|hidden-div)[^>]*>.*?</\1\s*>"
    ).unwrap());
    RE.replace_all(html, "").into_owned()
}
fn flatten(html: &str) -> String {
    // Empty tag set ⇒ all tags stripped, inner text kept; script/style content dropped.
    ammonia::Builder::new().tags(std::collections::HashSet::new()).clean(html).to_string()
}
```

### Anti-Patterns to Avoid
- **`ammonia::clean_text` for body flattening:** it HTML-*escapes* (`<` → `&lt;`), producing entity soup in the prompt instead of readable text [VERIFIED: ammonia-4 source `fn clean_text` + docs "roughly equivalent to PHP's htmlspecialchars"]. Use empty-tag `Builder`.
- **Default `ammonia::Builder::new().clean()` for the hidden-strip step:** default settings strip the *tags/attributes* but **keep inner text** — a `display:none` injection survives as live text inside the fence. The regex content-removal pre-pass is what makes it inert-by-removal. (Remaining visible text is fenced-as-data, which is the second layer.)
- **Generic template engines (`format!`, `strfmt`, handlebars):** unknown `{token}` typos would error or vanish; three literal `.replace()` calls leave them visible per locked decision.
- **Storing the key alongside settings** (JSON blob with mail creds, or a SQLite column): violates LLM-02; separate keyring entry, plain-string `set_password`, never logged/returned.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| URL parsing (IPv6, ports, userinfo, scheme) | String prefix/split checks | `reqwest::Url::parse` (unconditional re-export, zero new deps) | Hand-rolled parsing misclassifies `[::1]`/embedded ports; `Url` is already in the tree |
| HTML→text flattening | Regex tag stripper | ammonia empty-tag `Builder` (html5ever-grade parsing) | Regex breaks on `<` in text, comments, malformed tags — the exact obfuscation vector |
| Migration versioning | Manual `PRAGMA user_version` SQL | `rusqlite_migration::Migrations` M-chain | Forward-only + preserve-rows guarantees; baseline `schema.sql` never edited |
| Keyring I/O threading | Direct calls in async commands | `tauri::async_runtime::spawn_blocking` | Keyring blocks the calling thread; existing hard rule [VERIFIED: lib.rs:161,190,215,226] |

**Key insight:** Every primitive this phase needs is already a pinned dependency — the phase is pure composition. Any new crate in a plan is a red flag against the zero-new-deps constraint.

## Common Pitfalls

### Pitfall 1: `clean_text` ≠ tag stripper
**What goes wrong:** Prompt fills with `&lt;div&gt;` entity soup.
**Why it happens:** Name suggests "clean to text"; it actually escapes for HTML embedding [VERIFIED this session].
**How to avoid:** Empty-tag `Builder`; unit test asserts no `<`, `>`, `&lt;` in flattened output.

### Pitfall 2: Hidden-injection text survives default sanitization
**What goes wrong:** `display:none` instruction text reaches the model inside the fence (fenced but present; weaker posture than CONTEXT demands).
**Why it happens:** Ammonia keeps inner text of disallowed elements by design.
**How to avoid:** Regex content-removal pre-pass for the two CONTEXT markers; unit tests with `hidden-div` + `display:none` + uppercase/`&nbsp;`-style variants.

### Pitfall 3: Secret leaks through Debug/Display/logs
**What goes wrong:** API key in logs, error strings, or IPC payloads.
**Why it happens:** `#[derive(Debug)]` on a settings struct holding the key; `thiserror` messages interpolating values; returning the full row over IPC.
**How to avoid:** Key NEVER enters the SQLite struct — separate `LlmSettings` (non-secrets) vs key handled as a transient `String`/`Zeroizing<String>`; IPC view struct carries `has_api_key: bool` only; error messages name fields + ranges, never values of secrets (key and mail bodies never appear [locked]).

### Pitfall 4: Stale SCHEMA_VERSION const + test
**What goes wrong:** `SCHEMA_VERSION: u32 = 14` while the chain is M1+M2..M22 (22 applied migrations); a plan trusting the const miscounts.
**Why it happens:** Const not updated since early phases; only referenced by its own assert [VERIFIED: grep shows `SCHEMA_VERSION` at store/mod.rs:19 and test :830 only].
**How to avoid:** Planner sets const to 23 with M23 and updates the `schema_version_is_14_with_m22_learn` assert; or deletes the const if the team agrees it's vestigial (planner's call — flag in plan).

### Pitfall 5: First build after widening needs network
**What goes wrong:** `cargo build`/`test` fails offline — `hyper-tls`, `tokio-native-tls` are NOT in the current lockfile (only `native-tls`/`openssl` via async-native-tls are).
**Why it happens:** New crates must be fetched from crates.io once.
**How to avoid:** Run `cargo fetch`/`cargo tree` with network before gating offline execution; lockfile update is expected and must be committed.

### Pitfall 6: Timeout unclamped → Phase 40 must self-protect
**What goes wrong:** Absurd timeout (0 or multi-hour) accepted at save; Phase 40 hangs or instant-fails.
**Why it happens:** User exempted timeout from clamping (locked).
**How to avoid:** This phase validates only sanity (positive integer; reject 0/negative with field-named error [recommended]); Phase 40 owns effective abort/cancel. Document the handoff in PLAN.md.

## Code Examples

### Save-command skeleton (existing conventions)
```rust
// Source: lib.rs save_credentials (spawn_blocking) + connect_account (validate-first) patterns
#[tauri::command]
async fn save_llm_settings(
    state: tauri::State<'_, AppState>,
    input: LlmSettingsInput, // { base_url, model, temperature, max_tokens, timeout_secs, prompt_template, api_key: Option<String> }
) -> Result<LlmSettingsView, String> {
    let validated = llm::validate::validate(&input).map_err(|e| e.to_string())?;
    if let Some(key) = input.api_key {
        // Empty-string semantics: planner's call (recommend: separate clear command; None = leave unchanged).
        tauri::async_runtime::spawn_blocking(move || llm::key::save_llm_api_key(&key))
            .await.map_err(|e| format!("internal error: keyring task failed ({e})"))?
            .map_err(|e| e.to_string())?;
    }
    let guard = state.store.lock().unwrap();
    crate::store::queries::upsert_llm_settings(guard.conn(), &validated).map_err(|e| e.to_string())?;
    Ok(LlmSettingsView { has_api_key: llm::key::has_llm_api_key(), ..validated.into() })
}
```

### Keyring entry helper (separate-entry precedent)
```rust
// Source: creds.rs KEYRING_SERVER_CFG pattern (const + entry_for)
const KEYRING_LLM_API_KEY: &str = "sge-llm-api-key"; // [ASSUMED] exact string — planner confirms
fn llm_entry() -> Result<keyring::Entry, CredsError> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_LLM_API_KEY).map_err(unavailable)
}
// save: llm_entry()?.set_password(&key) | load: match get_password() { NoEntry → Ok(None) } | presence: Ok(load()?.is_some())
// Tests use a second MemoryStore instance (one slot per instance) — no Secret Service needed.
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| reqwest 0.12 `default-tls` = native-tls | reqwest 0.13 defaults to rustls | reqwest 0.13 (2025+ per ecosystem reports) | Stay on the 0.12 line — the native-tls-only posture is trivially satisfiable here; upgrading would fight the `cargo tree` gate |

**Deprecated/outdated:** `trust-dns` feature name (now `hickory-dns`) — irrelevant (neither enabled).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Default base URL `https://api.openai.com/v1` + OpenAI model default (exact model string at planner's discretion, e.g. `gpt-4o-mini`) | Standard Stack / M23 seed | Wrong default is cosmetic (user-editable) but should match "OpenAI-class" intent — confirm in discuss/plan-check |
| A2 | Temperature clamp `0.0–2.0` (OpenAI range), max-tokens clamp lower bound 1 with planner-chosen upper bound | Validation | Ranges should mirror provider docs; plan-check should confirm upper bound for max-tokens |
| A3 | Body truncation 8000 chars + `\n[truncated …]` marker | Renderer | CONTEXT says "~8k"; exact constant/marker wording is planner's call |
| A4 | Keyring account string `sge-llm-api-key` | Keyring helper | Any stable unique string works; must never change once shipped (entries persist across upgrades) |
| A5 | Empty-tag `Builder` keeps inner text / drops `script`+`style` content (standard ammonia semantics) | Renderer | If behavior differs, unit tests catch it immediately — tests are the backstop |
| A6 | `save_llm_settings(api_key: None)` = leave key unchanged; explicit clear via separate command | Commands | Planner must define this explicitly; wrong choice confuses reset-vs-clear UX in Phase 41 |

## Open Questions

1. **Empty-string `api_key` semantics in `save_llm_settings`**
   - What we know: CONTEXT locks reset-to-default = template only; key lifecycle otherwise unspecified.
   - Recommendation: `None` = unchanged, `Some(non-empty)` = overwrite, dedicated `clear_llm_api_key` command for removal; `Some("")` rejected as invalid. Planner decides, Phase 41 UI depends on it.
2. **Timeout sanity bound**
   - What we know: unclamped by user decision; recommend rejecting `0`/negative (non-positive Duration is meaningless).
   - Recommendation: validate-only (`timeout_secs >= 1`, fits `u64`→`Duration::from_secs`); no upper bound; note Phase 40 handoff.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cargo/rustc | Build + unit tests | ✓ | cargo 1.99.0 / rustc 1.99.0 [VERIFIED: shell] | — |
| crates.io network | First fetch of hyper-tls/tokio-native-tls after widening | ? | — | Run `cargo fetch` online once; commit updated lockfile |
| OS Secret Service | Live keyring path | ? (headless/CI often absent) | — | `MemoryStore` per-slot instances in unit tests (established pattern); live-keyring test `#[ignore]` like `creds.rs:249-259` |
| SQLite | M23 (bundled rusqlite) | ✓ | bundled | — |

**Missing dependencies with no fallback:** none (no live endpoint calls in this phase by locked decision).

## Validation Architecture

| Property | Value |
|----------|-------|
| Framework | `cargo test` (Rust built-in; inline `#[cfg(test)]` modules per `creds.rs`/`store/mod.rs` convention) |
| Config file | none (Cargo workspace default) |
| Quick run command | `cargo test -p sge llm` (new lane tests only) |
| Full suite command | `cargo test -p sge` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LLM-01 | https ok / http-loopback ok / http-remote + garbage rejected; stored URL normalized | unit | `cargo test -p sge llm::validate` | ❌ Wave 0 (new `llm/` lane) |
| LLM-02 | Key roundtrips via memory store; presence bool; key string absent from SQLite dump; key absent from view/errors | unit | `cargo test -p sge llm::key` | ❌ Wave 0 |
| LLM-03 | Exact-3 substitution, unknown `{token}` literal; hidden elements removed incl. content; ~8k truncation + marker; fence present | unit | `cargo test -p sge llm::prompt` | ❌ Wave 0 |
| LLM-04 | Temp/token clamps enforced, timeout unclamped-but-positive, field-named errors | unit | `cargo test -p sge llm::validate` | ❌ Wave 0 |
| M23 | Forward migration from v22 preserves rows; seed defaults present; single-row CHECK enforced | unit | `cargo test -p sge store::` | ❌ Wave 0 (extend existing migration tests) |
| Gate | `cargo tree` shows hyper-tls/native-tls, no rustls/ring | manual cmd | `cargo tree -i rustls -i ring` (expect empty) | n/a |

### Sampling Rate
- **Per task commit:** `cargo test -p sge llm`
- **Per wave merge:** `cargo test -p sge`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `src-tauri/src/llm/{mod,validate,prompt,key}.rs` — covers LLM-01..04
- [ ] `store/queries.rs::get/upsert_llm_settings` + M23 const + registration in `apply_migrations`
- [ ] Tauri commands + `generate_handler!` entries in `lib.rs`
- [ ] Fix stale `SCHEMA_VERSION` const + its assert (currently 14 vs 22 applied)

## Security Domain

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | No auth surface in this phase |
| V3 Session Management | no | No sessions |
| V4 Access Control | partial | Tauri IPC is local-only; no per-command ACL — commands mutate only the user's own settings |
| V5 Input Validation | yes | `reqwest::Url` parse + scheme/host allowlist; numeric clamps; template is data (never executed) |
| V6 Cryptography | yes | Key in OS keyring only (`sync-secret-service`); TLS via platform native-tls (no custom crypto); never log/return key |

### Known Threat Patterns for this stack
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via hidden HTML in mail body | Tampering / Elevation | Hidden-content strip + full flatten-to-text + explicit untrusted-data fence (defense in depth; Phase 40 must still send fenced prompt, never raw HTML) |
| Key exfiltration via logs/IPC/SQLite | Information Disclosure | Key isolated from settings struct; presence-bool IPC; `NoEntry`-tolerant loads; unit test greps DB dump for key material |
| SSRF-adjacent misconfig (file://, gopher, intranet http) | Tampering | Scheme allowlist (https + http-loopback only); no other schemes parse-passing validation |
| Downgrade to plaintext remote endpoint | Information Disclosure | http allowed ONLY for loopback hosts; everything else rejected at save with plain-language reason |

## Sources

### Primary (HIGH confidence)
- reqwest 0.12.28 `Cargo.toml` (authoritative manifest: `native-tls = ["default-tls"]`, unconditional `url = "2.4"` dep) + local registry `src/lib.rs:280` (`pub use url::Url;`)
- docs.rs reqwest 0.12.28 feature flags (cross-checked against manifest)
- docs.rs ammonia 4.2.3 (`clean_text` = escaper; `Builder::tags` setter) + local ammonia-4 source (`clean_content_tags`, `fn clean_text`)
- In-repo: `src-tauri/src/creds.rs:1-260`, `src-tauri/src/store/mod.rs:79-108` + M12 precedent, `src-tauri/src/lib.rs` command patterns, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (reqwest 0.12.28, url 2.5.8, native-tls 0.2.18, openssl 0.10.81; no ring/rustls)

### Secondary (MEDIUM confidence)
- `imap::is_loopback` / `normalize_host` semantics (in-repo, read this session)

### Tertiary (LOW confidence)
- None — all load-bearing claims verified above; open choices (A1–A6) explicitly marked `[ASSUMED]` where they appear

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — feature closure audited against the authoritative manifest; zero-new-deps path verified in local sources
- Architecture: HIGH — all patterns are in-repo precedents with file:line citations
- Pitfalls: HIGH — `clean_text` trap confirmed in source; remainder are direct consequences of locked decisions

**Research date:** 2026-10-15
**Valid until:** 2026-11-14 (stable domain: pinned crates + in-repo conventions)
