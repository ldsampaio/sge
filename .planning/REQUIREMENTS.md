# SGE v1.9 Requirements — Email Threading & Interface Revamp

Milestone: v1.9 Email Threading & Interface Revamp (brownfield; continues from Phase 41, v1.8 closed with deferred human UAT — see `milestones/v1.8-closeout/`).
Research: `.planning/research/SUMMARY.md` (+ STACK/FEATURES/ARCHITECTURE/PITFALLS.md), confidence HIGH.

User decisions of 2026-10-20:

- v1.8 closed with human UAT deferred (preceito v1.5). Deferred items are backlog, not claims.
- Threading strategy = **backend References lane** (subject-only client-side grouping rejected as unreliable).
- Visual direction = keep clay identity + refinement; fonts vendored locally.
- Scope = ALL categories (THR/CNV/READ/ACT/REVAMP/VER) + **P3 differentiators included** (user override of the recommended deferral).

Constraints (carried from `.planning/PROJECT.md` / research):

- BODY.PEEK-only on all read paths (the new References fetch lane is `BODY.PEEK[HEADER.FIELDS (REFERENCES)]`, `peek_audit` gains path 5 in the same commit); UID-only addressing; UIDVALIDITY-first gating; SessionManager single-session invariant; mail stays on server.
- Threading is scoped per folder: `thread_key = (mailbox_id, thread_id)`; no cross-folder joins; NULL `thread_id` = own-thread (byte-compat single-message rows).
- All SQL stays inside `store/` (the 25-migration "no raw SQL escapes queries.rs" invariant); M26 bumps SCHEMA_VERSION 25→26 in the same commit (M23 off-by-one precedent).
- One approved new Rust dep: `mail-threading 0.1.3` (JWZ/RFC 5256; chrono-only dep tree). Zero new npm deps; UI primitives are hand-rolled React + CSS tokens.
- msg-ids must NOT be routed through `decode_header_words` (RFC 2047 would corrupt exact-match joins); strict `<...>`-framing parser.
- No new UI surfaces read/write mail outside existing actions; the LLM privacy posture (v1.8) is untouched by v1.9.
- Live-server gates follow the project's deferred-with-reason convention (no creds in this environment).

## Requirements (v1.9 — scoped)

### THR — Threading data lane (backend)

- [x] **THR-01**: The app extracts `in_reply_to` from the ENVELOPE it already fetches and `references` via a new `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` UID-range lane for newly synchronized messages, with an optional per-folder backfill sweep, persisting both in migration 26 (`messages.in_reply_to`, `messages.references_json`, indexed `messages.thread_id`)
- [x] **THR-02**: The app computes a stable `thread_id` from the References/In-Reply-To chain (JWZ via `mail-threading`, `subject_merge: false`), recomputed as a per-folder dirty-set epoch at sync-end/backfill/send-file hooks with a documented `(date_utc, uid)` tiebreak order — never per-insert
- [ ] **THR-03**: The list payload returns conversation-aware rows behind the unchanged `list_messages` name/signature (`thread_id`, `thread_count`, latest-message preview + date, participants), and a new `get_thread` command returns header-only member summaries (uid/from/date/flags/preview — no bodies) for the reader
- [x] **THR-04**: All protocol/store invariants hold after the threading lane — BODY.PEEK-only, `\Seen` never set on fetch, UID-only + UIDVALIDITY gating, per-folder thread scoping, and `peek_audit`/`fetch_attrs_are_parenthesized` guards extended to the new lane in the same phase

### CNV — Conversation list

- [ ] **CNV-01**: User sees related messages grouped into one row (count badge shown only when ≥2); a single-message thread renders exactly like today's row (no badge, no layout shift)
- [ ] **CNV-02**: The conversation row aggregates the thread's unread state (bold dot + unread count) computed from per-member flags carried in the payload
- [ ] **CNV-03**: The conversation row shows deduplicated/truncated senders ("A · B +2", pt-BR suffix), the latest message's date (right-aligned, `formatRowDate` reused) and the latest message's preview
- [ ] **CNV-04**: Subjects render without `Re:`-prefix noise (minimal client-side trim) and keep the existing "(sem assunto)" fallback
- [ ] **CNV-05**: Refresh/reconnect never duplicates a conversation row; a newly arrived reply merges into the existing row (rows key on backend-stable `thread_id`)
- [ ] **CNV-06**: Pagination stays message-keyed (50/page); when a conversation is split across pages the slice count is indicated visibly and the behavior documented — never fetch-ahead
- [ ] **CNV-07**: The row renders a participant avatar-initial cluster and the split "N não lidas" chip apart from the count badge (P3)
- [ ] **CNV-08**: Type-ahead on the list is included only if it stays trivial (simple prefix match over the loaded page); otherwise it is explicitly recorded as deferred — no bespoke fuzzy engine (P3, triviality-gated)

