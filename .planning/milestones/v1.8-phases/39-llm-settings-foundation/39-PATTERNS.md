# Phase 39: LLM Settings Foundation - Pattern Map

**Mapped:** 2026-10-15
**Files analyzed:** 8 (4 new, 4 modified)
**Analogs found:** 8 / 8

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `src-tauri/src/llm/mod.rs` (new lane root: `LlmSettings` struct, `DEFAULT_*` consts, `DEFAULT_TEMPLATE`) | service/types | CRUD | `src-tauri/src/creds.rs` (`SavedCredentials`/`ServerConfig` structs) | role-match |
| `src-tauri/src/llm/validate.rs` (`LlmSettingsError` + `validate()` policy) | service/validation | request-response | `src-tauri/src/imap/mod.rs` (`normalize_host`/`is_loopback` + `ImapError`) | exact |
| `src-tauri/src/llm/prompt.rs` (`render_prompt()` pipeline) | utility/transform | transform | `src-tauri/src/sync/bodies.rs` (`sanitize_html` w/ ammonia) | role-match |
| `src-tauri/src/llm/key.rs` (thin keyring wrapper, optional — or fold into `creds.rs`) | service/secret | CRUD | `src-tauri/src/creds.rs` (`KEYRING_SERVER_CFG` + `entry_for` + `save/load_server_config`) | exact |
| `src-tauri/src/store/mod.rs` (M23 const + `M::up` registration + `SCHEMA_VERSION` fix) | migration/config | CRUD | `src-tauri/src/store/mod.rs` M12 (`M12_CLASSIFY_SQL` taxonomy single-row) | exact |
| `src-tauri/src/store/queries.rs` (`get_llm_settings`/`upsert_llm_settings`) | service/persistence | CRUD | `src-tauri/src/store/queries.rs` (`get_setting`/`set_setting`, lines 2368-2386) | exact |
| `src-tauri/src/lib.rs` (3–4 `#[tauri::command]` fns + `generate_handler!` entries) | controller/route | request-response | `src-tauri/src/lib.rs` (`save_credentials`/`save_server_config`/`load_server_config`) | exact |
| `src-tauri/Cargo.toml` (reqwest feature widening only) | config | batch | `src-tauri/Cargo.toml` (lines 48-50 reqwest pin + Phase 16 regex pin comment) | exact |

## Pattern Assignments

### `src-tauri/src/llm/mod.rs` (new — service/types, CRUD)

**Analog:** `src-tauri/src/creds.rs`

**Struct + const pattern** (creds.rs lines 14-18, 66-73, 81-83):
```rust
/// Keyring service id. Never changes: saved entries must survive upgrades.
pub const KEYRING_SERVICE: &str = "sge";

/// Server config persisted alongside credentials (written by
/// `connect_account` on first successful login, read by `start_sync`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: String,
}

/// Separate keyring entry for server config (host/port/security) so
/// `start_sync` can reconnect without re-prompting the user.
const KEYRING_SERVER_CFG: &str = "sge-server-cfg";
```

**Copy from this:** serde derive block on plain-data structs (`Debug, Clone, PartialEq, Eq, Serialize, Deserialize`); module-level `pub const` with "never changes" doc comment for the keyring account string and `DEFAULT_*` consts; doc comment stating what writes vs reads each struct (Phase 39: M23 seed writes defaults, commands read). Note: `LlmSettings` must NOT derive holding the key — key stays out of the struct entirely (Pitfall 3).

---

### `src-tauri/src/llm/validate.rs` (new — validation, request-response)

**Analog:** `src-tauri/src/imap/mod.rs` (lines 73-83, 85-101) + `src-tauri/src/lib.rs` (lines 98-104)

**Loopback allowlist precedent** (imap/mod.rs lines 73-83):
```rust
/// True for loopback hosts allowed to use unencrypted IMAP.
///
/// Allowlist is deliberately narrow (fail-closed): only `localhost`,
/// `127.0.0.1`, and `::1` (bracketed or bare) pass. ...
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_matches(['[', ']']);
    h.eq_ignore_ascii_case("localhost") || h == "127.0.0.1" || h == "::1"
}
```

**Validate-first error enum precedent** (lib.rs lines 98-104):
```rust
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("invalid host: server address must not be empty")]
    InvalidHost,
    #[error("invalid port: must be in range 1-65535")]
    InvalidPort,
}
```

**Normalize-before-validate precedent** (imap/mod.rs lines 95-101):
```rust
pub fn normalize_host(raw: &str) -> Result<String, ImapError> {
    let host = raw.trim();
    if host.is_empty() {
        return Err(ImapError::Protocol {
            detail: "invalid host: server address must not be empty".to_string(),
        });
    }
```

