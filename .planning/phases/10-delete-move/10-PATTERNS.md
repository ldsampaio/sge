# Phase 10: Delete + Move - Pattern Map

**Mapped:** 2026-10-06
**Files analyzed:** 10 (4 new/modified backend groups + 4 UI + 2 shared)
**Analogs found:** 10 / 10

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `src-tauri/src/imap/mod.rs` (new verbs: store_deleted/uid_expunge/uid_copy_to/uid_move_to/capabilities/create_mailbox) | service (transport) | request-response | `src-tauri/src/imap/mod.rs` `set_seen` impl (lines 435-450) + `seen_store_arg` (337-343) | exact |
| `src-tauri/src/imap/manager.rs` (mark_deleted_in/uid_expunge_in/copy_in/move_message_in/create_trash + lease-hold) | service (session owner) | request-response | `src-tauri/src/imap/manager.rs` `set_seen_in` (111-128) + `lease_for` (61-83) | exact |
| `src-tauri/src/sync/worker.rs` (imap_outbox pre-sweep replay + MockSession arms) | service (sync engine) | batch | `src-tauri/src/sync/worker.rs` `replay_outbox` (91-164) + `MockSession` (623-715) | exact |
| `src-tauri/src/store/mod.rs` + `queries.rs` (M7 imap_outbox migration + queries) | model/store | CRUD | `src-tauri/src/store/mod.rs` M2 migration (118-131) + `queries.rs` `enqueue_outbox`/`delete_outbox_op`/`record_outbox_error` (815-921) | exact |
| `src-tauri/src/commands/sync.rs` (delete_message/move_message/expunge_messages + start_sync lease routing) | controller (Tauri command) | request-response | `src-tauri/src/commands/sync.rs` `set_seen` (179-276) + `manager_for` (79-93) + `start_sync` (103-152) | exact |
| `src-tauri/src/sync/mod.rs` (SyncSummary moved/expunged counters) | model (event type) | event-driven | `src-tauri/src/sync/mod.rs` `SyncSummary` (36-64) + `BatchCompleted` (20) | exact |
| `src/components/MessageList.tsx` (delete/move optimistic hide + undo path) | component | request-response | `src/components/MessageList.tsx` `toggleFlag` (155-209) + `pendingUids` wash (74, 322) + `loadPage` pending-clear (126-128) | exact |
| `src/components/MoveMenu.tsx` (new; move-destination picker) | component | request-response | `src/components/Sidebar.tsx` `buildTree` (46-95) + `renderItem`/`renderNode` (115-150) | role-match |
| `src/components/ExpungeModal.tsx` (new; confirm modal) + undo toast | component | request-response | `src/components/SyncStatus.tsx` error/retry + pending indicator (244-260, 298-304) — status/toast copy pattern; no modal exists, use native dialog skeleton | partial |
| `src/components/SyncStatus.tsx` + `sync_status` (pending = both outboxes; poll loop) | component + controller | event-driven | `src/components/SyncStatus.tsx` `requestSync` (124-202) + `pollStatus` (104-112); `commands/sync.rs` `sync_status` (284-312) | exact |

## Pattern Assignments

### 1. `src-tauri/src/imap/mod.rs` — new SyncSession verbs (transport, request-response)

**Analog:** `src-tauri/src/imap/mod.rs` `set_seen` + `seen_store_arg`

**Imports pattern** (lines 25, 222):
```rust
use futures::TryStreamExt;
pub type PinBox<'a, T> = std::pin::Pin<std::boxed::Box<dyn std::future::Future<Output = T> + Send + 'a>>;
```

**Trait addition pattern** (lines 304-307) — copy for each new verb, keep UID-only + object-safe PinBox:
```rust
/// `UID STORE <uid> ±FLAGS.SILENT (\\Seen)` — set or clear the Seen
/// flag on exactly one message by UID. UID-only addressing: sequence
/// numbers must never reach this path (T-6-1).</
fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>>;
```

**Core impl pattern** (lines 435-450) — template for `store_deleted` / `uid_expunge` / `uid_copy_to` / `uid_mv` / `create` / `capabilities`:
```rust
fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>> {
    Box::pin(async move {
        let stream = self
            .uid_store(uid.to_string(), seen_store_arg(seen))
            .await
            .map_err(|e| SyncError::Protocol(format!("UID STORE Seen uid {uid}: {e}")))?;
        // The STORE silently never completes unless the returned
        // response stream is drained to completion — a dropped stream
        // aborts the write, so the acknowledgement is the drain.
        let _responses: Vec<_> = stream
            .try_collect()
            .await
            .map_err(|e| SyncError::Protocol(format!("UID STORE Seen uid {uid}: {e}")))?;
        Ok(())
    })
}
```

