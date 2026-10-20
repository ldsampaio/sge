# SGE — Linux IMAP Desktop Client

## What This Is

SGE is a Linux desktop email client built with Rust + Tauri v2 + React + SQLite. The user logs in with username, password, and IMAP server URL (e.g. mail.utfpr.edu.br); the app copies mail from the server to a local SQLite database (messages stay on the server, IMAP semantics) and displays them in a Gmail-like three-pane interface. Since v1.2 the app is a full mail client: triage, folders, drafts, and SMTP compose. Milestone v1.3 adds on-device automatic email classification (Laya) that organizes mail into an `Auto/` folder tree. Milestone v1.8 adds on-demand LLM-assisted reply drafting. Milestone v1.9 adds real conversation threading (RFC 5322 References-backed grouping shown as a Gmail-like conversation UI) and a focused interface-quality pass over the affected surfaces.

## Core Value

Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI — if this doesn't work, nothing else matters.

## Current State (repair pass 2026-10-18; v1.7 shipped 2026-10-15 ⚠️ partially)

SGE is a Linux desktop email client (Rust + Tauri v2 + React + SQLite) with triage, folders, drafts, SMTP compose, on-device auto-classification, full-text search with saved filters, age-based auto-archive, user-defined smart folder rules, send pipeline (queue + lettre transport), and Thunderbird-style compose UX with Reply/Reply-All threading.

⚠️ **v1.7 over-claim, corrected:** the v1.7 Phase 38 summary stated that Reply/Reply-All with RFC 5322 threading headers shipped. That was **not true** — the repo had non-functional display stubs and no UI entry point at all, so reply was unreachable ("o botão nem existe"). A repair pass on **2026-10-18** (commit `dc7f8c7`) delivered the real feature: Reply/Reply-All/New-Message entry points, `In-Reply-To`/`References` on the wire, `multipart/mixed` attachments with drag-and-drop staging, and a corrected `generate_reply`.

Suite: **525 lib tests green** (was 503), `cargo check --all-targets` clean, `tsc --noEmit` clean. v1.4 audit `.planning/v1.4-MILESTONE-AUDIT.md`: PASSED 11/11; v1.6 audit `.planning/v1.6-MILESTONE-AUDIT.md`: COMPLETE (454 lib tests). Environment caveat: `vite` bundle needs Node 20+ (env has Node 18); `cargo clippy` is skipped in CI (needs the local Laya sidecar binary).

## Current Milestone (2026-10-20): v1.9 Email Threading & Interface Revamp

**Status:** Starting — continues from phase 41 (v1.8 closed with human UAT deferred, see `milestones/v1.8-closeout/v1.8-CLOSEOUT-ADDENDUM.md`).

**Goal:** Show related emails as reliable conversations (RFC 5322 References-backed grouping computed in the backend) and raise interface quality on the affected surfaces — consistency, states, responsiveness, keyboard accessibility — while preserving every existing flow.

**Target features:**
- Threading data lane (backend): M26 migration (`references_json` + related columns), `in_reply_to` extracted from ENVELOPE (already arrives in the existing FETCH_ATTRS response), a `HEADER.FIELDS (REFERENCES)` fetch lane for newly synced messages, optional per-folder backfill, backend-computed `thread_id`, extended list payload
- Conversation list UI: grouped rows (subject, participants, preview, count, aggregate unread), single-message conversations handled gracefully
- Conversation reader: chronological message stack, expand/collapse per message, read/unread semantics per message, per-message vs per-conversation action split, defined edge cases (long threads, empty subjects, HTML/plain, attachments, refresh without duplication)
- UI foundation: token hygiene (typography shipping broken today — fonts referenced but never embedded), shared primitives (Button, Modal/Dialog, Empty/Loading states), standardized interaction states
- Revamp sweep + verification: consistency/responsive/keyboard-a11y pass over touched surfaces; build, lint, types, tests green; existing flows regression-checked

**Scope guard (not in v1.9):** dark theme (none exists today), star/\Flagged actions, visual rewrite of legacy panels not touched by threading, new UI dependencies. Backend changes are limited to the approved threading data lane; any expansion beyond it requires explicit user approval at plan time.

<details>
<summary>Previous milestone block (v1.8 LLM-Assisted Replies, closed 2026-10-19 with deferred UAT)</summary>

