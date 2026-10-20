# Project Research Summary

**Project:** SGE v1.9 — Email Threading + Interface Revamp
**Domain:** Desktop IMAP mail client (Rust + Tauri v2 + React 19 + SQLite) — backend References-threading lane + UI primitives/state-discipline revamp on an already-shipping three-pane client
**Researched:** 2026-10-20
**Confidence:** HIGH ( Stack & Architecture fully verified against source; Features MEDIUM; Pitfalls HIGH backend / MEDIUM UI)

## Executive Summary

v1.9 is a **two-lane milestone** on an existing, disciplined codebase. Lane 1 is a pure-backend threading lane: an M26 SQLite migration (`in_reply_to`, `references_json`, `thread_id`), a new `HEADER.FIELDS (REFERENCES)` IMAP fetch lane, and per-folder JWZ/RFC-5256 threading computed by the `mail-threading` 0.1.3 crate — the only justified new Rust dependency, chosen precisely because hand-rolled JWZ fails on duplicate Message-IDs and phantom containers. Lane 2 is a backend-independent UI foundation: vendored OFL woff2 fonts (Baloo 2 + Nunito), additive state tokens, and a small hand-rolled primitives kit (`Button`, `Dialog`, Empty/Loading/Error) — deliberately no new UI dependencies, per the scope guard. The two lanes collide in exactly one file: the message-list row. The research verdict is firm — **extract and write `MessageListRow.tsx` once, in the UI-foundation phase, before any grouping feature touches it**, so threading and revamp never re-rewrite each other's work.

The recommended build order is: backend lane first (shippable in isolation — flat list payload unchanged), UI foundation in parallel, then grouped conversation rows (payload extension, not a second row rewrite), then the conversation reader stack, then a consumption-only revamp sweep. All existing frozen contracts (`list_messages` signature, `fetch_message` BODY.PEEK per-message, `queue_send`) are preserved via additive payload fields and one new `get_thread` command.

The top risks are all precedented in this repo, not hypothetical: Tauri silently dropping unmatched invoke/payload fields (the 2026-10-18 four-breaks), a `\Seen`-setting copy-paste in the new fetch lane (peek_audit has only 4 enumerated paths today — a 5th must be added in the same commit as the lane), an M26 SCHEMA_VERSION off-by-one (the M23 race), folder-unsafe `thread_id` keys, and the "stub compiles" over-claim trap (Phase 38) that is HIGH-risk again because v1.9 is entirely UI-driven. Every one has a mechanical counter-gate identified below; none requires new research — they require plan-time embedding of the gates and live-server verification during execution.

## Key Findings

### Recommended Stack

**Keep everything.** The v1.9 pins stay exactly as-is: async-imap 0.11.3 + imap-proto 0.16.7 (verdict: do NOT take 0.12.0 — it pulls imap-proto 0.17 + Rust 2024 edition for zero v1.9 benefit, and async-imap/imap-proto lockstep upgrades have poisoned sessions before), mail-parser 0.11.9, rusqlite 0.37.0 + rusqlite_migration 2.3.0, tauri 2.12.1, React 19.3.x. Two additions total:

- **`mail-threading` 0.1.3** (crates.io, MIT/Apache-2.0, chrono-only dep we already carry) — JWZ/RFC-5256 threading with a shipped JSON conformance corpus. API: caller input `{id, message_id, in_reply_to, references, date, subject}` → thread roots + member lists. Configure `subject_merge: false` — subject-based grouping was explicitly rejected as unreliable. Fallback if vetoed at plan time: ~250 LOC in-house container algorithm + absorbed fixtures (zero migration cost either way).
- **Vendored woff2 fonts** (2 variable files, latin subset, OFL.txt each, no npm dep) — `tokens.css` already names Baloo 2/Nunito but no files ship. CSP verified: no `font-src` exists, but `default-src 'self'` already covers bundled fonts; add `font-src: 'self'` defensively.

