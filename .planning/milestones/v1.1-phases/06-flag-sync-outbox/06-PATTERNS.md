# Phase 6: Flag Sync + Outbox - Pattern Map

**Mapped:** 2026-10-04
**Files analyzed:** 9
**Analogs found:** 9 / 9

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `src-tauri/src/imap/manager.rs` (new SessionManager) | service | request-response | `src-tauri/src/imap/session.rs` (`connect_sync`, lines 755-783) + `src-tauri/src/lib.rs` (`AppState`, lines 27-30) | role-match |
| `src-tauri/src/imap/mod.rs` (`SyncSession::set_seen` extension) | service (trait) | request-response | `src-tauri/src/imap/mod.rs` (trait lines 253-274 + `BoxedSession` impl lines 276-351) | exact |
| `src-tauri/src/store/mod.rs` (M2 `flag_outbox` migration) | migration/config | file-I/O (DDL) | `src-tauri/src/store/mod.rs` (`apply_migrations`, lines 77-83) + `src-tauri/src/store/schema.sql` (v1 baseline) | exact |
| `src-tauri/src/store/queries.rs` (outbox CRUD + `set_local_seen`) | model/store | CRUD | `src-tauri/src/store/queries.rs` (`upsert_message` lines 143-174, `delete_missing_uids` lines 234-265, `existing_uids` lines 270-293) | exact |
| `src-tauri/src/sync/worker.rs` (pending-wins reconcile + replay hook) | service | batch | `src-tauri/src/sync/worker.rs` (step-5 loop lines 176-213, steps 1-7 structure lines 47-261, `MockSession` fixture lines 275-309) | exact |
| `src-tauri/src/commands/sync.rs` (new `set_seen` command + `sync_status` pending count) | controller (Tauri command) | request-response | `src-tauri/src/commands/sync.rs` (`start_sync` cred-loading lines 39-80, `spawn_blocking`+`block_on` lines 87-109, `sync_status` lines 117-135, `fetch_message` lines 229-281) | exact |
| `src/components/MessageList.tsx` (clickable unread dot + pending wash) | component | request-response | `src/components/MessageList.tsx` (row render lines 196-239, `invoke` + `isTauriRuntime` guard lines 76-125) | exact |
| `src/components/ReadingPane.tsx` (header toggle button + Seen-on-open) | component | request-response | `src/components/ReadingPane.tsx` (`invoke("fetch_message")` effect lines 18-39, header lines 104-121, `save_attachment` invoke lines 41-60) | exact |
| `src/components/SyncStatus.tsx` (pending aggregate + replay-failure text) | component | event-driven (Channel) + request-response (poll) | `src/components/SyncStatus.tsx` (`pollStatus` lines 74-81, `Channel<SyncEvent>` lines 88-113, render lines 140-189) | exact |

All analog paths verified git-tracked via `git ls-files -- src-tauri/src src` (all listed files present, non-mirror).

## Pattern Assignments

### `src-tauri/src/imap/manager.rs` (service, request-response) — NEW FILE

**Analog:** `src-tauri/src/imap/session.rs` (`connect_sync`, lines 755-783) + `src-tauri/src/lib.rs` (`AppState`, lines 27-30)

**Connection-establishment pattern** (`session.rs` lines 755-783 — SessionManager must delegate to this, not reimplement TLS/login):
```rust
pub async fn connect_sync(cfg: &AccountConfig) -> Result<BoxedSession, SyncError> {
    check_cert_policy(cfg).map_err(|e| SyncError::State(format!("{e}")))?;
    match cfg.security {
        SecurityMode::ImplicitTls => {
            let tcp = tcp_connect(cfg)
                .await
                .map_err(|e| SyncError::Protocol(format!("TCP: {e}")))?;
            let tls = tls_upgrade(cfg, tcp)
                .await
                .map_err(|e| SyncError::Protocol(format!("TLS: {e}")))?;
            login_sync(box_stream(tls), cfg).await
        }
        // ... StartTls / PlainLocal arms
    }
}
```