**Canonical-arg helper pattern** (lines 337-343) — add `deleted_store_arg(deleted: bool)` mirroring this:
```rust
pub fn seen_store_arg(seen: bool) -> &'static str {
    if seen {
        "+FLAGS.SILENT (\\Seen)"
    } else {
        "-FLAGS.SILENT (\\Seen)"
    }
}
// New: "+FLAGS.SILENT (\\Deleted)" / "-FLAGS.SILENT (\\Deleted)"
```

**Attribute-map pattern for Trash detect** (lines 346-362): `name_attribute_to_string` already maps `NameAttribute::Trash => "\\Trash"`; Trash auto-detect reads `MailboxInfo.attributes` via this — no new mapping needed.

**Rules for planner:** new verbs must drain returned streams with `try_collect`/`try_next` loop; map errors as `SyncError::Protocol("<VERB> …: {e}")`; UID-set args are comma-joined `"1,2,3"` strings (~200 UIDs per call); must NOT parse untagged `COPYUID`/`APPENDUID`/MOVE extended responses (imap-proto 0.16 NAMESPACE tripwire class — RESEARCH §1).

---

### 2. `src-tauri/src/imap/manager.rs` — lease-scoped delete/move/expunge (service, request-response)

**Analog:** `src-tauri/src/imap/manager.rs` `set_seen_in` + `lease_for` + `reconnect`

**Lease acquire pattern** (lines 61-83):
```rust
pub async fn lease_for(&self, mailbox: &str) -> Result<MailboxLease<'_>, SyncError> {
    let mut guard = self.state.lock().await;
    if guard.session.is_none() {
        let session = connect_sync(&self.config).await.map_err(|e| {
            SyncError::Protocol(format!("SessionManager connect: {e}"))
        })?;
        guard.session = Some(session);
        guard.selected_mailbox = None;
    }
    let needs_select = guard.selected_mailbox.as_deref() != Some(mailbox);
    if needs_select {
        let session = guard.session.as_mut().expect("connected above");
        let summary = session.select_mailbox(mailbox).await.map_err(|e| {
            SyncError::Protocol(format!("SessionManager SELECT {mailbox}: {e}"))
        })?;
        // ... log + record selected_mailbox
    }
    Ok(MailboxLease { guard })
}
```

**Reconnect-retry template** (lines 111-128) — copy verbatim for `mark_deleted_in` / `uid_expunge_in`:
```rust
pub async fn set_seen_in(
    &self, mailbox: &str, uid: u32, seen: bool,
) -> Result<(), SyncError> {
    let mut lease = self.lease_for(mailbox).await?;
    match lease.session().set_seen(uid, seen).await {
        Ok(()) => Ok(()),
        Err(first) => {
            eprintln!("[SGE imap] set_seen {mailbox} uid {uid} failed ({first}) — reconnecting once");
            drop(lease);
            self.reconnect().await?;
            let mut lease = self.lease_for(mailbox).await?;
            lease.session().set_seen(uid, seen).await
        }
    }
}
```

**Lease-hold rule for fallback sequences (critical):** public `move_message_in(src, uid_set, dest)` must hold ONE `lease_for(src)` guard and drive `lease.session().uid_copy_to/…` directly — must NEVER call `self.lease_for()` re-entrantly while holding a lease (async mutex → deadlock). Same for `uid_expunge_in` (SELECT-then-EXPUNGE under one lease). `MailboxLease::session()` (lines 161-170) gives `&mut BoxedSession`.

**Capability caching:** query `capabilities()` once per fresh connection, store alongside `selected_mailbox` in `ManagerState` (lines 32-36), invalidate on `reconnect()` (lines 95-103).

---

### 3. `src-tauri/src/sync/worker.rs` — imap_outbox replay + MockSession (service, batch)

**Analog:** `src-tauri/src/sync/worker.rs` `replay_outbox` (91-164), post-sync replay site (579-586), `MockSession` (623-715)

