# Phase 42: Threading Backend Lane — Research

**Researched:** 2026-10-20
**Domain:** IMAP `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` fetch lane + SQLite migration M26 + JWZ threading via `mail-threading` 0.1.3 + additive payload contract, on the existing SGE Rust+Tauri v2+SQLite backend
**Confidence:** HIGH — every in-repo anchor was re-opened this session; `mail-threading` verified directly against crates.io and docs.rs; imap-proto/async-imap anchors re-verified in the registry-copied sources; one live-codec probe executed offline

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Backfill runs automatically, once per folder, at that folder's first synchronization after the 26 upgrade: the same sync pass issues one extra batched `UID FETCH <range> (UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])` lane over not-yet-backfilled rows, then marks the folder's backfill state as done. Subsequent syncs fetch References for newly synchronized messages only. — **Reversibility:** reversible — deferred by a per-folder state bit; skipping the lane leaves rows with NULL references and NULL thread_id, which the payload already treats as own-threads.
- **D-02:** The backfill lane must reuse the project's sync-gate discipline: batched ranges (like `fetch_envelope_batch`), single-flight SyncGate protection, resizable batches, and resumable/idempotent accounting (a partially completed backfill resumes; re-running neither duplicates rows nor refetches folders already marked done). A huge folder must not block the interactive path — the backfill yields between ranges so sync_status stays live.
- **D-03:** thread_id is computed with the `mail-threading` crate 0.1.3 (JWZ message threading + RFC 5256 conformance corpus tested; dep tree = chrono only, already in the project tree; MSRV 1.88 < env rustc 1.99). Recompute is exercised with `subject_merge: false` (group by references, not by subject — subject-only grouping is the explicitly rejected strategy). The documented fallback, exercised only if the crate fails a gate, is an in-house JWZ core (~250 LOC) validated against the same corpus.
- **D-04:** Recompute is a per-folder dirty-set epoch at the sync-end / backfill-complete / send-file hooks, never per-insert: early-exit on zero new rows; `(date_utc, uid)` as the deterministic tiebreak; membership changes are idempotent (re-run duplicates nothing). `thread_key = (mailbox_id, thread_id)` is the grouping contract everywhere; NULL thread_id = own-thread.
- **D-05 (discretionary):** The References parser fixture suite ships with synthesized offline fixtures covering: folded continuation lines, `<...>`-framed addresses, bracket-less/case-variant inputs, malformed Message-IDs, duplicate ids, empty chains, chain-length cap. Live byte-capture from the real server (HEADER.FIELDS round-trip against mail.utfpr.edu.br) is **deferred with reason** per project convention (no creds in this environment); a capture script in the `scripts/live-*` pattern is provided so the user can append real as-received bytes later.

### Agent's Discretion
- Envelope `in_reply_to` parsing internals, exact M26 column/DDL shape, index choices, `get_thread` ordering details, payload field names, SQL placement (all inside `store/`), error taxonomy wording — planner/researcher territory per ROADMAP and research docs.

### Deferred Ideas (OUT OF SCOPE)
- None — discussion stayed within phase scope. (Subject-based grouping remains rejected project-wide; dark theme/stars remain v9-future per REQUIREMENTS.md.)
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description (REQUIREMENTS.md:25-:28) | Research Support |
|----|-------------|------------------|
| THR-01 | Extract `in_reply_to` from ENVELOPE + `references` via new `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` UID-range lane; persist both in M26 (`messages.in_reply_to`, `messages.references_json`, indexed `messages.thread_id`) | imap-proto `Envelope.in_reply_to` verified at vendored `src/types.rs:619`; async-imap `Fetch::section` at `src/types/fetch.rs:179`; M4 ALTER-column precedent at `store/mod.rs:251-261`; `FETCH_ATTRS` parenthesization rule at `imap/headers.rs:14-21` |
| THR-02 | Stable `thread_id` via JWZ (`mail-threading`, `subject_merge: false`), per-folder dirty-set epoch at sync-end/backfill/send-file hooks, `(date_utc, uid)` tiebreak, never per-insert | mail-threading 0.1.3 API fetched from docs.rs (exact `ThreadingOptions { subject_merge: false, .. }` recipe exists); worker.rs Step-5 tail + pending-gate sites located (`:1492`, `:1475-:1490`); PITFALLS #7 dirty-set + tiebreak fixtures prescribed |
| THR-03 | Additive conversation fields behind unchanged `list_messages`; new `get_thread` command, header-only members; TS+Rust side-by-side | `list_messages` SQL verified at `store/queries.rs:874-:900`; `MessageRow` at `:16-:28`; TS mirror at `src/types.ts:4-:15`; invoke-handler registration site at `lib.rs:503`; PITFALLS #1 serde round-trip rule |
| THR-04 | BODY.PEEK-only / `\Seen` never set / UID-only + UIDVALIDITY gating / per-folder scoping / `peek_audit` + `fetch_attrs_are_parenthesized` extended to new lane same-phase | `peek_audit` enumerates exactly 4 paths with `assert_eq!(audited_paths.len(), 4)` at `headers.rs:463-:504`; `fetch_attrs_are_parenthesized` guard at `:428-:443` incl. `!FETCH_ATTRS.contains("BODY.PEEK[HEADER")` tripwire |
</phase_requirements>

## Summary

Phase 42 is fully buildable offline with high confidence. Every integration seam exists and is narrow: `imap-proto 0.16.7` already parses `in_reply_to` into the ENVELOPE (`Option<Cow<'a,[u8]>>`) that SGE fetches today — **zero new wire bytes** for that field; the References lane is a new named constant riding the proven `(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])` shape that both imap-proto and async-imap 0.11.3 handle via `Fetch::section`; and M26 is three nullable `ALTER TABLE … ADD COLUMN` lines plus one index, following the exact M4/M5/M6 column-addition precedent in `store/mod.rs`.

The one new dependency, `mail-threading` 0.1.3, is real on crates.io (pub. 2026-06-04, MIT/Apache-2.0, MSRV 1.88, dep tree chrono-only, which is already pinned at `Cargo.toml:78`) and its documented API matches the STACK.md pin verbatim — but the legitimacy seam rates it **SUS (low downloads, 5-month-old crate)**, so the planner must gate its install behind a `checkpoint:human-verify` task per protocol. D-03 already locked the choice with due diligence; the checkpoint is a process requirement, not a re-decision.

The known trap that research must record as **proof, not prose**: running the house `decode_header_words` on raw REFERENCES bytes rewrites msg-id tokens that happen to contain `=?…?=`-shaped substrings (probe output below: `<X=?utf-8?Q?=42?=@host>` → `<XB@host>`), and it does NOT unfold folded chains (`<a@x>\r\n <b@y>` passes through unchanged). The References parser therefore operates on the raw `decode_bytes_opt` string with unfold-then-tokenize, never through `decode_header_words`.

