# Roadmap: SGE

Milestone: **v1.9 — Email Threading & Interface Revamp** (brownfield; continues from Phase 41 / v1.8).

Two lanes, one collision point: Lane 1 is a pure-backend threading lane (M26 migration, References header fetch, JWZ thread recompute); Lane 2 is the UI foundation (tokens/fonts/primitives + the ONCE-only `MessageListRow` extraction). The lanes meet only in `MessageList.tsx` row markup — the foundation phase writes that file exactly once, **before** any grouping feature touches it. Conversation UI then consumes both lanes; the revamp sweep is consumption-only and verification closes the milestone. Phases execute in strict numeric order (42 → 43 → 44 → 45 → 46); Phase 43 is parallel-safe with 42 in principle (zero backend dependency) but is not parallelized in this project unless the executor explicitly opts in.

> **Anti-collision directive** (from research ARCHITECTURE/SUMMARY): the shared-UI foundation phase (Phase 43, incl. the `MessageListRow` extraction with the nested-button a11y fix) must land before Phase 44 changes row markup; the threading backend lane (Phase 42) must land before any conversation UI phase. Up-front decisions owned by later phase planning: the `list_threads` pagination/COUNT–OFFSET contract (Phase 44 plan review), the thread-summary payload shape + batch-undo backend contract (flagged for explicit plan-review approval inside Phases 42/45).

## Phases

- [ ] **Phase 42: Threading Backend Lane** — in_reply_to/references persisted (M26), References fetch lane (BODY.PEEK-only), backend-computed per-folder `thread_id`, extended list payload + `get_thread`
- [ ] **Phase 43: UI Foundation & MessageListRow Extraction** — vendored fonts, shared primitives, standardized states, and the ONCE-only semantically valid row extraction
- [ ] **Phase 44: Conversation List** — grouped rows over the frozen `list_messages` contract: badge, aggregate unread, senders, refresh-safe merging, pagination policy
- [ ] **Phase 45: Conversation Reader + Message-vs-Conversation Actions** — chronological card stack with per-message semantics, batched conversation actions with single undo
- [ ] **Phase 46: Revamp Sweep + Verification** — consumption-only state/responsive/keyboard pass over touched surfaces, full gate suite + threading MVP gate

## Phase Details

### Phase 42: Threading Backend Lane

**Goal**: Related emails are reliably linked in the data layer — `in_reply_to`/`references` stored, a stable per-folder `thread_id` computed by the backend — while the flat list payload and rendering stay *completely unchanged*, making the lane independently shippable and zero-risk to the UI.
**Depends on**: Nothing (first phase; backend-only, no UI work)
**Requirements**: THR-01, THR-02, THR-03, THR-04
**Plans**: 1/7 plans executed (waves 1: 01+02, 2: 03+04, 3: 05+06, 4: 07)

- [x] 42-01-PLAN.md
- [ ] 42-02-PLAN.md
- [ ] 42-03-PLAN.md
- [ ] 42-04-PLAN.md
- [ ] 42-05-PLAN.md
- [ ] 42-06-PLAN.md
- [ ] 42-07-PLAN.md

**Success Criteria** (what must be TRUE):

  1. After a refresh that synchronizes a reply (a message whose `In-Reply-To`/`References` chain reaches a locally stored message), the reply is persisted with both header fields and carries a backend-computed `thread_id` shared with its parent in the same folder — while the list payload remains unchanged and the list renders exactly as today.
  2. The new References fetch lane sends only `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` as a separate named constant; `peek_audit` enumerates it as path 5 **in the same commit** and no fetch code path can set `\Seen` (extended guard tests green; no dev run against live mail before the gate passes).
  3. Both a fresh install and a 25→26 upgrade boot with every prior row preserved (M26 fresh+upgrade migration fixture pair; SCHEMA_VERSION bumped in the same change, stale-version test extended), and the References parser passes its fixture suite (folded continuations first, `<...>`-framed, deduplicated, capped — with as-received bytes captured from a real `HEADER.FIELDS (REFERENCES)` round-trip added to the corpus; Zimbra-refusal fallback pre-declared, live leg deferred-with-reason if no creds).
  4. Thread recompute runs as a per-folder dirty-set epoch at the sync-end/backfill/send-file hooks (never per-insert), idempotently: re-running it neither duplicates nor reshuffles membership, and recompute on send-file correctly joins a sent reply into its folder's thread.
  5. `list_messages` (unchanged name/signature) now carries additive conversation fields (`thread_id`, `thread_count`, latest-message preview/date, participant flags), and the new `get_thread` command returns header-only member summaries (uid/from/date/flags/preview — no bodies) with TS+Rust types defined side-by-side and serde round-trip proven in both directions.

