# SGE v1.8 Roadmap — LLM-Assisted Replies

Milestone: v1.8 LLM-Assisted Replies (brownfield; user-triggered reply drafting via configured OpenAI-compatible endpoint). Continues numbering from Phase 38.

> ⚠️ **Provenance correction (2026-10-18):** the v1.7 Phase 38 summary claimed Reply/Reply-All with threading headers shipped; it did not. Phase 38 produced display stubs only, and the real reply feature — plus the UI entry point that made it reachable at all — was delivered by the repair commit `dc7f8c7`. Phase 39 therefore did **not** actually depend on a working reply window; Phase 41's integration had no reply surface to integrate into until that repair landed.

Phase range: 39–41 (3 phases, strict dependency order per research; each independently verifiable).

## Phases

- [x] **Phase 39: LLM Settings Foundation** — backend store + validation + prompt render (no IPC, no UI) [3/3 plans done: 39-01 M23 store + validation; 39-02 keyring + renderer; 39-03 settings commands + SCHEMA_VERSION fix]
- [x] **Phase 40: Generation Flow** — LLM client + single-egress IPC chokepoint (mock-tested, no UI) [done — 2 defects found and fixed by the 2026-10-18 repair: hardcoded INBOX UID 1, per-call request tracker]
- [ ] **Phase 41: Compose Integration + Hardening/UAT** — settings panel + Generate button + privacy audit (blocked on human UAT of the repaired reply/send path)

## Phase Details

### Phase 39: LLM Settings Foundation
**Goal**: Validated LLM settings persist securely (non-secrets in SQLite, key in keyring) with a fenced prompt renderer, independent of any UI.
**Depends on**: Phase 38 compose windows (New Message + Compose window were real; the Reply half was stub-only until `dc7f8c7`)
**Requirements**: LLM-01, LLM-02, LLM-03, LLM-04
**Success Criteria** (what must be TRUE):
  1. User's base URL + model + template + defaults survive an app restart, and the API key is absent from the SQLite file (keyring-only, presence-bool check).
  2. Invalid settings are rejected at save time (non-https non-loopback URL, empty model, out-of-range temperature/tokens/timeout) with plain-language reasons.
  3. Prompt template renders `{sender}`/`{subject}`/`{body}` with the body fenced as data (invisible HTML stripped, truncation cap applied with disclosure marker).
  4. `cargo tree` shows reqwest → hyper-tls → native-tls only (no `rustls`/`ring` after feature widening to `json`/`stream`/`native-tls`).
**Plans**: 3/3 complete (39-01: M23 store + validation; 39-02: keyring custody + fenced renderer; 39-03: settings commands + SCHEMA_VERSION fix)

### Phase 40: Generation Flow
**Goal**: One backend command turns validated settings + a server-side original into draft text, with plain-language failures and no other egress path.
**Depends on**: Phase 39
**Requirements**: LLM-05, LLM-07, LLM-09
**Success Criteria** (what must be TRUE):
  1. `generate_reply{uid,mailbox}` returns draft text from `POST {base}/chat/completions` (non-streaming, configurable timeout, cancel, per-window single-flight) without any body crossing IPC to the frontend.
  2. `test_llm_connection` proves the endpoint works via `GET /models` (no mail content sent) before first generation.
  3. Every failure class maps to a plain-language error (unconfigured → settings link; 401 fails fast single-request; timeout/network → connectivity; provider 4xx/5xx → provider text) with retry only on 408/429/5xx — key and bodies never in errors or logs (metadata-only: status, latency, token usage).
  4. Mock-transport tests cover multi-server response/error shapes (OpenAI/Ollama envelopes, `{"error"}` vs bare, empty/`length`-stop as errors, redirect refused, truncation-on-wire).
**Plans**: 1/1 complete — delivered, then repaired 2026-10-18 (`dc7f8c7`): `generate_reply` read a hardcoded INBOX UID 1 instead of the opened message, and the single-flight `RequestTracker` was constructed per call, making both single-flight (criterion 1) and `cancel_generate` silent no-ops. Both fixed; criterion 1's per-window single-flight now holds.

