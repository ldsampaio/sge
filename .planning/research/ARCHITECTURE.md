# Architecture Research — Email Threading + Interface Revamp Integration (SGE v1.9)

**Domain:** Rust+Tauri backend conversation-threading lane + React UI-primitive foundation, integrated into an existing Gmail-like three-pane desktop mail client
**Researched:** 2026-10-20
**Confidence:** HIGH — every integration point verified by reading the actual source (`imap/headers.rs`, `store/mod.rs`, `store/queries.rs`, `sync/worker.rs`, `commands/sync.rs`, `lib.rs`, `MailboxView.tsx`, `MessageList.tsx`, `ReadingPane.tsx`, `types.ts`); dep choices cross-verified against crate sources by STACK.md.

---

## System Overview

SGE today is a clean three-layer split. v1.9 adds two **parallel lanes** that touch different layers and only meet in one place (the message-list row component):

```
┌───────────────────────────────────────────────────────────────────────┐
│  LANE 1 — THREADING BACKEND (Phases 42+, pure backend, no UI dep)     │
│                                                                       │
│  IMAP lane                        Store lane            Commands lane │
│  ┌──────────────────┐   insert    ┌─────────────┐   extend ┌────────┐ │
│  │ imap/headers.rs  │──────────►  │ store/      │────────► │ sync.rs│ │
│  │ +in_reply_to from│             │ queries.rs  │          │ list_  │ │
│  │  ENVELOPE (free) │   new lane  │ +M26 cols   │  select  │ messages│ │
│  │ +REFERENCES lane │──────────►  │ +references │────────► │ +get_  │ │
│  │  (UID range,     │             │ +thread_id  │          │ thread  │ │
│  │   PEEK audit P5) │  recompute  │ store/       │          └───┬────┘ │
│  │ sync/worker.rs   │──────────►  │ threads.rs   │              │      │
│  │  sweep + sync-end│  (JWZ via   │ mail-threading│             │ NEW  │
│  └──────────────────┘   crate)    └─────────────┘             ▼      │
├───────────────────────────────────────────────────────────────────────┤
│  LANE 2 — UI FOUNDATION + REVAMP (parallel lane, NO backend dep)      │
│                                                                       │
│  ┌──────────────────┐   ┌───────────────────┐   ┌──────────────────┐ │
│  │ fonts.css + woff2│   │ ui-primitives.css │   │ components/ui/*  │ │
│  │ (Baloo2, Nunito) │ ─►│ tokens first usage│ ─►│ Button, Dialog,  │ │
│  │ tokens.css stays │   │ (hover/focus-     │   │ Empty/Loading/   │ │
│  │ (font-src CSP)   │   │  visible/disabled)│   │ ErrorState       │ │
│  └──────────────────┘   └───────────────────┘   └────────┬─────────┘ │
├─────────────────────────────────────────────────────────────────────────┤
│  MEETING POINT — CONVERSATION UI (after BOTH lanes land)               │
│                                                                       │
│  MailboxView.tsx ─data─► MessageList.tsx ─select─► ReadingPane       │
│  (selection: thread+uid)    (MessageListRow built ONCE with           │
│                              Lane-2 primitives, rendering Lane-1      │
│                              thread_id payload)                       │
└───────────────────────────────────────────────────────────────────────┘
```

**Key architectural insight:** the two lanes share exactly ONE file collision — `MessageList.tsx` row markup. Everything else is additive. The build order below is designed to pay that rewrite cost exactly once.

---

## Recommended Project Structure (new/modified modules)