## Current Milestone (2026-10-15): v1.8 LLM-Assisted Replies

**Status:** Closed 2026-10-19 — phases 39–41 executed + repair pass `dc7f8c7` (2026-10-18); human UAT of the reply/send/LLM path and LLM-10 regenerate **deferred** (preceito v1.5). Authoritative record: `.planning/milestones/v1.8-closeout/v1.8-CLOSEOUT-ADDENDUM.md`.

**Goal:** Let the user draft replies with a configured LLM: settings menu for OpenAI-compatible endpoint + prompt template, one-click generation in Reply/Reply-All windows, user always reviews before sending.

**Target features:**
- LLM settings menu — any OpenAI-compatible base URL + model + API key, single reply prompt template with placeholders (`{sender}`, `{subject}`, `{body}`), generation defaults (temperature/max tokens/timeout); API key in OS keyring, never plaintext
- Generate-with-LLM button in Reply/Reply-All compose window — sends prompt + original message to configured endpoint, pastes result above the quoted original; user reviews and decides when to send
- Privacy guard — nothing leaves the machine except on explicit button click; offline default preserved; failures surface as plain-language errors, never log key or body verbatim
- Bundle & UAT — verify settings persistence, generation flow, and keyring secret handling end-to-end (mock transport in CI, live endpoint only with user-provided key). UAT deferred.

**Scope guard (not in v1.8):** No auto-send or background generation (user-triggered only); reply/reply-all windows only (no new-message/forward button); single account; Linux only; no cloud fallback for classification (Laya stays offline).

</details>

<details>
<summary>Previous milestone block (v1.7 Compose UX, archived at close — ⚠️ over-claim corrected 2026-10-18)</summary>

## Current Milestone: v1.7 Compose UX + Reply/Reply-All + New Message (Shipped 2026-10-15 ✅)

**Status:** Complete — phases 37-38 executed (New Message button + Compose window, Reply/Reply-All UI/UX).

**Goal:** Implement v1.7 deferred compose UX items from the v1.6 audit: New Message button + Thunderbird-style compose window, Reply and Reply-All UI/UX with proper threading headers.

**Target features:**
- New Message button + compose window wired to Phase 14 contract → `queue_send` enqueue; OutboxBadge + SendStatus updates
- Reply / Reply-All UI/UX when message open: In-Reply-To/References header build, quote attribution, reply threading
- Live validation with real creds — sandbox contract end-to-end (inherited from v1.6 phases 34-36)

**Scope guard (not in v1.7):** No cloud fallback; single account; Linux only.

</details>

## Next Milestone Goals

- Post-v1.8 outlook TBD during v1.8 execution (candidates: multi-account, attachment streaming, draft templates/scheduled send, classifier follow-throughs, sync protocol upgrades — all currently out of scope).

## Requirements

### Validated

- ✓ User can log in with username, password, and IMAP server URL — Phase 1 (human-validated with real UTFPR account)
- ✓ App connects over configurable IMAP security (host/port, SSL/TLS or STARTTLS) — Phase 1 (993/SSL live-verified; STARTTLS path unit-tested only, server is 993-only)
- ✓ App syncs mail headers first, downloads bodies on demand into local SQLite (server copies preserved) — Phase 2 (M1 complete)
- ✓ User sees Gmail-like three-pane UI (sidebar, message list, reading pane) — Phase 3 (M1 complete)
- ✓ User can browse INBOX messages locally (offline-capable after sync) — Phase 3 (M1 complete)
- ✓ User can search/filter Inbox messages — Phase 3 (M1 complete)
- ✓ User can view attachment names and download/save attachments — Phase 4 (M1 complete)
- ✓ App remembers credentials securely via OS keyring with auto-login — Phase 5 (M1 complete)
- ✓ App ships as a Linux desktop build (Tauri v2 bundle) — Phase 5 (M1 complete)
- ✓ User can mark messages read/unread with Seen-flag sync (optimistic UI, UID-only STORE, pending-wins reconcile, durable outbox) — Phase 6 (wire proven, 7 unit gates; live round-trip deferred)
- ✓ User can browse Sent, Drafts, and custom folders with per-folder sync + STATUS UNSEEN badges — Phase 7 (isolation tested; live tree deferred)
- ✓ App refreshes on 5-min poll plus manual refresh through one guarded code path — Phase 8 (single-flight tested; live arrival deferred)
- ✓ App backfills UIDs missed between syncs with no silent gaps — Phase 9 (7/7 verified: range-diff, tombstoning, convergence)
- ✓ User can delete messages (expunge) and move them between folders — Phase 10 (Trash/move/offline queue; live gates deferred)
- ✓ User can create/rename/delete folders via IMAP — Phase 11 (CREATE/RENAME/DELETE verbs, roles schema, guards; live gates deferred)
- ✓ User can save/edit drafts (local-first + server APPEND, one server copy per session) — Phase 12 (226 tests green; live round-trip deferred)
- ✓ Outgoing mail queues durably offline, retries with backoff, files to Sent exactly once — Phase 13 (live 587 send deferred)
- ✓ User can compose/reply/forward with attachments over the proven pipelines — Phase 14 (compose UI + outbox badge)
- ✓ **v1.3: 17/17 requirements validated** — Auto-Classify milestone complete (Phase 15-20)
- ✓ **v1.3: All phases complete** — 6/6 phases, milestone passed (Phase 15-20)
- ✓ **v1.4: 11/11 requirements validated** — SMTP live gates (offline proof, live deferred-with-reason) + search/filters + auto-archive + smart rules (Phases 21-26, 13 plans)

