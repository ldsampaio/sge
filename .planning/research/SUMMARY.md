# Project Research Summary

**Project:** SGE — Linux IMAP Desktop Client (v1.1 Triage & Folders)
**Domain:** Desktop IMAP email client — incremental v1.1 (flag sync, multi-folder browsing, poll refresh, UID backfill on existing read-only codebase)
**Researched:** 2026-10-04
**Confidence:** HIGH

## Executive Summary

SGE v1.1 promotes the M1 read-only INBOX viewer into a triage-capable multi-folder client: read/unread flag sync, real server folder tree with per-folder sync, poll + manual refresh, and UID gap backfill with no silent holes. Experts build this class of client the same way Thunderbird does — UID-addressed IMAP operations, per-folder sync state (UIDVALIDITY/UIDNEXT/high-water-mark), optimistic flag writes reconciled against server FLAGS, and poll-based refresh with single-flight guard — deferring IDLE push, CONDSTORE/QRESYNC, and delete/move to later milestones.

The headline finding is that **v1.1 adds zero new dependencies**: async-imap 0.11.3 already exposes `uid_store`/`select`/`status`/`noop`, rusqlite_migration 2.x handles the M2 per-folder migration, and poll is a timer around the existing sync worker. The real work is architectural, not dependency work: promote the IMAP session from per-command throwaway connections to a long-lived manager-owned single session, generalize M1's single-INBOX sync state to one row per folder, parameterize the 7-step worker by mailbox with tiered (Full/Incremental/GapFill) modes, and route every write through a durable outbox. The key risks are flag flap (sync overwriting unacked toggles), cross-folder state corruption from a leftover global cursor, overlapping poll syncs on one connection, infinite backfill on expunged UIDs, and lost offline toggles — each with a known prevention tied to a specific phase.

## Key Findings

### Recommended Stack

v1.1 composes entirely from locked dependencies — no new Rust crates, no new npm packages. `uid_store` with `±FLAGS.SILENT (\Seen)` is the flag-write primitive, `list`/`select`/`status` cover folder discovery and unread counts, `tauri::async_runtime::spawn_blocking` is the poll-loop host if a backend loop is chosen, and the M2 migration generalizes `mailboxes` sync-state rows per folder. See STACK.md for API verification and fallback variants. (Details: `research/STACK.md`)

**Core technologies:**
- async-imap 0.11.3 (locked): UID STORE flag writes, per-folder SELECT/STATUS, UID SEARCH diff — verified signatures, zero new IMAP surface
- rusqlite 0.37 + rusqlite_migration 2.x (locked): M2 migration adds folder columns (delimiter, selectable, subscribed) to existing per-row sync state
- tauri 2.12.1 async_runtime re-export: poll-loop spawning via existing `spawn_blocking` + `block_on` pattern — no direct tokio dep
- React 19.1 + @tauri-apps/api v2 Channel events (locked): poll timer + manual refresh reusing existing `SyncEvent` streaming, no scheduler library

### Expected Features

Thunderbird is the reference model: poll every N minutes + explicit Get Mail button, real LIST folder tree, Seen-on-open, STATUS unseen badges. Backfill (UID SEARCH range-diff vs local rows) is the trust differentiator; bulk triage, \Flagged, and IDLE are v1.1.x stretch, not v1.1. Delete/move, full-offline prefetch, and \Answered/keywords are explicitly v2+. (Details: `research/FEATURES.md`)

**Must have (table stakes):**
- Read/unread toggle with UID STORE \Seen sync (optimistic UI + rollback) + auto-Seen on reader open — headline v1.1
- Folder tree from LIST + open/SELECT any folder with headers-first sync — browsing itself
- Poll timer (default 5–10 min, configurable) + manual refresh sharing one sync path — freshness
- Per-folder message list with STATUS (UNSEEN) badges — primary triage signal
- UID gap backfill inside incremental sync + UIDVALIDITY-mismatch full resync — no silent gaps
- BODY.PEEK audit on all fetch paths — prevents the worst v1.1 regression class

**Should have (competitive):**
- Bulk multi-select mark read/unread (same STORE path, uidset) — v1.1.x stretch once single-message sync is stable
- \Flagged (star) sync (one-line extension of flag command builder) — v1.1.x stretch
- IDLE push for INBOX with poll fallback — post-v1.1, needs connection-lifecycle work