**Replay template** (lines 91-164):
```rust
pub async fn replay_outbox(
    &self, session: &mut dyn SyncSession, mailbox_id: u64,
    current_uid_validity: u32, live_uids: Option<&HashSet<u32>>,
) -> Result<ReplaySummary, SyncError> {
    let ops = { /* lock store briefly, list_outbox */ };
    // Epoch check FIRST: any op with stale uid_validity → drop whole mailbox queue
    if ops.iter().any(|op| op.uid_validity != current_uid_validity) {
        let n = queries::drop_outbox_for_mailbox(/* … */)?;
        summary.dropped = n;
        return Ok(summary);
    }
    for op in &ops {
        if let Some(live) = live_uids { /* absent UID → delete_outbox_op, dropped += 1, continue */ }
        match session.set_seen(op.uid, op.seen).await {
            Ok(()) => { /* delete_outbox_op; acked += 1 */ }
            Err(e) => { /* record_outbox_error (attempts+last_error); failed += 1, stays queued */ }
        }
    }
}
```

**Replay-order change:** current post-sync replay (lines 579-586) runs AFTER the sweep. Phase 10 moves `imap_outbox` replay to PRE-SWEEP (before step 5) so locally-deleted mail isn't resurrected by the same-pass sweep; `flag_outbox` stays post-sync or joins pre-sweep (planner decides). `ReplaySummary { acked, dropped, failed }` (lines 43-52) extends with `moved` count.

**Convergence-gate extension** (lines 292-301, 332-335): `pending_uids` fetch + `skip_sweep` condition must include imap_outbox depth (`skip_sweep` requires BOTH queues empty).

**MockSession arm pattern** (lines 623-641, 688-696) — add `deleted_calls/copied_calls/moved_calls/expunged_sets` vecs + `fail_*` toggles + canned capabilities in lockstep with each new trait method:
```rust
pub struct MockSession {
    pub summary: MailboxSummary,
    pub envelopes: Vec<MessageHeader>,
    pub fetch_calls: AtomicUsize,
    pub logout_called: AtomicBool,
    pub set_seen_calls: Vec<(u32, bool)>,
    pub fail_set_seen: bool,
    // …
}
fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>> {
    self.set_seen_calls.push((uid, seen));
    if self.fail_set_seen {
        return Box::pin(async move { Err(SyncError::Protocol("mock set_seen failure".to_string())) });
    }
    Box::pin(async move { Ok(()) })
}
```

**UID-set formatting** (lines 380-384): `chunk.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",")` with `BATCH_SIZE: u32 = 200` (line 31).

---

### 4. `src-tauri/src/store/mod.rs` + `queries.rs` — M7 `imap_outbox` (model, CRUD)

**Analog:** `src-tauri/src/store/mod.rs` M2 migration + `queries.rs` flag_outbox helpers

**Migration registration** (store/mod.rs lines 79-92, bump `SCHEMA_VERSION` 6 → 7):
```rust
pub const SCHEMA_VERSION: u32 = 6; // → 7
let migrations = Migrations::new(vec![
    M::up(include_str!("schema.sql")),
    M::up(M2_FLAG_OUTBOX_SQL),
    M::up(M3_UNSEEN_COUNT_SQL),
    M::up(M4_BACKFILL_SQL),
    M::up(M5_STATUS_TS_SQL),
    M::up(M6_DELIMITER_SQL),
    // + M::up(M7_IMAP_OUTBOX_SQL),
]);
```

**Table DDL template** (store/mod.rs lines 118-131):
```rust
const M2_FLAG_OUTBOX_SQL: &str = concat!(
    "CREATE TABLE flag_outbox (",
    "  id            INTEGER PRIMARY KEY,",
    "  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,",
    "  uid           INTEGER NOT NULL,",
    "  seen          INTEGER NOT NULL,",
    "  uid_validity  INTEGER NOT NULL,",
    "  created_at    TEXT NOT NULL DEFAULT (datetime('now')),",
    "  attempts      INTEGER NOT NULL DEFAULT 0,",
    "  last_error    TEXT,",
    "  UNIQUE (mailbox_id, uid)",
    ");",
    "CREATE INDEX idx_outbox_mailbox ON flag_outbox(mailbox_id);",
);
```
M7 DDL per RESEARCH §5: same shape with `op TEXT NOT NULL` (`'delete'|'move'`), nullable `dest_mailbox TEXT`, nullable Seen-intent column for the move-carries-toggle rule; `UNIQUE(mailbox_id,uid)` latest-wins. Do NOT alter `flag_outbox` (Phase 6 contract). Forward-only + preserve-rows test copying `m2_upgrades_v1_database_forward_preserving_rows` (store/mod.rs ~470-511).

