# Architecture Research: v1.1 Triage & Folders

**Domain:** IMAP desktop client — flag sync, multi-folder sync, background poll, UID backfill
**Researched:** 2026-10-04
**Confidence:** HIGH (grounded in the live SGE codebase: `sync/worker.rs`, `imap/session.rs`, `imap/mod.rs`, `store/schema.sql`, `commands/sync.rs`, `lib.rs`)

## Standard Architecture

### System Overview — v1.1 Target

```
┌─────────────────────────────────────────────────────────────────┐
│                      Frontend (React)                            │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐  │
│  │ FolderSidebar│  │ MessageList  │  │ Reader (Seen toggle,   │  │
│  │ (LIST cache) │  │ (per-folder, │  │ poll badge, refresh btn)│  │
│  │              │  │ OFFSET pages)│  │                        │  │
│  └──────┬───────┘  └──────┬───────┘  └───────────┬────────────┘  │
│         │  Tauri IPC (commands + app-wide sync-event bus)       │
├─────────┴──────────────────┴──────────────────────┴─────────────┤
│                   Backend (Rust / Tauri v2)                      │
│  ┌──────────────────┐  ┌──────────────┐  ┌───────────────────┐   │
│  │ SessionManager   │  │ SyncWorker   │  │ Poller            │   │
│  │ (owns THE single │──│ (mailbox-    │──│ (interval loop →  │   │
│  │  BoxedSession,   │  │  parameter-  │  │  request_sync,    │   │
│  │  serializes ops) │  │  ized passes)│  │  no-overlap guard)│   │
│  └────────┬─────────┘  └──────┬───────┘  └───────────────────┘   │
│           │  SyncSession trait (extended: select_mailbox,       │
│           │  store_flags, search_range)                         │
├───────────┴──────────────────┴─────────────────────────────────┤
│                   Store (SQLite, WAL)                            │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────┐   │
│  │ mailboxes    │  │ messages     │  │ message_bodies /      │   │
│  │ (+folder cols│  │ (unchanged   │  │ attachment_parts      │   │
│  │  via M2)     │  │  shape)      │  │ (unchanged)           │   │
│  └──────────────┘  └──────────────┘  └───────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

The single biggest architectural move of v1.1: **promote the IMAP session from
per-command throwaway to a long-lived, manager-owned resource.** Today
`start_sync`, `fetch_message`, and `save_attachment` each call `connect_sync`
independently (three LOGIN/LOGOUT round-trips per user flow). The v1.1 contract
— one session owned by the sync engine, through which all flag writes flow —
requires a new `SessionManager` component. Everything else (folders, poll,
backfill) hangs off it.

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| `SessionManager` (NEW, `imap/manager.rs`) | Owns the single `BoxedSession`; serializes every IMAP op (sync, flag write, body fetch); reconnects transparently; tracks currently-selected mailbox | `Mutex<Option<BoxedSession>>` + `Mutex<String>` selected-mailbox + `with_session(mailbox, op)` helper; held in `AppState` |
| `SyncSession` trait (EXTENDED) | Add `select_mailbox(name)`, `store_flags(uid, add/remove \Seen)`, `search_range(set)` to the existing read-only trait | New methods on `BoxedSession` impl + `MockSession` test fixture |
| `SyncWorker` (MODIFIED, `sync/worker.rs`) | Take a `mailbox: &str` parameter; support incremental (`UID last_next:*`) + gap-fill passes alongside the existing full sweep | `sync_folder(session, mailbox, cb)`; keep 7-step algorithm, generalize Steps 2/4/7 per folder |
| `Poller` (NEW, `sync/poller.rs`) | Interval loop that calls the same `request_sync` entry as manual refresh; never overlaps a running pass; emits over the app-wide event bus | `async_std` task + `AtomicBool` running-guard + `AtomicBool` cancel; interval from frontend pref (default 5 min) |
| `mailboxes` table (MIGRATED, M2) | Becomes the folder-list cache: one row per discovered folder with per-folder `uid_validity/uid_next/last_sync_at` (already per-row — no redesign needed) | New columns: `delimiter TEXT`, `selectable INTEGER DEFAULT 1`, `subscribed INTEGER DEFAULT 1`; `last_sync_at` already exists |
| Tauri commands (MODIFIED + NEW) | `set_seen(uid, mailbox, seen)` (NEW); `list_folders` (NEW, reads cache); `start_sync`/`fetch_message`/`save_attachment` rerouted through `SessionManager`; `cancel_sync` wired to a real flag | Commands become thin: resolve mailbox → `manager.with_session(...)` → query/event |
| `SyncEvent` bus (MODIFIED) | Per-invocation `Channel<SyncEvent>` cannot serve a background poller; move to app-wide `app.emit("sync-event", …)` + new variants | New variants: `FlagSynced{mailbox,uid,seen}`, `FolderSynced{mailbox,summary}`, `PollTick{…}`; keep existing sweep variants unchanged |

## Recommended Project Structure

```
src-tauri/src/
├── imap/
│   ├── mod.rs            # SyncSession trait EXTENDED (select_mailbox, store_flags, search_range)
│   ├── session.rs        # connect_sync stays (manager uses it for reconnect); probe untouched
│   └── manager.rs        # NEW — SessionManager: single-session owner + with_session helper
├── sync/
│   ├── mod.rs            # SyncEvent EXTENDED (FlagSynced, FolderSynced, PollTick)
│   ├── worker.rs         # MODIFIED — sync_folder(session, mailbox, mode, cb)
│   ├── backfill.rs       # NEW — gap detection (local vs server UID sets) + range builder
│   └── poller.rs         # NEW — interval loop, no-overlap guard, cancel flag
├── store/
│   ├── schema.sql        # M2 migration appended (folder columns; messages untouched)
│   └── queries.rs        # NEW fns: upsert_folder, list_folders, folder_uids, update_flags
├── commands/
│   └── sync.rs           # MODIFIED — route via manager; NEW set_seen, list_folders, refresh_now
└── lib.rs                # AppState += SessionManager handle; connect_account signature UNCHANGED
```

### Structure Rationale

- **`imap/manager.rs`:** session ownership is a transport concern, not a sync-algorithm
  concern — it lives next to `connect_sync`, which becomes the manager's private
  reconnect primitive rather than a per-command public call.
- **`sync/backfill.rs`:** gap detection is pure set logic over `Vec<u32>` (local UIDs
  vs `UID SEARCH` result); isolating it makes it unit-testable without any IMAP
  fixture, unlike the worker.
- **`sync/poller.rs`:** the poll loop owns timing/cancellation only — it must not
  contain sync logic, or manual-refresh and poll will drift into two code paths.
  Both call one `request_sync(mailbox)` entry.
- **No new tables.** `mailboxes` already stores per-folder sync state
  (`uid_validity`, `uid_next`, `last_sync_at`); v1.1 only adds folder-discovery
  columns. `messages` is already keyed `(mailbox_id, uid)` — multi-folder safe
  as-is. A separate `sync_state` or `folders` table would duplicate what exists.

## Architectural Patterns

### Pattern 1: Single-Session Owner with Mailbox-Scoped Leases

**What:** One component (`SessionManager`) holds the only `BoxedSession`.
Every operation runs inside `with_session(mailbox, |session| …)`, which
(1) locks, (2) reconnects if the session died, (3) `SELECT`s the mailbox only
if it differs from the last-selected one, (4) runs the op, (5) keeps the
session open. A `generation: u64` counter invalidates stale state after
reconnect.

**When to use:** Every v1.1 IMAP touch — sync passes, `STORE` flag writes,
body fetches. No exceptions; that is the contract.

**Trade-offs:** Serializes all IMAP traffic (a flag write waits for a running
sweep — correct for IMAP, which is a single-selected-mailbox protocol anyway);
adds one mutex hop to every command. The alternative (per-command connections)
breaks server-side `RECENT`/cond-store coherence and multiplies LOGIN load
against university servers that rate-limit.

**Example:**
```rust
// All v1.1 commands funnel through this — never connect_sync directly.
manager.with_session("Sent", |session| async move {
    session.store_flags(uid, FlagOp::AddSeen).await
}).await?;
// Sync reuses the same lease; SELECT is skipped when already on "Sent".
manager.with_session("INBOX", |session| async move {
    worker.sync_folder(session, "INBOX", SyncMode::Incremental, cb).await
}).await?;
```

### Pattern 2: Optimistic Flag Toggle with Server Reconciliation

**What:** `set_seen` writes the local `messages.flags` JSON immediately,
returns to the UI, then issues `UID STORE <uid> ±FLAGS \Seen` through the
manager. On success, re-fetch that UID's `FLAGS` and reconcile; on failure,
roll the local row back and emit `FlagSynced{…ok:false}` so the UI flips back.

**When to use:** Read/unread toggle — the highest-frequency write in a triage
milestone; blocking the UI on an IMAP round-trip feels broken.

**Trade-offs:** Brief window where local and server disagree (acceptable for
`\Seen`; never use optimistic writes for expunge-worthy state). Requires the
flags column to stay the single source of truth for `is_unread` — it already is.

**Example:**
```rust
// 1. local write (fast, UI updates)
queries::update_flags(conn, mailbox_id, uid, add_seen(seen))?;
// 2. server write through the single session
manager.with_session(mailbox, |s| s.store_flags(uid, seen.into())).await
    // 3a. ok → re-fetch FLAGS, reconcile row
    // 3b. err → restore previous flags JSON, emit failure event