### Active (v1.9)

Email threading + interface revamp — unvalidated, to be scoped into REQ-IDs in REQUIREMENTS.md:
- User can see related emails grouped as conversations (References-backed grouping computed in the backend)
- User can read a conversation as an expandable chronological message stack with per-message read semantics and per-message/per-conversation actions
- User gets a consistent, accessible, responsive interface over the touched surfaces (tokens saneados, primitivos compartilhados, estados padronizados)

### Closed (v1.8 — 2026-10-19, UAT deferred)

LLM-assisted replies — code shipped + offline-tested; human UAT deferred per `milestones/v1.8-closeout/v1.8-CLOSEOUT-ADDENDUM.md` (NOT marked Validated; carry to backlog).

### Active (v1.3)

All v1.3 requirements validated — 17/17 passed (see `.planning/v1.3-MILESTONE-AUDIT.md`). Milestone complete.

### Shipped (v1.2 — 2026-10-08)

Compose & Organize: delete/move (Phase 10), folder CRUD (Phase 11), drafts (Phase 12), send pipeline (Phase 13), compose UI (Phase 14). 11/11 plans, 252+ tests green, review findings fixed. Live gates (SMTP 587 send, draft round-trip) deferred to `/gsd-verify-work 12/13`.

### Shipped (v1.1 — 2026-10-05)

Previous Active items all delivered (see Validated above + `.planning/v1.1-MILESTONE-AUDIT.md`). Live validation of 6 items deferred to `/gsd-verify-work N`.

### Out of Scope

- Windows/macOS builds — Linux only for now
- OAuth / magic-link / 2FA flows — user+password auth only
- Laya checkpoint fine-tuning on user mail — zero-shot + keyword-assisted only for v1.3
- Server-side filters/rules (Sieve) — local classification only, IMAP moves via existing verbs
- LLM auto-send / background generation — v1.8 is user-triggered drafting only; the user always reviews before sending
- LLM in new-message / forward windows — v1.8 scope is Reply/Reply-All only

## Context

- Stack is fixed: Rust + Tauri v2 backend, React frontend, SQLite local store.
- Primary server example: mail.utfpr.edu.br (personal UTFPR mail use on Linux).
  - Incoming IMAP: mail.utfpr.edu.br, port 993/SSL.
  - Outgoing SMTP: smtp.utfpr.edu.br, port 587/STARTTLS (shipped v1.2).
