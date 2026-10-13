# Stack Research: v1.5 Additions (IDLE / CONDSTORE-QRESYNC / Sieve / Override Learning)

**Domain:** SGE v1.5 — push sync, fast resync, server-side rule export, classifier learning infra
**Researched:** 2026-10-13
**Confidence:** HIGH (IDLE/CONDSTORE verified in pinned crate source; ManageSieve verified on docs.rs)

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| async-imap IDLE (`Session::idle` → `extensions::idle::Handle`) | 0.11.3 (already pinned, no bump) | Push notifications for new mail | Already in the tree and verified in local registry source (`src/extensions/idle.rs`): `idle()` consumes the session, `Handle::init/wait` blocks until *anything* changes, docs prescribe DONE/re-IDLE ≥ every 29 min. Zero new deps — the cheapest push path available. |
| async-imap `select_condstore` + `run_command*` escape hatches | 0.11.3 (already pinned) | CONDSTORE fast path + hand-rolled QRESYNC | `select_condstore` is built in (verified line 394 of pinned `client.rs`); QRESYNC's `ENABLE` / `SELECT (QRESYNC…)` / `UID FETCH … (CHANGEDSINCE …)` go through the existing `run_command` / `run_command_untagged` raw-command hatch. imap-proto 0.16 (pinned transitive) already parses `HIGHESTMODSEQ` response codes and `MODSEQ` fetch attributes (verified in registry source `types.rs`). No new IMAP crate needed. |
| io-managesieve | 0.2.0 (2026-09-29) | Sieve script upload/activate (ManageSieve RFC 5804) | Only maintained Rust ManageSieve client: extracted from Himalaya/Pimalaya (production mail client, NLnet-funded), full RFC 5804 coverage (LIST/PUT/CHECK/ACTIVATE/RENAME/DELETE scripts, STARTTLS, SASL via io-sasl), blocking std client with a **`native-tls` feature flag** that matches SGE's existing TLS stack (native-tls 0.2 / async-native-tls 0.5). MIT/Apache-2.0. Local Sieve *text generation* from smart rules is hand-rolled string templating (no crate); this crate covers only the *upload* leg. |
| rusqlite + rusqlite_migration (existing) | 0.37 / 2.x (already pinned) | Override-learning log, MODSEQ store, rule import/export persistence | No new dep: learning-from-overrides = new tables (`override_events`, message `modseq`, per-folder `highestmodseq`, rule `version/etag` columns) behind the established M20+ migration pattern; rule import/export serializes with existing serde/serde_json. |
| tokio `sync` + `time` (existing) | 1.x with `rt, sync, time` (already pinned) | IDLE task supervision, debounce, spawn_blocking bridge | No new dep: `tokio::task::spawn_blocking` hosts io-managesieve's blocking client off the Tauri async runtime; `watch`/`Notify` fan IDLE wakeups into the existing SyncGate single-flight path; `time::sleep` implements the 29-min IDLE refresh. `futures 0.3` (already pinned) polls the IDLE `Handle` stream. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| io-managesieve with `native-tls` feature only | 0.2.0, `default-features = false, features = ["native-tls"]` | ManageSieve session (TCP + STARTTLS/implicit TLS + AUTHENTICATE + script commands) | Only in the Sieve-export phase, and only after a capability probe confirms the server offers ManageSieve (port 4190). Default features pull rustls-ring — disable to avoid a second TLS stack alongside native-tls. |
| `log` facade | 0.4 (already transitive via async-imap) → promote to direct dep if diagnostics wanted | Structured backend diagnostics for IDLE/QRESYNC state machine | Only if the phase wants log output beyond SQLite journals; otherwise skip — override learning lives in SQLite, not in logs. |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| `live-*.sh` harnesses (already built) + CAPABILITY probe snippet | Verify IDLE/CONDSTORE/QRESYNC/ManageSieve advertisement on mail.utfpr.edu.br before building | First step of each protocol phase: `CAPABILITY` decides IDLE-vs-poll, CONDSTORE-vs-baseline, Sieve-upload-vs-export-only. Do not build the feature before the probe. |
| Local registry source (`~/.cargo/registry/src/*/async-imap-0.11.3`) | Ground truth for IDLE/escape-hatch APIs | Already used for this research; planners should read `extensions/idle.rs` + `client.rs` `run_command*` rather than trusting web docs (most web examples target 0.9/0.10). |