**Defer (v2+):**
- Delete/move with Trash semantics + UIDPLUS/EXPUNGE — separate protocol surface, own milestone
- Full-offline body prefetch per folder — storage/sync-time cost, on-demand stays default
- \Answered / custom keywords, CONDSTORE/QRESYNC fast path — later optimizations

### Architecture Approach

The single biggest move: promote the IMAP session from per-command throwaway to a long-lived `SessionManager`-owned resource through which all sync, flag writes, and body fetches flow via mailbox-scoped leases. Around it: extended `SyncSession` trait (select_mailbox, store_flags, search_range), mailbox-parameterized worker with tiered sync modes, isolated gap-detection module, single-entry poller, M2 folder-cache migration with no new tables, and an app-wide event bus for poller-originated events. See ARCHITECTURE.md for diagrams, patterns, and anti-patterns. (Details: `research/ARCHITECTURE.md`)

**Major components:**
1. SessionManager (NEW, `imap/manager.rs`) — owns the single BoxedSession, serializes all IMAP ops, reconnects, tracks selected mailbox
2. SyncWorker (MODIFIED, `sync/worker.rs`) — `sync_folder(session, mailbox, mode)`; Full once, Incremental + GapFill after
3. Poller (NEW, `sync/poller.rs`) — interval loop calling one `request_sync` entry with no-overlap guard; manual refresh is the same call
4. backfill.rs (NEW) — pure set-diff gap detection (local vs server UID sets), unit-testable without IMAP
5. Store M2 + queries (MIGRATED) — folder-discovery columns on existing `mailboxes` rows; new `upsert_folder`, `folder_uids`, `update_flags` fns
6. Commands + SyncEvent bus (MODIFIED) — NEW `set_seen` (optimistic toggle + reconcile/rollback), NEW `list_folders`, app-wide `emit("sync-event")` with FlagSynced/FolderSynced/PollTick variants

### Critical Pitfalls

Six critical pitfalls researched with RFC grounding (3501/9051, 4549, 4551/7162, 4315, 2177) plus debt, integration, performance, security, and UX catalogs. (Details: `research/PITFALLS.md`)

1. **Flag toggle treated as "just a STORE" without reconcile** — toggle visibly flaps or lands on the wrong message. Avoid: UID-only STORE, FETCH UID+FLAGS together and merge (pending toggles win until ACK), `.SILENT` to suppress FETCH storms, CONDSTORE UNCHANGEDSINCE where advertised.
2. **Single global UIDVALIDITY/UIDNEXT applied to all folders** — wrong wipes or stale recycled-UID matches; renames orphan rows. Avoid: every sync-state field keyed by folder, per-SELECT UIDVALIDITY compare with per-folder purge only, LIST re-queried regularly, never branch on NAMESPACE (imap-proto 0.16 poison).
3. **Overlapping syncs on one connection** — poll fires mid-sync/STORE, responses interleave, duplicates/drops. Avoid: single-flight async Mutex/actor around the session, one shared `request_sync()` entry for timer + manual, never send while IDLE outstanding.
4. **Naive backfill (every gap = missing)** — expunged UIDs re-requested forever, UIDNEXT race skips new arrivals. Avoid: present/expunged/unknown tri-state, tombstone after ~2 empty results, UIDNEXT sampled before AND after, chunked UID FETCH (50–100), never sequence-number FETCH.
5. **Offline toggle lost or double-applied** — no durable outbox. Avoid: SQLite pending-ops table written before optimistic update, ordered replay on reconnect, drop-per-op on gone message, drop-folder-queue on UIDVALIDITY change, pending indicator in UI.
6. **Session expiry treated as fatal / plaintext credential replay** — permanent "disconnected" or keyring violation. Avoid: NOOP health check per tick, full reconnect (keyring re-read, re-SELECT, re-read UIDVALIDITY/UIDNEXT), backoff, "reconnecting…" state.

## Implications for Roadmap

Based on research, suggested phase structure:

### Phase 1: Session + flag sync (with outbox + reconcile)
**Rationale:** Ends the read-only era (headline feature); reconcile logic must land in the same phase as the first STORE or toggles flap from day one.
**Delivers:** Extended `SyncSession` (select_mailbox, store_flags, search_range), `SessionManager` skeleton or full, `set_seen` optimistic toggle with reconcile/rollback, durable pending-ops outbox + replay rules, BODY.PEEK fetch-path audit.
**Addresses:** Read/unread flag sync; optimistic + pending-indicator UX.
**Avoids:** Pitfalls 1 (flag reconcile) and 5 (offline outbox).