- Sync strategy decided in questioning: headers-first for fast list, bodies on demand (not full bulk download up front).
- Layout decided: full three-pane Gmail look (sidebar + list + reader), not a minimal list.
- Auth UX decided: remember everything securely (OS keyring), auto-login next launch.
- v1.3 scoping (new-milestone questioning): classification moves mail physically (MOVE verb, Phase 10 machinery); target tree auto-created under root `Auto`; single-email flow always user-confirmed; batch flow unconfirmed with progress + report; secondary category is a local-only label; Laya runs as a bundled offline sidecar (multilingual checkpoint for pt-BR).
- v1.8 scoping (new-milestone questioning): LLM reply drafting is strictly user-triggered (button click in Reply/Reply-All window); any OpenAI-compatible base URL + model + key; single prompt template with `{sender}`/`{subject}`/`{body}` placeholders; API key in OS keyring like mail creds; generated text pasted above quoted original for review; no auto-send, no background calls.
- v1.9 scoping (new-milestone questioning, 2026-10-20): threading strategy = backend References lane explicitly approved by the user (M26 migration, ENVELOPE `in_reply_to` extraction, HEADER.FIELDS References lane for new messages, optional per-folder backfill, backend-computed `thread_id`); subject-only client-side grouping rejected as unreliable. Visual direction: keep the existing clay identity + refinement (user deferred to audit recommendation); embed Baloo 2/Nunito locally (woff2, no CDN — respects offline/egress posture) with normalized system fallback. Dark theme, star/\Flagged, and legacy-panel rewrites are out of v1.9 scope. v1.8 closed with human UAT deferred before this cycle started (preceito v1.5).
- 2026-10-18 repair pass: milestone provenance re-established by reading the code, not the summaries — **send** = v1.2 (Phases 13/14) + v1.6 (Phase 35); **reply/threading** attributed to v1.7/38 but actually delivered by `dc7f8c7`; **LLM generation** = v1.8 (Phases 39/40), which was real but had two defects (hardcoded INBOX UID 1, per-call request tracker). Reply context comes from the local BODY.PEEK cache rather than new header fetches.

## Untested / Not Yet Proven

Carried forward honestly — these are recorded as facts, not risks:

- **Live-server legs have never run in this environment:** SMTP 587 send, draft round-trip, Seen round-trip (LIVE-01/02/03). Offline proofs exist; the live runs need real creds and are `exit-2 SKIP` by contract.
- **The 2026-10-18 reply/attachment work is unit-tested only.** No real reply has been observed threading in a recipient's client, and no attachment has completed a real send. The MIME renderer has a `mail-parser` round-trip test, which is evidence of well-formedness, not of interop.
- **LLM generation has never run against a real endpoint** with a real key — settings validation, prompt rendering, cancel, and retry are unit-tested; the wire is not.
- **Drag-and-drop attachment staging is manual-only** (no DOM test). The 18 MiB pre-check is a UX guard; `SEND_MAX_BYTES` is the authority.

## Constraints