**Verified integration points:** `imap-proto 0.16.7` `Envelope.in_reply_to` already arrives free in the existing FETCH_ATTRS response bytes (~saves a lane); `Fetch::section()` + the exact `BodySection` parser shape for `HEADER.FIELDS` verified in crate source. New References lane must be a **separate named constant** (`REFERENCES_ATTRS`, parenthesized, `.PEEK`) so the `fetch_attrs_are_parenthesized` tripwire and `peek_audit` guards keep their semantics.

### Expected Features

**Must have (table stakes, P1):**
- Backend thread lane (M26 + References fetch + backend `thread_id`) — prerequisite for everything, explicitly approved backend References lane
- Grouped list rows: count badge (≥2 only), aggregate unread, deduped senders "A · B +2", latest-date/snippet, `Re:`-trim — single-message rows stay **byte-compatible** with today's list (no badge, no layout shift)
- Conversation reader: chronological stack (oldest→newest), per-message collapse/expand, expansion default = latest + through first-unread, read-on-expand per message (never whole-thread-on-open), per-message Reply/Reply-All/mark-unread re-homed to cards
- Conversation batch actions: delete/move thread to Trash, mark-all-read — reuse `ExpungeModal` confirm pattern with member-count copy
- Token + primitive foundation (fonts, Button/Dialog/Empty/Loading/Error, named interaction states, `:focus-visible`-only rings)
- Listbox semantics fix (NO nested interactive elements in options) + keyboard roving (Arrows/Home/End/Enter-open) — ordered BEFORE grouped rows ship

**Should have (P2/P3 differentiators):**
- Auto-scroll to first-unread card (respect `prefers-reduced-motion`) — P3
- Lazy iframe mount with cap per expanded card; "Mostrar N anteriores" long-thread fold — P3, but the iframe cap logic ships WITH the reader
- "N não lidas" chip, avatar cluster, participants line, plain-text quoted-trimmer (fragile — plain text only) — P3
- Type-ahead on the list — P3, defer unless trivial

**Defer (explicitly out of scope):**
- Dark theme, star/\Flagged, virtualization libs, Radix/shadcn/UI kits, multi-select toolbar, in-place list expansion, client-side subject grouping fallback, whole-thread-as-one-prompt LLM generation, legacy-panel rewrites

**Key anti-features to hold the line on:** all-bodies-on-open (fetches N × BODY.PEEK + N iframes + N server \Seen writes), batch-undo single toast without an approved backend contract, batch server transaction (IMAP has none — N real STORE calls is correct).

### Architecture Approach

Three-layer split preserved; two parallel lanes meeting only in `MessageList.tsx` row markup. Backend adds: `imap/references.rs` (strict `<...>`-framing parser — msg-ids are NOT RFC 2047; never route them through `decode_header_words`), `store/threads.rs` (thin mail-threading glue; ALL SQL stays in `queries.rs` per the 25-migration invariant), one command `get_thread` (headers-only member summaries), and the `list_messages` command body routing to `list_threads` behind an unchanged name/signature. Frontend adds `components/ui/*`, `MessageListRow.tsx` (written once), `ConversationReader.tsx` + `MessageCard.tsx` (lazy iframe), and a selection state of `{thread_key, open_uid}`.

**Major components:**
1. **Threading backend lane** (`imap/references.rs`, `store/threads.rs`, M26, sync-worker hooks) — thread membership as a per-folder epoch recompute (sync-end / backfill / send-file; never per-insert; dirty-set + early-exit on 0 new rows), `thread_key = (mailbox_id, thread_id)` as the universal identity, NULL thread_id = own-thread byte-compat row
2. **UI foundation lane** (`fonts.css`, state tokens, `ui-primitives.css`, `components/ui/*`) — zero backend dependency, runs fully in parallel
3. **Conversation UI** (`MessageListRow`, `ConversationReader`, `MessageCard`, `MailboxView` routing) — consumes both lanes; reader bodies still come from the frozen per-message `fetch_message`, one card at a time

### Critical Pitfalls