**Copy from this:** reuse `imap::is_loopback` semantics verbatim for the http-loopback exception (extract to shared helper or mirror narrowly — RESEARCH mandates same `localhost`/`127.0.0.1`/`::1` bracket-tolerant set); `thiserror` field-named plain-language variants (`InvalidBaseUrl`, `EmptyModel`, `TemperatureOutOfRange`, …) each naming the field + allowed range, never interpolating secret values; trim-then-validate ordering with trailing-`/` strip on the stored URL so Phase 40 can append `/chat/completions`. `reqwest::Url::parse` is a free re-export (`pub use url::Url`, reqwest 0.12.28 src/lib.rs:280) — zero new deps.

---

### `src-tauri/src/llm/prompt.rs` (new — transform, pure function)

**Analog:** `src-tauri/src/sync/bodies.rs` (lines 34-41)

**Ammonia sanitize precedent** (sync/bodies.rs lines 34-41):
```rust
/// Sanitize raw HTML for safe local rendering.
///
/// Uses ammonia with default tag/attribute/protocol allow-lists —
/// strips `<script>`, `on*` handlers, `javascript:` URLs, and bare
/// `<iframe>` (the renderer adds a sandboxed iframe wrapper separately).
pub fn sanitize_html(input: &str) -> String {
    ammonia::Builder::new().clean(input).to_string()
}
```

**Copy from this:** `ammonia::Builder::new()...clean(input).to_string()` call shape, but with an **empty tag set** (`Builder::new().tags(HashSet::new())`) — NOT default allow-lists and NOT `ammonia::clean_text` (which HTML-escapes like `htmlspecialchars`, producing entity soup). Pipeline order per RESEARCH: (1) regex-strip hidden elements **with content** (`display:none` / `hidden-div`, case-insensitive — default ammonia keeps inner text, so the regex pre-pass is what removes injected instructions); (2) empty-tag flatten; (3) char-boundary truncate ~8000 + `\n[truncated …]` marker; (4) three literal `.replace()` for exactly `{sender}`/`{subject}`/`{body}` (unknown `{tokens}` pass through by construction — no generic template engine); (5) explicit untrusted-data fence around body. Pure function, unit-tested inline per `creds.rs`/`store/mod.rs` `#[cfg(test)]` convention. `regex 1` already pinned (Cargo.toml line 47).

---

### `src-tauri/src/llm/key.rs` (new, optional — service/secret, CRUD)

**Analog:** `src-tauri/src/creds.rs` (lines 101-129, 144-165)

**Separate-entry save/load precedent** (creds.rs lines 101-115, 125-129):
```rust
/// Persist server configuration (host, port, security mode).
pub fn save_server_config(&self, cfg: &ServerConfig) -> Result<(), CredsError> {
    self.entry_for(KEYRING_SERVER_CFG)?
        .set_password(&encode_server_config(cfg))
        .map_err(unavailable)
}

/// Load server configuration. Returns `None` if no entry exists.
pub fn load_server_config(&self) -> Result<Option<ServerConfig>, CredsError> {
    match self.entry_for(KEYRING_SERVER_CFG)?.get_password() {
        Ok(blob) => decode_server_config(&blob).map(Some),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(unavailable(e)),
    }
}

fn entry_for(&self, account: &str) -> Result<keyring::Entry, CredsError> {
    keyring::Entry::new(&self.service, account).map_err(|e| CredsError::StoreUnavailable {
        detail: e.to_string(),
    })
}
```

**NoEntry-tolerant clear precedent** (creds.rs lines 159-164):
```rust
fn clear(&self) -> Result<(), CredsError> {
    match self.entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(unavailable(e)),
    }
}
```

**Copy from this:** new `const KEYRING_LLM_API_KEY: &str = "sge-llm-api-key"` beside `KEYRING_SERVER_CFG`; `entry_for(KEYRING_LLM_API_KEY)` helper; plain-string `set_password(&key)` (no JSON blob — single secret, unlike `SavedCredentials`); `NoEntry → Ok(None)` on load; `NoEntry`-tolerant clear; `unavailable(e)` error mapping to friendly `StoreUnavailable`. Tests use a second `MemoryStore` instance (one slot per instance) + `#[ignore]` live-keyring test pattern (creds.rs lines 245-259). May fold into `creds.rs` instead of a separate file — planner's call; either way copy this exact block shape.

---

### `src-tauri/src/store/mod.rs` (modify — migration, CRUD)

**Analog:** `src-tauri/src/store/mod.rs` M12 (lines 377-384) + registration chain (lines 82-105)

**Single-row CHECK precedent** (mod.rs lines 377-384):
```rust
const M12_CLASSIFY_SQL: &str = concat!(
    "CREATE TABLE taxonomy (",
    "  id           INTEGER PRIMARY KEY CHECK (id = 1),",
    "  version      INTEGER NOT NULL,",
    "  name         TEXT NOT NULL DEFAULT '',",
    "  json         TEXT NOT NULL,",
    "  installed_at TEXT NOT NULL DEFAULT (datetime('now'))",
    ");",
```

