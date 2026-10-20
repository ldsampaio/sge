# Phase 42: Threading Backend Lane - Context

**Gathered:** 2026-10-20
**Status:** Ready for planning

<domain>
## Phase Boundary

Deliver the threading data lane as an isolated, independently shippable backend phase: persist `in_reply_to` (from the already-fetched ENVELOPE) and `references` (via a new `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` UID-range lane) through migration 26; compute a stable per-folder `thread_id` from the References/In-Reply-To chain (JWZ); extend the list payload additively behind the unchanged `list_messages` name/signature; add a header-only `get_thread` command. The flat list and all rendering stay completely unchanged in this phase. Includes an optional per-folder backfill of References for mail already cached (trigger decided below). No grouping UI, no reader work.

</domain>

<decisions>
## Implementation Decisions

### Backfill of References for pre-existing mail
- **D-01:** Backfill runs automatically, once per folder, at that folder's first synchronization after the 26 upgrade: the same sync pass issues one extra batched `UID FETCH <range> (UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])` lane over not-yet-backfilled rows, then marks the folder's backfill state as done. Subsequent syncs fetch References for newly synchronized messages only. — **Reversibility:** reversible — deferred by a per-folder state bit; skipping the lane leaves rows with NULL references and NULL thread_id, which the payload already treats as own-threads.
- **D-02:** The backfill lane must reuse the project's sync-gate discipline: batched ranges (like `fetch_envelope_batch`), single-flight SyncGate protection, resizable batches, and resumable/r idempotent accounting (a partially completed backfill resumes; re-running neither duplicates rows nor refetches folders already marked done). A huge folder must not block the interactive path — the backfill yields between ranges so sync_status stays live.

### Thread-score strategy (thread_id computation)
- **D-03:** thread_id is computed with the `mail-threading` crate 0.1.3 (JWZ message threading + RFC 5256 conformance corpus tested; dep tree = chrono only, already in the project tree; MSRV 1.88 < env rustc 1.99). Recompute is exercised with `subject_merge: false` (group by references, not by subject — subject-only grouping is the explicitly rejected strategy). The documented fallback, exercised only if the crate fails a gate, is an in-house JWZ core (~250 LOC) validated against the same corpus. — **Reversibility:** costly — thread_id values are persisted data (indexed column + payload contract); swapping the algorithm later means a full recompute epoch across folders and payload churn, though the `thread_key = (mailbox_id, thread_id)` abstraction keeps callers stable.
- **D-04:** Recompute is a per-folder dirty-set epoch at the sync-end / backfill-complete / send-file hooks, never per-insert: early-exit on zero new rows; `(date_utc, uid)` as the deterministic tiebreak; membership changes are idempotent (re-run duplicates nothing). `thread_key = (mailbox_id, thread_id)` is the grouping contract everywhere; NULL thread_id = own-thread.

### Fixture corpus (agent's discretion — not user-discussed)
- **D-05 (discretionary):** The References parser fixture suite ships with synthesized offline fixtures covering: folded continuation lines, `<...>`-framed addresses, bracket-less/case-variant inputs, malformed Message-IDs, duplicate ids, empty chains, chain-length cap. Live byte-capture from the real server (HEADER.FIELDS round-trip against mail.utfpr.edu.br) is **deferred with reason** per project convention (no creds in this environment); a capture script in the `scripts/live-*` pattern is provided so the user can append real as-received bytes later.

### Agent's Discretion
- Envelope `in_reply_to` parsing internals, exact M26 column/DDL shape, index choices, `get_thread` ordering details, payload field names, SQL placement (all inside `store/`), error taxonomy wording — planner/researcher territory per ROADMAP and research docs.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Milestone planning docs
- `.planning/REQUIREMENTS.md` — v1.9 REQ-IDs; THR-01..04 own this phase; the D-batch decisions of this file bind the planner
- `.planning/ROADMAP.md` §Phase 42 — goal, success criteria, plan-review decision points flagged for this phase (mail-threading dep confirmation — now decided D-03; live round-trip gate — deferred per D-05)
- `.planning/PROJECT.md` — constraints (BODY.PEEK-only, UID-only, UIDVALIDITY-first, SessionManager, keyring), Key Decisions table (2026-10-20 rows)
- `.planning/research/STACK.md` — mail-threading 0.1.3 due diligence, imap-proto `Fetch::section` accessor verification, M26 shapes, what NOT to take (async-imap 0.12, use-email-id)
- `.planning/research/ARCHITECTURE.md` — the 4 verified hook sites (MessageHeader in `imap/headers.rs`, REFERENCES_ATTRS lane, M26 in `store/mod.rs`, recompute triggers in `sync/worker.rs` step 5-tail/7 + post-backfill + post-send), `store/threads.rs` wrapper plan, peek_audit path-5 same-commit invariant
- `.planning/research/PITFALLS.md` — #1 both-sides serde round-trips, #2 references parser edge cases, #3 SCHEMA_VERSION same-commit rule (M23 precedent), #4 bare BODY[] corruption, #5 thread_key scoping, #7 pending_delete predicate, #12 state.json/stale claim traps

