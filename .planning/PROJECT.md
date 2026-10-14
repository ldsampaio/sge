# SGE — Linux IMAP Desktop Client

## What This Is

SGE is a Linux desktop email client built with Rust + Tauri v2 + React + SQLite. The user logs in with username, password, and IMAP server URL (e.g. mail.utfpr.edu.br); the app copies mail from the server to a local SQLite database (messages stay on the server, IMAP semantics) and displays them in a Gmail-like three-pane interface. Since v1.2 the app is a full mail client: triage, folders, drafts, and SMTP compose. Milestone v1.3 adds on-device automatic email classification (Laya) that organizes mail into an `Auto/` folder tree.

## Core Value

Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI — if this doesn't work, nothing else matters.

## Current State (v1.4 shipped 2026-10-13 ✅)

SGE is a Linux desktop email client (Rust + Tauri v2 + React + SQLite) with triage, folders, drafts, SMTP compose, on-device auto-classification, full-text search with saved filters, age-based auto-archive, and user-defined smart folder rules. v1.4 audit `.planning/v1.4-MILESTONE-AUDIT.md`: PASSED 11/11 (LIVE-01/02/03, SRCH-01/02/03, ARCH-01/02, RULE-01/02/03), 6/6 phases (21-26), 13 plans. Live-server runs deferred-with-reason (no creds file in this environment; exit-2 SKIP contracts verified). Environment caveat: `vite` bundle needs Node 20+ (env has Node 18); `tsc --noEmit` clean.

## Current Milestone (2026-10-14): v1.6 SMTP Send Pipeline ✅ v1.5 closed

**Status:** Defining requirements — continues from phase 27 (v1.4 shipped 2026-10-13, audit `.planning/v1.4-MILESTONE-AUDIT.md`: PASSED 11/11).

**Goal:** Close every open thread — prove the live-server gates with real credentials, verify the bundle and UI end-to-end, ship the deferred classifier/sync features, and harden compose (send/drafts/reply/reply-all).

**Target features:**
- Live validation with real creds — `live-21/22/26.sh` runs vs own account under the sandbox contract + draft-send gap closure (FAIL BY DESIGN → fix or explicit deferral)
- Bundle & UAT — Node 20+ `npm run build` check (rolldown-vite needs `node:util` styleText) + human click-through UAT (search/filter restore, archive trigger→undo, rules tracer)
- Classifier follow-throughs — rule import/export, learning-from-overrides, per-category auto-confirm, override history view, batch dry-run + retry-failed
- Sync protocol upgrades — IDLE push (poll stays fallback), CONDSTORE/QRESYNC fast path
- Sieve export — server-side rule export (previously out-of-scope as "a second product"; in scope tentatively, validate complexity at plan time)
- Compose hardening — send new email, save drafts, reply, reply-all verified end-to-end over the v1.2 pipelines (verify + fix gaps, not a rebuild)

**Scope guard (not in v1.5):** No cloud fallback; single account; Linux only; no Laya fine-tuning runtime changes beyond logging hooks needed for learning-from-overrides.

<details>
<summary>Previous milestone block (v1.4 in progress, archived at close)</summary>

## Current Milestone: v1.4 SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules

**Status:** Starting — continues from phase 21 (v1.3 phases 15-20 complete, audit `.planning/v1.3-MILESTONE-AUDIT.md`: PASSED 17/17, deferred live gates documented).

**Goal:** Close v1.2/v1.3 deferred live-validation gates against the real servers (`mail.utfpr.edu.br` / `smtp.utfpr.edu.br:587`) using user-provided credentials (git-ignored `src-tauri/.live-test.env`, never committed), under a strict sandbox safety contract — and add final capabilities: advanced search/filter, auto-archive by taxonomy age, smart folder rules (keyword/pattern auto-file).

**Target features:**
- SMTP Send (Live Gate, SANDBOXED) — test mail only to own address, Message-ID dedupe, Sent filing, retry/backoff visible
- Draft & Seen Round-Trip (Live Gate, SANDBOXED) — APPEND one-copy inside `SGE-Test-<run-id>`, Seen survives reconnect, `imap_outbox` replay, no silent resurrection, post-run cleanup
- Search + Filters — SQLite search (subject/body/attach), saved filters, multi-folder, persisted state
- Auto-Archive — aging into `Auto/Archive/`, journaled batch, taxonomy-driven
- Smart Folder Rules — validated keyword/sender rules → folder, conflict = user confirm, SQLite-persisted
- Live Validation Bundle — verification plan closing deferred `/gsd-verify-work` under the sandbox contract; document new deferrals

**Scope guard (not in v1.4):** No cloud fallback; no Sieve server-side; single account; Linux only.

**Sandbox safety contract (structural, all live phases):** dedicated `SGE-Test-<run-id>` folder per run; ALL mutating ops scoped to it; mail sent ONLY to the user's own address; existing mail/folders never touched; pre-flight guard aborts on out-of-sandbox targets; run ends with full cleanup. Offline doubles (MockSession/FakeSession, record-replay SMTP) stay as CI fallback.

</details>

## Next Milestone Goals

- Post-v1.5 outlook TBD during v1.5 execution (candidates: multi-account, Sieve follow-through if deferred, mobile/remote-access stories — all currently out of scope).
- v1.5 consumes the previous next-goals list (live runs, Node 20+ bundle check, click-through UAT, deferred ideas) — see Current Milestone above.

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

## Context

- Stack is fixed: Rust + Tauri v2 backend, React frontend, SQLite local store.
- Primary server example: mail.utfpr.edu.br (personal UTFPR mail use on Linux).
  - Incoming IMAP: mail.utfpr.edu.br, port 993/SSL.
  - Outgoing SMTP: smtp.utfpr.edu.br, port 587/STARTTLS (shipped v1.2).
- Sync strategy decided in questioning: headers-first for fast list, bodies on demand (not full bulk download up front).
- Layout decided: full three-pane Gmail look (sidebar + list + reader), not a minimal list.
- Auth UX decided: remember everything securely (OS keyring), auto-login next launch.
- v1.3 scoping (new-milestone questioning): classification moves mail physically (MOVE verb, Phase 10 machinery); target tree auto-created under root `Auto`; single-email flow always user-confirmed; batch flow unconfirmed with progress + report; secondary category is a local-only label; Laya runs as a bundled offline sidecar (multilingual checkpoint for pt-BR).

## Constraints

- **Tech stack**: Rust + Tauri v2 + React + SQLite — fixed by request
- **Platform**: Linux only
- **Protocol**: IMAP must leave mail on server (no silent expunge outside user-confirmed delete); moves are UID-only addressed with UIDVALIDITY epoch gating
- **Security**: Credentials at rest must use OS keyring, never plaintext
- **Classification**: fully offline (no mail content leaves the machine); sensitive-data rule — never reproduce senhas/códigos/dados sigilosos in justifications or logs; `Auto` root name reserved for the classifier tree

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

## Evolution

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

*Last updated: 2026-10-13 — Milestone v1.4 shipped ✅*