```

### Pattern 3: Tiered Sync — Full Sweep Once, Incremental + Gap-Fill After

**What:** Three sync modes selected by `SyncWorker` from stored state, not by
the caller:
- `Full` (first sync ever, or UIDVALIDITY bump): existing 7-step sweep — `UID
  SEARCH ALL` + 200-UID `FETCH` batches + expunge diff. Unchanged code, now
  parameterized by mailbox.
- `Incremental` (steady state): `UID SEARCH <stored_uid_next>:*`, fetch only
  the delta, update `uid_next`. No `SEARCH ALL`, no expunge diff (cheap poll).
- `GapFill` (backfill): compare local UID set vs `UID SEARCH ALL` result;
  any server UID missing locally below `max(local)` is a gap (missed between
  syncs — e.g. app was asleep); fetch exactly those ranges in 200-UID batches.
  Runs piggybacked after every `Incremental` pass, and standalone on demand.

**When to use:** `Incremental` is the poll-loop workhorse; `GapFill` is what
makes "no silent gaps" true without paying a full sweep per poll.

**Trade-offs:** `Incremental` alone can miss expunges of old mail (server
deleted UID 5 while we only asked about `900:*`); therefore run a bounded
`Full` (or at least `SEARCH ALL` + expunge diff without re-fetching known
UIDs) every N polls (suggested: every 12th poll / hourly). UIDNEXT-only sync
without gap detection is the classic silent-gap bug — the `GapFill` step exists
specifically to forbid that shortcut.

**Example:**
```rust
pub enum SyncMode { Full, Incremental, GapFill }
let mode = match stored_state {
    None => SyncMode::Full,                              // never synced
    Some((uidv, _)) if uidv != server.uid_validity => SyncMode::Full, // bump → wipe
    Some((_, next)) if server.uid_next.unwrap_or(next) == next => SyncMode::GapFill, // nothing new, check gaps
    _ => SyncMode::Incremental,                          // normal delta
};
```

### Pattern 4: One Sync Entry, Two Triggers (Poll = Scheduled Manual Refresh)

**What:** A single `request_sync(mailbox, reason)` function owns the
no-overlap guard (`AtomicBool compare_exchange`), drives worker → manager →
events. The poller calls it on a timer; the refresh button / `refresh_now`
command calls it on demand with `reason: Manual`. A manual refresh during a
running poll pass is a no-op that re-emits current progress (never a second
connection, never a queue).

**When to use:** Always — poll and manual refresh must be the same code path
from day one.

**Trade-offs:** Manual refresh may appear to "do nothing" if a poll pass just
started; mitigate by emitting `PollTick{started_at}` so the UI shows "syncing…"
instead of silence. Debounce manual clicks (2 s) at the command layer.

## Data Flow

### Request Flow — Flag Toggle (new write path)

```
[Reader: click "mark unread"]
    ↓
