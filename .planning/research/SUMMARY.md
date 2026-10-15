# Project Research Summary

**Project:** SGE — Linux IMAP Desktop Client (v1.8 LLM-Assisted Replies)
**Domain:** Desktop email client — user-triggered LLM reply drafting (Tauri v2 + Rust + React + SQLite, Linux)
**Researched:** 2026-10-15
**Confidence:** HIGH

## Executive Summary

SGE v1.8 adds on-demand LLM-assisted reply drafting to a mature offline-first Linux mail client: a user-configured OpenAI-compatible endpoint plus a single prompt template generate a draft inside Reply/Reply-All windows, which the user reviews and sends manually. The closest real-world analog (Thunderbird + ThunderAI/AI-Mate) confirms the shape — BYOK endpoint settings, custom prompt with mail variables, compose-window injection, manual review gate — and SGE's stricter posture (OS keyring secrets, offline-by-default, button-only egress) is a competitive strength, not a limitation.

The unanimous architectural verdict across all four research streams: the LLM HTTP call lives in the Rust backend as a new `llm` lane module plus Tauri commands, never as frontend `fetch`. Two decisive reasons — the API key must never enter the WebView, and the locked CSP (`connect-src ipc:`-only) blocks frontend calls to user URLs anyway. Zero new core dependencies are needed: widen `reqwest 0.12` features on the pinned line, reuse keyring/SQLite/tokio/zeroize patterns already proven in earlier phases. The critical risks are key leakage (logs, IPC, SQLite), ambient egress violating the offline guarantee, and prompt injection from hostile incoming mail — all mitigated by design choices (keyring-only key, single-chokepoint command, fenced prompt + human gate) rather than by vigilance alone.

## Key Findings

### Recommended Stack

Zero-new-core-deps: everything v1.8 needs is a feature-flag widening or table addition on already-pinned crates. The single `Cargo.toml` change is `reqwest 0.12` with `features = ["json", "stream", "native-tls"]` (details in STACK.md). Non-streaming `POST → .json()` ships first; `stream` is pre-enabled so optional SSE later needs no second Cargo change. See STACK.md for full rationale, alternatives rejected, and version pins.

**Core technologies:**
- reqwest 0.12 (`json`, `stream`, `native-tls`, no default features) — HTTPS POST to `{base_url}/chat/completions`; already in tree, reuse compiled native-tls stack
- serde + serde_json 1 — typed chat-completions request/response with tolerant `#[serde(default)]` for provider variance
- keyring 3 + sync-secret-service — LLM API key under a new service/account pair; key read keyring-side, never returned to frontend
- rusqlite 0.37 bundled + rusqlite_migration 2.x — new single-row `llm_settings` table (no secrets; structural guarantee, tested)
- tokio 1 `time` — per-generation timeout (`tokio::time::timeout` + `Client::timeout` backstop)
- zeroize 1 + thiserror 2 — in-memory key hygiene (`Zeroizing`) + typed `LlmError` mapped to plain-language strings

### ⚠️ Resolved Conflict: TLS Feature (`native-tls` vs `rustls-tls`)

STACK.md recommends `native-tls`; ARCHITECTURE.md (§llm module, §Integration Points) says `rustls-tls` / `rustls`. **Decision: `native-tls`. Winner, no tie.**

Rationale: `native-tls 0.2` + `tokio-native-tls` are already compiled into the bundle (IMAP + SMTP paths use them), so `native-tls` adds zero new TLS code, zero new trust store, and zero new OpenSSL build requirement. `rustls-tls` would pull a second TLS stack (`rustls`/`ring`/`webpki`) plus its own root store for a single POST — pure cost. This also preserves the project's zero-new-core-deps posture: all three reqwest features resolve to crates already in the lockfile. Revisit only if OpenSSL dev headers ever become a build problem on a target distro. **Verification gate (both phases touching Cargo): `cargo tree` must show reqwest → hyper-tls → native-tls only; if any `rustls`/`ring` crate appears, a feature flag is wrong.** ARCHITECTURE.md's `rustls` mentions should be read as `native-tls` throughout.

### Expected Features

ThunderAI/AI-Mate (closest analog, verified) plus Superhuman/Shortwave/Missive UX patterns converge on the same v1.8 shape: settings + keyring key + template + button + insert-above-quote + plain-language errors + review gate. See FEATURES.md for competitor table and prioritization matrix.