## Installation

```bash
# Only addition for v1.5 (Sieve upload leg; everything else is already pinned)
cargo add io-managesieve --no-default-features --features native-tls
# Pin record: io-managesieve 0.2.x (MIT/Apache, pimalaya, verified on docs.rs 2026-10-13)

# Everything below is already in src-tauri/Cargo.toml — DO NOT re-add:
# async-imap 0.11 / async-native-tls 0.5 / async-std 1.13 (IDLE + CONDSTORE + transport)
# futures 0.3 (IDLE stream polling) / tokio 1 (rt/sync/time supervision)
# rusqlite 0.37 bundled + rusqlite_migration 2.x (modseq + override-event schema)
# serde / serde_json 1 (rule import/export) / chrono 0.4 (event timestamps)
# lettre 0.11 (compose hardening reuses v1.2 pipeline, no change)
```

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| Stay on async-imap 0.11.3 | Bump to async-imap 0.12 (2026-09-30, imap-proto 0.17) | Only as a separate migration milestone: 0.12 changes the proto parser under every existing verb mock. v1.5 needs nothing 0.12 adds (IDLE/CONDSTORE/escape hatches all verified present in 0.11.3). |
| io-managesieve 0.2.0 (upload leg) | Hand-rolled ManageSieve over async-native-tls | If io-managesieve 0.2.0's blocking API fights the single-session manager at plan time — RFC 5804 is a simple line protocol and the fallback is ~200 lines. Spike first; keep the crate as plan A. |
| io-managesieve 0.2.0 | `managesieve` protocol-only crate + own transport | If only framing/parsing is wanted and session/TLS/Auth is hand-rolled anyway. More work, no benefit over io-managesieve's I/O-free coroutine layer which offers the same split. |
| Hand-rolled Sieve text templates | `sieve-generator` crate | Never for v1.5: unmaintained since 2022. Sieve `require`/`if address :contains` generation for keyword/sender rules is trivial string building with snapshot tests. |
| QRESYNC via `run_command` hatch | New IMAP crate (daaki-imap, io-imap, rusmes-imap) | Never for v1.5: replaces the proven SessionManager transport for one fast path. The hatch + imap-proto 0.16 MODSEQ parsing is sufficient; capability-gate to baseline UID sync where the server lacks QRESYNC. |
| SQLite override-event tables | New logging/telemetry crate | Never: learning signal = queryable relational data ( Armed with timestamps, old/new category, confidence), not log lines. `log` facade only for operator diagnostics. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| sieve-rs / mailrs-sieve-core (Stalwart) | Server-side Sieve *interpreter*; pulls a 19k-LOC AGPL-tainted dependency to do work the server already does. SGE needs export (text), not interpretation. | Hand-rolled templates for text + io-managesieve for upload |
| async-imap 0.12 bump inside v1.5 | imap-proto 0.16→0.17 parser change re-opens the NAMESPACE-class of session poisoning bugs already tripwired; zero v1.5 features require it | Stay on 0.11.3; schedule 0.12 as its own phase with mock regeneration |
| rustls (via io-managesieve default features) | Duplicates the pinned native-tls 0.2 / async-native-tls 0.5 stack; bigger bundle, two TLS trust stores to reason about | io-managesieve with `default-features = false, features = ["native-tls"]` |
| Second always-on IMAP session per folder for IDLE | IDLE binds one connection to one selected mailbox; per-folder IDLE connections multiply server load and break the single-SessionManager ownership model | One dedicated IDLE connection on INBOX (or the user's most-active folder) + 5-min poll retained as fallback for the rest; poll path stays the single guarded code path |
| `UID FETCH … BODY[]` inside the IDLE wakeup handler | Defeats the push optimization — wakeup should enqueue a headers-first incremental sync through SyncGate, not fetch inline while holding the IDLE connection | Wakeup → `Notify`/event → existing single-flight sync path (headers-first, bodies on demand) |
| New compose/send crates | v1.5 compose hardening explicitly reuses the v1.2 lettre 0.11 pipelines (verify + fix gaps, not a rebuild) | lettre 0.11 as pinned; no SMTP stack change |

## Stack Patterns by Variant

**If the server advertises IDLE (verify with CAPABILITY probe on mail.utfpr.edu.br):**
- Spawn one supervised IDLE task on a dedicated connection (SessionManager mints a second session; IDLE `Handle` consumes it by design), `Handle::wait` loop with DONE/re-IDLE ≤ 29 min, wakeups fanning into SyncGate.
- Because the single-session manager owns the primary session; IDLE must never hold the primary connection (it blocks all other verbs while active).

**If the server lacks IDLE or CONDSTORE/QRESYNC:**
- Keep the 5-min poll + UID-range backfill exactly as-is (baseline already correct per Phase 8/9); gate fast paths behind capability checks with silent fallback, never hard-fail.

**If the server offers no ManageSieve (port 4190 closed / no SIEVE capability — likely on corporate Zimbra):**
- Ship Sieve *export-only* (downloadable `.sieve` file + copy-to-clipboard) with upload deferred and documented; do not block the milestone on the upload leg.

**If io-managesieve's blocking client blocks plan-time integration:**
- Drop to its I/O-free coroutine layer driven by the existing async-std transport, or hand-roll the ~6 ManageSieve commands needed (CAPABILITY → AUTHENTICATE → PUTSCRIPT → CHECKSCRIPT → SETACTIVE → LOGOUT) over async-native-tls. Target servers are one family (UTFPR), not the whole RFC matrix.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| io-managesieve 0.2.0 (`native-tls` feature) | native-tls 0.2 (pinned) | Feature-gated; verify at `cargo tree` time that no rustls crate enters the graph |
| io-managesieve 0.2.0 blocking client | tokio 1 `rt` (Tauri runtime) via `spawn_blocking` | Blocking calls must never run on async workers; wrap PUTSCRIPT/CHECKSCRIPT sessions in `spawn_blocking` or a dedicated std::thread |
| async-imap 0.11.3 IDLE `Handle` | async-std 1.13 runtime (pinned transport) | `Handle: Stream` impl requires `Send`; poll it from the Tauri async runtime with `futures::StreamExt`, reuse the existing single-flight bridge pattern (Phase 17 tokio-sync precedent) |
| MODSEQ schema additions (M20+) | rusqlite_migration 2.x + UIDVALIDITY epoch gating | `highestmodseq` stored per folder alongside `uidvalidity`; any UIDVALIDITY change invalidates stored modseqs (same rule as UIDs) |
| imap-proto 0.16 (transitive) | async-imap 0.11.3 | Locked pair; do not bump independently |

## Sources

- Local registry source `async-imap-0.11.3/src/client.rs` (`select_condstore` line 394, `idle()` line 1134, `run_command*` lines 1364–1443) + `src/extensions/idle.rs` (Handle/init/wait, 29-min guidance) — HIGH, ground truth for pinned version
- Local registry source `imap-proto-0.16.7/src/types.rs` (`HighestModSeq`, `ModSeq` attribute parsing) — HIGH
- docs.rs `io-managesieve 0.2.0` (2026-09-29, pimalaya, MIT/Apache, native-tls feature, full RFC 5804, io-sasl auth) — MEDIUM-HIGH (crate young at 0.2.0 but extracted from production Himalaya; needs plan-time integration spike)
- docs.rs `async-imap 0.12.0` Session docs (method list incl. `idle`, `select_condstore`) — corroborating only; 0.12 NOT recommended for v1.5
- `.planning/PROJECT.md` (v1.5 scope: IDLE/CONDSTORE/QRESYNC, Sieve tentative, learning-from-overrides logging hooks, compose reuses v1.2 pipelines) + `src-tauri/Cargo.toml` (current pins) — HIGH

---
*Stack research for: SGE v1.5 additions (IDLE / CONDSTORE-QRESYNC / Sieve / override learning)*
*Researched: 2026-10-13*