[set_seen command] → [local flags JSON update] → [UI flips instantly]
    ↓
[SessionManager.with_session(mailbox)] → [SELECT if needed] → [UID STORE uid -FLAGS \Seen]
    ↓                                                        ↓
[re-fetch FLAGS → reconcile row] ← [OK]            [ERR → rollback row + FlagSynced{ok:false}]
    ↓
[app.emit("sync-event", FlagSynced)] → [React updates list badge]
```

### Request Flow — Poll Refresh (new background path)

```
[Poller timer fires] ──or── [refresh_now command]
    ↓
[request_sync(INBOX, reason)] → [guard: running? → emit PollTick, return]
    ↓ (lease acquired)
[SessionManager.with_session(INBOX)] → [SELECT INBOX (cached, usually skipped)]
    ↓
[SyncWorker: Incremental pass] → [GapFill pass] → [set_sync_state + last_sync_at]
    ↓
[app.emit FolderSynced + MessageSynced…] → [React list re-queries SQLite, OFFSET pages unchanged]
```

### State Management

```
SQLite (source of truth for lists) ←writes── SyncWorker / set_seen (via Store Mutex, short scopes)
    ↓ (read, offline-first)
Tauri commands (list_messages / search_messages / sync_status — UNCHANGED signatures, +mailbox param flows through)
    ↓ (app.emit sync-event bus — NEW app-wide, replaces per-call Channel for poller-originated events)