1. **Tauri silently drops unmatched invoke args/payload fields** (camel/snake drift; repair-precedent) — define TS + Rust types side-by-side in one file, same-commit both sides, round-trip serde test per new command; Phase 45 UAT re-verifies the wire live, never unit-only
2. **`\Seen` corruption in the new fetch lane** — one copy-pasted `BODY[...]` (no `.PEEK`) permanently read-marks a real mailbox with no undo; prevents via separate `REFERENCES_ATTRS` constant + `peek_audit` path-5 extension in the SAME commit, and no dev run against live mail before that gate is green (recovery cost HIGH and trust-breaking; prevention nearly free)
3. **References-chain parse holes** — fold continuations FIRST (RFC 5322 §2.2.3), normalize to `<...>` framing, dedupe order-preservingly, cap chain size; fixture suite (folded, bare-token, dup ID, empty-refs, never-synced parent) + live "as-received" bytes captured once into the corpus; Zimbra refusal fallback pre-declared (In-Reply-To-only, documented Known Gap)
4. **M26 SCHEMA_VERSION off-by-one (M23 precedent)** — bump 25→26 in the same commit as the migration; two-path fixture pair (fresh install + upgrade); zero `CREATE TABLE` copies of existing tables; extend the stale SCHEMA_VERSION test so it screams
5. **Folder-isolation / event-bus / pagination keys** — `thread_key = (mailbox_id, thread_id)` everywhere (list key, reader key, batch identity, event listener); member queries share the `pending_delete=0` predicate fragment so counts can't include tombstoned rows; FLAG_UPDATE listeners re-derive aggregates from member flags (never patch a derived thread row); page stays message-space with grouping-within-page decided up front ("cross-page thread continuation shows as a second row")
6. **"A stub compiles" over-claim (Phase 38 precedent)** — v1.9 is UI-driven so every phase embeds acceptance-gated exit criteria verbatim from FEATURES.md's MVP gates at plan time (live References pair → one row; WebKitGTK keyboard + focus-restore checks; live payload round-trips), not "tests green"

## Implications for Roadmap