```
src-tauri/src/
├── imap/
│   ├── headers.rs          # MODIFIED — see §Integration Points A1,A2
│   └── references.rs       # NEW     — References-header byte parser (~80 LOC + tests)
├── store/
│   ├── mod.rs              # MODIFIED — M26 migration appended, SCHEMA_VERSION 25 → 26
│   ├── queries.rs          # MODIFIED — UPSERT_MESSAGE_SQL cols, list payload, thread selects
│   └── threads.rs          # NEW     — recompute_threads() (mail-threading wrapper + batched UPDATE)
├── sync/
│   └── worker.rs           # MODIFIED — References fetch for new UIDs (step 5 tail),
│                           #            recompute call at sync-end (step 7 region)
└── commands/
    └── sync.rs             # MODIFIED — list_messages payload carry thread fields;
                            #            NEW get_thread command; lib.rs registration

src/
├── assets/fonts/           # NEW     — Baloo2[wght].woff2, Nunito[wght].woff2, OFL.txt each
├── styles/
│   ├── fonts.css           # NEW     — @font-face (variable, latin subset, no CDN)
│   ├── tokens.css          # UNTOUCHED # semantics; + state tokens (shadow edit, additive)
│   └── ui-primitives.css   # NEW     — primitive styles + standardized interaction states
├── lib/threads.ts          # NEW     — payload adaptation helpers (agg unread, sender dedup)
├── types.ts                # MODIFIED — MessageRow +thread fields; ThreadRow, ThreadMemberSummary
└── components/
    ├── ui/                 # NEW     — Button.tsx, Dialog.tsx, EmptyState/LoadingState/ErrorState
    ├── MessageList.tsx     # MODIFIED — becomes thread-aware list container + pagination
    ├── MessageListRow.tsx  # NEW     — THE row component (built once, consumed by list AND search)
    ├── ConversationReader.tsx # NEW  — chronological stack, expansion state machine
    ├── MessageCard.tsx     # NEW     — per-member card (lazy iframe, per-card actions)
    ├── ReadingPane.tsx     # MODIFIED — becomes ConversationReader host; single-message path =
    │                        #            reader with ONE card (upil-preserving behavior)
    └── MailboxView.tsx     # MODIFIED — selection state: thread_id + open uid; get_thread wiring
```

**Structure rationale:**
- **`references.rs` as its own imap module:** the house pattern (`decode_header_words` fail-visible style) deserves an isolated unit-test surface; it's the only new wire-format parsing in the milestone.
- **`store/threads.rs` separate from `queries.rs`:** the existing invariant says "no raw SQL escapes `queries.rs`" — the recompute does SQL via `queries.rs` helpers and keeps only the mail-threading glue; this keeps the single-SQL-module invariant verbatim intact.
- **`components/ui/` folder:** primitives are framework-agnostic (no SGE domain types) so legacy panels can adopt them opportunistically later without importing mail-domain code.
- **`MessageListRow.tsx` extraction is the anti-collision move:** threading and revamp both want to rewrite row anatomy; owning it in ONE component written once means neither lane "re-rewrites" the other's work.

---

## Data Flow Changes

### Flow 1 — Sync → thread assignment (backend lane, existing pipeline + 2 hooks)

```
IMAP UID FETCH (ENVELOPE BODYSTRUCTURE)        [unchanged lane]
    ↓ parse_fetch_item()  — NOW copies in_reply_to (free, same payload bytes)
    ↓                       [A1: MessageHeader + upsert + UPSERT_MESSAGE_SQL extend]
step 5 sweep inserts header rows (references/in_reply_to = NULL when lane not run)
    ↓
NEW: References lane for newly-fetched UIDs (UID range = this sweep's fetched set)
    UID FETCH <range> (UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])
    → references.rs parse → queries::set_references(mailbox_id, uid, irt, refs_json)
    ↓                                                        [A3 hook: step 5 tail]
step 7 sync-end:
    queries::load_thread_inputs(mailbox_id)                  [id, msg-id, irt, refs, date,
            ↓                                                 subject, mailbox scope]
    store/threads.rs::recompute_threads(conn, mailbox_id)
        mail_threading::thread_messages_with(input, ThreadingOptions{subject_merge:false})
        → thread assignments (root messages.id as thread_id, NULL = own-thread)
        → queries::apply_thread_ids (batched UPDATE, single txn)
    ↓                                                        [A4 hook: after step 7 write]
SyncSummary emitted (unchanged shape) → UI FLAG/SYNC events (unchanged listeners)
```