**Shared-ownership pattern** (`lib.rs` lines 27-30 — manager joins this state; single session owned behind Mutex):
```rust
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub active_account: Mutex<Option<ActiveAccount>>,
}
```
Manager shape per RESEARCH Pattern 2: own one `BoxedSession` (`tokio`/`async_std` Mutex), expose `mailbox("INBOX")` lease that SELECTs INBOX on first lease and re-SELECTs after reconnect, serialize leases single-flight. Retire per-command `connect_sync` calls in `commands/sync.rs` (`start_sync` line 89, `fetch_message` line 271, `save_attachment` line 407) toward the manager incrementally — do not delete `connect_sync` itself (manager calls it for (re)connect + retry-once).

---

### `src-tauri/src/imap/mod.rs` (`SyncSession::set_seen` extension)

**Analog:** `src-tauri/src/imap/mod.rs` (trait lines 253-274, impl lines 276-351)

**Trait-extension pattern** (lines 244-274 — new method follows `PinBox` object-safe convention; update the doc comment's read-only ban):
```rust
pub trait SyncSession: Unpin + Send {
    /// `SELECT INBOX` — validates the mailbox is selectable and returns
    /// UIDVALIDITY / UIDNEXT / exists counts.
    fn select_inbox(&mut self) -> PinBox<'_, Result<MailboxSummary, SyncError>>;
    // ... search_uids / fetch_envelopes / fetch_body / logout
}
```

**`BoxedSession` impl pattern** (lines 294-304 — error mapping with operation prefix; `set_seen` copies this plus stream drain):
```rust
fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
    Box::pin(async move {
        let uids_set = self
            .uid_search("ALL")
            .await
            .map_err(|e| SyncError::Protocol(format!("UID SEARCH ALL: {e}")))?;
        let mut uids: Vec<u32> = uids_set.into_iter().collect();
        uids.sort_unstable();
        Ok(uids)
    })
}
```

**Stream-drain pattern** (lines 312-324 — `fetch_envelopes` shows `try_next` loop over async-imap stream; `set_seen` MUST `try_collect` the `uid_store` stream per RESEARCH Pitfall 1):
```rust
let mut stream = self
    .uid_fetch(&range_owned, headers::FETCH_ATTRS)
    .await
    .map_err(|e| SyncError::Protocol(format!("UID FETCH: {e}")))?;
let mut out = Vec::new();
while let Some(fetch) = stream.try_next().await? {
```
Required spelling (RESEARCH verified against async-imap 0.11.3 `client.rs:845-866`):
```rust
let arg = if seen { "+FLAGS.SILENT (\\Seen)" } else { "-FLAGS.SILENT (\\Seen)" };
let stream = self.uid_store(uid.to_string(), arg).await?;
let _updates: Vec<_> = stream.try_collect().await?; // drain — STORE completes only here
```
Also extend `MockSession` in `sync/worker.rs` tests (lines 281-309) with a `set_seen` stub recording calls.

---

### `src-tauri/src/store/mod.rs` (M2 `flag_outbox` migration)

**Analog:** `src-tauri/src/store/mod.rs` (lines 77-83) + `src-tauri/src/store/schema.sql` (v1 baseline, 76 lines)

**Migration pattern** (lines 77-83 — M2 appends as second `M::up`, `schema.sql` stays UNCHANGED v1 text per RESEARCH anti-pattern):
```rust
fn apply_migrations(conn: &mut Connection) -> StoreResult<()> {
    // WAL for concurrent reader (UI) + writer (sync worker)
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    let migrations = Migrations::new(vec![M::up(include_str!("schema.sql"))]);
    migrations.to_latest(conn)?;
    Ok(())
}
```
Becomes:
```rust
let migrations = Migrations::new(vec![
    M::up(include_str!("schema.sql")), // v1 — UNCHANGED
    M::up("CREATE TABLE flag_outbox ( ... ); CREATE INDEX ..."),
]);
```
Bump `SCHEMA_VERSION` (line 19) `1 → 2`. Suggested DDL (RESEARCH Code Examples): `id INTEGER PRIMARY KEY, mailbox_id INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE, uid INTEGER NOT NULL, seen INTEGER NOT NULL, uid_validity INTEGER NOT NULL, created_at TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, UNIQUE (mailbox_id, uid)` + `CREATE INDEX idx_outbox_mailbox ON flag_outbox(mailbox_id)`.

**Test pattern** (`store/mod.rs` tests lines 117-157 — `open_in_memory` + `sqlite_master` assertions; add `flag_outbox` presence test here).

---

### `src-tauri/src/store/queries.rs` (outbox CRUD + `set_local_seen` + pending-wins helper)

**Analog:** `src-tauri/src/store/queries.rs` (self — single-SQL-module invariant, file header lines 1-10)

**Single-writer rule** (lines 1-5 — ALL new SQL lives here, no raw SQL in worker/commands):
```rust
//! All SQL for the local store lives here (single-SQL-module invariant).
//!
//! Per the architectual sketch, every `messages` write path — upsert,
//! expunge-diff, body/attachment insert — is channelled through the
//! functions below. No raw SQL escapes this module.
```

**Upsert pattern to copy** (lines 122-174 — `UPSERT_MESSAGE_SQL` const + `ON CONFLICT ... DO UPDATE`; outbox enqueue uses `ON CONFLICT(mailbox_id, uid) DO UPDATE` = latest-wins):
```rust
const UPSERT_MESSAGE_SQL: &str = concat!(
    "INSERT INTO messages ",
    "(mailbox_id, uid, message_id, subject, from_addr, to_addrs, ",
    " cc_addrs, date_utc, flags, has_attachments, preview) ",
    "VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ",
    "ON CONFLICT(mailbox_id, uid) DO UPDATE SET ",
    // ...
);
```

**Batch-safe lookup pattern** (lines 270-293 `existing_uids`, lines 234-265 `delete_missing_uids` with TEMP TABLE — copy for `pending_uids(mailbox_id) -> HashSet<u32>` and RFC 4549 drop paths):
```rust
pub fn existing_uids(
    conn: &Connection,
    mailbox_id: u64,
    uids: &[u32],
) -> StoreResult<Vec<u32>> { /* ... */ }
```

**Flags-helper pattern** (lines 27-38 — extend with `set_seen_flag(flags_json, seen) -> String`; UPDATE the stale display-only comment):
```rust
/// `true` when the `\Seen` flag is absent — the message is unread.
///
/// Read/unread is **display-only** in M1: flags are stored on the row but
/// the client never writes `\Seen` back to the server (D-flags).
pub fn is_unread(flags_json: &str) -> bool {
    !parse_flags(flags_json).iter().any(|f| f == "\\Seen")
}
```
New functions needed: `enqueue_outbox(conn, mailbox_id, uid, seen, uid_validity)`, `pending_uids(conn, mailbox_id)`, `delete_outbox_op(conn, mailbox_id, uid)`, `drop_outbox_for_mailbox(conn, mailbox_id)` (UIDVALIDITY bump), `set_local_seen(conn, mailbox_id, uid, seen)` (flags-JSON add/remove `\Seen`). Tests go in `queries.rs` tests module (pattern: `insert_msg` helper line 515 + `Store::open_in_memory`).

---

### `src-tauri/src/sync/worker.rs` (pending-wins reconcile + replay hook)

**Analog:** `src-tauri/src/sync/worker.rs` (self — step-5 loop lines 176-213, `MockSession` lines 275-309)

**Write-back loop to gate** (lines 176-197 — fetch `pending_uids` once before this loop; for pending UIDs write every column EXCEPT `flags`):
```rust
{
    let guard = self.store.lock().unwrap();
    let conn = guard.conn();
    for header in &headers {
        let uid = header.uid;
        let message_id = header.message_id.as_deref();
        queries::upsert_message(
            conn,
            mailbox_id,
            uid,
            message_id,
            &header.subject,
            &header.from_addr,
            &header.to_addrs,
            &header.cc_addrs,
            &header.date_utc,
            &header.flags,
            header.has_attachments,
            &header.preview,
        )
        .map_err(|e| SyncError::Protocol(format!("upsert uid {uid}: {e}")))?;
```

**Mutex discipline** (file header lines 12-15 — lock only for sync DB ops, never across `.await`):
```rust
//! The Store is behind an `Arc<Mutex<>>` so it can be shared
//! safely between the Tauri command thread and the sync worker.
//! The mutex is held only during synchronous DB operations,
//! never across `.await` points (IMAP fetches).
```

**UIDVALIDITY integration** (lines 74-80 — outbox drop hooks here: on bump, `drop_outbox_for_mailbox` BEFORE any replay):
```rust
let uid_validity_bump = prev_uidv != 0 && prev_uidv != summary.uid_validity;
if uid_validity_bump {
    let guard = self.store.lock().unwrap();
    queries::delete_missing_uids(guard.conn(), mailbox_id, &[])
        .map_err(|e| SyncError::Protocol(format!("wipe on UIDVALIDITY bump: {e}")))?;
}
```

**Mock fixture to extend** (lines 275-309 — add `set_seen` recording + `seen_calls` counter per RESEARCH Wave 0 gaps):
```rust
pub struct MockSession {
    pub summary: MailboxSummary,
    pub envelopes: Vec<MessageHeader>,
    pub fetch_calls: AtomicUsize,
    pub logout_called: AtomicBool,
}
```

---

### `src-tauri/src/commands/sync.rs` (new `set_seen` command + `sync_status` pending count)

**Analog:** `src-tauri/src/commands/sync.rs` (self)

**Credential-loading pattern** (`start_sync` lines 39-80 — prefer in-memory `active_account`, fall back to keyring; copy for `set_seen`):
```rust
let account_cfg: AccountConfig = {
    let mem = state.active_account.lock().unwrap().clone();
    if let Some(acc) = mem {
        eprintln!("[SGE sync] Using in-memory credentials for {}", acc.username);
        AccountConfig {
            host: acc.host,
            port: acc.port,
            security: SecurityMode::parse(&acc.security).map_err(|e| e.to_string())?,
            // ...
```

**Blocking-thread pattern** (lines 87-109 — `spawn_blocking` + `async_std::task::block_on`, never Tokio runtime threads):
```rust
let store = state.store.clone();
tauri::async_runtime::spawn_blocking(move || {
    async_std::task::block_on(async {
        let session = connect_sync(&account_cfg).await
            .map_err(|e| format!("IMAP connection: {e}"))?;
        // ...
    })
})
.await
.map_err(|e| format!("internal error: sync task failed ({e})"))?
.map_err(|e| e)?;
```

**Read-command pattern** (`sync_status` lines 117-135 — extend `SyncStatus` struct (lines 197-204) with `pending_count: i64` from outbox; keep `Result<_, String>` + `.map_err(|e| format!("store: {e}"))`):
```rust
#[tauri::command]
pub async fn sync_status(state: State<'_, crate::AppState>) -> Result<SyncStatus, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap();
        let conn = guard.conn();
        crate::store::queries::sync_status(conn, "INBOX")
            .map_err(|e| format!("store: {e}"))
            // ...
    })
    .await
    .map_err(|e| format!("internal error: sync status task failed ({e})"))?
}
```

**Registration** (`lib.rs` lines 233-247 — add `commands::sync::set_seen` to `generate_handler!`).

`set_seen` flow per RESEARCH: (1) `set_local_seen` flags-JSON write + `enqueue_outbox` UPSERT (instant, both under one lock — optimistic truth); (2) try manager lease → `uid_store` now; on ack `delete_outbox_op`; on failure leave queued, surface in `sync_status` text, retry next sync. UID is `u32` from local DB; flag arg is a fixed two-variant literal — no user-input interpolation (ASVS V5).

---

### `src/components/MessageList.tsx` (clickable unread dot + pending wash)

**Analog:** `src/components/MessageList.tsx` (self, lines 196-239 + invoke pattern lines 100-111)

**Row + dot pattern** (lines 199-214 — dot becomes a nested clickable toggle with `stopPropagation`; `aria-label` flips per UI-SPEC; row gets `pending` class → `--color-accent-soft #ffedd5` wash):
```tsx
{messages.map((msg) => {
  const unread = isUnread(msg.flags);
  const selected = selectedUid === msg.uid;
  return (
    <button
      type="button"
      key={msg.uid}
      role="option"
      aria-selected={selected}
      className={`message-row ${unread ? "unread" : "read"} ${selected ? "selected" : ""}`}
      onClick={() => onMessageSelect(msg)}
    >
      <span
        className={`unread-dot ${unread ? "unread" : "read"}`}
        aria-label={unread ? "Não lido" : "Lido"}
      />
```

**Invoke pattern** (lines 101-108 — `invoke<T>` with typed args; `set_seen` call mirrors this):
```tsx
const [rows, status] = await Promise.all([
  invoke<MessageRow[]>("list_messages", {
    mailbox,
    limit: PAGE_SIZE,
    offset: p * PAGE_SIZE,
  }),
  invoke<SyncStatusInfo>("sync_status"),
]);
```

**State/error pattern** (lines 76-125 — `isTauriRuntime()` guard, try/catch → `setError`, `reportState`; reuse for optimistic toggle with rollback on `set_seen` failure). `isUnread` helper in `src/types.ts` lines 46-53 stays the single read/unread classifier.

---

### `src/components/ReadingPane.tsx` (header toggle button + Seen-on-open)

**Analog:** `src/components/ReadingPane.tsx` (self, lines 18-39 + header 104-121)

**Fetch-on-select pattern** (lines 18-39 — Seen-on-open hooks here: after successful `fetch_message`, invoke `set_seen(uid, true)` when the message is unread; no toast, row updates optimistically per UI-SPEC):
```tsx
useEffect(() => {
  if (!selectedMessage) {
    setMessage(null);
    setStatus("idle");
    return;
  }
  const uid = selectedMessage.uid;
  setStatus("loading");
  setErrorMsg("");
  setMessage(null);
  invoke<MessageView>("fetch_message", { uid })
    .then((result) => {
      setMessage(result);
      setStatus("ready");
    })
    .catch((err: { message: string }) => {
      setErrorMsg(err.message || "Não foi possível abrir o aviso");
      setStatus("error");
    });
}, [selectedMessage]);
```

**Header pattern** (lines 104-121 — toggle button `Marcar como lido / Marcar como não lido` goes in `.reading-header`, label reflects target state):
```tsx
<header className="reading-header">
  <span className="reading-kicker">E-mail</span>
  <h2 className="reading-subject">{message.subject || <em>(sem assunto)</em>}</h2>
  <div className="reading-meta">
```

**Attachment-save invoke pattern** (lines 41-60 — `invoke("save_attachment", { uid, partNumber, filePath })` shows camelCase→snake_case arg mapping the `set_seen` invoke must follow: `{ uid, seen }`).

---

### `src/components/SyncStatus.tsx` (pending aggregate + replay-failure text)

**Analog:** `src/components/SyncStatus.tsx` (self, lines 74-81 + 140-189)

**Poll pattern** (lines 74-81 — extend `SyncStatusInfo` interface (lines 5-11) with `pending_count: number`; render aggregate line when > 0):
```tsx
async function pollStatus() {
  try {
    const s: SyncStatusInfo = await invoke("sync_status");
    setStatus({ kind: "synced", status: s });
  } catch {
    setStatus({ kind: "idle" });
  }
}
```

**Render pattern** (lines 146-159 — pending line goes inside the `synced` branch; error text truncates with ellipsis + `title` per UI-SPEC):
```tsx
{status.kind === "synced" && (
  <>
    <p>
      {status.status.last_sync_at
        ? `Em dia · última busca ${status.status.last_sync_at}`
        : "Offline · nunca sincronizado"}
    </p>
    <p className="sub">
      {status.status.message_count}{" "}
      {status.status.message_count === 1 ? "aviso guardado" : "avisos guardados"} para
      estudar offline
    </p>
  </>
)}
```
Copy contract (UI-SPEC): pending `"{N} alteração(ões) aguardando envio"` (hidden when zero; singular `1 alteração aguardando envio`); replay failure `"Não foi possível enviar N alteração(ões) de leitura — tentando de novo na próxima busca."`; offline-queued `"Offline · alterações guardadas — serão enviadas ao reconectar"`. Also mirror `pending_count` into the local `SyncStatusInfo` in `MessageList.tsx` (lines 22-28) and shared `src/types.ts` `SyncStatusInfo` (lines 34-40) if the planner routes row-wash state through it.

## Shared Patterns

### Tauri command shape (all backend commands)
**Source:** `src-tauri/src/commands/sync.rs` lines 34-37, 117-135
```rust
#[tauri::command]
pub async fn start_sync(
    state: State<'_, crate::AppState>,
    on_event: Channel<SyncEvent>,
) -> Result<(), String> {
```
Rules: `#[tauri::command]`, `State<'_, crate::AppState>`, return `Result<_, String>`, blocking DB/IMAP work inside `tauri::async_runtime::spawn_blocking` + `async_std::task::block_on`, errors via `.map_err(|e| format!("...: {e}"))`. Register every new command in `lib.rs` `generate_handler!` (lines 233-247).

### Store access (all DB reads/writes)
**Source:** `src-tauri/src/commands/sync.rs` lines 119-122; `src-tauri/src/sync/worker.rs` lines 53-58
```rust
let guard = store.lock().unwrap();
let conn = guard.conn();
crate::store::queries::sync_status(conn, "INBOX")
    .map_err(|e| format!("store: {e}"))
```
Rules: clone `Arc<Mutex<Store>>` out of state first; hold the lock only for synchronous SQL, never across `.await`; all SQL in `queries.rs` (single-writer invariant).

### Error type + secret hygiene (all IMAP paths)
**Source:** `src-tauri/src/imap/mod.rs` lines 207-242 (`SyncError` + `From` impls); `Transcript::render` lines 393-400; `AccountConfig` Debug lines 158-170
```rust
pub enum SyncError {
    Protocol(String),
    Io(String),
    Parse(String),
    State(String),
}
```
Rules: map async-imap errors with operation prefix (`format!("UID SEARCH ALL: {e}")`); never log password (`***` redaction in Debug + Transcript); extend any new STORE logging with the same redaction.

### FETCH read-only audit (BODY.PEEK invariant — Phase 6 must preserve)
**Source:** `src-tauri/src/imap/headers.rs` lines 21, 277-285, 415-430; `src-tauri/src/imap/mod.rs` lines 327-341
```rust
pub const FETCH_ATTRS: &str = "(UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)";
```
```rust
let mut stream = self
    .uid_fetch(&uid_str, "BODY.PEEK[]")
    .await
    .map_err(|e| SyncError::Protocol(format!("UID FETCH body: {e}")))?;
```
Rules: header sweep attrs contain no `BODY[]` (assertion test at `headers.rs:415-430` is the template for the new PEEK-audit test); `fetch_body` uses `BODY.PEEK[]` only. Audit all four paths: `imap/headers.rs`, `imap/bodies.rs`, `sync/bodies.rs`, `commands/sync.rs fetch_message`.

### Frontend invoke + pt-BR copy (all UI changes)
**Source:** `src/components/MessageList.tsx` lines 33-39 (`isTauriRuntime` guard); `src/types.ts` lines 46-53 (`isUnread`)
```tsx
function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}
```
Rules: guard every `invoke` with `isTauriRuntime()`; classify read/unread only via `isUnread(flags)`; all user copy pt-BR per UI-SPEC Copywriting Contract; typography weights 400/700 only; spacing from `--space-*` tokens; pending wash `--color-accent-soft #ffedd5`; unread dot stays amber `#ea580c`.

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `src-tauri/src/imap/manager.rs` internals (lease guard type, single-flight reconnect) | service | request-response | No session-ownership primitive exists yet — `connect_sync` is per-command one-shot; the lease/mailbox-scope shape is new (RESEARCH Pattern 2 is the spec, planner should follow it literally) |
| `flag_outbox` replay batching policy | utility | batch | Agent's discretion per CONTEXT — RESEARCH recommends one-STORE-per-op; no existing batching code to copy |

## Metadata

**Analog search scope:** `src-tauri/src/{imap,store,sync,commands}`, `src-tauri/src/lib.rs`, `src/components/{MessageList,ReadingPane,SyncStatus}.tsx`, `src/types.ts`
**Files scanned:** 12 (9 primary + `session.rs`, `headers.rs`, `schema.sql` supporting)
**Pattern extraction date:** 2026-10-04