Based on research, suggested phase structure (v1.9 numbering starts at 42; exact numbering is the roadmap's call):

### Phase 42: Threading Backend Lane
**Rationale:** every display feature depends on `thread_id` + `references_json`; this phase is pure backend with an unchanged flat list payload — shippable and verifiable in isolation with zero UI risk. PITFALLS.md explicitly assigns pitfalls 1–5, 7, 12 to this phase.
**Delivers:** M26 migration (schema 25→26, preserve-rows tests), `references.rs` parser + fixture suite, `in_reply_to` ENVELOPE extraction (free wire bytes), `REFERENCES_ATTRS` fetch lane + `peek_audit` path-5 (same commit), `store/threads.rs` per-folder recompute at sync-end/backfill/send-file hooks, live-server round-trip gate with captured as-received bytes.
**Addresses:** backend thread lane (P1); threading lane only.
**Avoids:** pitfalls 2, 3, 4, 5 (scope/ordering halves), 7, 12.

### Phase 43: UI Foundation (parallel lane — can run concurrently with 42)
**Rationale:** the revamp has ZERO backend dependency (FEATURES.md dependency note); starting it now means it never blocks on M26. Critically, this phase **extracts `MessageListRow.tsx` in its pre-thread form** — the canonical anti-collision move: the row markup is written exactly once here, visually byte-identical, so Phase 44 is a payload/placement change inside it, not a second rewrite.
**Delivers:** vendored woff2 + `fonts.css` + `font-src: 'self'` CSP add, additive state tokens, `ui-primitives.css`, Button/Dialog (with focus-trap + focus-restore proof tests)/Empty/Loading/Error, listbox semantics fix (no nested interactives) pre-grouping, keyboard roving + `:focus-visible` in the SAME task, `MessageListRow` extraction.
**Addresses:** token/primitive foundation (P1), listbox + keyboard (P2), fonts.
**Avoids:** pitfalls 9 (a11y), 10 (fonts/CSP half), 6 (dual-rewrite row).

### Phase 44: Conversation List UI
**Rationale:** requires Lane 1 merged (thread_id in store) and Lane 2's extracted row. Visual work is placement inside the already-existing `MessageListRow`; `list_messages` keeps its frozen name/signature while routing to `list_threads`.
**Delivers:** extended payload (`thread_id`, `thread_count`, `thread_unread`, latest-*/member flags), `get_thread` command (headers-only, registered in `lib.rs`), grouped rows (badge ≥2, agg unread, senders "+N", latest date/snippet), search stays message-keyed and renders identically, single-message byte-compat regression gate, conversation batch actions (delete/move/mark-read) with pending_delete-filtered members and per-member undo honesty (no single-toast claim without an approved batch-undo contract), pagination-within-page grouping decision documented.
**Addresses:** grouped rows (P1), batch actions (P2), batch-undo scope confirmation (P2 — must be confirmed at plan review).
**Avoids:** pitfalls 1 (payload contracts), 5 (thread_key), 6 (outbox interplay), 8 (pagination).

### Phase 45: Conversation Reader
**Rationale:** depends on Phase 44's `get_thread` + types; the reader is the expansion surface (list never expands in place). Consumes frozen `fetch_message` per expanded card only.
**Delivers:** `ConversationReader` chronological stack + expansion state machine (latest + through-first-unread expanded, history collapsed), `MessageCard` with lazy iframe + concurrent-mount cap, read-on-expand per message (set_seen + flag wash preserved), per-card action rows reusing existing action order, `ReadingPane` single-message path = reader with ONE card, `MailboxView` `{thread_key, open_uid}` selection.
**Addresses:** reader stack + per-message re-homed actions (P1), iframe lazy-mount (P3 but the cap rides here).
**Avoids:** pitfalls 10 (iframe budget), 11 (event-bus aggregation), 4 (whole-thread-on-open anti-feature).

### Phase 46: Revamp Sweep + Live Verification
**Rationale:** retroactive consistency pass on surfaces 43–45 touched; this phase must consume Phase 43's kit, never introduce new primitives.
**Delivers:** state-token consistency across touched surfaces, `prefers-reduced-motion` for auto-scroll/wash, empty/loading/error states consumed everywhere touched, WebGL/WebKitGTK keyboard + focus-restore + screen-reader spot checks, live UAT gates assembled verbatim from FEATURES.md "threading done" MVP gate, Tauri wire round-trip re-verification on a real account, Inbox+Sent cross-folder thread isolation check.
**Addresses:** revamp sweep; the anti-over-claim gate (pitfall 13).
**Avoids:** pitfalls 9 (spot-check), 13 (acceptance-gated exits).

### Phase Ordering Rationale

- **Backend before thread UI** (42 → 44/45): no display feature can ship without `thread_id` — the dependency is total, not partial.
- **UI foundation unconstrained** (43 parallel with 42): zero backend dependency, and it front-loads the one file collision (`MessageListRow`) so threading never competes with the revamp on the same lines.
- **List before reader** (44 → 45): the reader consumes `get_thread`/types from 44; the list's payload contract is what the reader's member summaries must agree with.
- **Sweep verified LAST and consumption-only** (46): standardizes consumption rather than re-inventing states, and assembles phase-exit criteria from the MVP gates so the Phase 38 over-claim class is mechanically blocked.
- **Same-commit invariants:** peek_audit path-5 lands with the lane (42); SCHEMA_VERSION bump with M26 (42); TS+Rust payload sides in one commit (44/45); keyboard roving paired with `:focus-visible` (43).

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 42 (Threading backend):** research is unusually complete (API, parser shapes, integration lines verified) — `/gsd-plan-phase` research-phase likely unnecessary, BUT the phase must embed a **live UTPR/Zimbra gate** (real `HEADER.FIELDS (REFERENCES)` round-trip with captured bytes) as an execution concern, plus plan-review confirmation of two open scope items: the thread-summary payload shape for the reader (new payload beyond `thread_id` — FEATURES.md flags it for explicit approval) and whether batch-undo gets a backend contract this milestone or ships N-toasts documented.
- **Phase 44 (Conversation list):** confirm the `list_threads` SQL shape and pagination-within-page semantics at plan review — FEATURES/PITFALLS both flag the COUNT/OFFSET contract disagreement as an up-front decision, not a discovered one.

Phases with standard patterns (skip research-phase):
- **Phase 43 (UI foundation):** vendored-woff2 + plain-CSS primitives + APG listbox contract are fully specified with verified current-tree integration points; no further research needed.
- **Phase 45 (Reader):** patterns verified against existing `fetch_message`/ReadingPane source; components map one-to-one to documented integration points.
- **Phase 46 (Revamp sweep):** consumption-only polish; gates defined by existing artifacts.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Versions verified against crates.io/npm registry APIs; async-imap/imap-proto parser + accessor source pulled and inspected; `mail-threading` API from README + conformance corpus; integration points verified against SGE source |
| Features | MEDIUM | Gmail Help Center + W3C APG fetched live; Gecko/TB/Geary/K-9 claims from stable knowledge (live KB fetches blocked in this environment); existing-code audit first-hand (HIGH) |
| Architecture | HIGH | Every integration point verified by reading actual source files; dep choices cross-verified; build order explicitly dependency-safe with collision pair resolved |
| Pitfalls | HIGH (backend) / MEDIUM (UI) | All backend pitfalls anchored to concrete repo precedents (M23 race, NAMESPACE, Phase 38, 2026-10-18 four-breaks); UI pitfalls code-shape-verified + APG-fetched |

**Overall confidence:** HIGH — for an existing-codebase milestone, integration points are unusually well-verified (all four researchers read the live tree independently and agree).

### Gaps to Address

- **Live server wire-shape variance:** `HEADER.FIELDS (REFERENCES)` is parser-verified but never live-verified against UTFPr Zimbra; Phase 42's first live gate must capture as-received bytes; pre-declared fallback (References = In-Reply-To) documented as Known Gap if Zimbra refuses.
- **Batch undo backend contract:** `undo_queued_op(uid)` is single-member; conversation batch undo is a backend change flagged for explicit plan-review approval — otherwise ship N labeled toasts and document, never a fake single toast.
- **Thread-summary payload shape:** reader needs per-member headers (uid/from/date/flags/preview) from a new `get_thread` payload — within the lane's spirit but must be confirmed at plan review.
- **Recompute cost on first big-folder pass:** conformance fixtures bound the algorithm, real UTFPR folder sizes unmeasured; dirty-set guard + early-exit mitigations identified; measure at Phase 42 gates.
- **`mail-threading` version floor:** 0.1.3 verified today; if plan time vetoes the dep, in-house JIT (~250 LOC + absorbed fixtures) is pre-costed — decision is a plan-review nod, not new research.

## Sources

### Primary (HIGH confidence)
- crates.io API + npm registry (versions verified 2026-10-20): async-imap, imap-proto, mail-threading, rusqlite, tauri, react, vite, typescript
- async-imap 0.11.3 / imap-proto 0.16.7 crate source (inspected directly): parser body mapping, `test_header_fields` fixture, `Fetch::section()` accessor, `Envelope.in_reply_to`
- `mail-threading` 0.1.3 GitHub README + conformance corpus: API, ThreadingOptions, dup policy, phantom pruning
- SGE source tree (first-hand reads): `store/mod.rs`, `store/queries.rs`, `imap/headers.rs`, `sync/worker.rs`, `commands/sync.rs`, `lib.rs`, `MessageList.tsx`, `ReadingPane.tsx`, `MailboxView.tsx`, `types.ts`, `tauri.conf.json`, `tokens.css`
- Tauri v2 docs (fetched live 2026-10-20): CSP directive fallback, `font-src` semantics

### Secondary (MEDIUM confidence)
- Gmail Help Center "Group emails into conversations" (fetched live 2026-10-20)
- W3C ARIA APG Listbox pattern (fetched live 2026-10-20)
- Thunderbird/Geary/K-9 behavior from stable training knowledge (live KB fetches blocked)
- jwz threading doc (canonical knowledge; direct fetch timed out)
- `.planning/PROJECT.md` Key Decisions + v1.8 closeout / deferred-UAT records

### Tertiary (LOW confidence)
- Geary handbook specifics (406s) — chronology + collapsed-history contract from stable knowledge
- live Zimbra response-shape variance ("as-received" bytes) — to be captured at Phase 42 live gates

---
*Research completed: 2026-10-20*
*Ready for roadmap: yes*