**Outbox query templates** (queries.rs lines 815-921) — mirror each for `imap_outbox` (`enqueue_imap_outbox`, `list_imap_outbox`, `delete_imap_outbox_op`, `drop_imap_outbox_for_mailbox`, `record_imap_outbox_error`, `imap_outbox_count`):
```rust
// enqueue (815-833): INSERT … ON CONFLICT(mailbox_id, uid) DO UPDATE SET … attempts=0, last_error=NULL
// pending_uids (839-848): SELECT uid FROM flag_outbox WHERE mailbox_id = ?1 ORDER BY uid
// delete_outbox_op (874-884): DELETE FROM flag_outbox WHERE mailbox_id = ?1 AND uid = ?2
// drop_outbox_for_mailbox (888-894): DELETE FROM flag_outbox WHERE mailbox_id = ?1
// record_outbox_error (897-910): UPDATE flag_outbox SET attempts = attempts + 1, last_error = ?1 WHERE …
// outbox_count (914-921): SELECT COUNT(*) FROM flag_outbox WHERE mailbox_id = ?1
```

**Enqueue rule:** enqueueing a delete/move drops the same-key `flag_outbox` row in the same-lock transaction; move captures pending Seen intent (delete flag op first). Epoch (`uid_validity`) comes from `get_sync_state` at enqueue (see command pattern below). `delete_missing_uids` wipe path (worker.rs 227-235 + queries.rs 506) must also `drop_imap_outbox_for_mailbox`.

**Optimistic local state:** `set_local_seen` no-op-on-missing-row pattern (queries.rs 930-953). For delete: `pending_delete`-style hidden flag is safer for undo (row stays, filtered from `list_messages`/`fts_search`, restored on undo); move: optimistic delete-from-src (+ optional dest placeholder), undo = reverse-MOVE while op still queued.

---

### 5. `src-tauri/src/commands/sync.rs` — delete/move/expunge commands + sync precondition (controller, request-response)

**Analog:** `src-tauri/src/commands/sync.rs` `set_seen` (179-276), `manager_for` (79-93), `sync_status` (284-312)

**Command skeleton** (lines 179-276) — exact template for `delete_message(uid, mailbox?)` / `move_message(uid, dest, mailbox?)` / `expunge_messages(uids, mailbox)`:
```rust
#[tauri::command]
pub async fn set_seen(state: State<'_, crate::AppState>, uid: u32, seen: bool, mailbox: Option<String>)
-> Result<SetSeenResult, String> {
    let mailbox = mailbox.unwrap_or_else(|| "INBOX".to_string()); // Phase 7 pattern: Option<String> defaults to INBOX
    let account_cfg = load_account_config(state.clone()).await?;  // in-memory active_account → keyring fallback (31-74)
    let manager = manager_for(&state, &account_cfg);              // cached per-account SessionManager (79-93)
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        async_std::task::block_on(async {
            // 1. Optimistic local write + durable enqueue under ONE lock (never across .await)
            let (mailbox_id, epoch) = { /* ensure_mailbox + get_sync_state + set_local + enqueue */ };
            // 2. Immediate verb via manager lease; ack → dequeue, fail → record error, stays queued
            let fail_reason: Option<String> = match manager.set_seen_in(&mailbox, uid, seen).await { … };
            // 3. Opportunistic queue drain on open session (no fresh SEARCH → no absent pruning)
            // …
            Ok(SetSeenResult { uid, seen, acked, pending_count, detail })
        })
    }).await.map_err(|e| format!("internal error: set_seen task failed ({e})"))?
}
```

**Key details to copy:** `load_account_config` (31-74); `manager_for` slot keyed on `account_key()` (79-93); `spawn_blocking` + `block_on` discipline (117-118); store lock never across `.await`; result struct `{ acked, pending_count, detail }` for toast + pending wash (154-167).

**Precondition (ships in this phase):** `start_sync` (lines 103-152) currently calls `connect_sync(&account_cfg)` (line 131) — a fresh session per pass. Route through `manager_for().lease_for(&mailbox)` BEFORE any destructive op ships (single-flight `SyncGate::try_begin` stays). `list_mailboxes` (386-445) already shows the manager path + `\Noselect` skip (412-417) + offline-cached fallback — copy the NoSelect-skip and offline-fallback for Trash detect / move picker data.

**`sync_status` pending pattern** (lines 284-312): `pending_count` must sum BOTH outbox depths (`flag_outbox` + `imap_outbox`) for the pending indicator.

---

