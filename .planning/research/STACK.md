# Stack Research

**Domain:** Rust + Tauri v2 + React + SQLite Linux desktop IMAP email client (Gmail-like, headers-first sync, OS keyring, INBOX-only read-only M1)
**Researched:** 2026-10-03
**Confidence:** HIGH for core pins (verified against crates.io/docs.rs/Tauri official release pages Sep 2026); MEDIUM for IMAP runtime-bridging pattern and frontend library minors.

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `tauri` (Rust backend + bundler) | `2.12.0` (pin `tauri = "2"`, `@tauri-apps/api = "^2.12"`, `tauri-cli = "2.12"`) | App runtime, `#[tauri::command]` IPC, Linux bundling (.deb/.AppImage) | Current stable line per Tauri official release page (2.12.0, Sep 26 2026). v2 is the only line receiving fixes; v1 is legacy. Capability-based permission model is required for least-privilege IPC (only expose the mail commands the UI needs). |
| React + TypeScript + Vite | React `19`, TS `~5.6`, Vite `6/7` (via `create-tauri-app` template) | Mailbox UI (sidebar, message list, reading pane) | Tauri's blessed frontend path; Vite dev-server + HMR is what `tauri dev` expects. React 19 + TS gives the component/typing discipline a three-pane mail UI needs. Scaffolding via `create-tauri-app 4.7.4` avoids hand-wiring `tauri.conf.json`. |
| `rusqlite` (direct, in Rust backend) | `0.37` with `bundled` feature | Local mail store: accounts, headers, bodies, sync state | The sync engine lives in Rust, so SQL must live in Rust too — going through a JS SQLite plugin would push every query over IPC for no benefit. `bundled` compiles a known SQLite (3.5x) statically, so behavior (notably FTS5) is identical on every target distro. `^0.37` is the same major the Tauri rusqlite plugin ecosystem targets, so no duplicate SQLite linkage surprises. Single-writer `Arc<Mutex<Connection>>` is enough for M1; add `r2d2-sqlite` pooling only if contention appears. Enable WAL mode (`PRAGMA journal_mode=WAL`) from day one for concurrent reader (UI list) + writer (sync) access. |
| `rusqlite_migration` | `2.x` | Schema migrations | Same crate the `tauri-plugin-rusqlite2` stack uses (`rusqlite_migration ^2` per its docs.rs dependency list). Versioned `Migrations::new(...)` applied at startup; M1 ships v1 schema, later milestones (folders, send-queue) become v2/v3. |
| `async-imap` | `0.11.x` (`cargo add async-imap` resolves latest 0.11) | IMAP session: SELECT INBOX, UID FETCH headers, on-demand body FETCH | The maintained async IMAP client; shares the battle-tested `imap-proto` parser lineage with the sync `imap` crate but is actively released (0.11 line current). Async fits Tauri's tokio runtime and lets header-fetch fan out without one-thread-per-connection. Generic over any `futures-io` stream, so both implicit-TLS and STARTTLS transports plug in. |
| `async-native-tls` (+ `async-std` with `attributes`/`tokio02`-free config) | `async-native-tls 0.5`, `async-std 1.13` | TLS transport under async-imap; async executor for the sync worker | Two deliberate choices: (1) **native-tls over rustls** on Linux because it uses the system CA store — a university server like `mail.utfpr.edu.br` behind institutional CAs/proxies validates without shipping custom roots; (2) **isolate the sync worker on its own executor**: run `async_std::task::block_on` inside a dedicated `std::thread` sync worker rather than mixing async-std futures into Tauri's tokio runtime. Commands talk to the worker via `tokio::sync::mpsc` / `Mutex<SyncState>` and progress flows back via Tauri `emit` events. Zero runtime interference, trivially testable, and the IDLE loop (post-M1) gets its own long-lived home. Bridge alternative if tokio-direct is preferred: `async-compat` shim around `tokio::net::TcpStream` — acceptable but adds a dependency to avoid; the dedicated-thread pattern is simpler to reason about. |
| `mail-parser` (+ `encoding_rs` via `full_encoding`) | `0.11.9` (pin `mail-parser = "0.11"`, features `["full_encoding"]`) | Parse fetched RFC5322/MIME into subject/from/addresses/bodies/attachments | Unambiguous best-in-class: 100% safe Rust, zero-copy, fuzzed + MIRI-tested, battle-tested on millions of real messages, RFC 8621 §4.1.4 body model (text parts / html parts / attachments — exactly what the reader pane needs), 41 charsets including UTF-7, RFC2231 attachment filenames. `MessageParser::parse_headers` exists for a headers-only fast path. 3.8M+ downloads, 143 dependents, updated Sep 2026. `full_encoding` pulls `encoding_rs` for CJK multi-byte charsets real university mail contains. |
| `keyring` (v3 API, Secret Service store) | `keyring = "3"` with feature `sync-secret-service` | Store IMAP password in GNOME Keyring / KWallet Secret Service; auto-login | Documented Linux path (`keyring = { version = "3", features = ["sync-secret-service"] }` per docs.rs 3.5.0): synchronous API backed by `dbus-secret-service`, no async runtime required — which is exactly why it fits the dedicated sync thread. Entry key `service="sge"`, `user=<imap-username>`. **Hard rule the roadmap must carry:** every keyring call runs inside `tauri::async_runtime::spawn_blocking` (or the sync thread) — the crate blocks the calling thread and deadlocks if invoked on a runtime thread. Never cache the password in memory longer than session bootstrap; re-read from keyring per sync start. Server host/port/username (non-secret) go in SQLite `accounts`, password never touches SQLite. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tokio` | `1.x` (full features as Tauri enables) | Tauri command async runtime, mpsc channels, `spawn_blocking` for keyring/SQLite | Already pulled in by Tauri; use `tauri::async_runtime::{spawn, spawn_blocking}` rather than depending on tokio directly where possible. |
| `serde` / `serde_json` | `1.x` | Command arg/result serialization across the IPC boundary | Every Tauri command payload; `mail-parser` also has a `serde` feature if raw parsed messages ever cross IPC (prefer pre-shaped DTOs instead — see Architecture note). |
| `thiserror` | `2.x` | Typed backend errors mapped to `Result<T, String>` command errors | All IMAP/SQLite/keyring failures become user-actionable strings (`auth failed` vs `network unreachable` vs `keyring locked`) — the login UX depends on this mapping. |
| `chrono` | `0.4` | Date header normalization, `INTERNALDATE` sorting, "today/yesterday" grouping | Needed the moment the list shows dates; `mail-parser` returns its own DateTime — convert once at ingest. |
| `html2text` / `ammonia` (frontend-adjacent, evaluate in UI phase) | latest | Plain-text fallback + HTML sanitization of message bodies | Bodies render in the WebView — unsanitized HTML email is an XSS/trackers vector. Sanitize before render; keep remote-content blocking (no auto image load) as an M1 default. |
| `@tanstack/react-query` | `5.x` | Frontend server-cache: message list queries, sync-status polling, body-on-demand caching | The UI reads SQLite via commands; react-query turns those command calls into cached queries with background refetch on `sync-progress` events. Prevents hand-rolled `useEffect` fetch spaghetti in the three-pane layout. |
| `@tanstack/virtual` (or `virtua`) | latest | Virtualized message list | UTFPR mailboxes can hold tens of thousands of headers; render only visible rows. Required once the list exceeds a few hundred rows — plan for it in the list component from the start. |
| `tauri-plugin-store` or plain JSON | `2.x` | Non-secret app prefs (selected account, pane sizes, theme) | Do NOT put credentials here; host/port/username may live in SQLite `accounts` instead — pick one home (recommend SQLite) and keep Store for pure UI prefs only. |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| `create-tauri-app 4.7.4` | Scaffold React+TS+Vite+Tauri v2 skeleton | `npm create tauri-app@latest`; select React+TypeScript. Verify `tauri.conf.json` bundle identifiers before first `tauri build`. |
| Ubuntu 22.04 / Debian 12 build baseline (+ CI) | Produce portable .deb/AppImage | Tauri docs mandate: build on the **oldest** supported base providing `libwebkit2gtk-4.1-dev`, otherwise glibc floor breaks older targets. Dev deps: `libwebkit2gtk-4.1-dev build-essential libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`. CI (GitHub Actions `ubuntu-22.04`) builds artifacts; never build release bundles on a rolling/newer host. |
| `cargo-deny` + `cargo-audit` | License/advisory gate on the Rust dep tree | IMAP/TLS/SQLite pull C-linked code (`native-tls`, `bundled` sqlite) — audit before each milestone bundle. |

## Installation

```bash
# Scaffold (React + TypeScript + Vite)
npm create tauri-app@latest sge -- --template react-ts

