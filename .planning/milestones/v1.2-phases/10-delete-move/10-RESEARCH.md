# Phase 10: Delete + Move — Research

**Date:** 2026-10-06 | **Status:** Ready for planning
**Scope:** DEL-01 (move-to-Trash + undo), DEL-02 (confirmed UID-scoped expunge), MOVE-01 (UID MOVE with COPY+STORE+EXPUNGE fallback)
**Depends on:** Phase 9 (v1.1 complete). Builds on: `SyncSession`/`SessionManager` (Phase 6), per-folder sync (Phase 7), `SyncGate`+poll (Phase 8), tombstone/convergence machinery (Phase 9).

User decisions (binding, from `10-CONTEXT.md`): delete = move-to-Trash + undo until next sync; CREATE `Trash` if missing; expunge modal names folder + count, UID-scoped only, never touch other clients' `\Deleted`; move picker = menu reusing Sidebar tree (no drag-and-drop); flags preserved across move; offline ops queue in `imap_outbox` with flag_outbox-style reconcile; move carries pending flag-toggle op along.
Agent's discretion: Trash detection strategy, undo toast copy/placement.

---

## 1. async-imap 0.11 verb APIs (verified against vendored 0.11.3 source)

No new crate. All verbs exist on `async_imap::Session` (`~/.cargo/.../async-imap-0.11.3/src/client.rs`). Method names below are exact — note `mv`/`uid_mv`, **not** `move`:

| Need | API | Signature / wire form | Notes |
|---|---|---|---|
| Mark deleted | `uid_store` | `uid_store(uid_set: S1, "+FLAGS.SILENT (\\Deleted)") -> Stream<Fetch>` | Same shape as existing `set_seen` (`imap/mod.rs:435`). **Must drain stream to completion** — dropped stream aborts the write (existing comment `mod.rs:441-443`). Canonical args: `+FLAGS.SILENT (\Deleted)` / `-FLAGS.SILENT (\Deleted)` for undo-of-mark. |
| Expunge all `\Deleted` | `expunge` | `expunge() -> Stream<Seq>` | **Never use as default.** Nukes every `\Deleted` in the selected folder including other clients' flags. Only acceptable as UIDPLUS-absent fallback with the unmark-others dance (see §2). |
| Scoped expunge | `uid_expunge` | `uid_expunge("1,2,3") -> Stream<Uid>` | RFC 4315, requires `UIDPLUS` capability. Only `\Deleted` UIDs **in the set** are removed. This is the DEL-02 workhorse. Drain stream to completion like STORE. |
| Copy (fallback leg 1) | `uid_copy` | `uid_copy(uid_set, dest_mailbox) -> Result<()>` | Runs `UID COPY <set> <quoted dest>`. Destination quoting via internal `validate_str` (CR/LF rejected, mailbox quoted — `client.rs: ~893`). Pass **raw wire names** (modified-UTF-7, from `MailboxInfo.name`, never `display_name`). |
| Move (preferred) | `uid_mv` | `uid_mv(uid_set, dest_mailbox) -> Result<()>` | Runs `UID MOVE <set> <quoted dest>` (RFC 6851 §3.3). Requires `MOVE` capability. Semantics: COPY + `\Deleted` + expunge as **one server action** — no intermediate `\Deleted` visible, flags + INTERNALDATE preserved per RFC. May partially succeed per-message; each message ends in ≥1 mailbox (never lost), generally not both. |
| Capability check | `capabilities` | `capabilities() -> Capabilities`; test with `.has(&Capability::Atom("MOVE"/"UIDPLUS"))` | Vendored test at `client.rs:1662-1680` shows the pattern. Call on the leased session before choosing MOVE-vs-fallback / UID EXPUNGE-vs-fallback. Cache per connection (capability set is stable per session; re-query after reconnect). |
| CREATE | `create` | `create("Trash") -> Result<()>` | Needed only for the Trash-CREATE fallback (per CONTEXT decision). Phase 11 owns general CREATE; Phase 10 needs exactly one call site. Wire name quoting同样 via `validate_str`. |
| NOT needed | `append`, `rename`, `delete`, `close`, `store` (seq) | — | APPEND belongs to Phase 12/13; rename/delete-mailbox to Phase 11. Never use sequence-number `store`/`copy`/`mv` — UID-only everywhere (T-6-01 invariant). `CLOSE` is an expunge variant that also deselects — avoid; explicit verbs are auditable. |