### Phase 2: Per-folder state + folder browsing
**Rationale:** Folder tree needs per-folder sync rows before any multi-folder FETCH; M2 migration + discovery unlock the sidebar with zero sync-algorithm changes.
**Delivers:** M2 migration (delimiter/selectable/subscribed columns), `upsert_folder`/`list_folders`, LIST discovery refresh in connect flow, sidebar from local cache, SELECT-any-folder with headers-first sync, LIST-refresh + rename/delete handling.
**Uses:** rusqlite_migration 2.x additive migration; existing `list` probe.
**Implements:** `mailboxes`-as-folder-cache; worker parameterization begins.
**Avoids:** Pitfall 2 (per-folder UIDVALIDITY/state).

### Phase 3: Poll + manual refresh (single entry + reconnect)
**Rationale:** Freshness layer orchestrates Phases 1–2; single-flight and reconnect are acceptance criteria, not polish.
**Delivers:** `request_sync(mailbox, reason)` single entry, poller loop (default 5–10 min, configurable + manual-only), no-overlap guard, real `cancel_sync`, NOOP health check + keyring reconnect + re-SELECT, STATUS unseen-count sweep, app-wide event bus + quiet-reconnecting UX.
**Addresses:** Poll + manual refresh; per-folder badges.
**Avoids:** Pitfalls 3 (single-flight) and 6 (reconnect).

### Phase 4: Tiered sync + UID backfill (with convergence)
**Rationale:** Gap-fill rides on the same UID-range SEARCH as incremental sync; it must not fight the poll loop, so it lands after poll exists.
**Delivers:** Full/Incremental/GapFill mode selection, `backfill.rs` set-diff + chunked ranged FETCH, periodic-Full policy (e.g. every 12th poll), expunged tombstoning, double-poll-zero-FETCH convergence proof.
**Addresses:** UID gap backfill as acceptance property of the sync function (not a separate runtime mode).
**Avoids:** Pitfall 4 (gap semantics + UIDNEXT race).

### Phase Ordering Rationale

- **Dependency order:** per-folder state underpins folders, poll, and backfill (FEATURES/ARCH); flag reconcile + outbox must ship with the first STORE (PITFALLS); poll orchestrates the others so it comes after session + folders; backfill is a property of incremental sync so it comes last. See build-order disagreement below — researchers agree on the components but not on whether flags or folders land first.
- **Architecture grouping:** transport (SessionManager + trait) → state (M2 + folder cache) → algorithm (worker parameterization + backfill) → writes (set_seen) → orchestration (poller + event bus). No new tables; `messages` already keyed `(mailbox_id, uid)`.
- **Pitfall avoidance:** each phase carries its prevention as acceptance criteria — reconcile + outbox with flags; per-folder rows + LIST refresh with folders; single-flight + NOOP/keyring reconnect with poll; tombstones + convergence test with backfill.

### Build-Order Disagreement (recorded, not resolved — roadmapper decides)

The four researchers disagree on whether flags or folders come first. Both positions are recorded here with rationales; the roadmapper picks the winner.

- **Position A — flags-first (PITFALLS researcher):** order flags (with outbox) → folders (with per-folder state) → poll (with single-flight + reconnect) → backfill (with convergence). Rationale: the first STORE is the moment the read-only "server overwrites local" assumption breaks, so reconcile + durable outbox must be designed alongside it, not retrofitted; shipping flags first forces the flag-integrity contract (UID-only addressing, pending-wins merge, playback rules) before multi-folder complexity multiplies the blast radius. Backfill last because it depends on per-folder high-water marks and must not fight the poll loop.
- **Position B — folders-first (FEATURES + ARCHITECTURE researchers):** per-folder sync state is the foundation of all v1.1 work — "do this first, it's the phase-ordering constraint everything else hangs on" (FEATURES dependency notes); ARCH build order runs SessionManager + trait → M2 migration + folder cache (unlocks sidebar with zero sync changes) → worker parameterization + tiered sync → `set_seen` → poller last because it orchestrates the others. Rationale: folders infrastructure (manager + per-folder rows + discovery) unblocks everything including flag writes (which need mailbox-scoped leases and per-folder reconcile state), and the INBOX regression suite stays green throughout by becoming the `mailbox="INBOX"` case.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 1 (flag sync):** UTFPR server STORE dialect — verify `.SILENT` acceptance vs RFC 3501 base syntax fallback, `MODIFIED`/CONDSTORE advertisement via CAPABILITY probe (`/gsd-plan-phase --research-phase` recommended if probe results are unavailable).
- **Phase 2 (folders):** UTFPR LIST hierarchy (delimiter, non-ASCII names, SPECIAL-USE advertisement) — needs live probe or planning-time research; name-heuristic fallback design if SPECIAL-USE is absent.
- **Phase 3 (poll):** standard pattern overall (single-flight + NOOP + keyring reconnect are well documented), but server idle-timeout and throttling behavior needs validation against UTFPR ("leave app open 1 hour" test).