# Frontend
cd sge && npm install
npm install @tanstack/react-query @tanstack/virtual
npm install -D @tauri-apps/cli  # or cargo install tauri-cli --version "^2.12"

# Rust backend (src-tauri)
cargo add tauri@2 serde serde_json thiserror chrono
cargo add rusqlite@0.37 --features bundled
cargo add rusqlite_migration
cargo add async-imap async-std async-native-tls futures
cargo add mail-parser@0.11 --features full_encoding
cargo add keyring@3 --features sync-secret-service
```

`tauri.conf.json` bundle sketch (Linux):

```json
{
  "bundle": {
    "linux": {
      "deb": { "depends": ["libwebkit2gtk-4.1-0", "libgtk-3-0"] },
      "appimage": { "bundleMediaFramework": true }
    },
    "targets": ["deb", "appimage"]
  }
}
```

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| `async-imap 0.11` | sync `imap 2.x` crate | If async executor bridging proves painful during the sync spike: `imap` shares `imap-proto`, is dead-simple on a `std::thread`, and M1's sequential single-INBOX sync needs no concurrency. Downgrade path, not first choice. |
| `async-imap 0.11` | `io-imap 0.6` (new, Rust-2024, no-std layered) | Watch only. 3.5k downloads/mo and 2026-fresh — promising RFC coverage (incl. IDLE) but unproven in production apps. Revisit post-M1 if async-imap stalls. |
| `rusqlite 0.37` direct | `tauri-plugin-rusqlite2 2.2.x` (JS API) / official `tauri-plugin-sql` (sqlx) | Only if a future milestone wants SQL issued from JS. For M1 the sync engine owns the DB in Rust; the plugin would add IPC hops and a second connection story. `sqlx`+async is overkill for a single-writer local mailbox. |
| `rusqlite_migration 2.x` | `refinery`, `schemamama` | `refinery` if the team prefers file-based `.sql` migration scripts checked into `migrations/`; functionally equivalent for M1's tiny schema. |
| `keyring 3` + Secret Service | `oo7` / `secret-service` direct, `libsecret` bindings | If `keyring`'s sync API deadlocks or its default-store resolution misbehaves on KDE (KWallet UTF-8 limits): drop to `oo7 0.4` (pure-Rust Secret Service portal API, Flatpak-friendly) or raw `secret-service` with explicit collection handling. Also note: kernel `keyutils` store is **wrong** here — non-persistent across reboots, breaks auto-login. |
| `native-tls` | `rustls` (+ `webpki-roots`) | If static-linking determinism beats system-store integration (e.g. AppImage on exotic distros, or corp proxy with custom CA you prefer to bundle). Costs: you own root-store updates. |
| React 19 | Svelte / Solid / Leptos | Only on team-preference grounds; React has the widest Tauri examples + component ecosystem (virtual lists, sanitizers). No technical driver to switch. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `imap` + `lettre` confusion (using lettre for IMAP) | `lettre` is SMTP-send only; it cannot read mail. M1 is read-only so lettre has no role at all. | `async-imap` for fetch; add `lettre` only when the send milestone lands. |
| Plaintext credential files / SQLite-stored passwords | Violates the project's hard security constraint; plaintext secrets also leak into backups and dotfiles. | `keyring` Secret Service; only host/port/username in SQLite. |
| `keyutils` (kernel keyring) as the credential store | In-memory only — wiped on reboot, so auto-login silently breaks after every restart; documented as "secure cache, not storage". | Secret Service via `keyring sync-secret-service`. |
| Full-mailbox `FETCH BODY[]` up front | Multi-GB UTFPR mailboxes → minutes-long first sync, huge SQLite, ANR-feeling UI; contradicts the decided headers-first strategy. | `FETCH (UID FLAGS ENVELOPE BODYSTRUCTURE)` + UID-windowed pages; `BODY[]`/`BODY[TEXT]` only on message open; attachments via `BODY[<section>]` partial fetch on download click. |
| JS-side SQLite plugin as the M1 store owner | Every list render and sync write crosses IPC; two connection owners invite lock contention and WAL-mode confusion. | `rusqlite` owned by the Rust backend; commands return shaped DTOs. |
| Building release bundles on newest Ubuntu/Fedora | Raises minimum glibc, breaking installs on older-but-supported targets (documented Tauri limitation for both .deb and AppImage). | Frozen `ubuntu-22.04` CI builder with `libwebkit2gtk-4.1` stack. |
| Auto-loading remote images in the reader pane | Tracking pixels + mixed-content + WebView attack surface on day one. | `ammonia`-sanitized HTML with remote content blocked; "load images" per-message opt-in (post-M1). |

## Stack Patterns by Variant

**If the UTFPR server requires STARTTLS on port 143 (vs implicit TLS on 993):**
- Use the same `async-imap` + `async-native-tls` pair; only the handshake differs (plain `TcpStream` → `STARTTLS` upgrade → TLS-wrapped stream vs direct TLS connect). This is exactly why IMAP host/port/security must be user-configurable fields in the login form and `accounts` table (`security: ImplicitTls | StartTls | Plain`), not build-time constants.

**If the mailbox is huge (10k+ messages) on first sync:**
- Page header sync by UID windows (`UID FETCH 1:500 (ENVELOPE BODYSTRUCTURE)`, then next window), commit each window in one SQLite transaction, `emit("sync-progress")` per window so the list paints incrementally. FTS5 index updates stay inside the same transaction to avoid reindex churn.

**If GNOME Keyring is locked/headless at first launch:**
- `keyring` returns an error, not a hang (with the sync-secret-service store) — map it to a "unlock your keyring / enter password" login state, fall back to in-memory-only session (no remember-me) rather than failing login outright.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| `tauri 2.12` | `@tauri-apps/api ^2.12`, `tauri-cli 2.12`, `tauri-bundler 2.10` | Keep Rust crate, JS API, and CLI on the same 2.x minor; mismatched CLI is the classic broken-bundle cause. Requires `libwebkit2gtk-4.1` (not 4.0) on build and target systems. |
| `async-imap 0.11` | `async-std 1.x` + `async-native-tls 0.5` + `imap-proto 0.16/0.17` (transitive) | `cargo add` resolves the compatible `imap-proto`; do not pin it directly unless `cargo tree` shows a duplicate. |
| `rusqlite 0.37` | `rusqlite_migration 2.x`, bundled SQLite 3.5x, FTS5 enabled | `bundled` implies FTS5 availability — verify once with `SELECT sqlite_version()` + `PRAGMA compile_options` in a smoke test; do not rely on distro sqlite. |
| `mail-parser 0.11.9` | `encoding_rs 0.8` (via `full_encoding`), Rust edition 2024 toolchain | Crate itself is edition-2024; project needs a current stable toolchain (≥1.85). No conflicts with the rest of the tree (dependency-free core). |
| `keyring 3` | `dbus-secret-service` backend, GNOME Keyring ≥40 / KWallet with secret-service API | Needs a D-Bus session + unlocked login keyring at runtime; `.deb` should recommend (not require) `gnome-keyring`/`kwallet`. KDE note: binary secrets must be UTF-8/base64 — passwords are, so no action for M1. |

## Reference SQLite Schema (M1 contract for the roadmap)

The roadmap's data phase should implement exactly this before any sync code:

```sql
CREATE TABLE accounts (
  id INTEGER PRIMARY KEY,
  username TEXT NOT NULL, host TEXT NOT NULL, port INTEGER NOT NULL,
  security TEXT NOT NULL CHECK (security IN ('implicit_tls','starttls','plain')),
  UNIQUE (username, host, port)
) STRICT;
-- password lives ONLY in OS keyring under service 'sge', user = username@host

