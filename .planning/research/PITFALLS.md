# Pitfalls Research — Email Threading + Interface Revamp on an EXISTING SGE

**Domain:** Adding backend References-threading + UI primitives revamp to the shipping Rust+Tauri v2+React+ SQLite IMAP client
**Researched:** 2026-10-20
**Confidence:** HIGH on backend pitfalls (verified against SGE source: `store/mod.rs`, `imap/headers.rs`, PROJECT.md Key Decisions / repair notes, STACK.md + FEATURES.md integration points); MEDIUM on UI pitfalls (code-shape verified from MessageList/ReadingPane reads + W3C APG fetched live; font/CSP behavior from Tauri v2 docs fetched 2026-10-20)

**Why this file is different from a generic pitfalls list:** every failure mode below is anchored to a concrete SGE precedent (the 2026-10-18 repair's 4 independent breaks, the M23 SCHEMA_VERSION off-by-one, the NAMESPACE session poisoning, the Phase 38 over-claim) or to a concrete integration surface that v1.9 must not damage (single-session SessionManager, BODY.PEEK peek_audit, UID-only addressing, durable outboxes, FTS triggers, guarded refresh).

**Suggested phase structure this mapping uses** (v1.9 phases start at 42; exact numbering is the roadmap's call):
- **Phase 42 — Threading data lane (backend):** M26 migration, `in_reply_to` ENVELOPE extraction, References fetch lane, `threads.rs` compute + backfill, payloads.
- **Phase 43 — Grouped list + conversation reader (UI over thread data):** grouped rows, reader stack, per-message/per-conversation actions.
- **Phase 44 — UI foundation (independent lane):** tokens/fonts, `ui-primitives.css`, Button/Dialog/Empty/Loading, listbox semantics + keyboard.
- **Phase 45 — Revamp sweep + verification:** consistency/a11y pass, gates, UAT.

---

## Critical Pitfalls

### Pitfall 1: Tauri silently dropping NEW invoke/payload fields (the repair's break #4, about to repeat on a much wider surface)

**Warning sign:** a new UI feature reads `undefined`/missing keys from an invoke result, or a backend handler never receives a field the frontend definitely sent — with **no error anywhere** (Tauri drops unknown runtime args silently).
- The 2026-10-18 pass shipped `queue_send` threading headers (pendingReply pending_queue threading) — the call never failed, the fields never landed. The threading feature compiled, and its payload did not fire.

**What goes wrong:**
1. Frontend sends new invoke args (e.g. `thread_id`, `member_uids` for a batch command, or the reader's thread-summary key) but the Rust command's typed struct doesn't have the arg: field silently vanishes. Conversely, backend adds a field to the result payload but the UI's `as TSX` cast reads old shape: `undefined` propagates until render.
2. camelCase/snake_case: Tauri v2 maps JS camelCase to Rust snake_case args (and the codebase has BOTH patterns — `mailbox` vs `messageId:` in `ReadingPane.tsx` invoke calls). A new arg written in the wrong convention is silently `None`, not an error.
3. Event payloads: FLAG_UPDATE_EVENT and other emitted events deserialize through a different mapper than invoke results — a new field added only to one side desyncs invisibly.

**Why it happens:** Tauri v2's deserializer ignores extra/unknown keys by design (serde defaults); tests mock the whole IPC boundary (lib tests), so both directions of the gap are invisible until human UAT — which v1.8 already deferred once.

**How to avoid:**
- Define EVERY new invoke's TS-args and Rust-input-arg types beside each other in one file (house pattern like `compose.ts`), and make the ts-side types non-optional for required fields so `tsc` fails when the backend payload drops one.
- Every new arg added to an existing command MUST be added as a struct field on the Rust side in the same commit — grep the exact command name in both languages, never trust "it looks connected."
- For event payloads: extend the TS event interface FIRST, then the Rust emit; a consumer reading a missing event field should render a visible error state, not silently skip.
- Add one integration-shaped test per new command that round-trips the exact arg key names through serde (cheap, catches the convention drift).

**Warning signs (repeatable):** any `if (!x)` fallback on an invoke arg; any TS field typed `?` matching a Rust field the handler doesn't own; "works in unit tests, dead in app" — treat as the default suspicion for every v1.9 field.

**Phase to address:** Phase 42 (payload contracts before UI consumes them), carried as a standing rule into 43; **Phase 45** re-verifies wire-level round-trip as an explicit UAT gate (never unit-only).

---

### Pitfall 2: References-chain parsing holes — folded headers, malformed/duplicate Message-IDs, angle-bracket normalization

**Warning sign:** two shipped replies never join a conversation; OR two unrelated conversations display as one row; OR the same message appears in TWO threads.

**What goes wrong:**
- The raw `BODY[HEADER.FIELDS (REFERENCES)]` lane returns the header UNFOLDED into lines; a long reply chain arrives continuation-folded (`<a@x>\n <b@y>`). A naive `split(' ')` keeps `\n` bytes inside IDs → matching fails → "my reply doesn't thread." Folding is not hypothetical: long chains fold at 998 chars.
- Malformed/absent Message-IDs: legacy Zimbra chains and old newsletters have `References: Re: <bad id>` comments, RFC 2047-ish garbage, or multiple IDs with duplicate-copy content. Keeping them opaque as JSON is fine for storage but poison for matching.
- Angle-bracket + case: `Message-ID` canonical form is `<a@x>`; SGE's existing v1.7 send path (`sanitize_header`, `new_message_id`) generates brackets, but servers/clients sometimes send bare tokens (`a@x`) or wrapped-with-clients in lowercase. `in_reply_to` from ENVELOPE for Zimbra is the RAW bytes — brackets may differ from the `references_json` copy of the SAME thread used in a different rollup.
- Duplicate Message-IDs: some newsletters and legacy clients reuse one `Message-ID` for two semantically separate messages. mail-threading 0.1.3's dup policy handles the JWZ rule — but our DB layer must not treat `message_id` as UNIQUE across folders (today's draft/send queue does UNIQUE on message_id — that is send-session-local and OK; the incoming copy path is the risk).

**Why it happens:** headers arrive pre-validated in unit tests via short synthetic strings; real traffic is long-folded, mixed-charset, dated. The 2026-10-18 bracketed Message-ID SEARCH bug is exactly this class — the "obviously fine" ID had to be wrapped in `<...>` against Zimbra.

**How to avoid:**
- `references.rs` (new, in-house per STACK.md): unfold continuations FIRST (join CRLF+WSP into a single space per RFC 5322 §2.2.3), THEN extract every `<...>`-wrappable token, normalize each to `<…>` form in a single pure function with tests.
- Deduplicate the parsed list order-preservingly; cap the chain at a sane max (jwz's practical bound — treat like the 200-char preview cap, e.g. 100 IDs / 32 KiB raw) so adversarial headers can't blow up recompute cost.
- Store `references_json` verbatim-normalized once at fetch time; never re-parse at thread-compute time (single parse point = single failure point, testable with the fail-visible `decode_header_words` house pattern).
- Fixture set: take real Zimbra lines if available at Phase 42 gates; until then, handcrafted fixtures cover (a) folded 2-line, (b) bare token no brackets, (c) duplicate ID twice in one chain, (d) empty References on a first-post, (e) In-Reply-To pointing to never-synced parent.

**Phase to address:** **Phase 42** — parser + fixtures + conformance-corpus tests land here; thread_id correctness depends on it. Wrong flat-file parse in 42 costs a re-migration to fix`references_json` normalization in 43+ (backfill is per-folder and optional — cheap to redo only if caught early).

---

### Pitfall 3: SCHEMA_VERSION off-by-one on M26 — fresh install AND upgrade both break (M23 precedent)

**Warning sign:** fresh database won't open OR existing user database's new columns vanish on restart; `duplicate column name: references_json` on every reopen of an already-opened store.

**What goes wrong:** the M23 race shows the exact shape: the current file's M23 block also creates `llm_settings` in-concat via default 'threading' AND preserves `UPDATE recipes` with the wrong `user_version`. Any of these four paths ambush the M26 production line:
  1. SCHEMA_VERSION constant not incremented to 26 → migration not applied; queries referencing `references_json`/`thread_id` index-cache `Unknown column`, but only at runtime, and in production it's mid-typing rather than on startup because tests use entirely fresh in-memory.
  2. schema.sql (v1 baseline) is untouched by forward-migrations; but `apply_migrations` builds parity checks: any dev DB running row-based tables on a version that already carries M26 blocks replay.
  3. The 'M26 shape' phase lands twice (once in a hotfix branch, once in main with slightly different column sets) — the second `ALTER TABLE` never re-applies.
  4. Fresh-install path (schema.sql + replay) skips the M26 body because the fresh CREATE path is in the M26 SQL const while upgrade in-memory tests use an old baseline fixture list at their topmost position of the vec.

**Why it happens:** forward-migration-by-list in this codebase exactly reproduced the M23 pattern where the audit identified a 'double shape' (M23 creation AND the M24 ALTER both covering one column) — that failure mode is the default outcome when you add a second shape of the same data.

**How to avoid:**
- M26 SQL adds `references_json TEXT`, `in_reply_to TEXT`, `thread_id INTEGER` as ALTER TABLE ADD COLUMN on `messages` (nullable, NULL = not-yet-computed / own-thread) plus `CREATE INDEX idx_messages_thread ON messages(mailbox_id, thread_id)` (per DECISION: per-folder scope; index on (mailbox_id, thread_id) — NOT a threads summary table, per STACK.md "no separate threads table in v1.9").
- SCHEMA_VERSION 25 → **26** in the same commit (grep the existing `schema_version_is_25_with_m23_llm_settings` test — extend or rename to a 26 test to make stale assertions scream).
- Two-gate regression pair (house pattern for M4/M5/M6 are the precedent): (a) fresh `open_in_memory` down through M26 AND (b) an upgrade test that replicates M23's shape exercising M24+M25 baseline then applying full M26 (preserve-rows).
- Verify M26 SQL contains ZERO `CREATE TABLE` copies of an existing table; rebuild-and-rename (the M11/M17 pattern) is NOT needed for ADD COLUMN — it's a bigger blast radius and shouldn't be the default instinct.

**Phase to address:** Phase 42, first task of the phase (migration is the atomic base the REST rides on — if it ships wrong, every thread payload and list-feature test is built on a store whose state doesn't match disk).

---

### Pitfall 4: \Seen invariant breakage in the NEW HEADER.FIELDS fetch lane (peek_audit path 5 not added)

**Warning sign:** the first live-sync after 42 marks unread messages as read in a real webmail client — and the peek_audit test still passes because it never enumerated the new lane.

**What goes wrong:** today's `peek_audit` test enumerates FOUR paths. Phase 42 adds a fifth: `UID FETCH <range> (UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])`. Failure shapes:
- Write it as `BODY[HEADER.FIELDS (REFERENCES)]` (no .PEEK — the most common IMAP-lane copy-paste bug): server sets \Seen on every fetched message, READ STATE perma-corrupts in a single sweep, no client-side undo exists (impossible to restore prior unread with this run), and the DOCTRINE "never set \Seen on fetch" is broken silently.
- Write it unparenthesized with multiple attrs → Zimbra BAD → async-imap empty stream = "0 messages fetched" (the FETCH_ATTRS comment's exact precedent: RFC 3501 §6.4.5 multi-attr requires parens).
- Add References to the EXISTING `FETCH_ATTRS` constant instead of a new named constant → blast radius: every headers-first sweep goes 2x wire bytes on ALL folders incl. Inbox-like sweeps where References never used to be asked; also the `fetch_attrs_are_parenthesized` test's negative guard (`!contains("BODY.PEEK[HEADER")`) is an INTENTIONAL tripwire that forces a separate constant.

**Why it happens:** reuse-the-existing-constant is the path of least resistance; \Seen is a wire-side behavior invisible in lib tests (all fetch paths use synthetic fixtures).

**How to avoid:**
- New named constant `FETCH_REFS_ATTRS = "(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])"` at the top of `headers.rs`, next to `FETCH_ATTRS` — parens enforced, body token not shared, subject to its own parenthesized assertion.
- Extend `peek_audit` with path 5 in the SAME commit: assert on the new constant's `.PEEK` presence AND keep auditing the "no bare BODY" negative in old paths. The audited_paths count assert (`len == 4`) becomes `len == 5`.
- Header-field case doesn't matter on the wire (IMAP fetch is case-insensitive), but include exactly `REFERENCES` (not `References`) in constant for grep-ability.
- Only fetch this lane when the message lacks it (new messages at sync + optional per-folder backfill) — not per-sweep unconditional.

**Phase to address:** **Phase 42**, enforced as the phase's FIRST verification gate (peek_audit path-5 must be green before any lane is issued in dev against a real mailbox — the "don't run a broken lane once" stance mirrors the classifier's 'never leave a bad search dangling'; the fix is cheap before first live run and not recoverable after).

---

### Pitfall 5: thread_id leaks across folders / UIDVALIDITY semantics (folder-isolation + local-PK collision)

**Warning sign:** the INBOX list shows members of a thread that lives in other folders (Sent copies of replies, Auto/ classified arrivals) OR a thread row's members' UIDs change meaning after a server UIDVALIDITY bump.

**What goes wrong:**
- thread_id computed over ALL messages ignoring `mailbox_id` → Sent folder's leak: every reply you SENT joins the INBOX conversation row, so unread aggregate and latest-snippet compute from out-of-folder copies; the "Grouping" becomes Gmail-correct only for the INBOX user realistically reading INBOX; worse, it silently changes what "delete conversation" batch targets (batch keys on (mailbox, uid) — if members span folders, the batch command's scope is ill-defined).
- thread_id stored as root `messages.id` (local PK) — a REUSER of pattern like "carry thread_id in the payload without mailbox pairing" leaks: two folders CAN both have a message with the same local PK? No — messages.id is a global PK; safe. BUT per-folder scope means the SAME message-id content appearing in TWO folders gets two thread_ids (row duplicates) and the SAME thread_id value is reused as a grouping key BOTH in payload AND DB — a consumer/UI combining by thread_id alone (FEATURES.md: 'MESSAGE list keyed on thread_id') WILL merge two distinct-room groups that happen to share root ids across folders unless the key includes mailbox_id.
- UIDVALIDITY bump wipes message rows behind CASCADE — but the thread_id column survives on surviving rows; recompute-at-sync-end handles it ONLY if every folder recompute path actually runs (a thread_id on a row that never sees the recompute path — e.g. migrated fresh via M26 with backfill skipped — misidentifies its group).

**Why it happens:** the approved decision says "backend-computed thread_id, per-folder scoped" — but the payload/list code naturally keys on `thread_id` alone because it reads the data as Gmail-like universal grouping.

**How to avoid:**
- Define `thread_key = (mailbox_id, thread_id)` as the contract EVERYWHERE (payload field, list key, batch command identity, reader fetch key). Name the payload field `thread_key` or nest; a plain `thread_id` request will get misused.
- Compute scope: `SELECT ... FROM messages WHERE mailbox_id = ?` strictly, per folder; recompute forced at (a) sync-end of that folder, (b) References-lane backfill completion, (c) the send-finalize event that files a copy into Sent (event from `queue_send` composition, riding Phase 43's arrival contract documented in STACK.md).
- The NULL column is valid state: 'thread_id IS NULL' → single-message own-thread, with the semantic "behave exactly like yesterday's row" (STACK.md & FEATURES.md single-message byte-compatibility test). Do not infer grouping from the column; leave NULL rows alone until recompute, and write the byte-compat test early as the sentinel.

**Phase to address:** **Phase 42** (compute scope + thread_key definition), enforced in **Phase 43**'s payload/UI as the single-threaded group key shape and in **Phase 45**'s regression pass (cross-folder validation against a real account INBOX + Sent in the same fixture server).

---

### Pitfall 6: pending_delete / flag_outbox / imap_outbox interplay with queued ops on a thread member

**Warning sign:** a conversation row's count badge includes a member whose delete is already queued (row is hidden); OR batch conversation-delete enqueues duplicates; OR undo restores one member but the row still shows deleted.

**What goes wrong:**
- `pending_delete = 1` rows are FILTERED from list/search views — but a `SELECT members WHERE thread_id = ?` that keys on the RAW column set will include hidden members, inflating count/unread (the "N (7)" that never goes away) and making the conversation reader try to fetch the deleted message body.
- Batch conversation-delete implemented as "delete every member" via the existing single `delete_message(uid)`: each call re-validates and each enqueues on `imap_outbox`. The `UNIQUE (mailbox_id, uid)` constraint collapses duplicates — fine — but a member ALREADY queued with a 'move' op whose op-type differs (delete after a move already queued) will SHAKE the queue: the enqueue path may write a DELETE row where a MOVE row collapsed it, or fail with a constraint error depending on implementation — a per-member error that's invisible at the batch level.
- Undo: `undo_queued_op(uid, mailbox)` is SINGLE-uid; a conversation delete enqueues N rows and the toast's one `Desfazer` attempt restores only the last. FEATURES.md already flags backend expansion confirms needed — the pitfall here is shipping the "one toast" UX with NO backend undo shape and calling it done (the Phase 38 stub-compiles lesson bolted to durable outboxes).

**Why it happens:** conversation scope was designed in UI terms; store invariants live server-side; the existing queue invariants (collapse-dedupe, RFC 4549 drop) were never designed to compose with a batch.

**How to avoid:**
- All member queries for conversation payloads filter `pending_delete = 0` (and pending_move future state if applicable) — mirror the exact predicate used by `list_messages` so counts and members agree, in ONE shared SQL fragment.
- Batch command (if approved) reuses delete message validation once and enqueues per member in a single transaction with a set-guard for existing outbox entries (skip already-queued members, report them back), never a UI loop of N invokes — that drops atomicity and reuses the 'device LOC window' already shown to swallow mid-loop state.
- If batch undo isn't approved as backend scope: ship conversation delete WITHOUT the one-toast claim — N real toasts (existing per-UID undo shape) and DOCUMENT it; do NOT render one undo button wired to one of N restores (that's the stub-compiles anti-pattern in a durability lane).

**Phase to address:** **Phase 43** (payload filtering + batch shape/membership rule), revisit at **Phase 45** with a livemailbox check that conversation delete against a mid-outbox-flush state round-trips.

---

### Pitfall 7: Recompute performance / consistency — order semantics inside a thread and out-of-order mail

**Warning sign:** sync-end on a folder with several thousand messages takes visibly longer than it did pre-M26 OR two consecutive syncs produce different member orderings; OR the thread's "latest" snippet changes to an older message after a recompute.

**What goes wrong:**
- Recompute set: JWZ over an entire folder is linear-hashmap complexity (STACK.md) — fine — but the naive recompute path re-UPDATES every row of the folder on EVERY sync-end (write storm on the messages table, pointless FTS trigger time since msg_au is gated to indexed cols — actually safe — but WAL churn + row locks under the single-writer Store mutex block the UI listing write path momentarily on traffic bursts).
- Ordering inside the thread: conversation reader sorted by... `date_utc`? but headers sweep often falls back to ENVELOPE date (RFC 5322 raw string) or INTERNALDATE — mixed precision across members gives ties and occasionally reversed order on forwards (FORWARD has LATER internaldate but EARLIER date header).
- Late parent arrival: reply (child, present) arrives first; parent later fetch from backfill or SEARCH — thread_id of the child CHANGES (per RHS recompute) on future sync; UI mid-session loses the group or, worse, holds the stale-but-correct-at-read-time thread that a background recompute updates under it.

**How to avoid:**
- Only 'dirty' recompute: pass ONLY members whose `references_json`/`in_reply_to` changed + their containers' neighbors, or per-folder recompute gated behind a 'did this folder's header set change or backfill run?' flag — STACK.md already recommends recompute at sync-end; add the cheap guard (dirty set persisted in-memory per folder, invalidated at each sweep).
- Ordering contract: sort by normalized UTC (use `date_utc` that header sweep already consolidates to RFC3339), tiebreak by UID (arrival order) — deterministic across recomphes; write the tiebreak as a fixture test with two same-UTC rows.
- Late arrival handled by 'recompute + refreshKey emit': any thread_id change of an IN-SESSION visible row triggers the existing guarded refresh flow — don't try to keep UI state warm across a recompute; replace it.

**Phase to address:** **Phase 42** (compute scopes / dirty set / order contract fixtures); UI acceptance of the refresh-under-you semantic in **Phase 43** (the reader/list handling of a mid-session thread change is that phase's UX requirement, not a 42 fix).

---

### Pitfall 8: Pagination vs grouping disagreement — a 50/page cut can slice ONE conversation across two pages

**Warning sign:** a conversation with 10 messages shows as 7 on page 1 and 3 on page 2 — same thread_id, user perceives two conversations; OR the pager's total count says "17 results" while the list shows 9 rows (a thread of 9 members).

**What goes wrong:** today's `list_messages` paginates per-message SQL with a per-row key — the shape is correct because row == message. With grouping, three contracts diverge:
- COUNT: how many entries in the pager (`count(*)` of messages vs rows of threads),
- OFFSET: page boundaries in message-space vs thread-space,
- UNREAD user mental model: "read everything" — per-message; but on a page where one thread is cut, a "mark this page read" gesture hits N members of a thread whose remaining members live on page 2 (state split inside a conversation).
- Empty subject threads and single-message rows make the disagreement non-triggerable in tests that use only tiny 1–2 member fixtures.

**Why it happens:** pagination was built when list space and message space were identical; grouping changes one but page math is left doing the other. Note the deferred converse is also valuable context: 50/message page is a WORKING bound, not a place to redesign to thread-space paging in the same commit.

**How to avoid (be opinionated):**
- Keep page = message space, NOT thread space, for the pager math (lowest blast radius). But GROUP AWARENESS: the GROUPED display derives from the returned page's rows — a thread cut across pages shows as two rows on different pages (a known-acceptable Gmail-alike cost that REAL clients with server-backed pagination also show in SEARCH results).
- Make it explicit instead of accidental: the thread-grouping aggregation happens on the button of the returned rows (grouping-of-the-page view), not on invisible cross-page data — never fetch ahead to 'complete' a group (extra invocations break the pagination contract and hit BODY.PEEK lane counts).
- Soften: aggregate count into the row per the page slice (7 on the first page / 3 on the second) is fine; but the COUNT BADGE per row must show the slice count with a tiny "+more" only if the UI admits partial threads, otherwise a 7/9-count row lies about thread size.
- Document one line in the phase plan: 'conversations are grouped WITHIN page; cross-page continuation of one thread shows as a second row on the next page' — decided up front, not discovered in review.

**Phase to address:** Phase 43 (payload + list math as one coherent slice; TEST at 3+ page boundaries), re-validated in **Phase 45** UAT.

---

### Pitfall 9: a11y / DOM regression — nested interactive control inside the listbox option and focus model churn (button-in-button precedent)

**Warning sign:** keyboard-only user cannot reach/delete using keyboard once the list row is rebuilt; Safari/WebKit (WebKitGTK!) renders the nested `<button>` silently as nothing clickable — but a click-log test on Chromium shows "it works" and misses it.

**What goes wrong:**
- Current MessageList renders the row `<button>` WITH a nested unread-dot `<button>` inside (FEATURES.md audit) — invalid HTML inside a listbox/option. The v1.9 redesign CORRECTS this with either the option div + separate activation surface or the grid pattern — but the FIX ITSELF creates new traps:
   - moving actions out-of-row while keeping hover-behavior means keyboard users must still TAB to them via a non-obvious activation step; if the row Enter opens the reader AND the dot toggles read, both can't live on the same Enter.
   - switching the focus model mid-milestone (roving tabindex): without Home/End and focus-visible CSS pairing, keyboard users get lost — APG listbox contract explicitly requires selection≠focus and the ring visible only on keyboard. FEATURES.md already pins this pairing — dev often implements roving without `:focus-visible`, so keyboard users pick nothing visible.
- Widgets: shadcn/Radix temptation explicitly banned; but hand-rolled Dialog primitives replicate expunge-confirm focus trap three imperative implementations converge — a bug here (e.g. Escape closes DON'T restore focus back TO the trigger) damages both keyboard a11y and 'does the modal reopen on the new catalog'; regression risk on the FIVE established modals.

**Why it happens:** accessibility bugs are invisible in unit/CI (no visual tree) and invisible on the happy-path Chromium click test; only keyboard-first or screen-reader testing exposes them. Your env (WebKitGTK on Linux) is additionally the one where nested-button bugs manifest differently per WebKit version.

**How to avoid:**
- Rebuild the row ONCE with the a11y contract in FEATURES.md as the spec: roving tabindex + Enter activation + hover-action affordances OUTSIDE the option traversal surface + `.focus-visible`-only rings from tokens; do NOT ship a 'quick fix' nested-button removal that leaves focus unmodeled and call it sparse.
- Every dialog rebuilt over the shared Dialog primitive preserves: focus moves IN on open, Escape closes and Restores focus to the invoked-from element; test with `role="dialog"` assertions; the Chrome-free on Linux (WebKitGTK only) means no Headless Test matrices — a PROOF test for focus restore per modal is more valuable than screenshots.
- Per FEATURES dependency: rebuild semantics BEFORE shipping keyboard roving — inverted order ships a broken surface.

**Phase to address:** **Phase 44** (prim/lane, and roving+ring pairing in the SAME task), spot-checked in **Phase 45** UAT (keyboard-only + dialogue focus restore explicitly, not just 'tab cycling looks fine').

---

### Pitfall 10: iframe-per-message memory blowup in long threads + font-loading FOUT/CLS/CSP in the NEW fonts.css

**Warning sign:** opening a long thread (20+ messages) visibly freezes (each body mounting its sandboxed iframe triggers re-frame + sanitizer parse) OR the day-one fonts.css makes the app render in fallback then pop once, moving rows (CLS badge GPX on load); under WebKitGTK the font-src denial shows as raw fallback across ALL surfaces.

**What goes wrong:**
- Reader stack as designed (FEATURES.md anti-feature already covers the DEFAULT-ALL-EXPANDED trap) but even a well-designed deck can bloat: each expanded card iframe is a full document with the sanitizer and tokens; 10+ concurrent iframes = one host-container per card, WebKitGTK needs more than Chromium per i-frame budget.
- Bothにな pitfalls are同类 "silent perf regression the dev machine doesn't feel": your machine is fast and fonts already-cancel on disk — FOUT and iframe gymnastics manifest on slower hardware approximating the user's real laptop.
- CSP: STACK.md verified no font-src needed because `default-src 'self'` falls back — but that SAME covering cascade means the new `fonts.css` must live under `'self'` paths relative to the ASSETS folder; if the app is packaged via Tauri v2 and the file ships into the bundle root, forget nothing — it's covered by default-src. Add the RECOMMENDED explicit `font-src: 'self'` line NOW, not later — the returning of a default-src collapse under OTHER future additions silently breaks font load.

**Why it happens:** in the dev environment, the fonts load instantly from disk and iframe budgets seem unexhausted; in a fresh user profile with no cached font, first-run FOUT is guaranteed.

**How to avoid:**
- Lazy-mount iframe per card (FEATURES.md differentiator) with a small cap (e.g. max 4–6 simultaneously mounted i-frames — beyond it, superseded cards unmount iframes but keep title/meta/snippet; re-expand re-fetches from the local cache, cheap).
- `font-display: swap` + `font-family` fallback normalization tokens set BEFORE fonts.css lands (STACK.md recommends adding explicit `font-src: 'self'`), sized ~150 KB woff2 files preserved to `src/assets/fonts`.
- Long-threads fold (Mostrar N anteriores) is a P3 in FEATURES.md but the IF-CAP logic of lazy mount should ride in WITH reader stack in 43 — no-it fails to open a 40-message thread.
- Reduced-motion preference is honored not only in the auto-scroll (already spec'd) but also the thread-wash animation.

**Phase to address:** iframe mounting in **Phase 43** (reader stack task); fonts/CSP in **Phase 44**; both rechecked as measurable gate (`chromium --trace` not available on Linux/WebKitGTK env — gate on observed per-open max-iframe-count and on `document.fonts.check()` and layout stability through DOM test if any, else UAT).

---

### Pitfall 11: Event-bus assumptions — FLAG_UPDATE_EVENT and row state when a row becomes a CONVERSATION row

**Warning sign:** marking one member unread updates only the member card; the conversation row shows stale unread count / bold state until a full refresh (no status wash); OR the unread-dot indistinguishably displays on a member whose parent THREAD is collapsed.

**What goes wrong:**
- `FLAG_UPDATE_EVENT` carries (mailbox, uid) — the entire list/pane washing currently reacts and rewrites the row. In conversation grouping, the row reacts on (mailbox, thread_key): when a member flags, the LIST must re-derive (aggregate unread count) not just re-render a flat row keyed on (mailbox,uid) — a listener that 'optimistically patches one row' now must patch the row containing it.
- The v1.7 repair precedent ('4 independent breaks' including a tracker built per-call) — silent no-op damage from a wrong identity key: a listener patched on the wrong key produces a no-op without error. Same class of bug here but at EVERY message, EVERY flag operation, EVERY unread dot.

**Why it happens:** the optimistic-state wash was written against flat rows; it keeps working for singles and silently stops affecting grouping because the key changed — matching is neither loud nor structured.

**How to avoid:**
- The payload contract: every conversation row carries per-member flags (`thread_members` summary being the agreed new payload from stack# list one-way flow) — 'wash' handling must re-aggregate, not patch — the grouping is DERIVED from member data, never patched directly; patching a 'thread' shape is derived-state corruption.
- The thread-key (including mailbox) per Pitfall 5 is what the listener reacts to; compute it once in the payload layer, not in the listener.
- Test: an unread toggle on a 5-member thread updates the row badge within the same event tick, no refresh needed;両 cover single (row keyed by its own thread with 1 member) too.

**Phase to address:** payload-listener contract in **Phase 43**; regression-checked in Phase 45 live UAT (deferred v1.8 UAT paused: this must NOT fall into the same deferred bucket).

---

### Pitfall 12: imap-proto parser/shape gaps across the NEW fetch lane — silent empty streams (NAMESPACE lesson)

**Warning sign:** the References lane returns FEWER responses than UIDs requested — or zero — with NO error raised and no log; the only visible artifact is missing threads after sync.

**What goes wrong:** the NAMESPACE tripwire is the repo's warning: imap-proto 0.16.7 doesn't parse every IMAP response shape and a failed parse in a stream doesn't error — it may swallow the item into an unidentified response the parser skips. The References lane is a NEW response shape the parser HAS (STACK.md verified the exact `AttributeValue::BodySection { section: Header, data: Some(bytes) }` mapping via parser source + `test_header_fields` fixtures) — but verify the bytes actually show the fold-friendly section even on real Zimbra: (a) whether the section envelope is `BODY[HEADER.FIELDS (REFERENCES)]` or `BODY[HEADER.FIELDS("REFERENCES")]` on the reply; (b) whether the field repeats in some servers; (c) whether server strips the trailing field name / emits CRLF whitespace.
- If a fixture test uses an idealized response string the parser matched in-source, but a live server emits a variant shape (the 2026-10-18 APPEND FLAGS wire variances were exactly that), the lane returns empty folds and every lib test green says nothing.

**Why it happens:** lib tests test parser MATCH with the exact produced shape; live shape variance is invisible to library tests. And this failure mode is specifically per the decision not to upgrade async-imap: we are staying on 0.11.3 precisely to avoid parser behavior drift — but that couples us to the old parser's quirks we must verify against our own server.

**How to avoid:**
- Kill-switch + fallback plan pre-declared (STACK.md variant): if Zimbra refuses/returns malformed section bytes for `HEADER.FIELDS (REFERENCES)`, fall back to `References = In-Reply-To` when the field is absent (the 2026-10-18 convention — shallow chains still thread correctly) — document as Known Gap, not blocked phase.
- Phase 42's live gates must include a small REAL mailbox round-trip, not just lib fixtures — the 2026-10-18 bracketed Message-ID SEARCH bug was ONLY caught because a live gate ran; live gates versus this codebase's build pipeline are mechanically the only smoke test of wire shape. The gate is buffered by the same 530-lib tests that ran for v1.8 — the audit trail for the v1.9 lane should add the spec-live fixture bytes 'as received' to the test corpus (paste an actual response once, so future phases can test 'as received' strings instead of re-discovering the wire).

**Phase to address:** **Phase 42** live-gate design (write the round-trip test BEFORE trusting the header sweep materialization); capture the real-bytes fixture once to hand to future phases.

---

### Pitfall 13: "A stub compiles" — the over-claim trap, guaranteed to recur unless mechanically counter-gated

**Warning sign:** a phase summary says 'conversation reader works' while your list is keyed on derivation-time aggregation but the actual path defensively `or` fallback to single-message `fetch_message`, cutting thread API work entirely; tests green, feature absent.

**What goes wrong:** exactly the Phase 38 precedent: a phase summary states a feature works while the repo contains only compiled stubs — reference to the entry point exists, tests are green, and human UAT is deferred "just this once." v1.9 compounds the risk because the UI looks "done" the first time a reader renders a thread — but the hazards are all DOWNSTREAM paths (batch actions over a thread; per-card flags mid-conversation; batch undo shape; thread members as source of truth for flag events). Each of those is trivially stubbable while everything else compiles.

**Why it happens:** the seat-of-pants form is the same in this codebase: an entry point took a big build; test coverage for 'backend had the feature' exists and is easy to write; a UI button exists and easy to stub; and human UAT is deferred 'just this once'. Repetition risk HIGH in a milestone whose entire feature set is UI-driven.

**How to avoid (mechanical, not operator-discipline):**
- Acceptance-defined phase exit: each phase has a named set of success activities in plan-check, NOT 'test suite green' — e.g. 43's gate is 'a real References pair on UTFPR mail sets one Conversation row via UAT (member list on stacked cards), and no single-message regression row', not 'list thread payload added'.
- Per-phase grep-based claim veriFiers: before summary signoff, the SAME commands the vite build runs must ALSO verify the batch command dispatch actually reaches the DURABLE confirmation path — FEATURES.md DoD checklist 'MVP gate' is the blueprint from which the plan-check gate consumes verbatim.
- Every phase whose approach is UI-heavy must include ONE screenshot-level manual harness run (even chrome-less via WebKitGTK screen-capture or the existing vitest DOM test) — v1.7 shipped because NO UI path had a harness; the browser-QA absence wasn't a plan time failure, it was a phase-exit-criteria failure. Fix the criteria, not the model behavior.

**Phase to address:** **Phase 45** (assemble exit criteria from FEATURES.md MVP gates per phase); each phase plan in 42–44 embed its gate at plan time (plan-check consumes).

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Client-side fallback subject-grouping "for threads missing References" | Threads always render grouped | Re-introduces the exact unreliable grouping the user rejected (false joins on common subjects) | Never — single-message row is the honest state (Features.md anti-feature) |
| Always mounting every card's iframe "since bodies are cached" | No lazy-mount complexity | Long-thread GPU/DOM blowup on low-end hardware | Never — lazy-mount is cheap and the P3 fold button is a phase 43 e-tick |
| Inline `vitest`-only a11y check "because it's easy" | Green CI | WebKitGTK runtime divergence goes untested; the nested-button class is exactly what CI tools DON'T catch | Acceptable as an addition, never as the only check |
| Skipping the per-folder dirty-set on recompute | Recompute is trivially correct | Folder with 10K messages re-JWZs on every sweep — cost compounds even after outage/sync duplicates hit | MVP acceptable IF volumes are small; add dirty-set before M27 |
| Extending `FETCH_ATTRS` with References inline "one new attr" | No new constant | Every headers sweep pays the wire bytes; `fetch_attrs_are_parenthesized` tripwire fight; sweep shape couples lanes | Never — separate named constant (Pitfall 4) |
| Reading thread members WITHOUT filtering `pending_delete` | Simpler SQL | Count badge lies; reader fetches tombstoned bodies; undo shape wrongs itself | Never — share the `list_messages` predicate fragment |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Tauri invoke new args | Adding a field on either side only (silently dropped, snake/camel convention drift) | Both sides in one commit; round-trip serde test (Pitfall 1) |
| async-imap 0.11 References lane | Assuming headers arrive as stored byte strings; writing BODY[...] instead of BODY.PEEK[...] variants | New parenthesized named constant + live-gate bytes capture (Pitfalls 4, 12) |
| rusqlite_migration | "One more migration = one new M::up" without the two-path test | Fresh + upgrade fixture pair every time; SCHEMA_VERSION bump same commit (Pitfall 3) |
| mail-threading crate | Assuming output `Thread.messages` order is chronological — it's not documented | Re-sort by (date_utc, uid) in `threads.rs` before persisting; fixture the tiebreak (Pitfall 7) |
| FLAG_UPDATE_EVENT | Patching row state key directly on (mailbox, uid) when the row's key is now (mailbox, thread_key) | Listeners re-derive from member payload; key computed in payload layer (Pitfall 11) |
| Existing `queue_send` / drafts | Recomputing thread_id on the Sent row at APPEND time (moving the compute INSIDE the send pipeline) | Keep compute at sync-end/box events on reads only; the send pipeline stays byte-stable (v1.8 wire tests stay green) |
| font-src CSP | Trusting default-src fallback forever | Add explicit `font-src: 'self'` when the fonts land (Pitfall 10) |
| WebKitGTK dialog focus restore | focus trap without return-focus; Escape works in testing browser only | Shared Dialog primitive with restore-by-reference; per-modal proof assertion (Pitfall 9) |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Whole-folder per-sweep recompute | sync-end latency grows with folder size | Dirty-set guard on recompute; per-folder only; only on references/backfill changes | ~2K+ messages/folder with sync churn; invisible in a 50-message test inbox |
| 20+ mounted iframes in one reader thread | reader scroll jank, WKWebView memory balloon | Lazy-mount + cap concurrent iframes; unmount collapsed bodies | Open of any thread with ≥10 expanded cards on slow disks |
| Full REFERENCES lane on every sweep | every sync pulls N refs never previously needed for old UIDs | Only newly-synced UIDs + one optional per-folder backfill + on-milestone re-fetch | First sync has heavy diff but subsequent syncs fold if scoped correctly |
| Classified `msg_au` FTS trigger storms | WAL churn during dirty-set recompute | updates to thread columns bypass msg_au (WHEN-gated to indexed cols — verify the trigger doesn't fire on new cols); batch thread_id UPDATE in one tx | Immediately on first recompute of a big folder |
| Web font without preload on slow disk | Row-font pop + CLS on first paint per session | font-display: swap + system-fallback normalization; `block` only for display font if proven needed | First run, or every launch where the fontcache is cold |

## Security Mistakes

| Mistake | Risk | Prevention |
|--------|------|-----------|
| References values logged in plain `_logger` "for debugging lane shape" | Header contents of strangers' mail now reachable in local logs; v1.9 lanes expose new data shapes that tempt logging | Lane failures log ONLY shape stats + uids (message_id never verbatim); keep house redaction rules. Also: `references_json` is already content-pointer — leave as is |
| iframe source storing `references_json` data rendering verbatim in the reader | Ma11model/unsafe injection on ember UI (References isn't user-content, subject is; but IN-THREAD participants derive from envelope ADDRESSES which are attacker content) | All thread participant strings render by the exact same tokenized React path as existing `from_addr` — no dangeRouslySetInnerHTML in NEW components, PERIOD |
| prompt-injection surface growth per-thread | LLM generation from a thread card inherits the per-message PHI/guideline behavior — but threads now inject a larger combined body ('chat history' pull) into the generate window | Generate-from-a-card keeps using the ADDRESSED message's body (per-card context only) — do NOT assemble a whole thread into one prompt in v1.9 (out of approved lane; if desired, separate phase) |
| Batch thread ops with no confirm modal | One click deletes U+N messages (vs 1) | ExpungeModal pattern reused with member-count in the copy; destructive batch never keyboard-mistakeable |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Single-message rows shifting when their count badge (≥2) makes a row taller mid-session | Layout jumps as replies arrive; scroll target lost while user reads | Single-message rows render BYTE-COMPATIBLE (no badge); count changes via guarded refresh only; row height change requires the aggregate state machine (FEATURES.md) not conditional inline style |
| "Reply" on the thread vs the exact card ambiguous in the reader header | Sends to the wrong group of holders when the conversation contains a multi-recipient reply-all | Per-card reply threading headers follow CARD identity (v1.7 headers stay per-message); thread-level actions in the header are only (mark-read, move, delete), never compose |
| Thread badge count disagrees with the reader | Row says "3" but the reader shows 4 (late-arrival between refresh) | Count is derived ONCE per payload from the same member list the reader uses; mid-session arrival is followed by guarded refresh of BOTH, not a clamp |
| Undo toasts for conversation batch = N toasts visually stacked | Users can't tell which altitude they're undoing | Either ONE batch undo (needs approved backend contract) or N toasts labeled per message — but no hidden partial restore; pick inside the v1.9 scope decision explicitly |
| Fonts change identity on day one (FOUT) | Weeks of stable look shift under the user in a minor update | token stack loads woff2 via @font-face with swap AND the normalization stack updates the SAME commit so post-load font matches pre-load fallback metrics as closely as the subset allows (FONT-CL: keep tokens both sides qualifiable) |

## "Looks Done But Isn't" Checklist

(verify path or evidence, not code presence)

- [ ] **References lane:** verify by a live server round-trip (or a byte-captured real server response), NOT by parser-path unit test — mark 'as received' bytes into the fixture corpus.
- [ ] **thread grouping:** verify computed against a personal INBOX where a KNOWN reply pair exists (v1.8 UAT artifact) — a `thread_id` in the DB is not 'threads work'.
- [ ] **Conversation group / batch delete:** verify an outbox tied to a thread that ALSO contains a member with pending_move closes with a correct serialized undo (or documented N-toasts); the summary must NOT claim 'batch undo' unless the backend batch undo contract was approved and shipped.
- [ ] **Unread semantics in-thread:** verify a member toggling unread updates the row badge within the same event tick without a full refresh; verify `set_seen` remains per-message wire behavior unchanged.
- [ ] **Keyboard a11y:** verify in WebKitGTK (the actual runtime) not just Chromium-tested CI, roving + enter + Esc-modal-focus-restore per modal.
- [ ] **Fonts:** verify `document.fonts.check('700 16px "Baloo 2"')`-equivalent on a launch, and the app is stable LOOKING if fonts fail to load (fallback stack missing is a scoped defect — the tokens shall define a real approximated stack).
- [ ] **Migration M26:** verify BOTH fresh-install and one-SCHEMA-STEP-shorter upgrade fixtures; grep the SCHEMA_VERSION test name, don't assume it caught up.
- [ ] **peek_audit path 5:** verify the test enumerates the new lane (audited_paths.len assert bumped).
- [ ] **Tauri payload round-trip:** verify every NEW payload/arg key round-trips in a live invoke (not just TS types match).

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Body-preview missing/folded reference parse (Pitfall 2) | LOW | Normalization is a pure function; pop fixtures, patch normalization, remigrate (references_json reparseable from stored raw only if that add — else the per-folder backfill re-fetches) |
| \Seen corrupted by a bad lane (Pitfall 4) | HIGH | No client-side unshift; recover by re-marking unread from a fresh uid-level FETCH FLAGS + reconciliation — costly and trust-breaking; PREVENT in gates st. dev never runs the broken lane once |
| bad thread scrape across folders (Pitfall 5) | MEDIUM | Recompute is idempotent: recompute with proper scope resets thread_id cleanly; UI refresh required post-correction; no data loss |
| M26 migrate broken fresh path (Pitfall 3) | MEDIUM | Migration tests are cheap to repair in-place; per-fresh-test client spread is rare but rerun M26 declared; production DB may need a concise manual ALTER to catchup the straggler columns |
| Event listener identity ambiguity (Pitfall 11) | MEDIUM | Derive from payload; no state carryover; single refresh cycle cleans residual |
| Pagination-group split (Pitfall 8) | LOW | Decision fix; payload is unchanged; UI grouping only |
| Nested-button/DOM regression (Pitfall 9) | MEDIUM | Rebuild row on the list option contract; per-modal focus restore proof test; no re-landed features to repair downstream |
| iframe bloat (Pitfall 10) | LOW | Cap + lazy-mount is a contained reader-stack change; no backend |
| Over-claim at phase exit (Pitfall 13) | HIGH | Feature may ship much later than the summary claimed — the repair cycle (2026-10-18) cost ~2 days + a hardened credibility note; PREVENTation via phase exit criteria is the cheapest option by far |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| 1 Tauri payload silent-drop (invoke args/fields both directions) | 42 (contracts), standing rule | Round-trip serde test per new command; UAT after 45 |
| 2 References parse edge cases (fold/malformed/dup/brackets) | 42 | references.rs fixture suite; live-gate bytes captured into the corpus |
| 3 M26 SCHEMA_VERSION / fresh+upgrade | 42 | Fresh + upgrade fixture pair; SCHEMA_VERSION=26 const test |
| 4 \Seen break in new lane (peek_audit path 5) | 42 | peek_audit extended assert; no dev run against live before gate green |
| 5 thread_id folder/UIDVALIDITY isolation & thread_key | 42 def, 43 use | thread_key-named payload; inbox+sent cross-fixture; refresh idempotence |
| 6 pending_delete/outbox interplay with thread members | 43 | Shared pending-delete predicate; batch command notes; UAT outbox collision |
| 7 Recompute cost / order semantics / late arrival | 42 | Dirty-set guard fixture; ordering tiebreak fixture; refresh-under-you accepted at 43 |
| 8 Pagination vs grouping disagreement | 43 | Cross-page thread continuation test at ≥3 page size boundary |
| 9 a11y semantics (nested interactive, focus model, modal, WebKitGTK) | 44 | Per-modal focus-restore assert; roving+focus-visible pairing task |
| 10 FOUT/CLS/CSP + iframe budget | 43 (iframes), 44 (fonts/CSP) | lazy-mount cap test; font-fallback token compare; `font-src: 'self'` line |
| 11 Event-bus re-derived state on conversation rows | 43 | Listener re-aggregation test within one event tick |
| 12 imap-proto shape variance on the live server | 42 | Live-gate round trip BEFORE lane goes REAL mailbox-wide default |
| 13 Over-claim (stub-compiles) at phase exit | 45 assembly + 42/43/44 embedded gate | FEATURES.md MVP DoD per phase, plan-check enforced |

## Sources

- **Project precedent (HIGH):** `.planning/PROJECT.md` — Key Decisions (NAMESPACE tripwire, Phase 38 over-claim, 2026-10-18 four-breaks, M23 SCHEMA_VERSION race documented in-source at `store/mod.rs:154-166`, the pending_delete/M7 and outbox UNIQUE contract, thread lane decision 2026-10-20, fonts-embed decision); `.planning/milestones/v1.8-closeout/` deferred-UAT record.
- **SGE source (HIGH, read directly):** `src-tauri/src/store/mod.rs` (SCHEMA_VERSION=25, M-list, preserve-rows test pattern, M23/M24 conflict comment); `src-tauri/src/imap/headers.rs` (FETCH_ATTRS parenthesization rationale + Zimbra BAD documentation, peek_audit four-path audit + audited_paths count assert, decode_header_words fail-visible pattern); `src-tauri/src/queries.rs` functional surface via written research (list_messages flat MessageRow, queue contract); frontend invoke shape via `grep` of `src/components/*` (ReadingPane camelCase args, ComposeWindow `onEvent` channel arg).
- **Cross-research (HIGH):** `.planning/research/STACK.md` (thread lane integration points, async-imap 0.11.3 parser shapes verified, CSP/font-src analysis, mail-threading API + per-folder recompute recommendation); `.planning/research/FEATURES.md` (nested-button hazard in MessageList, IFRAME per-card claim, DoD per-message semantics, MVP gates).
- **W3C ARIA APG — Listbox pattern** (w3.org, fetched live 2026-10-20): nested-interactive prohibition, selection≠focus, `Content focus/selection` rules. Confidence MEDIUM-HIGH.
- **Tauri v2 CSP docs** (`v2.tauri.app`, fetched live 2026-10-20): directive fallback; font-src semantics.
- **mail-threading 0.1.3 README** (GitHub, 2026-10-20): ThreadingOptions, dup policy, phantom pruning, subject_merge=false recommendation. Confidence MEDIUM (project small but conformance corpus noted).
- **RFC 5322 §2.2.3 header folding / RFC 2045 MIME** — stable training knowledge, HIGH historical confidence.

---
*Pitfalls research for: SGE v1.9 — Email Threading + Interface Revamp*
*Researched: 2026-10-20*