### READ — Conversation reader

- [ ] **READ-01**: User reads a conversation as a chronological stack (oldest at top, newest at bottom) with per-message expand/collapse
- [ ] **READ-02**: Opening defaults expand messages up to and including the first unread (or only the latest when there is no unread); older history collapses to headers
- [ ] **READ-03**: Bodies load lazily per expanded card through the existing `fetch_message` (never batched); loading/error states are per card, not per conversation
- [ ] **READ-04**: Read semantics are per message: an expanded card marks read via the existing `set_seen` + `FLAG_UPDATE_EVENT` path; per-message mark-unread is preserved
- [ ] **READ-05**: Reply / Reply-All act on the specific card and keep the v1.7 threading-header contract (`In-Reply-To`/`References` seeding unchanged)
- [ ] **READ-06**: Message bodies keep the existing sanitization pipeline (ammonia + sandboxed iframe / text `<pre>`) rendered per card; attachments render per message
- [ ] **READ-07**: The reader header shows the thread's `To:` participants line (payload-rendered) (P3)
- [ ] **READ-08**: Opening a conversation auto-scrolls to the first unread (or last expanded) card with `prefers-reduced-motion` respected (P3)
- [ ] **READ-09**: Plain-text bodies get a quoted-text trimmer for the inline quote blob (heuristic, per-card toggle, default ON; HTML quotes untouched — no destructive editing) (P3)
- [ ] **READ-10**: Long threads fold earlier messages into a header-only "Mostrar N mensagens anteriores" disclosure that loads lazily (P3)

### ACT — Actions: message vs conversation

- [ ] **ACT-01**: Hover actions on a multi-message conversation row apply to the whole conversation by default (delete/move through one new batched backend command reusing the existing outbox UNIQUES), labeled so the user can tell conversation-scope from message-scope
- [ ] **ACT-02**: A conversation-level operation produces exactly ONE undo entry (expansion of the undo path to a batch form — backend contract change approved in this requirements round)
- [ ] **ACT-03**: The user can mark a whole conversation read from the list in one action
- [ ] **ACT-04**: Per-message actions (delete/move that one member) remain reachable and visually distinguishable from the card's body frame in the reader

### REVAMP — Interface foundation + sweep

- [ ] **REV-01**: Design tokens are de-drifted (typography actually ships — Baloo 2 + Nunito vendored woff2 with OFL licenses, `@font-face` + `font-display: swap`, no CDN) and the system fallback stack normalized; CSP gains `font-src 'self'`
- [ ] **REV-02**: Shared UI primitives (`Button`, `Dialog/Modal`, `EmptyState`, `Loading`) live in `components/ui/` on top of tokens.css — hand-rolled, zero new npm deps — and are adopted on the touched surfaces
- [ ] **REV-03**: `MessageListRow.tsx` is extracted ONCE before grouping lands and is semantically valid (no interactive element nested inside the row surface — fixes the current button-in-button a11y bug)
- [ ] **REV-04**: Hover/focus-visible/active/selected/disabled/loading/empty/error states are standardized across the touched surfaces (list rows, reader, compose-adjacent chips)
- [ ] **REV-05**: The list supports keyboard triage — arrow-key roving focus, Home/End, Enter opens, selection ≠ focus, `:focus-visible`-only rings — implemented after/with REV-03's semantic fix, not before
- [ ] **REV-06**: The touched surfaces remain responsive (three-pane compression behavior defined for narrow desktop windows)

### VER — Verification gates