UID-set formatting: comma-joined `"1,2,3"` (same as sweep `range_str` in `worker.rs:380-384`). Chunk multi-message ops to ~200 UIDs per call (matches `BATCH_SIZE`, keeps poll responsive).

### Capability fallback matrix (plan as explicit branches + tests)

- **MOVE advertised →** `SELECT src` → `uid_mv(set, dest)` — done in one verb. Hold the manager lease across the whole op (see §3).
- **No MOVE →** `SELECT src` → `uid_copy(set, dest)` → `uid_store(set, +FLAGS.SILENT (\Deleted))` → `uid_expunge(set)` if UIDPLUS else plain-`expunge` fallback dance: read which UIDs in folder carry `\Deleted` that are NOT ours, `-FLAGS.SILENT (\Deleted)` them temporarily, `expunge()`, then restore their `\Deleted`. Prefer failing loudly over silent plain-expunge when the unmark dance can't be verified — plain `EXPUNGE` is the one op in this phase that can destroy another client's mail.
- **No UIDPLUS →** same dance for DEL-02 path (`STORE \Deleted` + scoped removal). If neither UIDPLUS nor safe-unmark is possible, DEL-02 must refuse with a plain-language error, not expunge blindly.

Server probe (open question for live gate): whether `mail.utfpr.edu.br` advertises `MOVE` / `UIDPLUS` / `SPECIAL-USE` is **still unverified** (STATE.md blocker). First live gate in Phase 10: `CAPABILITY` + `LIST` attribute dump. Plan must include a MockSession arm for each capability combination (MOVE+UIDPLUS / neither / mixed).

### NAMESPACE / parser tripwire (do NOT regress)

`imap-proto 0.16` cannot parse NAMESPACE responses and a parse failure **permanently poisons the session's read side** (`session.rs:386-394`, tripwire test `namespace_response_is_unparseable`). Same hazard class applies to untagged `COPYUID`/`APPENDUID`/`MOVE` extended responses — `uid_copy`/`uid_mv` return `Result<()>` via `run_command_and_check_ok` (no response parsing), so they are safe, but if anyone adds response-code parsing for UID mapping later it needs the same tripwire-test treatment. Rule for planners: **new verbs must not parse untagged extended responses** without a replay-stream regression test.

---

## 2. `SyncSession` trait + `BoxedSession` impl pattern (copy `set_seen`)

Trait lives `imap/mod.rs:276-322`; impl `mod.rs:364-507`.

New trait methods (suggested names; keep UID-only, object-safe `PinBox` returns):

```rust
fn store_deleted(&mut self, uid: u32, deleted: bool) -> PinBox<'_, Result<(), SyncError>>;
fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>>;          // Seq numbers returned; drain only
fn uid_expunge(&mut self, uid_set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>>;
fn uid_copy_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>>;
fn uid_move_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>>;
fn capabilities(&mut self) -> PinBox<'_, Result<Capabilities, SyncError>>;  // or return Vec<String>
fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>>; // Trash fallback only
```

Impl template = `set_seen` verbatim: build owned strings, `Box::pin(async move)`, map errors as `SyncError::Protocol("<VERB> …: {e}")`, **drain every returned stream with `try_collect`** (`try_next` loop for fetch-style). `expunge`/`uid_expunge` parse-streams return expunged seq/uid numbers — collect and drop (local cache reconciles via next SEARCH, not via expunge responses; never trust seq numbers as UIDs).

`name_attribute_to_string` (`mod.rs:346-362`) already maps `\Trash`/`\Sent`/`\Junk`/etc. — Trash auto-detect reads these (see §5).

`MockSession` (`sync/worker.rs:623-715`) must grow matching arms in lockstep: recorded-call vecs (`deleted_calls`, `copied_calls`, `moved_calls`, `expunged_sets`), `fail_*` toggles, and a canned `capabilities` set. Every fallback branch gets a mock combination — this is where Phase 10's unit coverage lives.

---

## 3. `SessionManager` lease pattern (copy `set_seen_in`)

Manager: `imap/manager.rs:27-151`. The `set_seen_in` template (`manager.rs:111-128`):

1. `lease_for(mailbox)` (connects lazily, re-SELECTs when folder differs),
2. verb attempt,
3. on failure: `drop(lease)` → `reconnect()` → fresh `lease_for(mailbox)` → retry **exactly once**.