### Code the lane touches (verify current state at plan time)
- `src-tauri/src/imap/headers.rs` — FETCH_ATTRS + `parse_fetch_item`, peek_audit / fetch_attrs_are_parenthesized guards (must gain path 5)
- `src-tauri/src/store/mod.rs` — migration list M1..M25, SCHEMA_VERSION 25, migration test pattern (preserve-rows)
- `src-tauri/src/store/schema.sql` — `messages` table (message_id nullable exists; no references/thread columns yet)
- `src-tauri/src/store/queries.rs` — the ONLY home for new SQL; `list_messages` query + MessageRow; `get_message_by_uid`/`get_reply_context`
- `src-tauri/src/commands/sync.rs` — `list_messages` command, `fetch_message` → MessageView (struct frozen; payload additive only)
- `src-tauri/src/commands/classify.rs` — register `get_thread` in `lib.rs:503` invoke_handler site
- `src/types.ts` — TS mirrors of Rust payloads; comment documents References "NOT parsed today" (stale after this phase — update in the same commit)
- `CODESTYLE.md` — repo code-style rules for Rust/TS edits

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `fetch_envelope_batch` batching pattern (imap/headers.rs): reuse the UID-range batching + parse loop shape for the References lane
- M4 tombstones backfill sweep pattern (store): precedent for resumable, idempotent per-folder sweeps with done-marker accounting — the backfill follows this shape
- `peek_audit` / `fetch_attrs_are_parenthesized` guard tests: extend, don't bypass, in the same commit
- `get_reply_context` + `messages.message_id` (from the 2026-10-18 repair): message_id extraction and header formatting house patterns (`<...>` framing)
- `flag_outbox`/`imap_outbox` UNIQUES: batch-precision on rows already queued — member SQL shares `pending_delete = 0` with list_messages
- `sync_status` plumbing + SyncGate single-flight: backfill must report through it and never break the interactive sync path

### Established Patterns
- "No raw SQL escapes queries.rs" invariant (25-migration history): all new SQL in store/
- Frozen command discipline: `list_messages` keeps name/signature; body routes to a new internal select (`list_threads` shape) — additive fields only
- Fresh+upgrade migration fixture pair (M23 SCHEMA_VERSION precedent): M26 bumps 25→26 in the same commit both fixtures tested
- Deferred-with-reason live gates (LIVE-01/02/03 precedent + v1.5 closeout): offline green, live leg deferred with documented reason + script

### Integration Points
- `imap/headers.rs` MessageHeader struct: add in_reply_to (ENVELOPE already carries it — zero new wire bytes on the attrs lane) + references fields for the new lane
- `store/mod.rs` migration vec: append M::up as M26; bump SCHEMA_VERSION
- `sync/worker.rs` step 5-tail/step-7: recompute epoch hooks after new-row/backfill processing
- `send_queue.rs`/drafts send-file hook: a sent reply must join its folder's thread via the send-file recompute
- `lib.rs` invoke_handler: register `get_thread`
- `types.ts` + `commands/sync.rs` structs: TS+Rust payload types land side-by-side in one commit with serde round-trip tests

</code_context>

<specifics>
## Specific Ideas

- User explicitly chose: backfill automatic per folder at first sync post-upgrade (D-01); mail-threading crate over in-house JWZ (D-03). No specific UI/design references — backend phase, rendering unchanged.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. (Subject-based grouping remains rejected project-wide; dark theme/stars remain v9-future per REQUIREMENTS.md.)

</deferred>

---

*Phase: 42-Threading Backend Lane*
*Context gathered: 2026-10-20*