**Plans**: TBD

### Phase 43: UI Foundation & MessageListRow Extraction

**Goal**: The shared interface foundation is real — fonts actually ship, primitives and named interaction states exist — and `MessageListRow.tsx` is extracted exactly once with valid semantics, so Phase 44's grouping becomes a payload/placement change inside an existing file instead of a second rewrite.
**Depends on**: Nothing LAN-wise (fully parallel-safe with Phase 42 — zero backend dependency); executes as Phase 43 in numeric order
**Requirements**: REV-01, REV-02, REV-03 (+ roving-focus readiness groundwork for REV-05, delivered and verified in Phase 46)
**Success Criteria** (what must be TRUE):

  1. Baloo 2 and Nunito are vendored locally as woff2 (OFL licenses bundled, `@font-face` with `font-display: swap`, no CDN) and visibly render in the running app offline; the system fallback stack is normalized and CSP gains `font-src 'self'`.
  2. `Button`, `Dialog/Modal`, `EmptyState`, and `Loading` primitives live in `components/ui/` on top of tokens.css (hand-rolled, zero new npm deps) and are adopted on at least one existing dialog surface, with Dialog focus-trap and focus-restore proven by tests.
  3. The message list renders through the extracted `MessageListRow.tsx`, visually byte-identical to today (regression gate), and the row surface contains no nested interactive element — the current button-in-button a11y bug is fixed.
  4. The row exposes a single focusable surface with `:focus-visible`-only rings and hover/focus-visible/active/selected/disabled/loading/empty/error states modeled as named tokens — the readiness groundwork so Phase 46 can add roving focus **without touching row markup again**.

**Plans**: TBD
**UI hint**: yes

### Phase 44: Conversation List

**Goal**: The user sees related emails as one conversation row — count badge, aggregated unread, participants, latest snippet — with single-message rows staying pixel-compatible with today, merges idempotent across refreshes, and the pagination contract decided up front.
**Depends on**: Phase 42 (thread_id in store + extended payload + `get_thread`) **and** Phase 43 (extracted, semantically valid `MessageListRow`) — grouping is placement + payload inside the already-extracted row
**Requirements**: CNV-01, CNV-02, CNV-03, CNV-04, CNV-05, CNV-06, CNV-07, CNV-08
**Success Criteria** (what must be TRUE):

  1. For a real References-linked pair (offline fixture pair; live pair when creds exist — deferred-with-reason per convention) the list shows ONE row with a count badge shown only when ≥2; a single-message thread renders exactly like today's row — no badge, no layout shift (byte-compat regression gate).
  2. A thread with unread members shows the aggregate unread state (bold dot + unread count) derived from per-member flags carried in the payload; a real flag change arrives via `FLAG_UPDATE_EVENT` and the row's aggregate is re-derived from member flags (never patched).
  3. The conversation row shows deduplicated/truncated senders ("A · B +2", pt-BR suffix), a participant avatar-initial cluster, the "N não lidas" chip apart from the badge (P3), the latest message's right-aligned date via `formatRowDate`, its preview, and a subject trimmed of `Re:` noise with the existing "(sem assunto)" fallback intact.
  4. Refresh/reconnect never shows a duplicated conversation row, and a newly arrived reply merges into the existing row (rows key on the backend-stable `thread_key = (mailbox_id, thread_id)`); search stays message-keyed and renders rows identically.
  5. Pagination stays message-keyed (50/page); when a conversation is split across pages the slice count is visibly indicated and the COUNT/OFFSET-within-page contract is documented in the plan — never fetch-ahead, never a second rewrite of the row.

**Plans**: TBD
**UI hint**: yes

### Phase 45: Conversation Reader + Message-vs-Conversation Actions