**Registration precedent** (mod.rs lines 82-105):
```rust
let migrations = Migrations::new(vec![
    M::up(include_str!("schema.sql")),
    M::up(M2_FLAG_OUTBOX_SQL),
    ...
    M::up(M22_LEARN_SQL),
]);
```

**Copy from this:** `const M23_LLM_SETTINGS_SQL: &str = concat!(...)` with `id INTEGER PRIMARY KEY CHECK (id = 1)` + in-migration `INSERT ... VALUES (1, <defaults>)` seed in the same SQL string (RESEARCH Pattern 1 — prefer over lazy-seed); append `M::up(M23_LLM_SETTINGS_SQL)` after `M22_LEARN_SQL`. Forward-only: `schema.sql` baseline untouched, all prior `M{N}` consts untouched. Also fix stale `SCHEMA_VERSION: u32 = 14` (line 19) → 23 and update its assert (~line 830, `schema_version_is_14_with_m22_learn`); next free is M23. Tests via `Store::open_in_memory` (lines 73-77) + forward-migration preserve-rows pattern.

---

### `src-tauri/src/store/queries.rs` (modify — persistence, CRUD)

**Analog:** `src-tauri/src/store/queries.rs` (lines 2368-2386)

**Key-value get/set + upsert precedent** (queries.rs lines 2368-2386):
```rust
/// Read a classify setting (threshold, auto_root marker). None when unset.
pub fn get_setting(conn: &Connection, key: &str) -> StoreResult<Option<String>> {
    conn.query_row(
        "SELECT value FROM classify_settings WHERE key = ?1",
        rusqlite::params![key],
        |r| r.get(0),
    )
    .optional()
    .map_err(super::StoreError::Sql)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO classify_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}
```

**Copy from this:** all SQL lives in `queries.rs` (single-writer invariant — no raw SQL in `llm/` or `lib.rs`); `?1`-style `rusqlite::params!` bindings; `.optional()` for `query_row` miss → `Option`; `INSERT ... ON CONFLICT(id) DO UPDATE SET ...` upsert shape for `upsert_llm_settings`; `StoreResult<T>` return + `super::StoreError::Sql` mapping. Read path returns non-secret row only — key never enters the SQLite struct.

---

### `src-tauri/src/lib.rs` (modify — controller, request-response)

**Analog:** `src-tauri/src/lib.rs` (lines 186-200, 223-256, 483+)

**Keyring command + spawn_blocking precedent** (lib.rs lines 188-200):
```rust
#[tauri::command]
async fn save_server_config(host: String, port: u16, security: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        creds::KeyringStore::new().save_server_config(&creds::ServerConfig {
            host,
            port,
            security,
        })
    })
    .await
    .map_err(|e| format!("internal error: keyring task failed ({e})"))?
    .map_err(|e| e.to_string())
}
```

**Validate-first precedent** (lib.rs lines 123-142):
```rust
#[tauri::command]
async fn connect_account(...) -> Result<ConnectSummary, String> {
    let host = imap::normalize_host(&host).map_err(|e| e.to_string())?;
    if port == 0 {
        return Err(ConnectError::InvalidPort.to_string());
    }
    ...
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(imap::probe::run_probe(&cfg))
    })
```

**NoEntry-tolerant load precedent** (lib.rs lines 245-256):
```rust
#[tauri::command]
async fn load_server_config() -> Result<Option<ServerConfig>, String> {
    let stored = tauri::async_runtime::spawn_blocking(|| KeyringStore::new().load_server_config())
        .await
        .map_err(|e| format!("internal error: keyring task failed ({e})"))?;

    match stored {
        Ok(opt) => Ok(opt),
        Err(creds::CredsError::StoreUnavailable { .. }) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
```

**Copy from this:** `#[tauri::command] async fn ... -> Result<T, String>` signature; validate-first (`llm::validate::validate(&input).map_err(|e| e.to_string())?`) before any I/O; keyring writes inside `tauri::async_runtime::spawn_blocking` (hard rule, creds.rs lines 8-9) with the `internal error: keyring task failed ({e})` join-error message; SQLite write via `state.store.lock().unwrap()` → `queries::upsert_llm_settings(guard.conn(), ...)` (AppState line 61: `pub store: Arc<Mutex<Store>>`); `StoreUnavailable → Ok(None)` Degraded path for presence check; register new commands in `generate_handler![...]` (line 483+); `Result<_, String>` via `.map_err(|e| e.to_string())`. `get_llm_settings` returns `LlmSettingsView { …, has_api_key: bool }` — key never crosses IPC. Zero network I/O in this phase (no `reqwest::Client`, no `.send()`).