Phases with standard patterns (skip research-phase):
- **Phase 4 (backfill):** UID SEARCH range-diff + tombstoning is textbook RFC 4549; chunked FETCH and convergence test are established — plan directly.
- **Stack/integration:** no new crates, no new IPC shapes — skip dependency research; verify `uid_store` spelling against the live server during execution.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Verified against docs.rs 0.11.3 (Sept 2026) + local Cargo.lock + local source (`sync.rs`, `imap/`, `worker.rs`) |
| Features | HIGH | RFC 3501/9051 + Thunderbird/eM Client/Apple Mail conventions cross-checked; v1.1 scope fixed by PROJECT.md |
| Architecture | HIGH | Grounded in live SGE codebase file-by-file (worker, session, schema, commands, lib.rs); no speculative components |
| Pitfalls | HIGH | RFC-grounded (3501/4549/4551/7162/4315/2177) + Dovecot/Evolution implementation history |

**Overall confidence:** HIGH

### Gaps to Address

- **UTFPR server capabilities unverified:** CONDSTORE/QRESYNC advertisement, `.SILENT` STORE acceptance, SPECIAL-USE/LIST-EXTENDED support, idle timeout + throttling thresholds — handle via a live CAPABILITY/LIST probe at the start of Phase 1/2 planning; design graceful fallbacks (plain SEARCH diff, base-syntax STORE, name heuristics) as defaults.
- **Poll cadence default (60–120 s vs 5–10 min):** researchers differ on the exact default; both agree sub-30 s only as user opt-in — validate during planning against server politeness and UX expectations.
- **Keyring reconnect latency:** re-read-per-reconnect is mandated but unmeasured — confirm no UX jank on slow keyring backends during Phase 3 execution.
- **50k+ folders/messages scaling:** OFFSET pagination and full-row scans are known future bottlenecks with known fixes (keyset pagination, UNSEEN fast path) — explicitly deferred, not v1.1 work.

## Sources

### Primary (HIGH confidence)
- docs.rs async-imap 0.11.3 `Session` API — `store`, `uid_store`, `select`, `status`, `list`, `noop`, `idle` signatures (STACK)
- Local SGE codebase: `sync/worker.rs` (7-step sweep, BATCH_SIZE=200, UIDVALIDITY guard), `imap/mod.rs` + `session.rs` (SyncSession trait, connect_sync, NAMESPACE tripwire), `store/schema.sql` + `store/mod.rs`, `commands/sync.rs`, `lib.rs` (ARCH/STACK)
- `src-tauri/Cargo.lock` — async-imap 0.11.3, tauri 2.12.1, tokio 1.53.1, imap-proto 0.16.7 (STACK)
- RFC 3501 / RFC 9051 §2.3.1 (UID/UIDVALIDITY/UIDNEXT), RFC 4549 §§3–5 (sync + playback recovery), RFC 4551/7162 (CONDSTORE/QRESYNC), RFC 4315 (UIDPLUS), RFC 2177 (IDLE) (PITFALLS/FEATURES)

### Secondary (MEDIUM confidence)
- Thunderbird poll + Get Mail conventions, eM Client/Apple Mail IDLE-with-fallback, MailBee/Limilabs keepalive guidance (FEATURES)
- Dovecot docs + Nylas UIDVALIDITY troubleshooting; Evolution/camel-imapx history (NOOP races, IDLE-cancel races) (PITFALLS)

### Tertiary (LOW confidence)
- None — no v1.1 finding rests on a single low-confidence source; UTFPR-specific server behaviors are flagged as gaps to probe, not assumed.

---
*Research completed: 2026-10-04*
*Ready for roadmap: yes*