New lease-scoped methods:

- `mark_deleted_in(mailbox, uid, deleted)` — same shape as `set_seen_in`.
- `uid_expunge_in(mailbox, uid_set)` / `expunge_in(mailbox)`.
- `copy_in(src, uid_set, dest)` and `move_message_in(src, uid_set, dest, has_move: bool)` for the fallback branch.
- `create_trash()` — CREATE `Trash` (raw name; mutf7-encode only if non-ASCII, `Trash` is pure ASCII).

**Critical lease rule (from CONTEXT specifics + SUMMARY §5):** a MOVE-with-fallback sequence issues `SELECT src` then COPY/STORE/EXPUNGE assuming the selection is stable. Because `MailboxLease` holds the manager mutex for its lifetime, the correct implementation keeps **one lease held across the whole fallback sequence** — but `lease_for(dest)` mid-sequence would try to re-SELECT and break the source selection. So manager internals need a low-level path: `lease_for(src)` once, then drive `lease.session().uid_copy_to/…` directly without re-leasing. Plan this as: public `move_message_in` holds one `lease_for(src)` guard and calls session verbs on `lease.session()`; it must **never** call `self.lease_for()` re-entrantly (async mutex → deadlock). Same for `uid_expunge_in` (SELECT-then-EXPUNGE under one lease). Add a deadlock-regression test (second `lease_for` while one is held must not be awaited inside the method).

Capability caching: query `capabilities()` once per fresh connection, store alongside `selected_mailbox` in `ManagerState`. Invalidate on `reconnect()`.

---

## 4. Precondition: route `start_sync` under manager leases FIRST

`commands/sync.rs:131` currently calls `connect_sync(&account_cfg)` — a **fresh session per sync pass**, bypassing `SessionManager`. With EXPUNGE/MOVE in play this is a correctness hazard: a sweep SELECTs folder A on its private connection while a lease EXPUNGEs folder B — plain-EXPUNGE blast radius becomes unreasoned-about. SUMMARY §5 and roadmap ("Backend hardening ships inside" Phase 10) both require: **`start_sync` routes through `manager_for().lease_for(mailbox)` before any destructive op ships.**

Concretely: `start_sync` already has `manager_for` helper (`sync.rs:79-93`, currently only used by `set_seen`/`list_mailboxes`). The sync worker takes `Box<dyn SyncSession>` — a `MailboxLease` derefs to `&mut BoxedSession`, so the lease can be boxed/reborrowed as the session, OR the worker gains a lease-driven entry. Minimum viable: obtain `lease = manager.lease_for(&mailbox)` in the command, pass `lease.session()` as `&mut dyn SyncSession` into a refactored `sync_with_session`. Watch the `spawn_blocking` + `block_on` discipline (sync.rs:117-118): lease futures are `Send`-safe under `block_on` on the dedicated thread — same as `set_seen` does today. Gate: Phase 6-9 test suites green + a single-flight contention test (lease + sync serialize, no second connection — assert via `connect_sync` call count or manager session identity).

Do NOT add a second IMAP connection for moves (roadmap hard constraint). Chunk multi-message moves (~200 UIDs) so poll stays responsive.

---

## 5. `imap_outbox`: new durable table, `flag_outbox` untouched

SUMMARY §4 decision is explicit and roadmap-reinforced: **do NOT add `op`/`dest` columns to `flag_outbox`** (Phase 6 contract + tests). New M7 migration:

```sql
CREATE TABLE imap_outbox (
  id            INTEGER PRIMARY KEY,
  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,
  op            TEXT NOT NULL,          -- 'delete' | 'move'
  dest_mailbox  TEXT,                   -- target raw wire name for 'move', NULL for 'delete'
  uid_validity  INTEGER NOT NULL,
  created_at    TEXT NOT NULL DEFAULT (datetime('now')),
  attempts      INTEGER NOT NULL DEFAULT 0,
  last_error    TEXT,
  UNIQUE (mailbox_id, uid)
);
CREATE INDEX idx_imap_outbox_mailbox ON imap_outbox(mailbox_id);
```