### 6. `src-tauri/src/sync/mod.rs` — SyncSummary counters (model, event-driven)

**Analog:** `src-tauri/src/sync/mod.rs` `SyncSummary` (36-57) + `BatchCompleted` (20)

```rust
pub enum SyncEvent {
    BatchCompleted { new: usize, updated: usize, deleted: usize }, // extend call sites, not shape, if needed
    SyncCompleted { summary: SyncSummary },
    // …
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SyncSummary {
    pub new: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize, // existing meaning: server-side disappearances (expunge-diff step 6) — KEEP
    pub uid_validity_bump: bool,
    pub fetched: usize,
    pub gap_refetches: usize,
    pub converged: bool,
}
impl SyncSummary {
    pub fn total(&self) -> usize { self.new + self.updated + self.unchanged } // unchanged
}
```

Add `moved: usize` + `expunged: usize` (locally-initiated acks this pass); keep `deleted` semantics; update `SyncEvent::BatchCompleted` emission sites (worker.rs 541-545) and frontend `SyncCompleted`/`BatchCompleted` shapes accordingly.

---

### 7. `src/components/MessageList.tsx` — optimistic delete/move + pending wash (component, request-response)

**Analog:** `src/components/MessageList.tsx` `toggleFlag` (155-209), `pendingUids` (74, 314-322), `loadPage` pending-clear (114-129)

**Optimistic toggle template** (lines 155-209) — copy for delete/move (hide optimistically, wash, rollback only on invoke reject):
```rust
const toggleFlag = useCallback(async (uid, targetSeen, previousFlags) => {
  setMessages((prev) => prev.map((m) => m.uid === uid ? { ...m, flags: setSeenInFlags(m.flags, targetSeen) } : m));
  setPendingUids((prev) => new Set(prev).add(uid));
  try {
    const result = await invoke<SetSeenResult>("set_seen", { uid, seen: targetSeen, mailbox });
    dispatchFlagUpdate({ uid: result.uid, seen: result.seen, acked: result.acked, pending_count: result.pending_count, detail: result.detail, origin: "list" });
    if (result.acked) { setPendingUids((prev) => { const next = new Set(prev); next.delete(uid); return next; }); }
  } catch (err) {
    // rollback optimistic flip, clear wash, dispatch error sentinel pending_count: -1
  }
}, [mailbox, reportState]);
```

**Pending wash** (line 322): `style={pending ? { backgroundColor: "var(--color-accent-soft)" } : undefined}` — reuse for delete/move rows. **Outbox-drained clear** (lines 126-128): `if (status.pending_count === 0) setPendingUids(new Set())`. **Cross-pane event** (lines 214-237): `FLAG_UPDATE_EVENT` listener with `pending_count === -1` error sentinel — extend `FlagUpdateDetail` or add a parallel delete/move event in `src/types.ts` reusing `dispatchFlagUpdate` shape.

**Helpers to reuse:** `isTauriRuntime` guard (43-46) + `OUTSIDE_DESKTOP_MESSAGE` (48-49); `invoke` with `mailbox` param defaulting to INBOX; folder chip `folderLabel` raw→display lookup (145-147) via `mailboxes` prop.

---

### 8. Move picker + expunge modal + undo toast (components, request-response)

**Move menu analog — `src/components/Sidebar.tsx` `buildTree` (46-95):**
```tsx
function buildTree(mailboxes: MailboxRow[]): FolderTree {
  const system = mailboxes.filter((mb) => systemRank(mb.name) !== null).sort(/* SYSTEM_ORDER rank, then pt-BR alpha */);
  const custom = mailboxes.filter((mb) => systemRank(mb.name) === null);
  // delimiter nesting: split mb.name on mb.delimiter, parent-path join; shortLabel = last display segment
}
```
Reuse `buildTree` (export or extract) for the 'Mover para…' menu. Picker excludes `\Noselect` rows (backend already skips them — commands/sync.rs 412-417; filter on attributes client-side too), excludes src==dest (INBOX→INBOX no-op guard) and Sent; moving OUT of Trash = restore path (must be supported). No drag-and-drop per CONTEXT. `MailboxRow` shape (`name` raw wire / `display_name` / `delimiter` / attributes) in `src/types.ts`; pass RAW `name` to `move_message` (never display_name).