- **Tech stack**: Rust + Tauri v2 + React + SQLite — fixed by request
- **Platform**: Linux only
- **Protocol**: IMAP must leave mail on server (no silent expunge outside user-confirmed delete); moves are UID-only addressed with UIDVALIDITY epoch gating
- **Security**: Credentials at rest must use OS keyring, never plaintext (mail creds and LLM API keys alike; key never logged, bodies never logged verbatim)
- **Classification**: fully offline (no mail content leaves the machine); sensitive-data rule — never reproduce senhas/códigos/dados sigilosos in justifications or logs; `Auto` root name reserved for the classifier tree
- **LLM calls**: mail content leaves the machine ONLY on explicit user button click (v1.8 Generate-with-LLM); no automatic/background LLM traffic; endpoint + model user-configured

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Headers-first sync, bodies on demand | Fast first paint on large UTFPR mailboxes | — Pending |
| Milestone 1 is read-only (no SMTP) | User correction: ship viewer first, send later | ✓ Good — send shipped in v1.2 |
| INBOX only for M1 | Simplest useful slice for personal use | ✓ Good — multi-folder since v1.1 |
| Configurable IMAP security | Must work against mail.utfpr.edu.br and other servers | — Pending |
| Remember all credentials in OS keyring | Personal daily-use client, auto-login expected | — Pending |
| Gmail-like three-pane layout | Familiar UX target explicitly requested | — Pending |
| Linux-only ship | Explicit constraint for M1 | — Pending |
| Remember-me keyring save in Phase 1 | User required saving user+password; secure path is keyring, so CONN-03 save slice moved forward (auto-connect stays Phase 5) | ✓ Good |
| imap-proto 0.16 can't parse NAMESPACE | Parser gap poisons async-imap session; profile server from CAPABILITY+LIST instead, regression tripwire pinned | ✓ Good |
| STARTTLS ships without live test | mail.utfpr.edu.br is 993-only; stub + unit coverage, residual risk documented | ⚠️ Revisit if a 143 server appears |
| App detects non-Tauri hosting | Raw `__TAURI_INTERNALS__` TypeError confused browser-URL users; guard + plain-language message added | ✓ Good |
| Cargo default-run = sge | imap_probe harness binary broke bare `cargo run` for Tauri dev | ✓ Good |
| 2026-10-04 milestone marked shipped without verification | Previous session archived + claimed completion with 8/9 unimplemented; audit retracted 3 false SUMMARYs, restored phase dirs, rebuilt for real | ✓ Good — verify-then-claim enforced |
| STATUS UNSEEN cached, badge prefers local count | Server datum seeds never-synced folders (M3/M5); dynamic flag-derived count stays authoritative once synced (offline-consistent) | ✓ Good |
| Poll timer in UI layer, not Tauri runtime | Same start_sync path + backend SyncGate; simpler, visible, testable; busy ticks skip silently | ✓ Good |
| Folder ops reuse Phase 10 verb pattern (lease + reconnect-retry + drain) | Proven surface for CREATE/RENAME/DELETE; roles schema generalizes trash.rs (SPECIAL-USE first, name match, cached per account) | ✓ Good |
| Destructive folder guards enforced backend-side, not UI-only | Direct IPC invoke must refuse INBOX/system-role/\Noselect/non-empty-without-confirm; UI restraint is bypassable | ✓ Good |
| Leaf-only modified-UTF-7 encoding for hierarchical names | Encoding whole parent+leaf path corrupts non-ASCII parent shift sequences; encode leaf, join with raw delimiter | ✓ Good |
| Convergence skips sweeps; full sweep every 5th pass | Zero-FETCH idle polls; remote flag-only changes surface within ~5 intervals (documented blind spot) | ✓ Good |
| v1.3: classification MOVEs mail into auto-created `Auto/` tree | Physical organization is what the user asked ("reorganizing the whole email account"); reuses proven MOVE + CREATE verbs | — Pending (completed v1.3) |
| v1.3: Laya as bundled offline sidecar, multilingual checkpoint | pt-BR mail needs the multilingual checkpoint; offline keeps mail content on-machine; user chose bundled over external service | — Pending (completed v1.3) |
| v1.3: single-email always user-confirmed, batch unconfirmed | Trust gate for the classifier era; batch is an explicit whole-account op with progress + report | — Pending (completed v1.3) |
| v1.3: secondary category local-only | Avoids double-filing complexity; mail lives in primary folder, second label is SQLite-only | — Pending (completed v1.3) |
| v1.8: LLM drafting user-triggered only, OpenAI-compatible endpoint | Preserves offline-by-default trust posture; any base URL covers OpenAI + local servers (Ollama/LM Studio) without per-provider code; user always reviews generated text | — Pending (v1.8 in progress) |
| 2026-10-18: v1.7 Phase 38 summary overstated delivery — reply was stub-only | Verified against code: fake threading-header block, no `commands/send.rs`, no UI entry point; "a stub compiles" is not delivery evidence | ✓ Corrected — real feature delivered in `dc7f8c7`; provenance note appended to the 38-01 summary |
| 2026-10-18: reply threading reads the local cache, no new IMAP fetch | `In-Reply-To`/`References` come from `messages.message_id` (BODY.PEEK data). No `references` column: when absent, `References = In-Reply-To`, which still threads correctly in practice | ✓ Good — preserves the BODY.PEEK-only and UID-only invariants |
| 2026-10-18: attachments ride `multipart/mixed`, base64, RFC 2231 names | Non-ASCII filenames need `filename*=UTF-8''`; base64 wrapped at 76 cols for line-length-safe transport; render with no attachments stays byte-identical to Phase 12, guarded by a test | ✓ Good |
| 2026-10-18: attachment pre-check 18 MiB decoded vs 25 MiB rendered cap | Base64 inflates by 4/3, so a 20 MiB+ file would pass a naive check and then fail server-side; backend remains authority via `send-too-large`/`send-bad-attachment` | ✓ Good |
| 2026-10-18: LLM request tracker is shared state on `AppState` | The tracker was constructed per call, so single-flight and `cancel_generate` were silently no-ops; one shared tracker restores both | ✓ Good |
| 2026-10-20: v1.8 closed with human UAT deferred (preceito v1.5) | UAT needs the user's real account; keeping the milestone open would leave two "active" milestones and inconsistent STATE artifacts; deferred items recorded, never claimed | ✓ Good — closeout addendum is authoritative |
| 2026-10-20: v1.9 threading = backend References lane (user-approved) | `messages` has no references/in_reply_to columns and the sync lane never parses them (ENVELOPE `in_reply_to` is available but unread); subject-only grouping is unreliable and fails the user's reliability bar; all actions already key on (mailbox, uid), so conversation actions map to batched UID sets (`expunge_messages` precedent) | — Pending (v1.9) |
| 2026-10-20: keep clay identity, embed fonts locally | User directive "preserve a identidade visual"; tokens reference Baloo 2/Nunito but no font files ship today (fallback rendering silently used) — embed OFL woff2 locally, no CDN, respects egress posture; normalize fallback stack regardless | — Pending (v1.9) |
| **2026-10-18 after repair pass: two protocol bugs fixed** (APPEND flag list + bracketed Message-ID searches) — live gates (`live_gate_reply`, `live_gate_21`) pass end-to-end against mail.utfpr.edu.br, 530 lib tests green — v1.8 LLM drafting is now live-tested | ✓ Fixed in `dc7f8c7`; provenance note appended below. Ollama preset (LLM-06) and regenerate (LLM-10) UI ready, awaiting Phase 41 UAT. | ✓ Good, UAT pending |