**Goal**: Reading a conversation is a chronological, per-message stack with lazy single-card loads — and the action surface finally splits cleanly: conversation-scope batch actions from the list (one undo entry) vs message-scope actions per card in the reader.
**Depends on**: Phase 44 (`get_thread` payload + types + grouped rows; the reader is the expansion surface — the list never expands in place, per APG)
**Requirements**: READ-01, READ-02, READ-03, READ-04, READ-05, READ-06, READ-07, READ-08, READ-09, READ-10, ACT-01, ACT-02, ACT-03, ACT-04
**Success Criteria** (what must be TRUE):

  1. Opening a multi-message conversation shows a chronological stack (oldest at top, newest at bottom) with per-message expand/collapse; expansion defaults to messages up to and including the first unread (or only the latest when nothing is unread), older history collapsed to headers, and opening auto-scrolls to the first-unread card respecting `prefers-reduced-motion` (P3).
  2. Bodies load lazily per expanded card through the existing `fetch_message` — opening a thread never fetches bodies for unexpanded cards — each card has its own loading/error state, renders through the existing ammonia + sandboxed-iframe / text-`<pre>` pipeline with per-message attachments, and long threads fold earlier messages into a lazy "Mostrar N mensagens anteriores" header-only disclosure (P3).
  3. Expanding a card marks exactly that message read via the existing `set_seen` + `FLAG_UPDATE_EVENT` path; per-message mark-unread is preserved; per-card Reply/Reply-All keep the v1.7 threading-header contract (`In-Reply-To`/`References` seeding unchanged), and the reader header shows the thread's `To:` participants line (P3) with a plain-text quoted-trimmer toggle per card, default ON, HTML untouched (P3).
  4. From the list, hover actions on a multi-message conversation delete/move/read the whole conversation by default through one batched backend command (reusing the existing outbox UNIQUES; the batch-undo backend contract is decided at **plan review** — exactly ONE undo entry only with that approval, otherwise honest N labeled toasts shipped and documented), with copy that unambiguously distinguishes conversation-scope from message-scope, while per-message delete/move remains reachable in the reader and visually distinct from the card's body frame.
  5. Keying survives the plan's funnels: `{thread_key, open_uid}` selection in `MailboxView`, `pending_delete=0` member predicate (no tombstoned rows in counts), and the whole-thread-as-one-prompt LLM generation stays out of scope.

**Plans**: TBD
**UI hint**: yes

### Phase 46: Revamp Sweep + Verification

**Goal**: Every surface touched by 43–45 presents standardized, accessible, responsive interaction states consuming the Phase 43 kit exclusively (no new primitives invented), and the milestone's verification gates — including the assembled threading MVP gate — block any "stub compiles" over-claim (Phase 38 precedent).
**Depends on**: Phase 45 (sweep is consumption-only, AFTER threading UI; verification last)
**Requirements**: REV-04, REV-05, REV-06, VER-01, VER-02
**Success Criteria** (what must be TRUE):

  1. Every touched surface (list rows, reader, compose-adjacent chips) consumes the Phase 43 kit's standardized hover/focus-visible/active/selected/disabled/loading/empty/error states with no new primitives, colors, or tokens introduced in the sweep.
  2. Keyboard triage works on the **final grouped list**: arrow-key roving focus, Home/End, Enter opens, selection ≠ focus, `:focus-visible`-only rings — spot-checked under WebKitGTK, with dialog focus-restore verified after Phases 44/45 added new surfaces (REV-05's final form on the real row; readiness groundwork from Phase 43).
  3. The touched surfaces hold a defined three-pane compression behavior for narrow desktop windows — defined and exercised, no clipped/overlapping panes and no scroll traps (REV-06).
  4. All mechanical gates pass: `tsc`, `eslint`, `cargo fmt --check`, `cargo test` — including the new References parser fixtures (byte-captured as-received responses), the M26 fresh+upgrade fixture pair, the preserve-rows test, and both-sides serde round-trip tests for every new IPC command.
  5. The research MVP gate holds end-to-end: a real References-linked pair shows as ONE row (badge / aggregate-unread / latest date / snippet); single-message threads render byte-compatible; a 3+ message thread expands WITHOUT fetching unexpanded bodies; card-reply keeps threading headers; a conversation op produces one undo entry; refresh/reconnect shows no dupes (live UTFPR leg deferred-with-reason if no creds available, per project convention).

**Plans**: TBD
**UI hint**: yes

## Coverage

| Requirement | Phase | Status |
|-------------|-------|--------|
| THR-01..04 | Phase 42 | Pending |
| REV-01, REV-02, REV-03 | Phase 43 | Pending |
| CNV-01..08 | Phase 44 | Pending |
| READ-01..10, ACT-01..04 | Phase 45 | Pending |
| REV-04, REV-05, REV-06, VER-01, VER-02 | Phase 46 | Pending |

**34/34 v1.9 requirements mapped ✓** (no orphans, no duplicates). Traceability mirrored into REQUIREMENTS.md.

## Progress

**Execution Order:** Phases execute in numeric order: 42 → 43 → 44 → 45 → 46

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 42. Threading Backend Lane | 1/7 | In Progress|  |
| 43. UI Foundation & MessageListRow Extraction | 0/0 (TBD) | Not started | - |
| 44. Conversation List | 0/0 (TBD) | Not started | - |
| 45. Conversation Reader + Actions | 0/0 (TBD) | Not started | - |
| 46. Revamp Sweep + Verification | 0/0 (TBD) | Not started | - |