**Primary recommendation:** Ship the lane in four plans — (1) M26 + fresh/upgrade fixture pair + `SCHEMA_VERSION` 26 test first (atomic base), (2) `imap/references.rs` parser + fixture suite + the new `FETCH_REFS_ATTRS` lane with `peek_audit` path-5 in the same commit, (3) `store/threads.rs` compute + per-folder dirty-set epoch + backfill hook, (4) additive `list_messages` fields + `get_thread` + TS mirror + serde round-trip tests. Live-server legs are deferred-with-reason + capture script per D-05.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| References wire fetch | `imap/` (headers.rs lane fn + new attrs constant) | `sync/worker.rs` (drives ranges, gates, yields) | All IMAP verbs and their parsers live in `imap/`; the worker owns batching/single-flight |
| `in_reply_to` extraction | `imap/headers.rs` (`parse_fetch_item`/`parse_fetch_attributes`) | — | ENVELOPE already fetched on the existing sweep; zero new lanes |
| Persist header fields + thread_id | `store/` (M26 in `mod.rs`; all new SQL in `queries.rs`) | — | "No raw SQL escapes this module" invariant at `queries.rs:1-5` |
| JWZ thread computation | `store/threads.rs` (new wrapper: SQL ↔ mail-threading ↔ UPDATE) | third-party crate `mail-threading` | Wrapper owns folder scoping, tiebreak sort, idempotent UPDATEs; crate owns the algorithm |
| Recompute triggers (epoch) | `sync/worker.rs` (step-5 tail / backfill-complete) | `send_queue`/drafts send-file hook | Sync pass owns the folder's dirty set; send-file event joins Sent copy |
| Payload extension + `get_thread` | `commands/sync.rs` (or classify.rs per CONTEXT:51 pointer) | `src/types.ts` mirror, `lib.rs` invoke_handler | Tauri command tier owns serde-visible payload shapes; TS side must land same commit |
| UI consumption of thread data | **OUT OF SCOPE this phase** (Phase 43+) | — | ROADMAP:45 — Phase 44 consumes "thread_id in store + extended payload + get_thread" |

## Standard Stack

### Core (already pinned — keep, do not bump)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | 0.37 (bundled) | M26 is plain ALTER SQL; queries.rs home | `Cargo.toml:68` — STACK.md:15 keeps 0.37: 0.40.2 exists but bumps "touch every queries.rs signature for zero benefit" |
| rusqlite_migration | "2" (2.3.x line) | `M::up(sql)` list append = M26 | `Cargo.toml:69`; STACK.md:16 keeps; M26 is "one new `M::up` + the two-path test" (PITFALLS remediation :300) |
| async-imap | "0.11" (0.11.3) | Fetch for the new lane + existing sweep | `Cargo.toml:64` — deliberately NOT bumping to 0.12 (STACK.md "what NOT to take") |
| chrono | "0.4" | `mail-threading` input `Message.date` + date_utc RFC3339 handling | `Cargo.toml:78` already present → `mail-threading` adds **zero marginal crates** without the serde feature (STACK.md:28) |

### New (this phase)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `mail-threading` | 0.1.3 (crates.io, pub. 2026-06-04, MIT OR Apache-2.0, MSRV Rust 1.88) | JWZ / RFC 5256 `THREAD=REFERENCES` compute → flat `Vec<Thread>` membership → per-folder `thread_id` | D-03 locked. Verified this session on crates.io + docs.rs: `thread_messages(&[Message]) -> Vec<Thread>` / `thread_messages_with(&[Message], &ThreadingOptions)`; `Message { id, message_id, in_reply_to, references: vec!, date, subject }`; `Thread.root_message_id` + `Thread.messages` are **caller ids**. Knobs: `subject_merge` (set **false** per D-03), `prune_phantoms` (default true — public thread root = first visible msg), `subject_prefixes`. Ships a JSON conformance corpus + RFC coverage matrix (`testdata/`) our fixture claims can point at. |