### Phase 41: Compose Integration + Hardening/UAT
**Goal**: User configures the LLM in settings and generates editable reply drafts from Reply/Reply-All windows, with the button-only privacy boundary proven end-to-end.
**Depends on**: Phase 40
**Requirements**: LLM-06, LLM-08, LLM-10, LLM-11
**Success Criteria** (what must be TRUE):
  1. User can fill base URL + model + key + template + defaults in a settings section (masked `••••` key, reset-to-default template, one-click Ollama localhost preset) and test-connection passes before first use.
  2. User can click Generate-with-LLM in Reply/Reply-All windows (loading + cancel), see the draft inserted above the quoted original fully editable, with threading headers and recipients untouched and send still manual via `queue_send`.
  3. User can regenerate a missed draft (re-call same prompt) without losing existing compose content — failed generation never clobbers the body.
  4. Network-off audit proves zero egress without a button click (no prefetch, no draft-sync of generated text, no telemetry with bodies); injection fixtures (hidden-div, fragmented-URL) render as inert draft text behind the human review gate.
**Plans**: TBD. **Prerequisite closed 2026-10-18:** criterion 2 requires a Reply window that can reach `queue_send` with real threading headers — that surface did not exist before `dc7f8c7` (Phase 38's buttons were inert), so this phase could not have passed its own criteria as previously scoped. The reply window, threading headers, and recipient population are now real; criterion 2 is reachable.

## Coverage

| Requirement | Phase | Status |
|-------------|-------|--------|
| LLM-01 (base URL + model settings) | Phase 39 | Implemented |
| LLM-02 (keyring API key) | Phase 39 | Implemented |
| LLM-03 (prompt template) | Phase 39 | Implemented |
| LLM-04 (generation defaults) | Phase 39 | Implemented |
| LLM-05 (test connection) | Phase 40 | Implemented |
| LLM-06 (Ollama preset) | Phase 41 | Pending |
| LLM-07 (Generate button + call) | Phase 40 | Implemented |
| LLM-08 (insert above quote) | Phase 41 | Pending |
| LLM-09 (plain-language errors) | Phase 40 | Implemented |
| LLM-10 (regenerate) | Phase 41 | Pending |
| LLM-11 (button-only egress) | Phase 41 | Pending |

✓ All 11 v1.8 requirements mapped (LLM-01 → Phase 39 … LLM-11 → Phase 41). No orphans, no duplicates.

Constraints carried from PROJECT.md / research:
- Backend-owned `llm` lane module + Tauri commands, never frontend fetch (CSP `connect-src ipc:`-only; key never enters WebView).
- Secrets split: non-secrets in new single-row SQLite `llm_settings` table (forward-only migration), API key ONLY in new keyring entry reusing Phase 5 helpers; never plaintext, never logged, never returned over IPC.
- BODY.PEEK-only on all read paths; `\Seen` invariant preserved; UID-only addressing; UIDVALIDITY-first gating; SessionManager single-session invariant.
- Zero new core deps: single Cargo.toml change = reqwest 0.12 features `["json", "stream", "native-tls"]` (native-tls wins over rustls; `cargo tree` gate: no rustls/ring).
- Mail content leaves the machine ONLY on explicit Generate-button click; no auto-send, no background generation; Reply/Reply-All windows only; single account; Linux only.
- Non-streaming first; Ollama preset + Regenerate are P1 (user-confirmed); streaming deferred to v1.8.x.
- Live endpoint calls only with user-provided key; CI uses hand-rolled loopback mock; live gate deferred-with-reason per project convention.

Plan-time decisions (for `/gsd-plan-phase`):
- `test_llm_connection` shape: `GET /models` probe (sends no mail content; pitfall-2 compliant).
- Body truncation cap: confirm at plan time (suggested default ~8k chars with disclosure marker; research range 8–12k).
- Migration index: confirm next free `M` number at Phase 39 plan time (research assumed M23 — verify).
- ARCHITECTURE.md `rustls` mentions are stale — read as `native-tls` throughout (Phase 39 planning may do a one-line correction pass).

Trace: builds on v1.7 Phase 37 (compose window) and the reply surface delivered by the **2026-10-18 repair commit `dc7f8c7`** — Phases 37–38 alone did *not* provide working threading headers or a reachable Reply path, so the earlier wording here ("threading headers + `queue_send` path") was inaccurate. Also builds on Phase 5 keyring helpers. Research: `.planning/research/SUMMARY.md` (HIGH confidence).

Status legend for the coverage table: **Validated** = proven end-to-end against the real thing; **Implemented** = code + tests exist, never exercised against a live endpoint/server; **Pending** = not built.

> ⚠️ No v1.8 requirement is **Validated**. The LLM lane has never been called against a real endpoint with a real key, so every Implemented row below rests on mock-transport and unit tests only.