- `SCHEMA_VERSION` 6 → 7; forward-only migration with preserve-rows test (copy the `m2_upgrades_v1_database_forward_preserving_rows` pattern in `store/mod.rs:470-511`).
- `UNIQUE(mailbox_id,uid)` latest-wins, same as flag_outbox.
- **Enqueueing a delete/move drops the same-key `flag_outbox` row** — a flag write to a soon-dead message is moot (SUMMARY §4). Implement as same-lock transaction in the command: `delete_outbox_op` + `enqueue_imap_outbox`.
- **Move carries the pending flag-toggle along** (CONTEXT decision): when a message has a queued `flag_outbox` op and the user moves it, the move op must capture the intended Seen state and apply it at the destination after the move (destination UID is new — server assigns it; without MOVE's COPYUID parsing we can't know it, so: after move, SEARCH dest for the moved Message-ID, or simply let next sync reconcile flags and record the Seen intent as a `flag_outbox` op on the matched dest UID). Simplest correct plan: on move-enqueue, read pending Seen value, delete flag op, store `seen` intent in `imap_outbox` row (add nullable `seen` column or encode in `op`); replay applies `set_seen` on the destination UID resolved by Message-ID SEARCH. If resolution fails, drop with log (next sync converges).
- Replay order in worker: **`imap_outbox` BEFORE `flag_outbox` BEFORE header sweep** (SUMMARY §5) so locally-deleted mail isn't resurrected by the same-pass sweep. Current post-sync replay (`worker.rs:579-586`) becomes pre-sweep replay; keep the post-sync position too for flag ops or move both pre-sweep — planner decides, but pre-sweep `imap_outbox` is non-negotiable.
- Replay rules mirror `replay_outbox` (`worker.rs:91-164`): epoch check first (any op with `uid_validity != current` → drop whole mailbox queue), absent-UID drop via fresh `live_uids` set, per-op failure → `attempts`+`last_error`, stays queued. Reuse `ReplaySummary { acked, dropped, failed }` — possibly extended with `moved` count (see §7).
- Convergence `pending` gate (`worker.rs:293-301`, `pending_uids`) must extend to `imap_outbox` depth: `skip_sweep` requires both queues empty, else a queued delete looks "converged" and never replays. `send_queue` stays independent (Phase 13).

Optimistic local state: delete = remove row locally immediately (or a `pending_delete` hidden flag per CONTEXT "optimistic local delete + `pending_delete` hidden state" — hidden flag is safer for undo: row stays with `pending_delete=1`, filtered from list queries, cleared on ack, restored on undo). Add column or separate set — planner's call, but list/search queries (`list_messages`, `fts_search`) must filter it, and undo = clear flag + drop `imap_outbox` row (valid only while op still queued, i.e. "until next sync"). Move = optimistic delete-from-src + insert placeholder in dest (or just delete-from-src; dest appears on next sync — but undo needs the src row, so keep tombstone info in the outbox row: subject/from/preview snapshot is overkill; the src row with `pending_delete` + `dest_mailbox` recorded suffices for reverse-MOVE).

---

## 6. Trash auto-detect (agent's discretion — recommended strategy)

Layers, in order (SUMMARY §3 + CONTEXT specifics):

1. **SPECIAL-USE `\Trash`** from LIST attributes (`name_attribute_to_string` already maps `\Trash`; `MailboxInfo.attributes` carries it). Requires server to advertise SPECIAL-USE (RFC 6154) — unverified on UTFPR, check in live CAPABILITY/LIST probe.
2. **Name match** (case-insensitive, on wire or display name): `Trash`, `Deleted`, `Deleted Items`, `Lixeira` (PT — UTFPR is a Brazilian university, webmail likely PT), `Papelera`, Gmail-style `[Gmail]/Trash` / `[Gmail]/Lixeira`. Probe live LIST output first; hardcode the observed name as primary with the rest as fallback list.
3. **CREATE `Trash`** once, behind user confirmation (CONTEXT decision says "via CREATE na hora" — plan the confirm dialog; silent CREATE on a server with a differently-named Trash yields two trash folders).

