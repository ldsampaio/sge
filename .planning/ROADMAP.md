# SGE v1.8 Roadmap — LLM-Assisted Replies

Milestone: v1.8 LLM-Assisted Replies (brownfield; user-triggered reply drafting via configured OpenAI-compatible endpoint). Continues numbering from Phase 38 (v1.7 compose UX complete: phases 37–38).

Phase range: 39–41 (3 phases, strict dependency order per research; each independently verifiable).

## Phases

- [ ] **Phase 39: LLM Settings Foundation** — backend store + validation + prompt render (no IPC, no UI) [Plan 1/3 done: 39-01 M23 store + validation]
- [ ] **Phase 40: Generation Flow** — LLM client + single-egress IPC chokepoint (mock-tested, no UI)
- [ ] **Phase 41: Compose Integration + Hardening/UAT** — settings panel + Generate button + privacy audit

## Phase Details

### Phase 39: LLM Settings Foundation
**Goal**: Validated LLM settings persist securely (non-secrets in SQLite, key in keyring) with a fenced prompt renderer, independent of any UI.
**Depends on**: Phase 38 (v1.7 compose windows exist to integrate into later)
**Requirements**: LLM-01, LLM-02, LLM-03, LLM-04
**Success Criteria** (what must be TRUE):
  1. User's base URL + model + template + defaults survive an app restart, and the API key is absent from the SQLite file (keyring-only, presence-bool check).
  2. Invalid settings are rejected at save time (non-https non-loopback URL, empty model, out-of-range temperature/tokens/timeout) with plain-language reasons.
  3. Prompt template renders `{sender}`/`{subject}`/`{body}` with the body fenced as data (invisible HTML stripped, truncation cap applied with disclosure marker).
  4. `cargo tree` shows reqwest → hyper-tls → native-tls only (no `rustls`/`ring` after feature widening to `json`/`stream`/`native-tls`).
**Plans**: 1/3 complete (39-01 done: M23 store + validation; 39-02 keyring + renderer and 39-03 commands pending)

### Phase 40: Generation Flow
**Goal**: One backend command turns validated settings + a server-side original into draft text, with plain-language failures and no other egress path.
**Depends on**: Phase 39
**Requirements**: LLM-05, LLM-07, LLM-09
**Success Criteria** (what must be TRUE):
  1. `generate_reply{uid,mailbox}` returns draft text from `POST {base}/chat/completions` (non-streaming, configurable timeout, cancel, per-window single-flight) without any body crossing IPC to the frontend.
  2. `test_llm_connection` proves the endpoint works via `GET /models` (no mail content sent) before first generation.
  3. Every failure class maps to a plain-language error (unconfigured → settings link; 401 fails fast single-request; timeout/network → connectivity; provider 4xx/5xx → provider text) with retry only on 408/429/5xx — key and bodies never in errors or logs (metadata-only: status, latency, token usage).
  4. Mock-transport tests cover multi-server response/error shapes (OpenAI/Ollama envelopes, `{"error"}` vs bare, empty/`length`-stop as errors, redirect refused, truncation-on-wire).
**Plans**: TBD

### Phase 41: Compose Integration + Hardening/UAT
**Goal**: User configures the LLM in settings and generates editable reply drafts from Reply/Reply-All windows, with the button-only privacy boundary proven end-to-end.
**Depends on**: Phase 40
**Requirements**: LLM-06, LLM-08, LLM-10, LLM-11
**Success Criteria** (what must be TRUE):
  1. User can fill base URL + model + key + template + defaults in a settings section (masked `••••` key, reset-to-default template, one-click Ollama localhost preset) and test-connection passes before first use.
  2. User can click Generate-with-LLM in Reply/Reply-All windows (loading + cancel), see the draft inserted above the quoted original fully editable, with threading headers and recipients untouched and send still manual via `queue_send`.
  3. User can regenerate a missed draft (re-call same prompt) without losing existing compose content — failed generation never clobbers the body.
  4. Network-off audit proves zero egress without a button click (no prefetch, no draft-sync of generated text, no telemetry with bodies); injection fixtures (hidden-div, fragmented-URL) render as inert draft text behind the human review gate.
**Plans**: TBD

## Coverage

| Requirement | Phase | Status |
|-------------|-------|--------|
| LLM-01 (base URL + model settings) | Phase 39 | Pending |
| LLM-02 (keyring API key) | Phase 39 | Pending |
| LLM-03 (prompt template) | Phase 39 | Pending |
| LLM-04 (generation defaults) | Phase 39 | Pending |
| LLM-05 (test connection) | Phase 40 | Pending |
| LLM-06 (Ollama preset) | Phase 41 | Pending |
| LLM-07 (Generate button + call) | Phase 40 | Pending |
| LLM-08 (insert above quote) | Phase 41 | Pending |
| LLM-09 (plain-language errors) | Phase 40 | Pending |
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

Trace: builds on v1.7 Phases 37–38 (compose windows + threading headers + `queue_send` path, untouched by v1.8) and Phase 5 keyring helpers. Research: `.planning/research/SUMMARY.md` (HIGH confidence).