**Recompute triggers (exhaustive, matches "not per-insert" decision):**
1. sync-end of each mailbox pass (skip entirely when sweep fetched 0 new rows — cheap early exit),
2. after a per-folder backfill pass completes (Phase 9 machinery hook),
3. after send-flush / draft APPEND files a row into Sent/Sent-like folder.

### Flow 2 — List → Conversation row (existing command, extended payload)

```
MessageList useEffect (mailbox, refreshKey) — unchanged trigger
    ↓
invoke("list_messages", {mailbox, limit, offset})          [command signature UNCHANGED]
    ↓
queries::list_threads(conn, mailbox, limit, offset)         [NEW select behind same fn sig
    one row per CONVERSATION, ordered newest-first           OR new list_threads + same
    → thread_id, count, unread_count, latest_uid,            command name — see IP A6]
      latest subject/preview/from/date, member flags json]
    ↓
typed payload: MessageRow (flat, single-msg search path) + ThreadRow (list path)
    ↓
MessageListRow render: count badge (≥2 only), agg-unread dot, senders dedup "+N",
    latest-date; key = thread_id (inherit the existing refresh-Never-duplicates
    property — MessageList already re-invoke-keys per mailbox)
```

**Decision locked here:** the list keys rows on `thread_id`; single-message conversations (thread_id NULL or count=1) render byte-compatible with today's row (no badge, no layout shift) — the FEATURES.md "graceful single" gate.

### Flow 3 — Row select → Reader stack (existing `fetch_message` per card, NOT re-fetch-all)

```
row click (or Enter — keyboard nav)
    ↓
invoke("get_thread", {mailbox, thread_id})                    [NEW command, IP A7]
    ↓ returns ThreadMemberSummary[]: uid, from, to, date_utc, flags,
      preview, has_attachments (headers only — NO bodies, no PEEK)
    ↓
ConversationReader renders stack (oldest → newest), expansion rule:
    "expanded up to + including first-unread; else last card"
    ↓ per card EXPAND only:
invoke("fetch_message", {uid, mailbox})                       [UNCHANGED frozen cmd —
    → MessageView incl. sanitized html/text, attachments       BODY.PEEK invariant intact]
    ↓ seen-on-expand fires set_seen + dispatchFlagUpdate      [existing wash preserved]
    ↓
MessageCard lazy-mounts the sandboxed iframe per expanded card (IP A8)
```

**Critical invariant note:** `fetch_message` is a **frozen contract** (BODY.PEEK[], ammonia sanitize, per-message). The reader consumes it **per expanded card** — it does NOT batch, does NOT auto-expand all (FEATURES anti-feature), and never touches other IMAP verbs. This keeps the peek-audit surfaces at exactly the paths already documented.

### Flow 4 — Optimistic flag wash aggregates correctly (existing event, new consumer)

Today `sge:flag-update` + Tauri SYNC/FLAG events update `MessageList` unread state per uid. With grouped rows, the component recomputes the thread's aggregate locally from the member flags it already holds in the payload — no new event, no backend round-trip on toggle. Server-authoritative correction arrives via the existing refreshKey cycle.

---

## Integration Points (file-path specific)

### A. Backend lane