**Must have (table stakes):**
- LLM settings section (base URL + model + keyring key + template + temperature/max-tokens/timeout + test-connection) — without this nothing works
- Generate-with-LLM button in Reply/Reply-All only (non-streaming, cancel, loading) — the milestone's core value
- Insert-above-quote, fully editable, threading headers untouched — the review mechanism
- Plain-language errors + retry in existing `notice` area (unconfigured / 401 / timeout / provider) — every failure otherwise looks like an app bug
- Button-only privacy boundary (audit-proven, documented in settings UI) — the trust contract

**Should have (competitive, v1.8.x iff time/UAT demands):**
- Regenerate button (cheap re-call) — add only if users dislike first drafts
- Streaming output — add only on latency complaints (local Ollama on CPU is slow)
- Ollama/localhost preset — polish for the blank-base-URL problem
- Per-draft instruction override — only on explicit user request

**Defer (v2+):**
- Multi-prompt library (contradicts single-template scope guard)
- Forward/new-message generation (different context semantics, needs own spec)
- Voice matching from sent mail (PII expansion + retrieval machinery)
- Never: auto-send, background/pre-generation, per-provider native SDKs, plaintext key storage, prompt/response body logging

### Architecture Approach

New `src-tauri/src/llm/` lane module (mirroring `classify/`) + `commands::llm` IPC surface + `llm_settings` migration + keyring entry + `LlmSettingsPanel.tsx` + ~40-line `ComposeWindow.tsx` modification; `queue_send` path explicitly untouched. Prompt renders backend-side from `{uid, mailbox}` (backend re-reads the BODY.PEEK original — no body over IPC, one privacy choke point). Settings split (SQLite non-secrets / keyring secret) is a security invariant from schema.sql T-02-02, not a preference. See ARCHITECTURE.md for the system diagram, data flows, and dependency-respecting build order (with `rustls` mentions corrected to `native-tls` per the resolution above).

**Major components:**
1. `src-tauri/src/llm/` (NEW: `settings.rs` validate, `prompt.rs` render, `client.rs` POST + timeout + single-flight) — the LLM lane, testable without a WebView
2. `commands::llm` (NEW: get/save/generate/test/clear) — sole IPC surface; `generate_reply` is the single network-egress chokepoint
3. `llm_settings` table + M-next migration + keyring `sge/llm-api-key` entry — persistence split enforcing no-secrets-in-SQLite structurally
4. `LlmSettingsPanel.tsx` + `lib/llm.ts` (NEW) — settings form reusing existing panel idioms; thin invoke wrappers, no secrets, no prompt logic
5. Generate button in `ComposeWindow.tsx` (MODIFY, reply/reply-all only) — pure string splice above quote; send path untouched so the review gap is enforced by doing nothing

### Critical Pitfalls

Seven critical pitfalls identified (all HIGH confidence); top 5 below, full set with debt patterns and recovery costs in PITFALLS.md.

1. **API key leaks into logs, errors, SQLite, or the WebView** — keyring-only from day one; no key-returning IPC (presence-bool only); header-only auth; metadata-only logs; CI grep gate rejecting `get_api_key` return paths and `api_key` in schema files.
2. **Breaking the offline-default guarantee** — exactly one Tauri command performs egress, user-gesture-gated; no timers/hooks/queue-replay; `GET /models` probe sends no mail content; prove with a network-off UAT test.
3. **Prompt injection via hostile incoming mail** — fence `{body}` as data (never orders), strip invisible HTML before interpolation, treat output as untrusted draft, never auto-send; injection fixtures in UAT.
4. **Blocking the UI thread on a slow LLM call** — async command + shared client + configurable timeout + Cancel + per-window single-flight; no `reqwest::blocking` anywhere.
5. **Unbounded token/cost exposure** — truncate body with disclosure, wire max_tokens/temperature/timeout into every request, retry only 408/429/5xx (never 401/400), log `usage` per call.
6. **User-configured endpoint = SSRF/TLS footgun + schema drift** — https-only except loopback opt-in, never bypass cert verification, no-redirect policy for LLM POSTs, tolerant SSE/error parsing, save-time URL validation + live probe. *(Prevention leans on the `native-tls` decision above: one TLS stack, one trust store.)*
7. **Streaming partial-text presented as complete** — mostly deferred with streaming, but the non-streaming path must still handle empty/`length`-stop responses as errors with actionable text, never as finished drafts.

## Implications for Roadmap