Cache resolved Trash identity per account (mailboxes table role column arrives in Phase 11 — Phase 10 can use a simple pref/local constant + re-detect on LIST refresh; do NOT invent the roles schema early, keep Phase 11's surface clean).

Move picker excludes `\Noselect` (hierarchy placeholders — `sync.rs:412-417` already skips them), INBOX-as-dest when src is INBOX is allowed (no-op guard), and Sent (per SUMMARY §3; also exclude Trash-as-dest for delete-path which targets Trash anyway — moving *out of* Trash = restore, must be supported as the undo-adjacent path).

---

## 7. UIDVALIDITY-gated deletes + FTS/attachment cleanup + SyncSummary

- **UIDVALIDITY gating:** every `imap_outbox` op stores epoch at enqueue (from `get_sync_state`, same as `set_seen` command `sync.rs:200-203`). Replay drops on mismatch (whole-mailbox, before any STORE — `worker.rs:108-119` pattern). `delete_missing_uids` wipe path (`worker.rs:227-235`) must also `drop_imap_outbox_for_mailbox`. Never replay a stale UID against a renumbered mailbox — wrong-message deletion is the catastrophic case.
- **FTS cleanup:** automatic — `msg_ad` trigger (`schema.sql:73-76`) deletes FTS rows on `messages` delete. No planner action except a test asserting FTS no longer returns the expunged message.
- **Attachment cleanup:** manual — files live under `<app_data>/attachments/<uid_validity>/<uid>/` (`store/mod.rs:189-193`), never in SQLite. On confirmed expunge (local row deleted via `delete_missing_uids` or explicit delete), remove the corresponding dir. Needs `uid_validity` + `uid` at delete time — capture before row deletion. Best-effort with log (never fail the sync on an fs error). Same for `message_bodies`/`attachment_parts` rows — those cascade via FK (`ON DELETE CASCADE`), verify cascade is enabled (SQLite FK pragma — check `PRAGMA foreign_keys`; if off, explicit deletes needed. Open item for planner to verify).
- **SyncSummary counters** (`sync/mod.rs:38-57`): add `moved: usize` and `expunged: usize` (or reuse `deleted`?). Recommendation: keep `deleted` = server-side disappearances (existing meaning, expunge-diff step 6), add `expunged` = locally-initiated permanent deletes acked this pass, `moved` = imap_outbox move ops acked. `SyncEvent::BatchCompleted` and frontend `SyncStatus` surfaces update accordingly. `total()` semantics unchanged.

---

## 8. Frontend patterns to reuse (Tauri commands + MessageList/Sidebar)

- **Command shape:** `set_seen` (`sync.rs:179-276`) is the exact template for `delete_message(uid, mailbox?)` / `move_message(uid, dest, mailbox?)` / `expunge_messages(uids, mailbox)`: `Option<String> mailbox` defaulting to INBOX (established Phase 7 pattern), `load_account_config` → `manager_for` → `spawn_blocking`+`block_on`, store lock never across `.await`, optimistic write + enqueue under one lock, immediate verb attempt, ack→dequeue / fail→`record_*_error` + stays queued. Return result structs with `acked/pending_count/detail` for toast + pending wash.
- **Optimistic UI + pending wash + rollback:** `MessageList.tsx:151-222` — instant flip, accent-soft wash (`pendingUids` set) until ack, rollback only on invoke reject. Delete/move reuse this: hiding optimistically, wash on pending, rollback restores row. Undo toast (~5–10 s, until next sync) calls reverse op (move back / clear pending_delete + drop outbox row).
- **Move destination menu:** reuse `Sidebar.tsx` tree (`buildTree`, delimiter nesting `Sidebar.tsx:46-92`, system-vs-custom ranking). Picker excludes `\Noselect`, INBOX==src, Sent. No drag-and-drop per CONTEXT.
- **Expunge modal:** explicit confirm naming folder + count ("Apagar para sempre? X mensagens" + folder name). Only ever sends the selected UIDs (`uid_expunge` set), never bare `expunge()`.
- **Poll loop:** Phase 8 poll + manual refresh already loop over all folders via `requestSync` — imap_outbox replay rides the existing loop (pre-sweep position), no new scheduler. `sync_status.pending_count` must sum **both** outbox depths for the pending indicator.

---

## 9. Test + live-gate plan shape (for the planner)

Unit (MockSession, no network) — mirror existing worker tests:
- `store_deleted` UID-addressing record test (T-6-01 style: UID reaches wire, no seq).
- Drain-to-completion: mock STORE stream must be fully consumed (existing tests assert call counts; add drop-behavior coverage if feasible).
- Fallback matrix: MOVE+UIDPLUS / neither / mixed caps → assert verb sequence (`moved_calls` vs `copied+calls+expunged`).
- Lease-hold test: fallback COPY+STORE+EXPUNGE issues no intermediate SELECT (record `select_mailbox` calls in mock).
- No-reentrancy: `move_message_in` never awaits a second lease while holding one.
- RFC 4549 for `imap_outbox`: epoch-bump drops whole queue; absent UID drops single op; failure stays queued with attempts (copy the three `outbox_rfc4549_*` tests).
- Flag-op dropped on delete/move enqueue (same-key `flag_outbox` row gone).
- `pending_delete` rows filtered from `list_messages`/`fts_search`; undo restores.
- Migration M7: v6→v7 preserves rows (copy `m2_…_preserving_rows` pattern); each migration gets preserve-rows coverage (roadmap gate).
- Multi-client survival: UIDs with `\Deleted` set by "another client" (mock: pre-marked, not in our set) survive our `uid_expunge`; plain-expunge fallback performs the unmark dance (assert `-FLAGS` calls for foreign UIDs + restore).
- Trash detect: attribute `\Trash` wins; name fallback list; CREATE path calls `create("Trash")` once.

Live gates (at least one per roadmap hard constraint; UTFPR `mail.utfpr.edu.br:993/SSL`):
1. CAPABILITY + LIST dump (MOVE? UIDPLUS? SPECIAL-USE? Trash name? delimiter?) — settles all open probes; do this first, it may simplify the plan.
2. Move round-trip on a `SGE-TEST-*` folder: move lands in dest with `\Seen` preserved, no body re-FETCH (assert fetch-call count), undo reverses.
3. Offline-delete → reconnect → server expunged; locally-deleted never resurrected same-pass.
4. Expunge scoping: foreign `\Deleted` in same folder survives our UID EXPUNGE.

## 10. Open probes (settle in discuss/early plan)

- UTFPR caps: MOVE / UIDPLUS / SPECIAL-USE / CONDSTORE; Trash folder name; hierarchy delimiter; idle timeout (STATE.md blocker, unchanged).
- `PRAGMA foreign_keys` state — cascade vs explicit deletes for bodies/attachment_parts.
- Whether `Capabilities` type is exposable through the `SyncSession` trait object-safely (vendored `Capabilities` is a concrete struct — likely fine as return-by-value in `PinBox`).
- Undo window anchor: CONTEXT says "até o próximo sync (outbox pendente como âncora)" — precise rule (undo valid iff op still in `imap_outbox`) is already decided; toast copy/placement is the remaining discretion.

## 11. Suggested plan split (planner owns final shape)

1. **Transport:** `SyncSession` verbs + `BoxedSession` impls + `MockSession` arms + capability helper + quoting tests. No store/commands. Gate: new verb unit tests green, Phase 6-9 suites green.
2. **Manager + precondition:** `start_sync` under `manager_for().lease_for()` + lease-scoped delete/move/expunge methods + reconnect-retry + single-flight contention test. Gate: full backend suite + no-second-connection assertion.
3. **Store + replay:** M7 `imap_outbox` migration + queries + pre-sweep replay order + convergence-gate extension + FTS/attachment cleanup + `SyncSummary` counters. Gate: RFC-4549-parity tests + migration preserve-rows + cleanup tests.
4. **Commands + UI:** `delete/move/expunge` Tauri commands (optimistic + undo) + Trash auto-detect + move picker (Sidebar reuse) + expunge modal + pending/undo toast + poll integration. Gate: live round-trips (move/expunge/offline-replay/foreign-`\Deleted` survival).

## Sources

- `src-tauri/src/imap/mod.rs` (trait + `set_seen` impl), `manager.rs` (lease template), `session.rs` (NAMESPACE tripwire, `connect_sync`), `sync/worker.rs` (`replay_outbox`, sweep, `MockSession`), `sync/mod.rs` (`SyncSummary`, `SyncGate`), `commands/sync.rs` (`start_sync` fresh-session hazard, `set_seen` command template, `list_mailboxes` NoSelect skip), `store/mod.rs` (migrations M2–M6), `store/schema.sql` (FTS triggers, FK cascades), `store/queries.rs` (outbox helpers, `delete_missing_uids`, `set_local_seen`).
- `src/components/MessageList.tsx` (optimistic toggle + pending wash), `Sidebar.tsx` (folder tree).
- Vendored `async-imap 0.11.3` `src/client.rs` (`uid_store`, `expunge`, `uid_expunge`, `uid_copy`, `mv`/`uid_mv`, `capabilities`, `validate_str` quoting).
- `.planning/research/SUMMARY.md` §§2–5, `.planning/ROADMAP.md` Phase 10, `10-CONTEXT.md` decisions.