| # | File | New / Mod | Change |
|---|------|-----------|--------|
| A1 | `imap/headers.rs` | **MOD** | `MessageHeader` + `in_reply_to: Option<String>`; `parse_fetch_item()` reads `env.in_reply_to` (imap-proto 0.16.7 field already present in FETCH_ATTRS response bytes — zero wire change); `parse_fetch_attributes()` mirrors it for the CHANGEDSINCE path |
| A2 | `imap/headers.rs` | **MOD** | new constant `REFERENCES_ATTRS: &str = "(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])"` — separate from `FETCH_ATTRS` so the `fetch_attrs_are_parenthesized` + `peek_audit` guards keep their exact existing semantics; new `fetch_references_batch(session, range)` mirroring `fetch_envelope_batch` |
| A3 | `sync/worker.rs` | **MOD** | step 5 tail: for newly-fetched UIDs, call `fetch_references_batch` and route results via `queries::set_references`; step 7 region: call `store::threads::recompute_threads` once per pass. Note: `peek_audit` test enumerates exactly 4 paths today — **add path 5** (References lane → `REFERENCES_ATTRS` contains `BODY.PEEK[HEADER`) and extend the audit array; otherwise the Trusted invariant test silently lies |
| A4 | `store/mod.rs` | **MOD** | `SCHEMA_VERSION 25→26`; append `M26_THREAD_SQL`: `ALTER TABLE messages ADD COLUMN in_reply_to TEXT`, `ADD COLUMN references_json TEXT`, `ADD COLUMN thread_id INTEGER`, `CREATE INDEX idx_messages_thread ON messages(mailbox_id, thread_id)` — plus a `migrations_through_m26` preserve-rows test following the M22 model |
| A5 | `store/queries.rs` | **MOD** | `UPSERT_MESSAGE_SQL` gains the 3 new cols (write NULL on sweep insert; DON'T overwrite a fetched reference on re-sweep unless the sweep fetched it — use `COALESCE`-style guard or a richer upsert clause); `MessageRow` untouched (it stays the search/flat payload) |
| A6 | `store/queries.rs` | **MOD** | new `list_threads(conn, mailbox, limit, offset) → Vec<ThreadRow>` + `thread_members(conn, mailbox, thread_id) → Vec<ThreadMemberSummary>` — the one SQL surface for both new payloads. **`list_messages` command KEEPS its name/signature** so the frozen Phase-4/13/14 contract wording stays surfaced; the command body routes to `list_threads` (frontend types gain the thread fields) |
| A7 | `commands/sync.rs` + `lib.rs` | **MOD** | `get_thread` command registered in `generate_handler![]` (list at line 503 — single registration site); returns `ThreadMemberSummary[]`, store-mutex only, same spawn_blocking pattern as `list_messages` |
| A8 | `store/threads.rs` | **NEW** | `recompute_threads(conn, mailbox_id)` — `chrono` for `Message.date`, `mail-threading` 0.1.3 with `subject_merge: false`; batched `UPDATE … SET thread_id`; NULL = own-thread |

### B. UI lane

| # | File | New / Mod | Change |
|---|------|-----------|--------|
| B1 | `assets/fonts/` + `styles/fonts.css` | **NEW** | Vendored woff2 (2 files, variable, latin subset), `@font-face`, imported **before** `tokens.css` in `main.tsx` (Vite requires `@import` first if inlining); OFL.txt per family. **`tauri.conf.json` CSP:** current string has NO `font-src` → falls back to `default-src 'self'` which already permits same-origin woff2; **recommend adding `font-src: 'self';` defensively** |
| B2 | `styles/tokens.css` | **MINOR MOD** | additive state tokens (`--color-state-hover/focus/disabled/…`); no removal, no cascade reorder |
| B3 | `styles/ui-primitives.css` + `components/ui/*` | **NEW** | Button (primary/ghost/destructive), Dialog (focus-trap + Esc; consumed by ExpungeModal, FolderDialog, FolderDeleteModal with ZERO contract-copy changes — backend error text nonsense passes through verbatim), Empty/Loading/Error states; `data-state` attr pattern, `:focus-visible` only for keyboard rings |
| B4 | `types.ts` | **MOD** | add `thread_id`, `thread_count`, `thread_unread`, `latest_*` to payload types; add `ThreadRow`, `ThreadMemberSummary` mirroring Rust |
| B5 | `lib/threads.ts` | **NEW** | client helpers: sender dedup per row (`A · B +N`), aggregate-unread flip from member flags on flag-update event, latest-vs-root date choice, `(sem assunto)` / Re:-trim reuse |
| B6 | `MessageListRow.tsx` | **NEW** | the single row anatomy file — written ONCE with B3 primitives + FLOW-2 payload. **No other file hand-rolls row markup after this** |
| B7 | `MessageList.tsx` | **MOD** | consumes `MessageListRow` (thread-flat, or search-flat — both keyed by thread_id/uid respectively, unified payload adapter in B5) |
| B8 | `ConversationReader.tsx` + `MessageCard.tsx` | **NEW** | stack renderer + per-card body/iframe lazy-mount; expansion state machine in ONE place |
| B9 | `ReadingPane.tsx` | **MOD** | host shell becomes thread-aware: if selected row is multi-card → ConversationReader; single-message → exactly today's flow reusing MessageCard (act-on-one-message semantics unchanged) |
| B10 | `MailboxView.tsx` | **MOD** | selection state tuple: `{thread_id, open_uid}`; routes to B8/B9; existing refreshKey + CustomEvents untouched |

### Anti-collision callout (the quality-gate item)

**Collision pair: MessageList row markup.** `MessageList.tsx:662-687` (button+option nesting) needs rewriting by BOTH the a11y revamp (replace nested `<button>` inside `<button>` — invalid HTML, broken roving semantics) AND the threading UI (badge/unread/senders/latest-date field placement).

**Resolution — write the row once, in this order:**

1. Phase (UI foundation) creates `MessageListRow.tsx` with canonical a11y/emitting states, imported by `MessageList.tsx`, visually byte-identical in the diff snapshot so existing behavior is preserved.
2. Phase (conversation list) is then a **payload + placement** change INSIDE `MessageListRow` (badge etc.), NOT a second rewrite of the row declaration.
3. Phase (revamp sweep) only touches state styling and keyboard polish on the surfaces the earlier phases touched — never rewrites structure.

Same treatment for `ReadingPane`: build `MessageCard` first with correct per-message action order; render ConversationReader using it; the sweep phase then only patches states/spacing.

**Second, smaller collision: `fetch_attrs_are_parenthesized` / `peek_audit`** in `headers.rs` — A2's new lane must extend the audit array (path 5) in the SAME phase as the lane lands, or the compile-time guard counts paths that never run.

---

## Build Order (dependency-safe, minimal rework)

Recommended phase decompositions (order is the contract; each phase is independently verifiable + test-green):

**Phase 42 — Backend threading lane** (lane 1 fields only, no UI)
A1 + A2 + A3 + A4 + A5 + A8 + recompute hooks; unit tests: migration preserve-rows, JWZ conformance echoes against the crate's shipped testdata, references parser (Zimbra-shaped samples), recompute idempotency + late-arrival joins, peek_audit path-5. **No commands changed yet** (list payload stays flat, no UI change) = shippable in isolation.

**Phase 43 — UI foundation (parallel lane, CAN run concurrently with 42 — never blocked on M26)**
B1 (fonts + CSP) + B2 (state tokens) + B3 (primitives consumed nowhere new yet, just vendored + import order). One foundation-phase mistake to avoid: don't wait for M26; the revamp has ZERO backend dependency (FEATURES.md dependency note). **Plus** the `MessageListRow` extraction (B6) done in its pre-thread form — row markup is written here ONCE, before grouping changes it. This is the critical anti-collision step.

**Phase 44 — Conversation list UI** (lane 1 must have merged by now)
A6 + get_thread command header (A7, but unused by UI yet) + B4 + B5 + B6-extend + B7; visual work is a placement pass inside the already-extracted row. Add flat/single-message compatibility gate + regression: search results render identically to before (search rows keyed by uid, search stays message-keyed).

**Phase 45 — Conversation Reader**
B8 + B9 + B10 + threads-layer polish (React key stability, lazy iframes, expansion defaults); depends on phase 44's `get_thread` + types.

**Phase 46 — Revamp sweep + verification pass**
Retroactive polish on all phase 43-45 surfaces: a11y listbox roving (arrow/Home/End, Enter opens, focus≠selection), `:focus-visible` callbacks, `prefers-reduced-motion` for reader auto-scroll, empty/loading/error states rolled into NO new primitives — consumed from B3. **This phase must NOT introduce new primitives** — it standardizes CONSUMPTION of phase 43's kit.

**Phase 42b? — optional per-folder backfill** can ride with 44 (lane is already validated; the UI simply gets richer history as backfill progresses; no UI coupling).

**Build-order invariants:**
- state tokens + primitives BEFORE any phase that touches list/reader markup (phase 43 → 44/45 → 46) ✅ matches downstream_consumer rule 1
- backend lane BEFORE thread UI consumption (phase 42 → 44/45) ✅ but with 43 in parallel
- revamp sweep LAST but only consuming (46 → 44/45 already-consumed primitive surfaces) ✅
- `MessageListRow` written ONCE (43), extended in place (44) ✅ resolves the collision pair
- `peek_audit` path-5 added the same phase the REFERENCES lane lands (42) ✅ no drift window

---

## Architectural Patterns (project-specific)

### Pattern 1: Frozen-contract extension with additive payload
`list_messages` / `fetch_message` / `queue_send` carry "Frozen command" doc comments. v1.9 avoids breaking them by: additively extending payload structures (new fields, never removed) and adding new commands (`get_thread`) rather than changing signatures. The React side gates on presence: `if (row.thread_count > 1) renderGrouped(...) else renderLegacy(...)`.
**Why it works here:** the JSON payload is structurally tolerant of extra fields at the TS boundary — new fields simply arrive as non-existing/optional keys until `types.ts` declares them.

### Pattern 2: Recompute at epoch, never at insert (JWZ-correct)
Thread membership is a function of the *whole* folder's References graph — a late-arriving root after a reply changes phantom-parent constraints. `thread_id` is recomputed per-folder at sync-end (O(folder) linear via hash-map containers), NEVER per insert.
**Scope escape valve:** the recompute early-exits on 0 newly-fetched rows, so converged polls are free.

### Pattern 3: The one-component-row hedge
All row anatomy lives in `MessageListRow.tsx` (B6). Conversation grouping changes *what data flows in* and *which badges show*, not the markup skeleton. Future style/feature passes never compete with a grouping rewrite for the same lines.

### Pattern 4 (anti-pattern, called out): charset-blind References joins
msg-ids are ASCII (`<>`-framed dotted atoms per RFC 5322 §3.6.4). Do NOT route them through `decode_header_words` — it eats angle brackets' semantically inactive bytes and can corrupt keys for JWZ container grouping. `references.rs` parses strictly on `<...>` framing + `String::from_utf8_lossy` only.
**Act on instead:** save `decode_header_words` for subject/display fields where RFC 2047 lives.

---

## Minimal New Store Schema (M26, verbatim SQL flavor)

```sql
-- M26_THREAD_SQL (forward-only, preserve-rows validated by M22-style test)
ALTER TABLE messages ADD COLUMN in_reply_to TEXT;     -- RFC 5322 In-Reply-To (ENVELOPE)
ALTER TABLE messages ADD COLUMN references_json TEXT; -- JSON array of msg-ids (References lane)
ALTER TABLE messages ADD COLUMN thread_id INTEGER;    -- INTEGER = root message.messages.id or
                                                      -- NULL when the row is its own thread
CREATE INDEX idx_messages_thread
  ON messages(mailbox_id, thread_id);                 -- get_thread + aggregate queries hit this
```

Note: no separate `threads` table in v1.9. Deferred: a materialized aggregate table only if per-thread mutable state (e.g. collapsed-preference persistence, thread-level flags) ever becomes required (STACK.md already marks this "not v1.9").

---

## Scaling Considerations

| Scale | /mailbox message count | Architecture adjustment |
|-------|------------------------|--------------------------|
| Personal (v1.9 target) | ≤ ~50k (UTFPR INBOX) | Everything in this doc works as-is; recompute is linear, batched UPDATE is O(rows set), SQLite WAL handles the writer/reader concurrency in place |
| Heavy personal mailbox | 100k–200k | Recompute still linear but the 5-poll cycle adds ms; per-folder scope (mailboxes with new fetch only) is the free mitigation already in-flow |
| Multi-account (future) | N accounts × mailbox | thread_id scoping ALREADY keys to `mailbox_id` (not global message id), so accounts are isolated by construction — zero refactor needed when multi-account arrives |

**First bottleneck (honest):** the recompute on a giant folder the FIRST time M26 lands, before the References lane has backfilled (all rows have NULL references → each row becomes its own thread on first pass; trivially cheap). Real cost only bites if many rows share Message-ID References graph AND the folder is enormous; measurements stay unbound until phase 42 runs the conformance fixtures.

**Second bottleneck:** reader with N-expanded cards each mounting one sandboxed iframe — bounded by B8's lazy-mount (only expanded card mounts body).

---

## Anti-Patterns (SGE-specific traps)

### Anti-Pattern 1: Per-insert thread_id assignment
Precomputing at insert yields transiently wrong groups when the parent arrives later or references dedup applies. **Always batched recompute at sync-end / backfill / send.**

### Anti-Pattern 2: Routing References bytes through `decode_header_words`
msg-ids are not RFC 2047; mixing the two decoders corrupts exact-match joins. **Dedicated parser in `references.rs`.**

### Anti-Pattern 3: Route `list_threads` SQL anywhere except `queries.rs`
The "no raw SQL escapes queries.rs" invariant has survived 25 migrations by discipline. A convenient "just write the update here" inside `threads.rs` breaks it in one PR. Keep only the thin mail-threading glue in `threads.rs`, ALL SQL via queries.rs public functions.

### Anti-Pattern 4: Fetching all member bodies on thread open
Auto-expands every card → N × (BODY.PEEK round-trip + sanitize + iframe mount + seen-flag SERVER write). Violates the FEATURES anti-feature + silently changes the `\\Seen`-on-real-read contract. **Only expanded cards fetch.**

### Anti-Pattern 5: `aria-live` chatter / interactive-nests in listbox options
Two a11y traps already partially present (row has a nested interactive `<span role="button">` inside `<button>`). Phase 43-46 must FIX this (row → one activation surface + per-control stopPropagation with proper targets), never stack more intrusion on top. `aria-live` stays ONLY on the existing toast/list-state nodes.

### Anti-Pattern 6: Dual-rewrite row markup (revamp vs threading)
The single biggest rework vector this milestone creates. Treat `MessageListRow.tsx` as a merge boundary — only phase 43 writes it fresh; phase 44 extends; phase 46 consumes.

---

## Sources

- **First-hand code reads (2026-10-20, HIGH confidence):** `src-tauri/src/imap/headers.rs` (FETCH_ATTRS constant, `MessageHeader` 12-field shape, `parse_fetch_item`/`parse_fetch_attributes`, `peek_audit` 4-path test enumeration, `fetch_attrs_are_parenthesized` guard), `src-tauri/src/store/mod.rs` (SCHEMA_VERSION 25, M-list, preserve-rows test precedent at M22), `src-tauri/src/store/queries.rs` (MessageRow, single `UPSERT_MESSAGE_SQL` write path, `list_messages` select shape + `pending_delete=0` clause, reply-context contract), `src-tauri/src/sync/worker.rs` (7-step sweep step names, SyncSummary region lines 1336/1751-1794, backfill/sweep counters), `src-tauri/src/commands/sync.rs` (frozen `list_messages` line 1103, `MessageView` line 2224, frozen `queue_send` reply args line 1156), `src-tauri/src/lib.rs` `generate_handler![]` (line 503, single registration site), `src/components/MailboxView.tsx` (selectedMessage/refreshKey/folder event wiring), `src/components/MessageList.tsx` (row anatomy 650-720, listbox role nesting hazard, pagination, undo/flag CustomEvents), `src/components/ReadingPane.tsx` (props, invoke-on-select, sanitize + iframe mount, per-message action rows), `src/types.ts` (MessageRow/MessageView mirror contract)
- **Sibling research:** `.planning/research/STACK.md` (async-imap 0.11.3 / imap-proto 0.16.7 `Envelope.in_reply_to` + `Fetch::section()` + parser test shape; `mail-threading` 0.1.3 API + conformance corpus; CSP/font verification; vendored woff2 posture), `.planning/research/FEATURES.md` (dependency graph, MVP priority matrix, per-message vs per-conversation action split, a11y listbox contract)
- **Project context:** `.planning/PROJECT.md` (v1.9 scope guard, Key Decisions 2026-10-20 threading = backend References lane + clay identity/foundation-lane, frozen-contract precedent record, dc7f8c7 repair pass references)

---
*Architecture research for: SGE v1.9 — Email Threading + Interface Revamp*
*Researched: 2026-10-20*