---

### `src-tauri/Cargo.toml` (modify — config, batch)

**Analog:** `src-tauri/Cargo.toml` (lines 45-50)

**Pinned-dep with phase comment precedent** (Cargo.toml lines 45-50):
```toml
# Phase 16 pin: regex 1.13.1 (already in lock as transitive) for the
# classification redaction sanitizer. Cached offline — no network needed.
regex = "1"
# Phase 15 pin: reqwest 0.12 line, default-features off — loopback-only plain
# HTTP health probe (no TLS needed); async via tauri::async_runtime (tokio).
reqwest = { version = "0.12", default-features = false }
```

**Copy from this:** widen in place with phase comment, no new crates:
```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "native-tls"] }
```
Keep `default-features = false` (native-tls-only posture; `rustls`/`ring` stay off). Post-change gate: `cargo tree -i rustls -i ring` must print nothing; `cargo tree | grep -E 'hyper-tls|native-tls|openssl'` shows the native chain. First build needs network (`hyper-tls`, `tokio-native-tls` not yet in lock); commit the updated lockfile.

---

## Shared Patterns

### Keyring I/O threading (hard rule)
**Source:** `src-tauri/src/creds.rs` lines 8-9; `src-tauri/src/lib.rs` lines 161, 190, 215, 226
**Apply to:** `llm/key.rs`, all new settings commands touching the keyring
```rust
//! Every keyring call blocks the calling thread: all commands run them inside
//! `spawn_blocking` (STACK hard rule), never on a runtime thread.
tauri::async_runtime::spawn_blocking(move || { /* keyring I/O here */ })
```

### Single-writer SQLite invariant
**Source:** `src-tauri/src/store/mod.rs` lines 1-6, 41-48; `src-tauri/src/lib.rs` lines 60-61
**Apply to:** `store/queries.rs` additions, all new commands
```rust
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    ...
}
// All SQL statements live in [`queries`] (single-writer invariant — no raw SQL
// escapes `queries.rs`).
let guard = state.store.lock().unwrap();
crate::store::queries::upsert_llm_settings(guard.conn(), &validated)...
```

### Plain-language thiserror enums (never leak secrets)
**Source:** `src-tauri/src/creds.rs` lines 20-28; `src-tauri/src/lib.rs` lines 98-104; `src-tauri/src/store/mod.rs` lines 28-36
**Apply to:** `llm/validate.rs` (`LlmSettingsError`)
```rust
#[derive(Debug, Error)]
pub enum CredsError {
    #[error(
        "could not access the OS keyring ({detail}) — continuing with a memory-only session is safe"
    )]
    StoreUnavailable { detail: String },
    #[error("saved credentials are unreadable ({detail})")]
    Corrupt { detail: String },
}
```
Rule: error messages name fields + allowed ranges only; key and mail bodies never appear in errors, logs, or IPC payloads.

### Unit-test convention (in-memory store + memory keyring + ignored live test)
**Source:** `src-tauri/src/creds.rs` lines 167-179, 206-259; `src-tauri/src/store/mod.rs` lines 71-77
**Apply to:** all `llm/` modules, M23 migration test
```rust
/// Open an in-memory SQLite database with migrations applied.
/// Used by unit tests that don't need persistence.
pub fn open_in_memory() -> StoreResult<Self> { ... }

/// Live Secret Service roundtrip. Ignored by default (needs an unlocked
/// login keyring); run explicitly to prove the real path:
/// `cargo test -p sge creds:: -- --ignored`.
#[test]
#[ignore]
fn live_keyring_save_load_roundtrip() { ... }
```
`MemoryStore` per-slot instances (one instance per secret slot); `cargo test -p sge llm` per-commit, full `cargo test -p sge` per wave.

### Tauri command error surface
**Source:** `src-tauri/src/lib.rs` lines 161-166, 196-199
**Apply to:** all new settings commands
```rust
.await
.map_err(|e| format!("internal error: keyring task failed ({e})"))?
.map_err(|e| e.to_string())
```
Commands return `Result<T, String>`; validation errors convert via `.to_string()` preserving the field-named plain-language text.

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| — | — | — | None. Every file in this phase has a direct in-repo precedent (see table). RESEARCH.md code sketches cover the only novel composition (renderer pipeline order, M23 DDL shape) and are grounded in the analogs above. |

## Metadata

**Analog search scope:** `src-tauri/src/` (`creds.rs`, `store/mod.rs`, `store/queries.rs`, `store/schema.sql`, `lib.rs`, `imap/mod.rs`, `sync/bodies.rs`, `Cargo.toml`)
**Files scanned:** 8 analog files (all git-tracked — verified via `git ls-files`)
**Pattern extraction date:** 2026-10-15