- [ ] **VER-01**: Gates pass — `tsc`, `eslint`, `cargo fmt --check`, `cargo test` (including: new References parser fixtures with byte-captured as-received IMAP responses, fresh+upgrade migration fixture pair for M26, preserve-rows test, both-sides serde round-trip tests for every new IPC command)
- [ ] **VER-02**: The research MVP gate holds end-to-end — a real References-linked pair shows as ONE row with badge/aggregate-unread/latest-date/snippet; single-message threads render byte-compatible; a 3+ message thread expands without fetching unexpanded bodies; card-reply keeps threading headers; conversation ops undo in one entry; refresh/reconnect shows no dupes; keyboard triage works (live UTFPR leg deferred-with-reason if no creds available, per project convention)

## Future Requirements (deferred) — v9.x candidates

- Conversation marking with \Flagged/stars (no \Flagged STORE path exists today)
- Dark theme (no theme system today; token layer must stay readable without it)
- Cross-folder conversation stitching (SENT-INBOX unified thread view) — today scoped per folder by design
- aria-activedescendant list navigation and combobox pattern on search
- Virtualized infinite scroll (revisit only if pagination UX ever becomes the bottleneck)

## Out of Scope (explicit exclusions with reasoning)

- Client-side subject-only grouping — false joins on `Re:<same subject>`/empty subjects; the References lane exists precisely to avoid this bug; single-message thread until the parent arrives (honest, user-visible)
- In-line list expansion (nested focus inside `role="option"` rows) — APG forbids interactive content within the option surface; the reader is the expansion surface
- All-expanded-by-default reader — N sandboxed iframes + N BODY.PEEK fetches on open; contradicts lazy-load design
- Gmail "whole-conversation-read-on-open" semantics — conflicts with per-message UID/`\Seen` rules and churns server flags
- Inbox >100-message thread caps — personal-mailbox volumes never reach it; complexity without benefit
- Drag reorder/pinning of threads — IMAP ordering is date-keyed by design; no persistence surface
- New message-summary libraries (`use-email-id` etc.) — in-house strict-RFC parser follows the fail-visible house pattern
- Visual rewrites of panels untouched by threading (TaxonomyEditor/BatchPanel/ArchivePanel/RulesPanel beyond primitive reuse) — revamp discipline: touched surfaces only

## Traceability

| Requirement | Phase | Plan | Status |
|-------------|-------|------|--------|
| THR-01 | Phase 42 | TBD | Complete |
| THR-02 | Phase 42 | TBD | Complete |
| THR-03 | Phase 42 | TBD | Pending |
| THR-04 | Phase 42 | TBD | Complete |
| REV-01 | Phase 43 | TBD | Pending |
| REV-02 | Phase 43 | TBD | Pending |
| REV-03 | Phase 43 | TBD | Pending |
| CNV-01 | Phase 44 | TBD | Pending |
| CNV-02 | Phase 44 | TBD | Pending |
| CNV-03 | Phase 44 | TBD | Pending |
| CNV-04 | Phase 44 | TBD | Pending |
| CNV-05 | Phase 44 | TBD | Pending |
| CNV-06 | Phase 44 | TBD | Pending |
| CNV-07 | Phase 44 | TBD | Pending |
| CNV-08 | Phase 44 | TBD | Pending |
| READ-01 | Phase 45 | TBD | Pending |
| READ-02 | Phase 45 | TBD | Pending |
| READ-03 | Phase 45 | TBD | Pending |
| READ-04 | Phase 45 | TBD | Pending |
| READ-05 | Phase 45 | TBD | Pending |
| READ-06 | Phase 45 | TBD | Pending |
| READ-07 | Phase 45 | TBD | Pending |
| READ-08 | Phase 45 | TBD | Pending |
| READ-09 | Phase 45 | TBD | Pending |
| READ-10 | Phase 45 | TBD | Pending |
| ACT-01 | Phase 45 | TBD | Pending |
| ACT-02 | Phase 45 | TBD | Pending |
| ACT-03 | Phase 45 | TBD | Pending |
| ACT-04 | Phase 45 | TBD | Pending |
| REV-04 | Phase 46 | TBD | Pending |
| REV-05 | Phase 46 | TBD | Pending |
| REV-06 | Phase 46 | TBD | Pending |
| VER-01 | Phase 46 | TBD | Pending |
| VER-02 | Phase 46 | TBD | Pending |