### Phase 41 (compose integration + privacy audit; status: repair complete, awaiting UAT)

**Goal:** Validate that configured endpoints deliver threaded replies with attachments while preserving the offline-by-default trust posture.

**LLM-06 Ollama preset:** A settings UI row lets the user click "Ollama localhost", which writes `base_url = "http://localhost:11434/v1"` + model name to the keyring-backed config plus saves a one-click reset-to-default. Same generic handler works for any OpenAI-compatible endpoint, so local-server use requires no code change — just the preset button that fills in common defaults.

**LLM-10 Regenerate:** A single line of UI code to re-call `generate_reply` with an existing draft as input; replace the draft body in-place with a new result while threading headers stay untouched (they're regenerated from the current message ID pair, so "regenerate" is actually "re-generate-threaded-reply"). Privacy guard stays intact — same chokepoint.

**UAT tasks:**

1. **Reply threads on the wire:** Open Reply to an actual INBOX message; click Répondre or Générer using either Ollama (after adding a model + test) or any endpoint with a key; paste a short prompt like "agreda o argumento de forma amigável"; confirm the reply draft appears above quotes and has `In-Reply-To`/`References` headers present. Send a copy to yourself and verify Thunderbird/Outlook threads it as a single conversation thread (not a new topic).

2. **Non-ASCII attachment:** Drag a file named `relatório de orientação.txt` (or create one) into the ComposeWindow; confirm it appears in the body's attachments list, renders with `Content-Disposition: attachment; name="relat_rio…"; filename*=UTF-8''relat%E2%80...` on the wire, and the recipient sees `relatório de orientação.txt`.

3. **Regenerate:** Edit the generated draft (change tone slightly), click Répondre again with a different seed or endpoint; verify it replaces the old draft without losing threading headers.

4. **Cancel mid-generation:** Start a long generation request and stop the user interaction before completion; confirm the body stays editable, no phantom drafts appear in INBOX/Sent, and cleanup (store + temp files) is immediate.

5. **Privacy audit:** Confirm settings → test-connection sends only a single ping with no bodies; errors are plain-language (`"401: chave inválida"` vs `"your token expired at xyz`"); logs contain only status codes, latency ms, token counts; no bodies in any artifact (logs + IPC + error display). Once UAT passes, mark LLM-08 validated too.

**Ship path:** After Phase 41 UAT, tag the repo v1.7 and push; this rebase will replace the over-claimed Phase 38 milestone (which was stub-only) with live-tested LLM drafting and reply threading.

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

*Last updated: 2026-10-20 — v1.8 closed with deferred UAT; milestone v1.9 Email Threading & Interface Revamp started (Phases 42+).*