Based on combined research, v1.8 decomposes into three phases in strict dependency order (condenses ARCHITECTURE.md's five-step build order into shippable phases; each phase is independently verifiable):

### Phase 1: LLM Settings Foundation (backend store + validation)
**Rationale:** Everything depends on validated settings + key storage; it is also where the two cheapest catastrophic pitfalls (key leakage, endpoint validation) are prevented structurally. Pure backend logic, CI-safe with mocked keyring — no IPC, no UI.
**Delivers:** M-next migration (`llm_settings`, CHECK constraints, no-secret-column assertion test), `queries` get/set, `creds.rs` key entry (save/load/clear), `llm/settings.rs` validation (https-or-loopback, model non-empty, ranges) + `prompt.rs` render (fenced `{body}`, invisible-HTML strip, truncation cap), reqwest feature widening with `cargo tree` no-second-TLS-stack check.
**Addresses:** Settings + keyring + template + defaults (FEATURES.md P1); `LlmSettings` struct contract (ARCHITECTURE.md Pattern 3).
**Avoids:** Pitfalls 1 (key leakage), 5-partial (caps persisted), 6-partial (URL validation).
**Uses:** rusqlite_migration, keyring, zeroize, thiserror; reqwest `native-tls` per conflict resolution.

### Phase 2: Generation Flow (client + IPC chokepoint)
**Rationale:** The milestone's core value and highest-risk surface (egress, injection, blocking, cost, TLS) converge in one command; building it before any UI keeps the privacy choke point auditable and testable against a loopback mock.
**Delivers:** `llm/client.rs` (shared client, total + connect timeout, single-flight `LlmGate`, tolerant response/error parsing, `usage` logging, no-redirect policy), `commands/llm.rs` (5 commands; `generate_reply{uid,mailbox}` loads settings + key + BODY.PEEK original server-side), `test_llm_connection` via `GET /models`, mock-transport tests (401 fails fast single-request, timeout, truncation-on-wire, multi-server error shapes).
**Addresses:** Non-streaming `chat/completions` call, test-connection (FEATURES.md P1); Patterns 1–2 (ARCHITECTURE.md).
**Avoids:** Pitfalls 2 (single chokepoint), 3 (fence + human gate), 4 (async + timeout + cancel + single-flight), 5-enforced, 6-client-side, 7-non-streaming handling.
**Implements:** `llm/` module + `commands::llm` + keyring-at-call-time + BODY.PEEK reuse (`\Seen` invariant preserved).

### Phase 3: Compose Integration + Hardening/UAT
**Rationale:** UI is last because it is a thin splice over proven pieces (~40 lines); hardening closes the loop by proving the trust posture end-to-end rather than asserting it.
**Delivers:** `LlmSettingsPanel.tsx` + menu entry + `lib/llm.ts`, Generate button + generating state + insert-above-quote splice (reply/reply-all only) + `notice`-area errors with retry, settings persistence UAT (save → restart → key absent from SQLite file), network-off egress audit, injection fixtures, double-click/Cancel tests, mock-matrix (OpenAI/Ollama/error shapes, redirect, oversized context), one deferred-with-reason live gate with a user-provided key.
**Addresses:** Generate button + insertion + errors + privacy boundary (FEATURES.md P1); State Management (ARCHITECTURE.md).
**Avoids:** Pitfalls 2-proven, 3-verified, 4-verified, 7-verified; UX pitfalls (frozen window, cryptic errors, lost draft on failure — failed generation never clobbers compose content).

### Phase Ordering Rationale

- **Dependencies dictate order:** settings store → client/IPC → UI is a hard chain (each phase's tests mock the next layer down, never up); no parallelization is possible beyond within-phase task splits.
- **Risk-front-loading:** the irreversible decisions (key storage shape, TLS feature, single-chokepoint topology, prompt fencing) all land in Phases 1–2 where they are cheapest to change; Phase 3 is deliberately thin UI + verification.
- **Scope guards as phase gates:** reply-only, single template, non-streaming, no auto-send are enforced by what each phase does NOT build (no second editor, no template CRUD, no SSE parser, no send-path coupling) — reviewers should check absences, not just presences.
- **Pitfall-to-phase mapping** (PITFALLS.md) assigns every critical pitfall a prevention phase and a verification method; the roadmapper should copy those verification items into phase success criteria verbatim.

### Research Flags

Phases likely needing deeper research during planning (`--research-phase` recommended):
- **Phase 2:** multi-server response/error-shape tolerance (Ollama vs OpenAI vs proxies: keepalives, vendor frames, `{"error"}` vs bare envelopes, context-clip detection) — provider quirks are the least stable knowledge in this stack; a phase-scoped probe of docs.rs client patterns + live-shape fixtures pays off.
- **Phase 3 (hardening slice):** prompt-injection fixture design (hidden-div, fragmented-URL, protocol-state masquerade per ESPI/NDSS) — adversarial-tradecraft sources evolve; refresh at plan time rather than trusting this snapshot.

Phases with standard patterns (skip research-phase):
- **Phase 1:** keyring/SQLite-migration/validation patterns are established SGE precedent (Phases 1/5, M14/M18, `classify_settings`) — well-documented internally, no external research needed.
- **Phase 3 (UI slice):** compose-window splice + settings-panel form follow existing `ComposeWindow.tsx`/`RulesPanel` idioms directly — standard React/Tauri invoke work.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Pinned `Cargo.toml` ground truth + verified docs.rs reqwest 0.12 feature/dependency list; single-Cargo-line change |
| Features | HIGH | Closest analog (Thunderbird + ThunderAI) verified via web; generation UX cross-checked across 3+ clients; SGE-side deps read from `ComposeWindow.tsx` directly |
| Architecture | HIGH | Grounded in SGE source files (lib.rs, creds.rs, store, tauri.conf.json CSP, ComposeWindow); one internal conflict (TLS feature) resolved here in favor of STACK.md |
| Pitfalls | HIGH | 7 critical pitfalls with prevention + verification each; sourced from Tauri production guides, OWASP, peer codebases (Humla, TokenGuard), and SGE's own PROJECT.md constraints |

**Overall confidence:** HIGH

### Gaps to Address

- **ARCHITECTURE.md `rustls` mentions are stale after the TLS resolution** — planner for Phase 1/2 must treat every `rustls` reference as `native-tls` and enforce the `cargo tree` gate; consider a one-line correction pass on ARCHITECTURE.md during Phase 1 planning.
- **Timeout value tension (30 s vs 60 s defaults across docs)** — not a real disagreement (all agree configurable 5–180 s); Phase 1 planner picks defaults (suggested: 60 s total default for local-model generosity, 10 s connect) and documents the choice.
- **Migration number (M23 assumed)** — confirm next free migration index at Phase 1 plan time; rename accordingly.
- **Streaming design is intentionally absent** — if UAT triggers the streaming follow-up, that work needs its own research pass (SSE state machine, `Channel` events, idle-vs-total timeouts); do not improvise it inside v1.8.
- **Live-endpoint verification needs a user-provided key** — same deferred-with-reason harness as prior milestones; never commit a key; mock transport is the CI gate.

## Sources

### Primary (HIGH confidence)
- SGE `src-tauri/Cargo.toml` (pinned reqwest 0.12, serde, keyring 3 + sync-secret-service, rusqlite 0.37, tokio, zeroize, thiserror) — stack ground truth
- SGE codebase: `lib.rs`, `creds.rs`, `store/` (M2–M22 precedent), `schema.sql` T-02-02, `tauri.conf.json` CSP, `ComposeWindow.tsx` + `lib/compose.ts` — architecture/feature dependencies
- `.planning/PROJECT.md` v1.8 scope + constraints (keyring-only, offline-default, no auto-send, mock-in-CI) — scope authority
- docs.rs reqwest 0.12 (0.12.28 verified; feature → dep mapping for `json`/`stream`/`native-tls`) — stack verification
- OpenAI-compatible `chat/completions` shape (OpenRouter/base-URL docs) — stable API contract

### Secondary (MEDIUM confidence)
- Thunderbird ThunderAI setup + custom-prompt guides; AI-Mate add-on listings — closest-analog feature patterns
- Superhuman/Shortwave/Missive generation UX (streaming, regenerate, review gates) — differentiator/deferral calls
- Tauri v2 docs (CSP/CORS rationale for backend-owned HTTP) — architecture pattern
- OWASP LLM injection prevention + LLM01; Google layered injection defense; Nylas email-agent recipe — pitfalls 3 design
- Rust OpenAI-compatible client patterns (docs.rs: timeouts, tolerant SSE, retry allowlists, no-redirect bearer) — pitfalls 4–7 design

### Tertiary (LOW confidence, needs validation at plan time)
- ESPI / NDSS 2027 protocol-state masquerade; attack-chain detection preprints; hidden-HTML gateway bypasses — injection fixture design refresh (Phase 3 research flag)
- Per-server quirks (Ollama keepalives, OpenRouter error envelopes) — tolerant-parsing matrix (Phase 2 research flag)

---
*Research completed: 2026-10-15*
*Ready for roadmap: yes*