CREATE TABLE sync_state (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id),
  uidvalidity INTEGER NOT NULL, last_uid INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE messages (
  uid INTEGER NOT NULL, account_id INTEGER NOT NULL REFERENCES accounts(id),
  message_id TEXT, subject TEXT, from_addr TEXT, to_addrs TEXT,
  date_utc INTEGER, flags TEXT NOT NULL DEFAULT '[]',
  snippet TEXT, body_text TEXT, body_html TEXT, -- NULL until fetched on demand
  PRIMARY KEY (account_id, uid)
) STRICT;

CREATE VIRTUAL TABLE messages_fts USING fts5(subject, from_addr, snippet, body_text);

CREATE TABLE attachments (
  id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL,
  uid INTEGER NOT NULL, filename TEXT, mime TEXT, size INTEGER,
  section TEXT NOT NULL, -- IMAP BODY section for on-demand fetch
  FOREIGN KEY (account_id, uid) REFERENCES messages(account_id, uid)
) STRICT;
```

Sync invariant: if server `UIDVALIDITY` differs from `sync_state`, wipe and resync (RFC 3501). Incremental sync = `UID FETCH last_uid+1:*`. Search = `messages_fts` (offline), not server `SEARCH` (keeps M1 offline-capable).

## Sources

- Tauri official release page (`v2.tauri.app/release`) — `tauri`/`tauri-cli`/`@tauri-apps/api` 2.12.0, Sep 26 2026 — HIGH
- Tauri Debian + AppImage bundling docs (`v2.tauri.app/distribute/*`) — webkit2gtk-4.1 deps, oldest-baseline build rule — HIGH
- crates.io + docs.rs `mail-parser` 0.11.9 (Sep 2026), Stalwart README/CHANGELOG — HIGH
- docs.rs `tauri-plugin-rusqlite2` 2.2.8 source + deps (`rusqlite ^0.37`, `rusqlite_migration ^2`) — HIGH
- docs.rs `keyring` 3.5.0 + `dbus-secret-service-keyring-store` 1.0.1 + keyring-rs wiki (sync-secret-service feature, blocking-call rule, KWallet UTF-8 note) — HIGH
- crates.io `async-imap` + `imap-proto` lineage, `io-imap` 0.6 newcomer signal — MEDIUM (version minor not re-verified at write time; `cargo add` at scaffold is source of truth)

---
*Stack research for: SGE Linux IMAP desktop client (Rust + Tauri v2 + React + SQLite)*
*Researched: 2026-10-03*