**Trash auto-detect (agent's discretion, recommended):** SPECIAL-USE `\Trash` attribute first (`name_attribute_to_string` maps it — imap/mod.rs 358), then case-insensitive name match (`Trash`, `Deleted`, `Lixeira`, Gmail-style `[Gmail]/Trash`), then CREATE `Trash` behind user confirmation (silent CREATE risks twin-trash). `SYSTEM_ORDER` (Sidebar.tsx 17-23) already ranks trash names (`trash/deleted/deleted messages/lixeira`) — reuse for the name-match list. Do NOT invent Phase 11's roles schema; cache resolved Trash per account locally.

**Expunge modal (no existing modal analog):** build a minimal confirm dialog naming folder + count ("Apagar para sempre? X mensagens" + folder name per CONTEXT). Only ever sends selected UIDs (`uid_expunge` set), never bare `expunge()`. Never touch other clients' `\Deleted` (UIDPLUS path or fail loudly — RESEARCH §1 matrix).

**Undo toast copy pattern — `src/components/SyncStatus.tsx` pending/error strings (244-260, 298-304):**
```tsx
{status.status.pending_count > 0 && (<p className="sub" role="status">
  {status.status.pending_count === 1 ? "1 alteração aguardando envio" : `${status.status.pending_count} alterações aguardando envio`}
</p>)}
```
Toast shows ~5–10 s, valid iff op still in `imap_outbox` ("até o próximo sync" anchor); undo = reverse op (move back / clear pending_delete + drop outbox row). Exact copy/placement is agent's discretion.

**Poll integration — `SyncStatus.tsx` `requestSync` (124-202):** single `requestSync` entry loops ALL known folders via `start_sync` (171-180); imap_outbox replay rides this loop (pre-sweep), no new scheduler. `pollStatus` (104-112) + `sync_status.pending_count` must reflect both outboxes.

## Shared Patterns

### Tauri command shape (all new commands)
**Source:** `src-tauri/src/commands/sync.rs` 180-276
`load_account_config` → `manager_for` → `spawn_blocking`+`block_on`; `mailbox: Option<String>` defaults to INBOX; store lock only for brief sync sections, never across `.await`; return `{ acked, pending_count, detail }`. Errors are `String` (`map_err(|e| e.to_string())`).

### Single-flight (destructive ops + sync)
**Source:** `src-tauri/src/sync/mod.rs` 70-121; use site `commands/sync.rs` 122-128
`gate.try_begin()` → `None` means skip, never queue; guard releases on drop. Lease + sync serialize; no second IMAP connection for moves (roadmap hard constraint).

### Error handling
**Source:** `src-tauri/src/imap/mod.rs` 224-264 (`SyncError::Protocol/State`); worker replay 144-160 (per-op `attempts`+`last_error`, stays queued; store-level errors abort pass; replay failure never fails the sync itself).
Refuse loudly > silent destruction: plain-`expunge` fallback without verified unmark dance must error in plain language, not expunge blindly (RESEARCH §1).

### UID-only + BODY.PEEK audit
**Source:** `src-tauri/src/imap/mod.rs` 304-307 doc + test 660-677 (`seen_store_arg` spelling/purity tests)
All new verbs UID-addressed; `uid_set` is `u32`-formatted comma-joined strings; no seq-number `store/copy/mv`; no full-fetch flag side effects. Mirror the T-6-01 style test for `store_deleted`.

### Offline-first store discipline
**Source:** `src-tauri/src/sync/worker.rs` 15-18 (Store `Arc<Mutex>` comment), `commands/sync.rs` 194-209
Store mutex held only for synchronous DB sections, never across `.await`. Optimistic write + enqueue under one lock; `ON CONFLICT … DO UPDATE` latest-wins upserts.

## No Analog Found

| File | Role | Data Flow | Reason |
|---|---|---|---|
| `src/components/ExpungeModal.tsx` | component (confirm modal) | request-response | No modal/confirm-dialog component exists in the codebase (only inline error/empty states in MessageList + status panels in SyncStatus). Planner should specify a minimal native-dialog skeleton following SyncStatus copy tone (PT strings). |
| Capability-gated fallback orchestration (MOVE/UIDPLUS matrix) | service logic | request-response | No capability-branching precedent exists (`capabilities()` is probe-only today). Planner must define the branch structure fresh per RESEARCH §1 matrix + MockSession cap combinations. |

## Metadata

**Analog search scope:** `src-tauri/src/imap/`, `src-tauri/src/sync/`, `src-tauri/src/store/`, `src-tauri/src/commands/`, `src/components/`
**Files scanned:** 10 (all git-tracked; verified via `git ls-files`)
**Pattern extraction date:** 2026-10-06