React (OFFSET pagination + folder selector + unread badges; per-call Channel kept for start_sync progress)
```

### Key Data Flows

1. **Folder discovery:** `LIST "" *` (already proven in the probe drive) →
   `upsert_folder` rows in `mailboxes` (selectable/delimiter cached) →
   `list_folders` command serves the sidebar offline. Refresh discovery on every
   `connect_account` and on manual refresh; cheap and keeps renames visible.
2. **Per-folder sync:** sidebar select → `request_sync(folder)` → worker's
   generalized 7-step pass with that folder's stored `(uid_validity, uid_next)`.
   INBOX behavior is byte-identical to today; other folders reuse the same code.
3. **UID backfill:** after each incremental pass, `backfill::missing_ranges
   (local_uids, server_uids)` → fetch exactly the gaps → upsert. Standalone
   `backfill_folder` runs inside `request_sync`, not as a separate command
   (avoids two callers racing over the same UID ranges).

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| Personal UTFPR mailbox (v1.1 target) | Monolith as-is: single session, 200-UID batches, OFFSET pagination. No change. |
| 50k+ messages per folder | `SEARCH ALL` + full `existing_uids` per batch gets slow: switch expunge-diff to chunked `HashSet` diff (already chunked fetches; keep) and consider `UID SEARCH UNSEEN` fast-badge path for unread counts instead of full-row scans. |
| Many folders (10+) × short poll | Round-robin: poll INBOX every interval, other folders every kth tick (stale-ok). Never open parallel sessions to parallelize folders — serialize through the manager; IMAP servers throttle concurrent LOGINS per user. |

### Scaling Priorities

1. **First bottleneck:** full-sweep cost per poll. Fixed by tiered sync
   (`Incremental` + periodic `Full`) — this is why Pattern 3 is load-bearing,
   not optional.
2. **Second bottleneck:** `list_messages` OFFSET over huge folders (OFFSET
   rescans). Defer: keyset pagination (`WHERE uid < ? ORDER BY uid DESC`) is
   the known fix when OFFSET visibly lags; do not pay for it in v1.1.

## Anti-Patterns

### Anti-Pattern 1: Per-Command Connections for Writes

**What people do:** `set_seen` calls `connect_sync` itself for a quick
STORE + LOGOUT, like `fetch_message` does today.
**Why it's wrong:** Breaks the single-session contract; races with a running
sweep (STORE on connection A while connection B holds SELECT can silently
apply to a stale mailbox view); doubles LOGIN load; defeats the poller's
SELECT cache.
**Do this instead:** Every IMAP touch goes through
`SessionManager.with_session`. Migrate `fetch_message`/`save_attachment` to
the manager in the same phase as `set_seen` — three call sites, one rule.

### Anti-Pattern 2: Holding the Store Mutex Across `.await`

**What people do:** Lock `Arc<Mutex<Store>>`, then `STORE`/`FETCH` over the
network while holding it.
**Why it's wrong:** The worker's documented invariant ("mutex only during
synchronous DB ops, never across `.await`") exists because the UI reader
thread needs the same mutex — a slow IMAP op under lock freezes the whole UI.
**Do this instead:** Snapshot what the network needs (uid, mailbox, prev
flags) under a short lock, drop the guard, do I/O, re-lock to write results.
The existing worker steps already model this; copy the shape.

### Anti-Pattern 3: SELECT-per-Message and STORE-without-SELECT-check

**What people do:** `SELECT` the mailbox fresh inside every flag write "to be
safe", or `STORE` assuming the session is still on the right mailbox.
**Why it's wrong:** The first adds a round-trip to the highest-frequency op;
the second applies flags to the wrong folder after a folder switch (IMAP STORE
is selection-scoped — a stale SELECT silently corrupts another folder's flags).
**Do this instead:** Manager tracks `selected_mailbox`; `with_session` compares
and SELECTs only on change. Flag writes never SELECT directly.

### Anti-Pattern 4: UIDNEXT-Only Incremental Sync (Silent Gaps)

**What people do:** Sync `UID last_next:*` and call it done.
**Why it's wrong:** Misses UIDs that arrived and stayed below the old
`uid_next` window between passes (server-side delay, app asleep mid-batch) —
the exact gap class v1.1 promises to eliminate.
**Do this instead:** Always follow `Incremental` with the `GapFill` set-diff;
it's one extra `SEARCH ALL` (cheap, no bodies) plus fetches only for actually
missing UIDs.

### Anti-Pattern 5: Two Sync Paths (Poll vs Manual) and Overlapping Polls

**What people do:** Poller has its own fetch loop; a second timer fires while
the first pass still runs.
**Why it's wrong:** Two writers over the same `(mailbox_id, uid)` rows with two
sessions = lost flag updates and `uid_next` clobbering; overlapping full sweeps
double server load.
**Do this instead:** Pattern 4 — one `request_sync` with an `AtomicBool`
guard; `cancel_sync` finally gets a real flag wired to batch boundaries
(today it is a documented no-op).

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| IMAP server (mail.utfpr.edu.br:993 + others) | Single `BoxedSession` via `async-imap 0.11`: `select`, `uid_search`, `uid_fetch`, `uid_store` | async-imap 0.11 `uid_store(set, "+FLAGS", "\\Seen")` is the write primitive; verify flag arg spelling against server (some servers want `+FLAGS` vs `+FLAGS.SILENT` — prefer `.SILENT` to avoid unsolicited FETCH noise during sweeps); never issue NAMESPACE (imap-proto 0.16 parser poison — existing tripwire test stays); BODY.PEEK-only reads preserved |
| OS keyring | Unchanged — manager reads creds once at first connect, caches `AccountConfig` in memory for reconnects | Reconnect path must reuse the in-memory config, not re-prompt; zeroize handling unchanged |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| Commands ↔ SessionManager | `manager.with_session(mailbox, op).await` | Replaces all direct `connect_sync` calls; `connect_account` signature UNCHANGED (still probe + set `active_account`; manager lazily connects on first op) |
| Poller ↔ Worker | `request_sync(mailbox, reason)` single entry | Poller never touches `SyncSession` directly; manual `refresh_now` is the same call with `reason: Manual` |
| Worker ↔ Store | Existing `queries::*` + new `upsert_folder`, `folder_uids`, `update_flags` | Keep single-SQL-module invariant: all new SQL lives in `queries.rs`; `update_flags` must also bump FTS? No — flags aren't in FTS; plain UPDATE suffices |
| Backend → Frontend events | `app.emit("sync-event", SyncEvent)` app-wide bus | Per-call `Channel<SyncEvent>` retained for `start_sync` (contract), but poller-originated events use the bus; frontend subscribes via `listen("sync-event")` — one listener serves both |
| SQLite schema M1 → M2 | `rusqlite_migration` append M2: `ALTER TABLE mailboxes ADD COLUMN …` ×3 | Non-destructive; existing rows (INBOX) default `selectable=1`; `SCHEMA_VERSION` 1→2; messages/bodies/attachments/F T S untouched |

## Suggested Build Order (dependency-aware)

1. **`SessionManager` + trait extension** (`imap/manager.rs`, `select_mailbox` /
   `store_flags` / `search_range` on `SyncSession` + `MockSession`). Foundation —
   unblocks everything; migrate `fetch_message`/`save_attachment` onto it now
   while the call sites are still three.
2. **M2 migration + folder cache** (schema columns, `upsert_folder`,
   `list_folders`, discovery refresh inside `connect_account` flow without
   changing its signature). Unlocks the sidebar with zero sync changes.
3. **Worker parameterization + tiered sync** (`sync_folder(mailbox, mode)`,
   `backfill.rs` gap logic, periodic-Full policy). INBOX regression suite
   (existing 4 worker tests) must stay green — they become the `mailbox="INBOX"`
   cases.
4. **`set_seen` optimistic toggle** (command + reconcile/rollback + `FlagSynced`
   event). Depends on 1; testable against a real folder immediately.
5. **Poller + unified entry** (`request_sync`, no-overlap guard, real
   `cancel_sync`, app-wide event bus, manual `refresh_now`). Depends on 1–3;
   last because it orchestrates the others.

## Sources

- Live codebase: `src-tauri/src/sync/worker.rs` (7-step sweep, BATCH_SIZE=200,
  UIDVALIDITY guard, inconsistency guard), `src-tauri/src/imap/mod.rs`
  (`SyncSession` read-only trait, `select_inbox/search_uids/fetch_envelopes/
  fetch_body`), `src-tauri/src/imap/session.rs` (`connect_sync`, NAMESPACE
  parser-poison tripwire), `src-tauri/src/store/schema.sql` + `store/mod.rs`
  (per-row sync state already in `mailboxes`), `commands/sync.rs` (per-command
  connections — the pattern v1.1 retires), `lib.rs` (`AppState`, `connect_account`
  permanent signature).
- IMAP semantics: RFC 3501 `STORE`/`UID STORE` selection-scoping, `UIDNEXT` /
  `UIDVALIDITY` guarantees; async-imap 0.11 `uid_store` API shape.

---
*Architecture research for: SGE v1.1 Triage & Folders*
*Researched: 2026-10-04*