Pin to add in `Cargo.toml` `[dependencies]` (currently absent — verified in this session's read of `src-tauri/Cargo.toml:1-94`):

```toml
mail-threading = "0.1"          # caret-free exact: 0.1.3; do NOT enable serde feature (map Thread output manually in store/threads.rs)
```

### Supporting / Alternatives
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `mail-threading` 0.1.3 | In-house JWZ core (~250 LOC) | Only if the crate fails the legitimacy/corpus gate — D-03's documented fallback; corpus shared so either impl tests identically |
| `mail-threading` | `use-email-id` (0.1.0) | STACK.md:29 already rejected it — 157 downloads, no traction; not reconsidered |
| Raw-bytes References parser (house) | any email/MIME crate | mail-threading explicitly **does not** parse raw header bytes ("Scope" section, docs.rs); `decode_header_words` is the wrong tool (probe below); house pure function is correct here |
| `Fetch::section(SectionPath::Full(MessageSection::Header))` | re-parsing imap-proto `AttributeValue` manually | `async-imap-0.11.3/src/types/fetch.rs:179` is the public accessor (verified this session) |

**Installation:** `cargo` add via Cargo.toml edit (no npm/pip). Version verification (this session): crates.io API returned `max_stable_version: 0.1.3`, `license: MIT OR Apache-2.0`, 4 versions since 2026-05-16; env `rustc 1.99.0` ≥ MSRV 1.88.

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| mail-threading | crates.io | crate created 2026-05-16, 0.1.3 pub. 2026-06-04 (~5 mo) | 2,625 total / ~129/wk (seam) / 1,656 recent | github.com/planetaryescape/mail-threading (bhekanik — same author lineage as the `mxr` mail reader it was extracted from) | **[SUS]** (reason: low-downloads) | **Flagged — planner must insert `checkpoint:human-verify` before the Cargo.toml install step** |
| chrono | crates.io | mature | huge | already pinned `Cargo.toml:78` | n/a (no new install) | Approved |

**Packages removed due to SLOP verdict:** none.
**Packages flagged as suspicious [SUS]:** `mail-threading` — kept because D-03 (a locked user decision with STACK.md due diligence, including the ~250-LOC in-house fallback if the crate fails a gate) already sanctioned it; the SUS tag is a process gate, not a re-decision. The planner must add one `checkpoint:human-verify` task ("confirm `mail-threading` 0.1.3 is the intended crate: docs.rs/mail-threading, planetaryescape repo, MIT/Apache-2.0") ahead of any `Cargo.toml` edit, and keep the D-03 fallback plan live in the same phase.

*No package in this phase was discovered via WebSearch/training data — `mail-threading` was verified directly against crates.io this session, but the seam still rates it SUS, so it carries the flag rather than a clean [VERIFIED] pass.*

## Architecture Patterns

### System Architecture Diagram (data flow, this phase only)

```
IMAP server (mail.utfpr.edu.br:993/SSL)
   │
   │  existing sweep (unchanged): UID FETCH <range> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)
   │     └─► ENVELOPE already carries In-Reply-To ──► parse_fetch_item gains `in_reply_to`   [zero new wire bytes]
   │
   │  NEW lane (only for not-yet-backfilled / newly-synced UIDs):
   │     UID FETCH <range> (UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])   ← separate named constant, parenthesized
   ▼
async-imap stream ──► Fetch.section(Full(Header)) ──► raw REFERENCES bytes
   │                                                        │
   │                                             references.rs parser (pure):
   │                                             unfold CRLF+WSP → tokenize <...>/bare
   │                                             → normalize → dedupe (order-preserving) → cap
   ▼                                                        ▼
store upsert path ──► messages.in_reply_to + messages.references_json   (M26 columns, NULL = none)
   │
   ├─ sync-end hook / backfill-complete hook / send-file hook ──► dirty-set per (folder)
   │        zero-new-rows? → EARLY EXIT (never per-insert)
   ▼
store/threads.rs recompute epoch (per mailbox_id):
   SELECT id, message_id, in_reply_to, references_json, date_utc, subject WHERE mailbox_id = ?
   → mail_threading::thread_messages_with(&msgs, &ThreadingOptions { subject_merge: false, ..default })
   → sort members (date_utc, uid) → UPDATE messages.thread_id (root's messages.id) in ONE tx
   │
   ▼
list_messages (name/signature FROZEN) ── additive fields: thread_id/thread_key, thread_count,
   latest preview+date, per-member flags ──► TS mirror in src/types.ts (same commit)
get_thread(mailbox, uid|thread_key) ── header-only member rows (uid/from/date/flags/preview) ──► registered in lib.rs invoke_handler
   │
   ▼
UI rendering — UNCHANGED this phase (Phase 43+ consumes the data)
```

### Recommended Project Structure (additions)
```
src-tauri/src/
├── imap/
│   ├── headers.rs            # FETCH_REFS_ATTRS const + fetch_refs_batch + in_reply_to copy + peek_audit path 5 (MODIFY)
│   └── references.rs         # NEW: pure References parser + fixture suite
├── store/
│   ├── mod.rs                # M26 SQL const + M::up append + SCHEMA_VERSION 26 + fixture pair (MODIFY)
│   ├── schema.sql            # UNTOUCHED (v1 baseline — forward migrations only)
│   ├── queries.rs            # all new SQL: upsert columns, thread SELECT, member queries, epoch UPDATE (MODIFY)
│   └── threads.rs            # NEW: mail-threading wrapper + dirty-set recompute + thread_key helpers
├── sync/worker.rs            # backfill lane driver (first-sync-post-26) + epoch hooks at step-5 tail (MODIFY)
├── commands/sync.rs          # additive MessageRow fields + get_thread command (MODIFY)
└── lib.rs                    # register commands::sync::get_thread at the :503 invoke_handler list (MODIFY)
src/types.ts                  # TS mirrors land same-commit (MODIFY: + new comment update at :39-:42)
scripts/live-42.sh            # NEW capture script (scripts/live-* pattern; live leg deferred-with-reason)
```

### Pattern 1: Separate named attrs constant (PITFALLS #4 tripwire)
**What:** the References lane gets its own parenthesized constant next to `FETCH_ATTRS`, never an extension of it.
**When to use:** any new FETCH attribute set — `fetch_attrs_are_parenthesized` at `headers.rs:428-:443` contains the intentional tripwire `!FETCH_ATTRS.contains("BODY.PEEK[HEADER")` (verified verbatim, `headers.rs:439-:441`), which actively fights an inline References addition.
```rust
// Source: shape mandated by PITFALLS.md:99 + ROADMAP criterion 2
pub const FETCH_REFS_ATTRS: &str = "(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])";
```

### Pattern 2: Envelope field copy-through (zero new wire bytes)
**What:** `imap-proto` ENVELOPE already carries the field; the sweep just copies it.
**When to use:** `in_reply_to` on BOTH existing parse paths (`parse_fetch_item`, `headers.rs:509` `parse_fetch_attributes` for the CHANGEDSINCE path).
```rust
// vendored imap-proto-0.16.7/src/types.rs:619 (verified this session, inside Envelope):
//     pub in_reply_to: Option<Cow<'a, [u8]>>,
// headers.rs house decode (headers.rs:103-109) is the right decoder for a raw msg-id field:
//     fn decode_bytes_opt(b: Option<&[u8]>) -> Option<String> — trims trailing '\r', NO 2047 pass
```

### Pattern 3: Per-folder epoch with dirty-set guard (D-04)
**What:** recompute runs at pass-end per folder; early-exit on zero dirty rows; membership UPDATEs in one transaction; `(date_utc, uid)` tiebreak enforced after crate output (the crate's docs do NOT order `Thread.messages` chronologically — PITFALLS remediation `Integration Gotchas:301` prescribes re-sort in threads.rs).
**When to use:** sync-end, backfill-complete, send-file hooks — never inside the per-message upsert path.

### Pattern 4: Fresh + upgrade fixture pair (M23 precedent)
**What:** every forward migration ships two tests: fresh `open_in_memory` through the new M and an upgrade test replaying a truncated `M::up` baseline (through M25 here) then applying M26 with pre-seeded rows preserved.
**When to use:** M26 — precedent verified this session at `store/mod.rs:2075` (`m23_upgrade_from_v22_preserves_rows_and_seeds_settings`, truncated-baseline rebuild at `:2066-:2072`) and `store/mod.rs:1560` (`m2_upgrades_v1_database_forward_preserving_rows`).

### Anti-Patterns to Avoid (keyed to PITFALLS.md #1,#2,#3,#4,#5,#7,#12)
- **#1 Tauri payload silent-drop:** NEVER add `thread_id`/`thread_count`/member fields on only one side of the IPC boundary. Both TS and Rust payload fields land **same commit**, non-optional on the TS side for required fields, plus one serde round-trip test per new command key set. Tauri v2 silently drops unknown args — "tests green, feature absent" is the default suspicion (`PITFALLS.md:19-39`).
- **#2 References parse holes:** NEVER `split(' ')` on raw lane bytes and NEVER route lane bytes through `decode_header_words` (proof probe below: it rewrites `=?…?=`-shaped msg-id substrings and does not unfold CRLF+WSP). Unfold FIRST, then tokenize `<…>`/bare, normalize to `<…>`, dedupe order-preservingly, cap chain length (`PITFALLS.md:43-61`; D-05 fixture list is the required suite).
- **#3 SCHEMA_VERSION off-by-one:** NEVER land M26 SQL without bumping `pub const SCHEMA_VERSION: u32 = 25;` (`store/mod.rs:19`) to 26 in the same commit — and extend (or rename) `schema_version_is_25_with_m23_llm_settings` (`store/mod.rs:888`) so a stale `assert_eq!(SCHEMA_VERSION, 25)` (`:891`) cannot survive. M26 SQL contains ZERO `CREATE TABLE` copies of existing tables — `ALTER … ADD COLUMN` only (the M4 line `store/mod.rs:260` `ALTER TABLE mailboxes ADD COLUMN sweeps_since_full INTEGER NOT NULL DEFAULT 0;` is the column-add precedent).
- **#4 `\Seen` invariant:** NEVER write the lane as `BODY[HEADER.FIELDS (REFERENCES)]` (missing `.PEEK`) — one sweep perma-corrupts unread state server-side, unrecoverable client-side. NEVER extend `FETCH_ATTRS` with the References section. `peek_audit` (`headers.rs:463-:504`) must gain path 5 in the **same commit**: its `assert_eq!(audited_paths.len(), 4)` becomes 5 — the count assert IS the tripwire (`PITFALLS.md:87-104`, remediation `:371`). And: no dev run against the live mailbox before this gate is green.
- **#5 thread scoping:** NEVER key grouping on bare `thread_id` — the contract everywhere is `thread_key = (mailbox_id, thread_id)` (D-04 verbatim; `PITFALLS.md:119-124`): a root `messages.id` value can legitimately head distinct threads in two folders. Compute scope is strictly `WHERE mailbox_id = ?`. NULL `thread_id` = own-thread sentinel — write the byte-compat sentinel test early.
- **#7 recompute cost/order:** NEVER re-UPDATE the whole folder each sweep; dirty-set guard per folder. NEVER trust crate member order — persist after `(date_utc, uid)` sort with a tiebreak fixture (`PITFALLS.md:148-162`). Batch the `thread_id` UPDATE in one transaction (WAL churn under the single-writer Store mutex).
- **#12 imap-proto shape gaps / silent empty streams:** the lane's response shape IS parsed (re-verified this session: `Fetch::section` match on `AttributeValue::BodySection { section: Some(sp), data: Some(data) }` at `async-imap-0.11.3/src/types/fetch.rs:179`), but a parse miss surfaces as **fewer stream items, no error** — the same empty-stream trap that makes the FETCH_ATTRS comment warn about silent "0 messages" (`headers.rs:16-20`). Mitigation: lane accounting must diff requested-vs-received UIDs (reuse the sync worker's range-diff/strike discipline) + a live-gate byte capture (deferred per D-05, script provided) + pre-declared fallback: absent/unparseable References ⇒ treat `References = In-Reply-To` shallow chain (`PITFALLS.md:259` Known Gap).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JWZ threading (split cycles, phantom ancestors, duplicate Message-ID policy, preferred-parent) | custom graph walk | `mail-threading` `thread_messages_with(subject_merge: false)` | Crate implements RFC 5256 dup policy + phantom pruning exactly where hand-rolled JWZ goes wrong; ships the conformance corpus our fixtures point at (docs.rs "Conformance", fetched this session) |
| Forward migration + fresh-vs-upgrade parity | manual version checks / ad-hoc ALTERs at startup | `rusqlite_migration` `M::up` append + the fresh/upgrade fixture pair | `apply_migrations` inline `Vec<M>` at `store/mod.rs:82-108`; two-path parity is PITFALLS #3's whole lesson |
| TS↔Rust payload agreement | eyeballing the two structs | serde-derive + one round-trip test per new command in each direction | Tauri silently drops mismatched keys — PITFALLS #1 |
| Thread member ORDER | trusting crate `Thread.messages` output order | re-sort `(date_utc, uid)` in `store/threads.rs` before persisting | docs.rs (fetched this session) never documents chronological member order; PITFALLS `Integration Gotchas:301` prescribes the re-sort |
| Range batching + resumability for the backfill lane | a new ad-hoc loop | reuse `fetch_envelope_batch` shape (`headers.rs:293-:313`) + worker strike/range-diff accounting (`worker.rs:1894` mock honors "only requested UIDs come back") | D-02 locked this reuse explicitly |
| Per-folder "backfill done" bit | a sidecar JSON state file | DB state column on `mailboxes` (ALTER inside M26; precedent `store/mod.rs:260` `ALTER TABLE mailboxes ADD COLUMN sweeps_since_full INTEGER NOT NULL DEFAULT 0;`) | durable state already lives in the SQLite store; state.json traps are PITFALLS #12-adjacent |

**Key insight:** every capability this phase needs already exists exactly once in the repo — the phase is composition, not invention. The only genuinely new logic is the References byte parser (pure function, ~60-80 LOC with fixtures) and the `threads.rs` wrapper (SQL ↔ crate ↔ UPDATE glue).

## Phase 42 Overview

**Recommended approach:** ship in four dependency-ordered plans — M26 base first, then the wire/parser pair, then compute, then payload. Every seam already exists; the phase is composition plus one new pure parser and one new wrapper module. The wire side adds exactly one FETCH shape (`FETCH_REFS_ATTRS`) and copies one already-on-the-wire field (`Envelope.in_reply_to`, imap-proto-0.16.7/src/types.rs:619, re-verified this session); the store side is three ALTER'd columns + one index; the compute side is one crate call with the ordering fixed by a house re-sort; the payload side is additive fields behind a frozen command name.

**Step-by-step data flow (each step names the file it touches):**
1. **Wire (existing sweep, unchanged bytes):** `sync/worker.rs:1584` `fetch_envelopes(&range)` → `imap/headers.rs:293-:313` `fetch_envelope_batch` issues the frozen `FETCH_ATTRS` (`headers.rs:21`). ENVELOPE bytes already contain In-Reply-To.
2. **Envelope parse:** `imap/headers.rs:233-:285` `parse_fetch_item` (+ `:509` `parse_fetch_attributes` for the CHANGEDSINCE path) copies `env.in_reply_to` via `decode_bytes_opt` (`:103-:109`) into a new `MessageHeader.in_reply_to` — never through `decode_header_words` (`:120`).
3. **New References lane (only new UIDs + one per-folder backfill):** new `imap/headers.rs` fn mirroring `fetch_envelope_batch`, issuing `UID FETCH <range> FETCH_REFS_ATTRS` where `FETCH_REFS_ATTRS = "(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])"` is a NEW sibling constant; bytes extracted via async-imap `Fetch::section` (registry src async-imap-0.11.3/src/types/fetch.rs:180 — returns `Option<&[u8]>`, `None` on shape miss ⇒ must count requested-vs-received UIDs with the strike discipline already at `worker.rs:1606-:1643`).
4. **Parse bytes:** NEW `imap/references.rs` — pure function: unfold CRLF+WSP → tokenize `<...>`/bare → normalize to `<...>` → order-preserving dedupe → cap (100 IDs / 32 KiB, PITFALLS.md:57). Output = JSON array stored verbatim-normalized once at fetch time.
5. **Store:** `store/mod.rs` M26 (`ALTER … ADD COLUMN` only, precedent at `:260`; SCHEMA_VERSION `:19` 25→26 same commit) + `store/queries.rs:491` `UPSERT_MESSAGE_SQL` / `:512` `upsert_message` extended with the two new columns; backfill done-bit is an ALTER on `mailboxes` (same M26).
6. **Recompute:** NEW `store/threads.rs::recompute_threads(conn, mailbox_id)` — one scoped SELECT (`WHERE mailbox_id = ?`, `pending_delete = 0`) → `mail_threading::thread_messages_with(&msgs, &ThreadingOptions{subject_merge:false, ..default})` → sort members `(date_utc, uid)` → one-tx batched UPDATE of `messages.thread_id` (= root's `messages.id`). Called from `sync/worker.rs` Step-5 tail, backfill-complete, and the three Sent-APPEND success paths (`:764-:800`, `:825`, `:873`); early-exit on zero dirty rows.
7. **Payload:** `store/queries.rs:16-:28` `MessageRow` gets additive fields; `list_messages` SQL (`:874-:900`) extended additively, name/signature/order (`:885`, `:886`) frozen; `commands/sync.rs:1103-:1118` command body untouched; NEW `get_thread` command returns header-only member rows (shape pattern: `get_message_by_uid` `:746-:763`); registered `lib.rs:503` `generate_handler!` (list entry anchor `:529`); TS mirrors `src/types.ts:4-:15` extended in the SAME commit with serde round-trip tests.

## Step-back Provisions

What this phase MUST satisfy — planner verifies each item at phase exit:

**Roadmap success criteria (ROADMAP.md:23-:28):**
- SC-1: refresh syncing a reply persists both header fields + `thread_id` shared with its parent in-folder; list payload still renders exactly as today.
- SC-2: lane sends ONLY `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` as a separate named constant; `peek_audit` enumerates it as path 5 **same commit**; no fetch path can set `\Seen`; extended guard tests green; no dev run against live mail before the gate passes.
- SC-3: fresh install AND 25→26 upgrade boot with every prior row preserved (fresh+upgrade fixture pair; SCHEMA_VERSION bumped in same change; stale-version test extended); References parser passes fixture suite (folded-first, `<...>`, deduped, capped); Zimbra-refusal fallback pre-declared; live leg deferred-with-reason.
- SC-4: recompute = per-folder dirty-set epoch at sync-end/backfill/send-file hooks, never per-insert; idempotent (no dup/reshuffle); send-file recompute joins a sent reply into its folder's thread.
- SC-5: `list_messages` unchanged name/signature carries additive `thread_id`, `thread_count`, latest preview/date, participant flags; new `get_thread` returns header-only member summaries (uid/from/date/flags/preview, no bodies); TS+Rust side-by-side with serde round-trip both directions.

**Requirements (REQUIREMENTS.md:25-:28):** THR-01 (extract + persist via lane + M26), THR-02 (JWZ `mail-threading` `subject_merge:false`, epoch hooks, `(date_utc, uid)` tiebreak, never per-insert), THR-03 (additive payload behind frozen `list_messages` + `get_thread`), THR-04 (BODY.PEEK-only, `\Seen` never set, UID-only + UIDVALIDITY gating, per-folder scope, guards extended same phase).

**Locked decisions (CONTEXT.md D-01..D-05):** D-01 automatic once-per-folder backfill at first sync post-26 (per-folder done-bit, reversible); D-02 backfill reuses batch ranges + SyncGate + resizable batches + resumable/idempotent accounting, yields between ranges; D-03 `mail-threading` 0.1.3 `subject_merge:false`, in-house ~250-LOC JWZ fallback only if crate gate fails; D-04 per-folder dirty-set epoch, `(date_utc, uid)` tiebreak, `thread_key = (mailbox_id, thread_id)` contract, NULL = own-thread; D-05 synthesized fixture corpus + live capture deferred-with-reason + capture script in `scripts/live-*` pattern.

**Frozen surface (hard constraints):** `list_messages` name/signature; `MessageView` (`commands/sync.rs:2224-:2239`) untouched; UI/rendering untouched; no SMTP/send work; M26 is ALTER-only with zero `CREATE TABLE` copies of existing tables.

## Integration Points

All anchors re-opened this session (2026-10-20). `store/mod.rs`, `imap/headers.rs`, `store/queries.rs`, `store/schema.sql`, `commands/sync.rs`, `lib.rs`, `sync/worker.rs`, `src/types.ts`, registry sources under `~/.cargo/registry/src/…/` read directly.

| File (anchor) | What changes | Invariant that must hold after |
|---|---|---|
| `src-tauri/src/imap/headers.rs:21` `FETCH_ATTRS` | UNTOUCHED; new `FETCH_REFS_ATTRS` constant directly below it | `fetch_attrs_are_parenthesized` (`:428-:443`, negative tripwire `!FETCH_ATTRS.contains("BODY.PEEK[HEADER")` at `:440`) still passes ⇒ References NOT folded into FETCH_ATTRS |
| `headers.rs:103-:109` `decode_bytes_opt` | reused decoder for msg-id fields | msg-id bytes never routed through `decode_header_words` (`:120-:188`, RFC 2047 pass) |
| `headers.rs:233-:285` `parse_fetch_item` | gains `in_reply_to` copy from `Envelope.in_reply_to` (registry imap-proto-0.16.7/src/types.rs:619 `pub in_reply_to: Option<Cow<'a, [u8]>>,`) | ENVELOPE lane wire bytes unchanged (`FETCH_ATTRS` frozen) |
| `headers.rs:509` `parse_fetch_attributes` (CHANGEDSINCE path) | same `in_reply_to` copy | both parse paths produce identical `MessageHeader` shape |
| `headers.rs:293-:313` `fetch_envelope_batch` | template for new `fetch_refs_batch`; bytes via `Fetch::section` (registry async-imap-0.11.3/src/types/fetch.rs:180, `Option<&[u8]>`) | lane diffs requested-vs-received UIDs (`None`/missing item = shape miss, not silence) |
| `headers.rs:463-:504` `peek_audit` | gains path 5: entry added to `audited_paths` (`:487-:498`), `assert_eq!(audited_paths.len(), 4)` (`:499-:503`) → 5, same commit as `FETCH_REFS_ATTRS` | guard count assert is the tripwire — lane without audit = red build |
| `src-tauri/src/imap/references.rs` | NEW: pure parser + ≥6 named fixture tests (see Concrete Examples) | parser is pure, owns no I/O; single parse point at fetch time |
| `src-tauri/src/store/mod.rs:19` `pub const SCHEMA_VERSION: u32 = 25;` | → 26 | same commit as M26 SQL; `schema_version_is_25_with_m23_llm_settings` (`:888`, `assert_eq!(SCHEMA_VERSION, 25)` `:891`) renamed/extended to 26 so the stale assert cannot survive |
| `store/mod.rs:79-:111` `apply_migrations`, vec `:83-:107` | append `M::up(M26_THREADING_SQL)` after M25 (`:107`) + new SQL const near the M4 precedent (`:260` `ALTER TABLE mailboxes ADD COLUMN sweeps_since_full INTEGER NOT NULL DEFAULT 0;`) | M26 SQL contains ZERO `CREATE TABLE messages`; three nullable ALTERs + `CREATE INDEX idx_messages_thread ON messages(mailbox_id, thread_id)` + mailboxes done-bit ALTER |
| `store/mod.rs:2060-:2072` (truncated-baseline helper) + `:2075` `m23_upgrade_from_v22_preserves_rows_and_seeds_settings` | M26 fixture pair clones this pattern: baseline through M25, pre-seed rows, apply M26, assert rows preserved | upgrade path leaves every pre-existing row byte-identical + new columns NULL |
| `src-tauri/src/store/schema.sql:29-:43` `messages` table | UNTOUCHED (v1 baseline; `message_id` nullable at `:33`, `UNIQUE (mailbox_id, uid)` `:42`) | forward-migration-only: `references_json`/`in_reply_to`/`thread_id` appear ONLY in M26 ALTERs, never here |
| `src-tauri/src/store/queries.rs:1-:5` | invariant comment ("No raw SQL escapes this module") | ALL new SQL (upsert cols, thread SELECT, member queries, epoch UPDATE, done-bit) lives in `store/` |
| `queries.rs:491` `UPSERT_MESSAGE_SQL`, `:512` `upsert_message` | two new columns + params | internal-only growth; commands boundary unchanged |
| `queries.rs:16-:28` `MessageRow` | additive fields (`thread_id`, `thread_count`, latest preview/date, member flags json, `thread_key` exposure) | serde-derive stays (`:15` `#[derive(… Serialize)]`); TS mirror same commit |
| `queries.rs:874-:900` `list_messages` | additive SELECT fields only; `WHERE … m.pending_delete = 0` (`:885`), `ORDER BY m.date_utc DESC, m.uid DESC` (`:886`), LIMIT/OFFSET (`:887`) untouched | member/thread SQL shares the SAME `pending_delete = 0` predicate fragment (PITFALLS:140) |
| `queries.rs:746-:763` `get_message_by_uid` | shape anchor for `get_thread`'s header-only member SELECT (add member fields only) | member rows carry no body fields — reader keeps lazy `fetch_message` |
| `queries.rs:793-:803+` `get_reply_context` / `ReplyContextRow` `:775-:786` | pattern reference (local-cache read, never a server call) | threading headers contract (`message_id` feeds `In-Reply-To`) untouched |
| NEW `src-tauri/src/store/threads.rs` | `recompute_threads` wrapper: scoped load → crate call → `(date_utc, uid)` re-sort → one-tx UPDATE; `thread_key` helpers | crate owns algorithm; wrapper owns scoping/order/idempotency; no SQL outside `store/` |
| `src-tauri/src/sync/worker.rs:1492` Step-5 marker / `:1584` fetch calls / `:1678` upsert loop | References lane for this sweep's new UIDs at Step-5 tail (before Step 6 `:1739`); recompute epoch hook at Step-5 tail (dirty set = new rows ∪ lane rows) | skip recompute when zero new rows (early exit); epoch before `:1751` Step-7 state write |
| `worker.rs:1467-:1473` Step 4d `flush_send_queue_in_pass`; success paths `:764-:800` (mark-sent `:781`), `:825`, `:873` | send-file hook: Sent-folder recompute epoch after each successful APPEND-filed copy (all three success branches, or a shared helper) | sent reply joins its folder's thread WITHOUT touching the send pipeline bytes (PITFALLS:303) |
| `worker.rs:1475-:1490` pending gate + `:1894` mock `fetch_envelopes` (`:1903` "drops them every call") | backfill lane reuses strike/range-diff accounting; mock mirrored for References lane so lib tests see "only requested UIDs come back" | resumable/idempotent (D-02); tombstoned UIDs excluded from backfill (`:1523` precedent) |
| `src-tauri/src/send_queue.rs` (crate ROOT — not `sync/`) `enqueue_send:291` | NOT the Sent-insert site (verified: no Sent upsert here; filing is worker.rs APPEND legs) — do not hunt the hook here | hook belongs to worker.rs flush success branches, keeps queue module byte-stable |
| `src-tauri/src/lib.rs:503` `invoke_handler(tauri::generate_handler![…])`, `list_messages` entry `:529` | register `commands::sync::get_thread` in the same list | command name exactly `get_thread`; Tauri v2 drops unknown args silently — registration missing = feature dead |
| `src-tauri/src/commands/sync.rs:1103-:1118` `list_messages` command | body untouched; return type still `Vec<MessageRow>` (additive struct growth only) | command signature (`mailbox`, `limit`, `offset`) frozen |
| `commands/sync.rs:2224-:2239` `MessageView` | UNTOUCHED | body-path payload stays byte-compatible; `get_thread` is a separate command |
| NEW `get_thread` (in `commands/sync.rs` per existing command layout) | args (mailbox, uid-or-thread_key); returns header-only members | TS+Rust types side-by-side, serde round-trip both directions |
| `src/types.ts:4-:15` `MessageRow` mirror | additive fields, REQUIRED fields non-optional (tsc fails when Rust drops one) | same commit as Rust struct change + one round-trip test per new key set |
| `src/types.ts:39-:42` stale comment ("NOT parsed by the backend today") + `references?` `:43` | comment updated in the SAME commit that lands the lane | no stale doc claims survive into Phase 43+ |

**CORRECTION for the planner:** CONTEXT.md:74 cites `send_queue.rs` — the actual file is `src-tauri/src/send_queue.rs` (crate root) and the Sent-filing/recompute hook belongs to `sync/worker.rs:764-:800`/`:825`/`:873` APPEND-success sites, not to the queue module.

## Common Pitfalls

### The References-parser trap table (the ONLY new parsing in the phase)

| Trap | Input shape (fixture) | Correct behavior | Why / proof |
|---|---|---|---|
| Folded continuations | `<a@x>\r\n <b@y>\r\n <c@z>` (fold at ≤998 cols; long chains ALWAYS fold) | unfold CRLF+WSP → 3 ids | `decode_header_words` does NOT fold — house decoder buffers whitespace then pushes CRLF verbatim (`headers.rs:123` `pending_ws`, push path `:140`); naive `split(' ')` leaves `\r\n` inside ids → join fails (PITFALLS:48) |
| `<…>` framing + bare/case variants | `a@x B@Y <C@Z>` | normalize every token to `<…>`, preserve case inside | 2026-10-18 bracketed-Message-ID Zimbra bug is this class (PITFALLS:53); ENVELOPE `message_id` (`headers.rs:249`) may lack brackets vs `references_json` copy |
| Duplicate ids | `<a@x> <b@y> <a@x>` | order-preserving dedupe → [`<a@x>`,`<b@y>`] | newsletter dup-MessageID policy lives in the crate (PITFALLS:51) — DB must not compound it |
| Chain cap | 150-id chain | keep first 100 / 32 KiB raw, record truncation | adversarial headers blow up recompute cost otherwise (PITFALLS:57) |
| **NEVER through `decode_header_words`** | `<X=?utf-8?Q?=42?=@host>` | pass through VERBATIM | house decoder scans `=?` prefixes (`headers.rs:127`) and REWRITES valid Q-words: `=?utf-8?Q?=42?=` → `B`, producing `<XB@host>` — silent msg-id corruption breaks exact-match joins. Mechanism re-read this session (`:120-:144`); concrete byte output per prior pass probe (42-RESEARCH.md:41). Re-proof probe: unit test asserting `decode_header_words("<X=?utf-8?Q?=42?=@host>")` has length 19 ≠ 11, kept beside parser tests |
| Empty/malformed | `References:` empty or absent on first post | `references_json = NULL`, no synthetic chain | NULL = own-thread sentinel (D-04) — never invent `[]`-chains that join everything |

### `\Seen` tripwire (SC-2)
`BODY[HEADER.FIELDS (REFERENCES)]` (missing `.PEEK`) perma-corrupts unread state server-side in ONE sweep — no client undo (PITFALLS:87-:104). Guards: `peek_audit` path count `4 → 5` SAME commit (`headers.rs:499-:503` assert is the mechanical tripwire); `fetch_attrs_are_parenthesized` gains `FETCH_REFS_ATTRS` assertions; **no dev run against the live mailbox until this gate is green** (PITFALLS:104 "don't run a broken lane once").

### SCHEMA_VERSION same-commit rule (SC-3)
M26 SQL without `SCHEMA_VERSION` 25→26 (`store/mod.rs:19`) = migration never applies in production while fresh in-memory tests stay green (PITFALLS:65-:81, M23 shape). Extend/rename `schema_version_is_25_with_m23_llm_settings` (`:888`/`:891`) — a surviving `assert_eq!(SCHEMA_VERSION, 25)` is the ambush. Zero `CREATE TABLE` copies — M11/M17 rebuild pattern NOT needed for ADD COLUMN (PITFALLS:81).

### thread_key scoping (THR-04 / SC-4)
Bare `thread_id` grouping leaks Sent copies into INBOX rows and collides across folders (root `messages.id` is a global PK, but per-folder scope makes the SAME value head distinct groups — PITFALLS:108-:124). Contract everywhere: `thread_key = (mailbox_id, thread_id)`; compute strictly `WHERE mailbox_id = ?`; NULL `thread_id` = own-thread — byte-compat sentinel test written EARLY (PITFALLS:122).

### Silent-empty-stream on HEADER.FIELDS (Pitfall #12)
Mechanism re-verified this session: async-imap `Fetch::section` (`fetch.rs:180`) returns `Option` — a parse miss yields `None`/fewer stream items, NO error; the exact precedent is the `FETCH_ATTRS` doc comment warning about strict-server BAD → empty stream = silent "0 messages" (`headers.rs:16-:20`). Mitigation: lane accounting diffs requested-vs-received UIDs (reuse strike/range-diff at `worker.rs:1606-:1643` + mock `:1894`/`:1903` discipline), pre-declared fallback `References = In-Reply-To` shallow chain when field absent/unparseable (PITFALLS:259) — the 2026-10-18 convention, already documented at `src/types.ts:40-:42`.

### Both-sides serde payload trap (SC-5)
Tauri v2 silently drops unknown/mis-cased IPC keys — "tests green, feature absent" is the default suspicion (PITFALLS:19-:39, the repair's break #4). Every new payload field lands TS + Rust SAME commit, TS required fields non-optional (`tsc` fails on a Rust-side drop), plus one serde round-trip test per new command key set in each direction. `get_thread` args follow the existing camelCase→snake_case invoke convention (`mailbox`/`uid` style, cf. `list_messages` command args `commands/sync.rs:1105-:1107`).

## Environment Availability

Audited 2026-10-20 (all probes executed this session).

| Dependency | Required by | Available | Version/evidence | Fallback |
|---|---|---|---|---|
| rustc / cargo | M26, parser, compute, guards | ✓ | `rustc --version` → 1.99.0 (2026-09-28); cargo 1.99.0 — ≥ crate MSRV 1.88 (docs.rs, fetched 2026-10-20) | — |
| pinned deps (resolved in `src-tauri/Cargo.lock`) | whole lane | ✓ | async-imap 0.11.3, imap-proto 0.16.7 (transitive), rusqlite 0.37.0, rusqlite_migration 2.3.0, chrono 0.4.45, ammonia 4.2.1, mail-parser 0.11.9 — read via `awk … src-tauri/Cargo.lock` | — |
| `mail-threading` | compute | ✗ (absent: `grep -c mail-threading src-tauri/Cargo.lock` → 0) | to be pinned `mail-threading = "0.1"` → resolves 0.1.3 (crates.io verified prior pass; API re-verified on docs.rs this session) | in-house JWZ core ~250 LOC (D-03 fallback, shared corpus) |
| `cargo test` runners | every offline gate | ✓ | 579 inline `#[test]` fns in `src-tauri/src` (counted 2026-10-20); package `sge` (`src-tauri/Cargo.toml` `[package]`) | — |
| `cargo fmt --check` | VER-01 gate | ✓ | same toolchain | — |
| node + tsc + eslint + prettier | TS mirror gates | ✓ | `node --version` → v26.11.1; `node_modules/.bin/tsc`, `node_modules/.bin/eslint`; `package.json:8` scripts (`build` = `tsc && vite build`, `lint` = `eslint .`, `format:check`) | — |
| TS test runner | round-trip proof | ✗ none exists (`package.json` has no test script) | serde round-trips must live on the Rust side (serde-derive tests) + `tsc` struct-shape check | vitest NOT introduced — zero-new-deps rule (REQUIREMENTS.md:16) |
| `scripts/live-*` capture pattern | deferred live legs | ✓ pattern | `ls scripts/live-*.sh` → live-21.sh, live-22.sh, live-26.sh exist → `scripts/live-42.sh` matches | — |
| IMAP creds (mail.utfpr.edu.br) | live References round-trip, live Zimbra-refusal probe | ✗ | no credentials in this environment (keyring-only per PROJECT.md; project convention REQUIREMENTS.md:19, CONTEXT D-05) | deferred-with-reason + `scripts/live-42.sh` (below) |
| git (grep tripwires) | SC-2/SC-3 claim guards | ✓ | repo is git | — |

**Missing with no fallback:** none — every offline leg is covered.
**Missing with fallback/deferral:** IMAP legs (creds) → deferred-with-reason with named script (Validation Architecture, below).

## Concrete Examples

### References-parser fixture corpus (D-05 — synthesized offline; names are the contract)

| Fixture | Input (as-received lane bytes, verbatim) | Expected parsed output |
|---|---|---|
| `folded_two_line` | `References: <a@utfpr.example>\r\n <b@utfpr.example>\r\n <c@utfpr.example>` | `["<a@utfpr.example>","<b@utfpr.example>","<c@utfpr.example>"]` (unfold FIRST — ids contain no CRLF bytes) |
| `framed_multi` | `References: <r1@x> <r2@x> <m1@x>` | three ids, order-preserving (crate consumes ordered References) |
| `bare_and_case` | `References: a@x B@Y <C@Z>` | `["<a@x>","<B@Y>","<C@Z>"]` — frame-only normalization, case preserved (msg-ids case-significant; RFC 5322 §4.3 exact-match) |
| `malformed_or_empty` | `References:` (absent/empty on a first-post) | `NULL` references_json — no synthetic chain; own-thread sentinel |
| `duplicate_ids` | `References: <a@x> <b@y> <a@x> <b@y>` | `["<a@x>","<b@y>"]` (order-preserving first-occurrence dedupe) |
| `chain_cap_overflow` | 150 well-formed ids | first 100 kept (32 KiB raw guard), truncation recorded in parse result — recompute cost bounded (PITFALLS:57) |
| `encoded_shaped_ids` | `References: <X=?utf-8?Q?=42?=@host> <p@q>` | ids VERBATIM (2047 pass forbidden — contrast probe, see Common Pitfalls row 5) |

Each fixture pairs a thread-compute expectation: `folded_two_line` + stored parent row ⇒ one `Thread` with both member caller-ids sharing one `thread_id`.

### M26 migration fixture pair (M23 precedent at `store/mod.rs:2060-:2072` + `:2075`)
- **Fresh:** `Store::open_in_memory()` through the FULL vec incl. M26 ⇒ `messages` has `in_reply_to`/`references_json`/`thread_id` (NULL-able), `idx_messages_thread` index present, `mailboxes` done-bit = 0, `SCHEMA_VERSION == 26`.
- **Upgrade 25→26:** rebuild baseline DB applying ONLY through M25 via the truncated-vec helper pattern, pre-seed ≥2 message rows + mailbox state, apply M26 ⇒ every prior row byte-identical (same column count/values), new columns NULL, done-bit 0 — mirror `m2_upgrades_v1_database_forward_preserving_rows` (`store/mod.rs:1560`, cited prior pass) + `m23_upgrade_from_v22_preserves_rows_and_seeds_settings` (`:2075`, read this session).
- **Stale test:** old `schema_version_is_25…` (`:888`/`:891`) renamed to 26 with `assert_eq!(SCHEMA_VERSION, 26)` — an un-renamed test is itself a red tripwire.

### Byte-compat test (SC-1/SC-5 sentinel — write EARLY)
Pre-seed a mailbox as today's sweep would (no References lane run), snapshot `list_messages` rows, run M26 + a no-op recompute (dirty set empty), re-query ⇒ every of the 9 original fields per row identical (`store/queries.rs:874-:900` SELECT order untouched); only additive fields differ (thread_id NULL ⇒ own-thread). This test doubles as the Phase 44 byte-compat regression anchor.

## Validation Architecture

### Test framework
| Property | Value |
|---|---|
| Framework | Rust inline `#[test]` — 579 fns, package `sge` (counted 2026-10-20); NO TS test runner exists (`package.json:8`) — TS side gated by `tsc` + `eslint` only |
| Config file | none needed (cargo built-in); TS: `package.json` scripts + `eslint` config |
| Quick run | `cargo fmt --check && cargo test -p sge <filter>` (run from `src-tauri/`) |
| Full suite | `cargo test` (all ~579+ inline tests, offline) |

### Success criteria → offline proof command map
| # | Criterion | Offline proof (exact) |
|---|---|---|
| SC-1 | reply persisted with both fields + shared `thread_id`; list unchanged | `cargo test -p sge thread_pair` (new: `thread_id_shared_with_parent`, `lane_persists_both_fields`) + `cargo test -p sge list_messages` existing suite green + byte-compat test above |
| SC-2 | lane PEEK-only, `peek_audit` path 5 same commit | `cargo test -p sge peek_audit` + `cargo test -p sge fetch_attrs_are_parenthesized` + tripwires: `git grep -n 'BODY\[HEADER.FIELDS' src-tauri/src` ⇒ every hit `.PEEK`-prefixed; `git grep -n FETCH_REFS_ATTRS` ⇒ definition + exactly the lane call sites; `git grep -c 'audited_paths' src-tauri/src/imap/headers.rs` count assert = 5 |
| SC-3 | fresh + upgrade boot, parser fixtures | `cargo test -p sge m26` (fresh + upgrade pair) + `cargo test -p sge schema_version` (renamed 26 test) + `cargo test -p sge references_` (fixture suite, names per Concrete Examples) + `cargo test -p mail-threading --all-features --tests` OPTIONAL corpus run (docs.rs recipe — runs crate's own `testdata/conformance`) |
| SC-4 | epoch hooks, idempotent, send-file join | `cargo test -p sge recompute_` (`recompute_epoch_idempotent`, `recompute_early_exit_zero_rows`, `send_file_join_sent_thread` — mirrors the three worker.rs APPEND-success branches via the lib mock session at `worker.rs:1894`) |
| SC-5 | additive payload + `get_thread` round-trips | `cargo test -p sge get_thread` (serde round-trip BOTH directions per key set) + `cargo test -p sge message_row_` additive shape + `npm run lint && npm run format:check && tsc --check-only src/types.ts` (or `npm run build` gate) — tsc fails if TS mirror misses a Rust field |

No gate binary additions; all proofs are cargo test filters + `git grep` tripwires above.

### Deferred-with-reason (live legs — no creds in this environment)
- **Live UTFPR `HEADER.FIELDS (REFERENCES)` round-trip byte capture** (feed as-received bytes into the fixture corpus; PITFALLS:259-:262 "capture once"): deferred — keyring creds unavailable offline (REQUIREMENTS.md:19, CONTEXT D-05). Closing script: `scripts/live-42.sh` (pattern matches existing `scripts/live-21.sh`/`live-22.sh`/`live-26.sh`), section A.
- **Live Zimbra-refusal fallback detection** (does the server BAD the parenthesized section?): deferred same reason; pre-declared fallback = `References = In-Reply-To` shallow chain (already the coded convention, `src/types.ts:40-:42`). Closing: `scripts/live-42.sh` section B.
Nothing may be claimed as live-achieved this phase (anti-over-claim rule); offline-green + deferred legs is the honest exit state (v1.5 / LIVE-01..03 precedent, CONTEXT.md:72).

### Wave 0 gaps (test files to create in this phase)
- [ ] `imap/references.rs` fixture tests (corpus above) — NEW file
- [ ] `store/mod.rs` M26 fresh + upgrade pair — new tests in existing test block
- [ ] `store/threads.rs` epoch/idempotency/tiebreak tests — NEW file
- [ ] `store/queries.rs` byte-compat sentinel + member-predicate tests
- [ ] `get_thread` serde round-trip tests (Rust side)
- [ ] `peek_audit` path-5 + `FETCH_REFS_ATTRS` guard extensions — MODIFY existing tests, same commit as lane
- [ ] `worker.rs` References-lane mock mirror beside `fetch_envelopes` (`:1894`) + backfill resumability test

## Anti-Patterns: Do NOT

- **Do NOT** group by subject (client-side or server `ORDEREDSUBJECT`) — D-03 locked `subject_merge: false`; subject-only grouping is the explicitly rejected strategy (REQUIREMENTS.md:85).
- **Do NOT** write `BODY[HEADER.FIELDS (REFERENCES)]` (no `.PEEK`) anywhere — one sweep perma-corrupts `\Seen` state server-side, unrecoverable.
- **Do NOT** extend `FETCH_ATTRS` with the References section — separate named constant (`headers.rs:440` tripwire actively fights it; every sweep would pay double wire bytes).
- **Do NOT** recompute threads per-insert / per-upsert — epoch only at the three hook sites; early-exit on zero dirty rows (WAL churn under the single-writer Store mutex, PITFALLS:309-:314).
- **Do NOT** route msg-id bytes through `decode_header_words` / any MIME-decoding step — exact-match joins; parser operates on `decode_bytes_opt` raw string + unfold/tokenize.
- **Do NOT** key grouping on bare `thread_id` anywhere — `thread_key = (mailbox_id, thread_id)` is the contract end-to-end (D-04).
- **Do NOT** place raw SQL outside `store/` — invariant comment at `store/queries.rs:1-:5` + 25-migration history.
- **Do NOT** do SMTP/send work this phase — send milestone reserved for later; the send-file hook READS the filed copy, never touches the send pipeline bytes (`send_queue.rs` stays stable; `queue_send`/`retry_send`/`send_*` commands untouched).
- **Do NOT** touch UI/rendering (MessageList/ReadingPane/MailboxView, CSS, any `.tsx`) — backend-only; Phase 43+ consumes the data; `MessageView` frozen.
- **Do NOT** claim live-server proof — no creds this environment; live legs are deferred-with-reason + `scripts/live-42.sh` until the user runs it.

## Sources

### Claims verified THIS session (2026-10-20), with provenance
- **In-repo anchors** — all `path:line` cites in Integration Points/Common Pitfalls read directly this session: `imap/headers.rs` (:21, :14-:20, :103-:109, :120-:144, :233-:285, :293-:313, :428-:443 incl. `:440` tripwire, :463-:504 incl. `:499-:503` count assert, :509+); `store/mod.rs` (:19, :79-:111 vec :83-:107, :260, :888/:891, :2060-:2072/:2075); `store/schema.sql` (:29-:43); `store/queries.rs` (:1-:5, :16-:28, :491/:512, :746-:763, :775-:803, :874-:900 incl. :885/:886/:887); `commands/sync.rs` (:1103-:1118, :2224-:2239); `lib.rs` (:503, :529); `sync/worker.rs` (:1467-:1473, :1475-:1490, :1492, :1584, :1678, :1739, :1751, :764-:800/:825/:873 APPEND-success, :1894 mock); `send_queue.rs` fn list (crate root — correction); `src/types.ts` (:4-:15, :24-:44, :39-:42, :43); `Cargo.toml` pins + `[package] name = "sge"`; `package.json:8` scripts.
- **Registry sources read this session:** `imap-proto-0.16.7/src/types.rs:619` `pub in_reply_to: Option<Cow<'a, [u8]>>,`; `async-imap-0.11.3/src/types/fetch.rs:180` `pub fn section(&self, path: &SectionPath) -> Option<&[u8]>` with the `AttributeValue::BodySection { section: Some(sp), data: Some(data) }` match — under `~/.cargo/registry/src/index.crates.io-…/`. [VERIFIED: local registry copy]
- **mail-threading 0.1.3 API** — re-fetched docs.rs this session (page dated 18 October 2026, MIT OR Apache-2.0, MSRV 1.88, deps chrono ^0.4 + optional serde): `thread_messages(&[Message]) -> Vec<Thread>` / `thread_messages_with(&[Message], &ThreadingOptions)`; `Message { id, message_id, in_reply_to, references (ordered), date (chrono), subject }` — `id` is a caller-stable **String**, output `Thread.root_message_id`/`Thread.messages` are caller ids (⇒ `threads.rs` maps `messages.id` u64→String and back); knobs `subject_merge`/`prune_phantoms` (default true)/`subject_prefixes`; "Scope"/"Non-goals": crate parses NOTHING raw — caller must supply decoded fields; conformance corpus at `testdata/conformance` + `testdata/rfc5256-coverage.md`, runnable `cargo test -p mail-threading --all-features --tests`. No documented chronological member-order ⇒ `(date_utc, uid)` re-sort in threads.rs stays mandatory. [VERIFIED: docs.rs/mail-threading/0.1.3, fetched 2026-10-20]
- **Toolchain/deps:** rustc/cargo 1.99.0; `src-tauri/Cargo.lock` resolutions (async-imap 0.11.3, imap-proto 0.16.7, rusqlite 0.37.0, rusqlite_migration 2.3.0, chrono 0.4.45, ammonia 4.2.1, mail-parser 0.11.9; `mail-threading` absent); node v26.11.1; 579 `#[test]` fns; `scripts/live-*.sh` existing set — all probed by shell this session.

### Cited from project docs (MEDIUM — planning-doc authority, not re-derived)
- Prior research pass (same file, sections through "Don't Hand-Roll"): crates.io download stats + legitimacy verdict (SUS), the `decode_header_words` concrete probe output, `store/mod.rs:1560` m2-test cite, PITFALLS/STACK verdicts. [CITED: 42-RESEARCH.md prior pass, 2026-10-20]
- `.planning/research/PITFALLS.md` rows cited by line (:19-:39, :43-:61, :65-:81, :87-:104, :119-:124, :148-:162, :259-:262, :290-:303, :309-:314); `.planning/research/ARCHITECTURE.md` flows/hooks (:98-:125); ROADMAP.md:23-:28; REQUIREMENTS.md:25-:28 + :13-:19 constraints; CONTEXT.md D-01..D-05.

### NOT VERIFIED (flag for planner)
- `mail-threading` crates.io download counts + SUS verdict — carried from prior pass, not re-run this session (treat verdict as locked from the audit already in this file).
- Any live-server wire behavior (Zimbra shape, fold widths) — by definition unprovable offline; deferred-with-reason above. Nothing else in the appended sections is unverified.

### Confidence assessment
| Area | Level | Reason |
|---|---|---|
| Standard stack / API surface | HIGH | mail-threading docs.rs re-fetch this session; registry anchors read directly; Cargo.lock probed |
| Integration seams | HIGH | every file/line re-opened this session; one path correction issued (send_queue.rs location) |
| Pitfalls | HIGH | mechanism-level verification of every trap (decoder code read, section() Option, guard asserts); concrete probe bytes from prior same-day pass |
| Live legs | LOW by design → deferred-with-reason | no creds; honest deferral + closing script named |

### Knowledge gaps
- `mail-threading` internal duplicate-Message-ID assignment: output keeps caller ids (docs.rs "Message-ID policy") — but whether two DB rows sharing one `message_id` across FOLDERS affect per-folder recompute is bounded by the scoped SELECT (folder-scoped by construction); no cross-folder join exists. Verify at plan time with a two-folder fixture.
- Fold width Zimbra actually emits for long References (998? server-tuned?) — closes only via `scripts/live-42.sh` capture.

### Usage
This file: `wc -c` after append ≈ 38 KB total (≈24.5 KB prior pass + ≈13.5 KB appended). Stats: 9 sections appended; ~40 distinct `path:line` anchors verified this session; 2 registry files read; 1 external doc fetch (docs.rs, 2026-10-20); date of all probes: 2026-10-20.
