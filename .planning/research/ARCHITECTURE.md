# ARCHITECTURE — v1.2 Compose & Organize (SMTP Send + IMAP CRUD)

> Research doc for the subsequent milestone. Answers: how SMTP send queue +
> IMAP CREATE/DELETE/RENAME/EXPUNGE/MOVE integrate with the existing
> single-session + outbox + sync architecture. New components vs modified,
> data-flow changes, send-queue durability design, suggested build order.
>
> Source of truth for existing behavior: repo at research time
> (`src-tauri/src/imap/manager.rs`, `imap/mod.rs`, `imap/session.rs`,
> `sync/worker.rs`, `sync/mod.rs`, `store/{mod,queries}.rs`,
> `store/schema.sql`, `commands/sync.rs`, `lib.rs`, `Cargo.toml`).
> PROJECT.md context: stack Rust + Tauri v2 + React + SQLite fixed;
> servers `mail.utfpr.edu.br:993/SSL` + `smtp.utfpr.edu.br:587/STARTTLS`;
> out-of-scope items from M1 (send, non-INBOX folders) are now the v1.2 Active set.

---

## 1. Existing architecture (baseline, what we must not break)

### 1.1 Single-session ownership — `imap/manager.rs`

- `SessionManager` owns **one** authenticated `BoxedSession` per account
  (`account_key = host:port:username`; command layer caches one manager per
  account in `AppState::session_manager`, replaces on account change).
- Access is via **mailbox-scoped leases**: `lease_for(mailbox)` connects
  lazily, (re-)SELECTs when fresh or folder changed, and holds the state
  mutex for the lease lifetime → **single-flight serialization** of all IMAP
  writes. At most one lease exists at a time.
- Write path pattern (canonical — `set_seen_in`): lease → attempt →
  on failure `reconnect()` + fresh lease + **retry exactly once**.
- Read helpers that don't disturb SELECT: `list_mailboxes()` (LIST),
  `mailbox_status()` (STATUS on any folder). Note both currently lease
  INBOX first (`lease_for("INBOX")`) — STATUS works on any mailbox so the
  SELECT is just a session-warmup.
- Password discipline: `AccountConfig` Debug redacts; no log formats secrets.

### 1.2 Transport trait — `imap/mod.rs` (`SyncSession`)

Object-safe trait, real impl on `BoxedSession` (async-imap 0.11), `MockSession`
in tests. Current verbs:

| Verb | Method | Notes |
|---|---|---|
| SELECT | `select_mailbox` / `select_inbox` | per-folder, returns UIDVALIDITY/UIDNEXT/exists |
| SEARCH | `search_uids` | `UID SEARCH ALL` |
| FETCH headers | `fetch_envelopes(range)` | 200-UID batches, `HEADERS::FETCH_ATTRS` |
| FETCH body | `fetch_body(uid)` | **`BODY.PEEK[]` only — never sets \Seen** |
| STORE Seen | `set_seen(uid, seen)` | UID-only, `±FLAGS.SILENT (\Seen)`; **the single write verb** |
| LIST | `list_mailboxes` | FOLD-01 discovery |
| STATUS | `mailbox_status` | FOLD-02 triage (UIDVALIDITY/UIDNEXT/UNSEEN) |
| LOGOUT | `logout` | |

Explicitly **never exposed**: EXPUNGE, APPEND, CREATE/RENAME/DELETE, MOVE/COPY
(doc comment in `mod.rs` § SyncSession). v1.2 lifts exactly this ban.

Parser landmine (must respect): imap-proto 0.16 **cannot parse NAMESPACE**
— any unparseable response permanently poisons the session read side.
Tripwire test `namespace_response_is_unparseable`. New verbs must add
same-style replay/mock coverage, and any new response type (e.g. APPENDUID,
MOVE extended responses) must be verified parseable or handled via
`run_command`-style raw paths that don't depend on the typed parser.

### 1.3 Sync engine — `sync/worker.rs` + `sync/mod.rs`

- `SyncWorker` holds `Arc<Mutex<Store>>`; session injected per call
  (`sync_with_session(Box<dyn SyncSession>, mailbox_name, cb)`). 7-step pass:
  1. ensure mailbox row + read sync state → 2. SELECT (+2b STATUS triage,
  graceful) → 3. UIDVALIDITY guard (wipe + drop outbox on bump) →
  4. `SEARCH ALL` + inconsistency guard (EXISTS>0 but empty SEARCH = hard
  error, never wipe) + empty-mailbox shortcut → 5. header sweep in
  200-UID batches with in-pass range-diff re-fetch + strike counting →
  6. expunge diff (`delete_missing_uids` vs live∪tombstoned) →
  7. write sync state + **post-sync outbox replay** + logout.
- `replay_outbox` (RFC 4549): epoch check first (any op with stale
  `uid_validity` → drop whole mailbox queue, zero STOREs); absent-UID drop
  per op when `live_uids` known; per-op failure → `attempts`+`last_error`,
  stays queued; store lock held only for brief sync sections, never across
  awaits. Replay failure never fails the sync.
- `pending_uids` gate ("pending-wins", FLAG-02): sweep upserts must not
  clobber local optimistic flags for queued UIDs.
- Convergence (Phase 9): skip sweep when server UID set == local, no epoch
  bump, no pending ops; full sweep every `FULL_SWEEP_EVERY=5`; tombstones
  after `TOMBSTONE_STRIKES=3` empty FETCHes; `sweeps_since_full` counter.
- `SyncGate` single-flight (Phase 8): one pass at a time; late ticks skip.
  Poll timer lives in the **UI layer**, backend only exposes the gate.
  Cooperative cancel flag checked between batches.
- `MockSession` is the test seam: deterministic envelopes, call counts,
  `set_seen_calls`, gap/tombstone injection. Every new verb needs mock arms.

### 1.4 Store — `store/{mod.rs,queries.rs,schema.sql}`

- SQLite WAL, `rusqlite_migration`, `SCHEMA_VERSION = 6`
  (v1 baseline + M2 flag_outbox + M3 unseen_count + M4 fetch_tombstones +
  sweeps_since_full + M5 status_synced_at + M6 delimiter).
- **Single-SQL-module invariant**: all SQL in `queries.rs`, no raw SQL
  elsewhere. Tables: `mailboxes` (sync state folded in, one row/folder),
  `messages` keyed `(mailbox_id, uid)`, `message_bodies` (sanitized only),
  `attachment_parts` (metadata only, bytes on disk), `messages_fts` + 2 triggers.
- `flag_outbox(mailbox_id, uid, seen, uid_validity, attempts, last_error,
  UNIQUE(mailbox_id,uid))` — latest-wins collapse; helpers
  `enqueue/list/pending/delete/drop/record_error/count`.
- Lock discipline: `Arc<Mutex<Store>>`; **sync Store mutex never crosses
  `.await`** (brief sync sections only). Async manager mutex *may* cross await.
- `BODY_CACHE_CAP_BYTES = 256 KiB`; attachments under
  `<app_data>/attachments/<uid_validity>/<uid>/`.

### 1.5 Commands layer — `commands/sync.rs` + `lib.rs`

- `start_sync(mailbox, Channel<SyncEvent>)`: loads account (in-memory
  `active_account` first, keyring fallback) → `SyncGate::try_begin` →
  **`connect_sync` fresh session per pass** (does NOT use SessionManager) →
  worker → progress events. ⚠️ architectural wrinkle for v1.2 (see §3.4).
- `set_seen(uid, seen, mailbox?)`: the optimistic+durable template v1.2 must
  copy — (1) local write + enqueue under one store lock, (2) immediate UID
  STORE via manager lease, ack deletes op / failure records error, (3) drain
  rest of queue opportunistically on open session; returns
  `{acked, pending_count, detail}`.
- `fetch_message` / `save_attachment`: direct `connect_sync` + SELECT +
  `BODY.PEEK[]`, parse with mail-parser, sanitize (ammonia), cache
  (bounded). Read-only invariant: never set \Seen.
- `list_mailboxes`: LIST via manager → per-folder STATUS → cache
  (`set_mailbox_status` + `ensure_mailbox` + `set_mailbox_delimiter`) →
  serve cached rows; **offline fallback** serves cache when LIST fails.
- `AppState`: `store`, `active_account` (memory-only), `session_manager`
  cache, `sync_gate`, `sync_cancel`.

---

## 2. v1.2 feature → integration-point map

| # | v1.2 requirement (PROJECT.md Active) | IMAP verbs | SMTP | Integration points |
|---|---|---|---|---|
| 1 | Delete messages (expunge) | `UID STORE +FLAGS (\Deleted)` + `EXPUNGE` (or `UID EXPUNGE`) | — | SyncSession + manager + outbox (new op kind) + sync expunge-diff + UI |
| 2 | Move between folders | `UID COPY` + `STORE \Deleted` + `EXPUNGE`, or `MOVE` (RFC 6851, if advertised) | — | Same as (1) + two-mailbox store bookkeeping + UIDVALIDITY epochs per folder |
| 3 | Save/edit drafts | `APPEND` to Drafts (+ `STORE \Draft` / replace by delete+append) | — | SyncSession APPEND + local drafts table or flag + sync pickup |
| 4 | Folders CREATE/RENAME/DELETE | `CREATE` / `RENAME` / `DELETE` (+ LIST refresh) | — | SyncSession + manager + `mailboxes` rows + tombstone/outbox cleanup on rename |
| 5 | Compose + reply/forward with attachments via SMTP | `APPEND` to Sent (save copy) | **new: SMTP send** `smtp.utfpr.edu.br:587/STARTTLS` | **new `smtp/` module + `send_queue` table + send worker + compose commands + APPEND Sent** |

---

## 3. Design

### 3.1 New trait verbs on `SyncSession` (MODIFIED `imap/mod.rs`)

Add, with real impls on `BoxedSession` + arms on `MockSession`:

```rust
// Folder lifecycle (req 4)
fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>>;
fn rename_mailbox(&mut self, from: &str, to: &str) -> PinBox<'_, Result<(), SyncError>>;
fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>>;
// Message lifecycle (req 1–3)
fn store_deleted(&mut self, uid: u32, deleted: bool) -> PinBox<'_, Result<(), SyncError>>;
fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32> /* or () */, SyncError>>;
fn uid_expunge(&mut self, uids: &[u32]) -> PinBox<'_, Result<(), SyncError>>; // if UIDPLUS
fn copy_message(&mut self, uid: u32, dest: &str) -> PinBox<'_, Result<Option<u32>> /* APPENDUID */, SyncError>>;
fn move_message(&mut self, uid: u32, dest: &str) -> PinBox<'_, Result<(), SyncError>>; // MOVE or COPY+Deleted+EXPUNGE fallback
fn append_message(&mut self, dest: &str, bytes: &[u8], flags: &[&str]) -> PinBox<'_, Result<Option<u32>, SyncError>>;
```

Notes:

- **Capability gating**: probe `CAPABILITY` once per session for
  `MOVE`, `UIDPLUS` (`UID EXPUNGE`), `APPENDUID`. `move_message` prefers
  `MOVE` when advertised, else falls back to `COPY + STORE \Deleted +
  EXPUNGE`. `uid_expunge` only when UIDPLUS; else plain `EXPUNGE`
  (expunges all \Deleted in the selected mailbox — caller must ensure the
  mailbox lease selected the right folder and no foreign \Deleted flags
  are pending, or restrict to the UIDPLUS path).
- **No sequence numbers**: all verbs take UIDs (extend the T-6-01 rule).
- **Wire names are raw modified-UTF-7** (`mailbox.name`, never display_name);
  quote/escape folder names (quoted-string or literal) — a folder named
  `Foo "Bar"` must not break the command line. Add a unit test.
- **Parser tripwire**: APPENDUID/MOVE extended responses go through
  imap-proto 0.16 — extend the `namespace_response_is_unparseable`-style
  tripwire: if the parser chokes, fall back to ignoring the extended data
  (treat destination UID as unknown → next sync reconciles) rather than
  poisoning the session.

### 3.2 SessionManager additions (MODIFIED `imap/manager.rs`)

Follow the `set_seen_in` template exactly (lease → attempt → reconnect +
fresh lease → retry once). New methods, all mailbox-scoped:

- `mark_deleted_in(mailbox, uid, deleted)` — `STORE ±\Deleted`.
- `expunge_in(mailbox)` / `uid_expunge_in(mailbox, uids)`.
- `move_message_in(src, dest, uid)` — internally may SELECT twice (COPY from
  src lease, EXPUNGE on src); holds the single-flight lock throughout so no
  interleaved SELECT can redirect the EXPUNGE. **This is the most
  lease-sensitive op**: plain EXPUNGE acts on the *selected* mailbox, so the
  method must own the lease from SELECT(src) through EXPUNGE without
  yielding it. Signature takes `&self` (not a lease) for this reason.
- `append_to(dest, bytes, flags)` — APPEND needs no SELECT; still goes
  through the manager (single-flight) so an APPEND can't interleave a
  SELECT-sensitive sequence.
- `create_mailbox / rename_mailbox / delete_mailbox` — no SELECT needed;
  single-flight via a lease on INBOX (keeps the "at most one user of the
  session" invariant without disturbing folder state).

No change to the lease struct itself; no second session. SMTP never touches
this manager (separate connection, §3.5).

### 3.3 Store: two new durable queues + folder bookkeeping (MODIFIED `store/`)

**M7 migration** (`SCHEMA_VERSION 7`): new tables. Keep the single-SQL-module
invariant — all SQL in `queries.rs`.

**A. Generalize or second table? — decision: SECOND table (`send_queue`),
keep `flag_outbox` untouched.**

Rationale: lifecycles differ fundamentally. `flag_outbox` rows are tiny
(mailbox_id, uid, bit) with latest-wins collapse and RFC 4549 epoch-drop.
Send ops carry a full MIME payload (KBs–MBs with attachments), need
multi-state lifecycle (queued → sending → sent/failed), per-op retry with
backoff, and must survive *SMTP* failures independently of IMAP epochs.
Unifying them forces nullable payload columns + divergent state machines in
one table. Shared pattern, separate tables.

```sql
-- M7-A: outbound SMTP queue (durability core of req 5)
CREATE TABLE send_queue (
  id            INTEGER PRIMARY KEY,
  created_at    TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
  state         TEXT NOT NULL DEFAULT 'queued',  -- queued|sending|sent|failed
  -- RFC822 payload (fully rendered MIME at enqueue time — see §3.6)
  mime_path     TEXT NOT NULL,                   -- file under <app_data>/outbox/<id>.eml, NOT a blob
  mime_size     INTEGER NOT NULL DEFAULT 0,
  -- envelope (for retry without re-parsing MIME)
  smtp_host     TEXT NOT NULL DEFAULT '',
  smtp_port     INTEGER NOT NULL DEFAULT 587,
  from_addr     TEXT NOT NULL DEFAULT '',
  to_addrs      TEXT NOT NULL DEFAULT '[]',      -- JSON
  cc_addrs      TEXT NOT NULL DEFAULT '[]',
  bcc_addrs     TEXT NOT NULL DEFAULT '[]',      -- envelope-only, never in MIME headers
  subject       TEXT NOT NULL DEFAULT '',
  in_reply_to   TEXT,                            -- threading for replies
  references_hdr TEXT,
  -- associated local draft / source message (nullable)
  draft_id      INTEGER REFERENCES drafts(id) ON DELETE SET NULL,
  -- retry bookkeeping (mirrors flag_outbox attempts/last_error)
  attempts      INTEGER NOT NULL DEFAULT 0,
  last_error    TEXT,
  next_retry_at TEXT,                            -- backoff gate
  sent_at       TEXT
);
CREATE INDEX idx_sendq_state ON send_queue(state, next_retry_at);

-- M7-B: local drafts (req 3 — save/edit without server round-trip)
CREATE TABLE drafts (
  id            INTEGER PRIMARY KEY,
  created_at    TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
  to_addrs      TEXT NOT NULL DEFAULT '[]',
  cc_addrs      TEXT NOT NULL DEFAULT '[]',
  bcc_addrs     TEXT NOT NULL DEFAULT '[]',
  subject       TEXT NOT NULL DEFAULT '',
  body_text     TEXT NOT NULL DEFAULT '',
  in_reply_to   TEXT,                            -- reply/forward context
  attachments   TEXT NOT NULL DEFAULT '[]',      -- JSON [{path,name,mime}] staged files
  server_uid    INTEGER,                         -- UID in Drafts folder after APPEND (nullable until synced)
  dirty         INTEGER NOT NULL DEFAULT 1       -- 1 = local edits newer than server copy
);
```

**B. Delete/move ops: extend the outbox pattern, not the table.**

Two options; recommended: **new `imap_outbox` table** (successor to
`flag_outbox`) rather than overloading it:

```sql
-- M7-C: durable IMAP mutation queue (req 1–2; Seen keeps its table)
CREATE TABLE imap_outbox (
  id            INTEGER PRIMARY KEY,
  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,
  op            TEXT NOT NULL,                   -- 'delete' | 'move'
  dest_mailbox  TEXT,                            -- move target (raw wire name), NULL for delete
  uid_validity  INTEGER NOT NULL,                -- epoch at enqueue (RFC 4549 same rule)
  attempts      INTEGER NOT NULL DEFAULT 0,
  last_error    TEXT,
  created_at    TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (mailbox_id, uid)                       -- latest-wins vs flag_outbox row for same uid
);
CREATE INDEX idx_imap_outbox_mb ON imap_outbox(mailbox_id);
```

Why a second table instead of adding `op` to `flag_outbox`: `flag_outbox`
has a UNIQUE(mailbox_id,uid)+`seen` column with latest-wins on the Seen bit;
a delete/move op for the same UID is a *different intent* that must not
collapse into a Seen toggle (deleting a message with a pending Seen toggle
must execute delete, not flip the bit). Interaction rule: when a
delete/move enqueues for (mailbox,uid), delete the `flag_outbox` row for the
same key (a flag write to a soon-dead message is moot); replay order is
delete/move *after* any surviving flag ops for other UIDs. UIDVALIDITY
epoch-drop and absent-UID-drop apply identically (share the helper logic,
parameterize by table).

Alternative (simpler, acceptable): keep three *separate* op columns in one
new table. Do NOT add `op`/`dest` nullables to `flag_outbox` — it changes
the Phase 6 contract under existing tests.

**C. Folder lifecycle bookkeeping (req 4):**

- No schema change needed for CREATE/DELETE: reuse `mailboxes` rows
  (`ensure_mailbox` on CREATE discovery; `DELETE FROM mailboxes` + cascade
  on DELETE). RENAME = `UPDATE mailboxes SET name=?` **plus** cascade-fix
  any `imap_outbox.dest_mailbox` / queued ops referencing the old wire name,
  and clear `fetch_tombstones` + reset `sweeps_since_full` for the renamed
  row (UIDs are stable across RENAME per RFC 3501 §6.3.5, so message rows
  survive — only the name changes; but strikes reference mailbox_id so they
  survive automatically; still force a full sweep `sweeps_since_full =
  FULL_SWEEP_EVERY` to re-verify flags post-rename).
- `\Noselect` placeholders: never enqueue ops against them (command layer
  validates via cached attributes or fresh LIST).

### 3.4 Sync-worker changes (MODIFIED `sync/worker.rs`)

The sweep itself isIDS read-mostly; mutations replay around it. Changes:

1. **Pre-sweep: replay `imap_outbox` (delete/move) BEFORE the header sweep
   for the mailbox** — a locally-deleted message must not be re-fetched and
   resurrected in the same pass. Order per mailbox pass:
   `SELECT → imap_outbox replay (delete/move) → flag replay (existing) →
   sweep → expunge-diff → sync-state write`.
   Move replay across two folders: COPY to dest (via manager, which handles
   the cross-SELECT internally), on success APPEND-side needs no local write
   (next dest-folder sync fetches it); on src, delete local row immediately
   (optimistic) and drop op on ack.
2. **Expunge-diff reconciliation**: server-side expunges (another client, or
   our own EXPUNGE now visible) already flow through `delete_missing_uids`.
   New: after our own delete-ack, also `clear_tombstone` + delete local row
   (the sweep's Step-6 would do it anyway next pass; immediate delete keeps
   UI snappy — optimistic delete at command time + confirm at sync).
3. **Convergence interaction**: `skip_sweep` currently requires
   `pending.is_empty()` where pending = flag_outbox only. Extend to
   `imap_outbox` depth: any queued delete/move forces a full pass (the UID
   set is about to change). `send_queue` depth does NOT gate the IMAP sweep
   (SMTP and IMAP are independent transports) — except Sent/Drafts folder
   syncs, which should run after a successful send+APPEND (see §3.6).
4. **STATUS/UIDVALIDITY per folder unchanged**; folder DELETE locally drops
   the mailbox row (cascade deletes messages + outbox rows — matches server
   truth after DELETE).
5. **`start_sync` wrinkle**: it currently opens a *fresh* `connect_sync`
   session per pass instead of the `SessionManager`. With EXPUNGE/MOVE in
   play this is a correctness hazard (two sessions: sync sweep SELECTs INBOX
   while a manager lease EXPUNGEs it → `EXPUNGE` on wrong selection or
   `UID EXPUNGE` mismatch). **Fix in v1.2**: route `start_sync` through
   `manager_for().lease_for(mailbox)` (pass the `&mut dyn SyncSession` from
   the lease into `sync_with_session`; logout becomes lease-drop, keep
   explicit logout for server hygiene). This unifies all IMAP under
   single-flight. Precondition for safe delete/move — do it first (§5, step 1).

### 3.5 New: SMTP module — `src-tauri/src/smtp/` (NEW)

New crate surface, parallel to `imap/`:

- `smtp/mod.rs`: `SmtpConfig { host, port, security (STARTTLS/implicit/plain-loopback), username, password (Zeroizing) }`,
  `SmtpError`, `send_raw(envelope, bytes)` via the **`lettre`** crate
  (add `lettre = { version = "0.11", features = ["tokio1-native-tls", "builder"] }`
  — check async-std compat; else drive lettre's sync transport inside
  `spawn_blocking`, consistent with the existing blocking-thread discipline).
- Reuse credential loading: same `load_account_config` shape; SMTP host
  defaults `smtp.` for `mail.` host? No — user-configurable with sensible
  default (`smtp.utfpr.edu.br:587/STARTTLS`), stored alongside IMAP server
  config in keyring (extend `ServerConfig` with `smtp_host/smtp_port/
  smtp_security`, defaulted for backward compat with saved configs).
- Password discipline identical to IMAP (redacted Debug, Zeroizing).
- `MockSmtp` test seam mirroring `MockSession` (record envelopes, inject
  failures) so send-retry logic is unit-tested without a live 587.

### 3.6 Send-queue durability design (NEW, core of req 5)

State machine per `send_queue` row:

```
queued → sending → sent ✓ (terminal; APPEND Sent; delete MIME file)
  │         │
  │         └─→ queued (retryable SMTP/IO error; attempts++, backoff)
  └─→ failed ✗ (terminal-after-N: 5xx permanent, or attempts > MAX_RETRIES=8)
```

Flow (compose → send):

1. **Enqueue (command `queue_send`)**: render full MIME **once** at enqueue
   (lettre `Message::builder`, attachments streamed from staged paths) →
   write bytes to `<app_data>/outbox/<uuid>.eml` → insert `send_queue` row
   (`state=queued`, envelope columns, `mime_path`). Return immediately —
   UI shows "Queued ⏳". MIME-on-disk (not blob) keeps SQLite small and
   lets retries re-read without re-rendering (attachment staging files may
   move; the .eml is self-contained).
2. **Dispatch (command `flush_send_queue` + opportunistic triggers)**:
   - Trigger points: after enqueue, after `start_sync` success, manual
     "Retry" button, app start (flush on launch — covers mail queued while
     offline). Single-flight via the existing `SyncGate`? No — **separate
     `SendGate`** (SMTP and IMAP are independent; an IMAP sweep must not
     block an SMTP flush). Same try_begin/skip semantics.
   - Worker loop: `SELECT ... WHERE state='queued' AND next_retry_at <= now
     ORDER BY id` → mark `sending` → `smtp.send_raw` → on success: mark
     `sent`, **APPEND copy to Sent** (via manager `append_to("Sent", mime,
     ["\\Seen"])` — sent mail is read), delete .eml, emit event; on
     retryable error: `attempts++`, `next_retry_at = now + min(2^attempts
     min, 30 min)`, state back to `queued`; on permanent (5xx / auth):
     `state=failed` with `last_error` surfaced in UI.
   - Crash-safety: rows left in `sending` at startup (crash mid-SMTP) are
     reset to `queued` on launch (`UPDATE send_queue SET state='queued'
     WHERE state='sending'`) — at-least-once delivery; duplicates possible
     if SMTP accepted but process died before marking sent. Mitigate with
     `Message-ID` header rendered at enqueue (server-side dedup is
     best-effort; document the at-least-once contract in UI copy
     "may duplicate on crash during send").
3. **Reply/forward**: `in_reply_to`/`references_hdr` set from the source
   message; body quoted (text) at compose time in frontend; attachments
   re-staged. No IMAP `ANSWERED` flag write required (optional follow-up:
   `STORE +\Answered` on the source UID via the flag path — trivially fits
   `flag_outbox` if desired, but keep out of v1.2 scope unless cheap).
4. **Offline**: enqueue works with zero connectivity (pure local write);
   flush fails fast with recorded error, backoff applies. `sync_status`
   gains `send_pending_count` for the UI badge.
5. **BCC correctness**: envelope recipients = to+cc+bcc; MIME headers contain
   only to/cc. Worker sends from envelope columns, never re-parses MIME.
   Unit test: bcc address receives but is absent from MIME bytes.

### 3.7 Drafts flow (req 3)

- Local-first: `drafts` table is the editor backing store (autosave =
  UPDATE row, `dirty=1`). No IMAP traffic on keystroke.
- **Server persistence**: on explicit "Save" (or autosave-debounced, TBD):
  `append_to("Drafts", mime, ["\Draft"])` → store returned UID in
  `drafts.server_uid`, `dirty=0`. Edit-after-save = APPEND new + DELETE old
  server copy (IMAP has no in-place replace; `server_uid` retargets) +
  local UPDATE. Sync sweep picks up Drafts like any folder (per-folder sync
  already exists from Phase 7) — foreign draft edits converge naturally.
- Deleting a draft: local DELETE + if `server_uid` present, enqueue
  `imap_outbox` delete op for Drafts folder.

### 3.8 Data-flow changes (before → after)

**Before (v1.1)** — all flows read-only except Seen:
UI → Tauri command → (optimistic local write + outbox enqueue) →
manager lease → UID STORE → ack → dequeue; sync sweep reconciles.

**After (v1.2)** — three durable lanes sharing the store, serialized per
transport:

```
Compose UI ──queue_send──▶ send_queue + .eml file ──flush──▶ SMTP ──ok──▶ APPEND Sent ──▶ sync(Sent)
Drafts UI ──save_draft──▶ drafts row ──save──▶ APPEND Drafts ──▶ sync(Drafts)
List UI ──delete/move──▶ imap_outbox (+optimistic local delete) ──replay──▶ STORE+EXPUNGE/COPY ──▶ sync
Folders UI ──create/rename/delete──▶ manager ──▶ LIST refresh ──▶ mailboxes cache ──▶ sidebar
Sync pass ──manager lease (was: fresh connect)──▶ SELECT ──▶ imap_outbox replay ──▶ flag replay ──▶ sweep
```

New Tauri commands (all following the `set_seen` optimistic+durable template):
`queue_send`, `flush_send_queue` (`send_status`), `save_draft`,
`delete_draft`, `delete_messages`, `move_messages`, `create_folder`,
`rename_folder`, `delete_folder`. New events on the existing Channel pattern:
`SendQueued/SendProgress/SendCompleted/SendFailed`, `FolderTreeChanged`.

---

## 4. New vs modified — explicit file list

### NEW files

| File | Purpose |
|---|---|
| `src-tauri/src/smtp/mod.rs` | SMTP transport (lettre), `SmtpConfig`, `send_raw`, `MockSmtp`, capability/auth errors |
| `src-tauri/src/commands/send.rs` | `queue_send`, `flush_send_queue`, `send_status`, `retry_send` |
| `src-tauri/src/commands/organize.rs` | `delete_messages`, `move_messages`, `create_folder`, `rename_folder`, `delete_folder`, `save_draft`, `delete_draft` |
| Frontend `Compose*.tsx`, `Drafts*.tsx`, outbox badge, folder-tree context menu | Compose/reply/forward UI, queued/failed states, folder CRUD UI |
| `.eml` files under `<app_data>/outbox/` | Durable MIME payloads (outside SQLite by design) |

New deps: `lettre` (SMTP). No new SQLite features (WAL + migrations cover it).

### MODIFIED files

| File | Change | Risk |
|---|---|---|
| `imap/mod.rs` | +7 trait verbs (§3.1), capability helpers, tripwire test for new response types | Medium — trait change touches MockSession + all impls |
| `imap/manager.rs` | +8 lease methods w/ reconnect-retry (§3.2); MOVE holds lease across double-SELECT | Medium — lease-holding logic is the correctness core |
| `sync/worker.rs` | imap_outbox pre-sweep replay, pending-gate extension, tombstone clear on self-delete | Medium — order matters; keep replay-failure-never-fails-sync |
| `sync/mod.rs` | `SendGate` (clone of SyncGate) + send events | Low |
| `commands/sync.rs` | `start_sync` via manager lease (§3.4); `sync_status` += `send_pending_count` + `imap_outbox` depth | Medium — the manager-lease cutover |
| `store/mod.rs` | M7 (+ maybe M8) migrations, `SCHEMA_VERSION` 7, forward-upgrade tests | Low-Medium (follow M2–M6 pattern) |
| `store/queries.rs` | send_queue + drafts + imap_outbox helpers (single-SQL-module invariant) | Medium — largest diff, but mechanical |
| `lib.rs` | `smtp` module, `SendGate` state, new commands in `invoke_handler`, SMTP fields in `ServerConfig`/`ActiveAccount` | Low |
| `creds.rs` | SMTP host/port/security in keyring server config (defaulted) | Low |
| `Cargo.toml` | `lettre` pin | Low |

### UNTOUCHED (must stay invariant)

- `BODY.PEEK[]` read path, ammonia sanitize, `BODY_CACHE_CAP_BYTES`.
- `flag_outbox` schema + RFC 4549 replay semantics (extended by analogy, not edited).
- Convergence/tombstone/sweep-counter logic (only the pending-gate input widens).
- `SyncGate` semantics; transcript secret discipline; `normalize_host`/loopback/cert-refusal rules (SMTP reuses them).

---

## 5. Suggested build order (dependency-ordered, each step shippable + tested)

1. **Unify sync under SessionManager** (§3.4 fix). Precondition for every
   destructive op. Tests: existing sweep suite green against lease-injected
   session; single-flight contention test (sync vs flag-STORE overlap).
2. **IMAP verbs + manager methods + mocks** (§3.1–3.2, no UI). Trait verbs,
   BoxedSession impls, MockSession arms, reconnect-retry tests, folder-name
   quoting test, parser tripwire. No store/commands yet — pure transport.
3. **`imap_outbox` + worker replay + delete/move commands** (req 1–2 vertical
   slice). M7-C migration, queries, pre-sweep replay order, optimistic local
   delete, `delete_messages`/`move_messages` commands, list UI wiring.
   Verify: offline delete → reconnect → server expunged; move lands in dest.
4. **Folder CRUD** (req 4). CREATE/RENAME/DELETE verbs already exist from
   step 2; add commands + `mailboxes` bookkeeping + LIST refresh + sidebar
   tree UI. Verify: rename preserves messages (same UIDs), delete cascades.
5. **Drafts local + APPEND** (req 3). M7-B, editor backed by `drafts`,
   save→APPEND→`server_uid`, edit=resend+delete-old. Verify: draft survives
   restart offline; server copy appears in Drafts.
6. **SMTP module + send_queue + flush worker** (req 5 core). `lettre`
   transport, M7-A, `queue_send`/`flush_send_queue`, backoff, crash-recovery
   reset, APPEND-to-Sent on success. MockSmtp unit suite (retry, permanent
   fail, bcc, crash-recovery). Live-verify against `smtp.utfpr.edu.br:587`.
7. **Compose/reply/forward UI + attachments** (req 5 surface). MIME render,
   staged attachments, threading headers, outbox badge, failed-retry UX.
   End-to-end: compose offline → online flush → arrives + Sent copy.
8. **Polish + audit**: `sync_status` pending counts, folder-tree optimistic
   states, FTS over Sent/Drafts, migration forward-upgrade tests (v6→v7
   preserving rows), full `cargo test` + live round-trip checklist.

Why this order: transport before queue (can't replay what can't be spoken);
unified session before destructive verbs (EXPUNGE on a stray session is data
loss); delete/move before drafts/send (they exercise the outbox-replay
machinery the send flow imitates); SMTP last among backends (independent
transport, longest live-verify tail); UI last per layer (commands testable
headless via store+mock).

---

## 6. Risks & open questions

1. **EXPUNGE blast radius**: plain `EXPUNGE` removes *all* \Deleted in the
   mailbox, including flags set by other clients. Prefer `UID EXPUNGE`
   when UIDPLUS advertised; else document "delete = expunge all deleted in
   folder" and consider selecting with a guard (re-SEARCH \Deleted set first,
   warn if foreign deleted UIDs exist). Decision needed before step 3.
2. **MOVE support on `mail.utfpr.edu.br`**: unknown until CAPABILITY probed.
   The COPY-fallback must be ready day one (assume no MOVE).
3. **APPENDUID absence**: without UIDPLUS, dest UID after COPY/MOVE is
   unknown until next dest-folder sync — UI must tolerate "moved (locating…)"
   states. Tombstone/pending machinery already models this shape.
4. **Lettre runtime fit**: project drives async-imap on async-std inside
   `spawn_blocking`; lettre's async story is tokio-centric. Simplest fit:
   lettre sync transport inside `spawn_blocking` (matches existing discipline,
   no runtime mixing). Confirm during step 6 spike.
5. **`start_sync` cutover regression**: lease-based sync changes logout
   semantics (lease-drop vs explicit LOGOUT) and error paths. Keep explicit
   `logout()` on the leased session at pass end; run the full Phase 9 suite.
6. **At-least-once send**: crash between SMTP-accept and `sent` marking
   duplicates. Message-ID-at-enqueue + UI copy noting the contract; no
   two-phase commit available across SMTP+SQLite.
7. **Attachment size**: .eml on disk unbounded by `BODY_CACHE_CAP_BYTES`
   (that's the *download* cache cap). Add a *send* cap (e.g. 25 MB, matching
   common server limits) with a friendly composer error — prevents
   queueing mail the server will always 5xx-reject into `failed`.

---

*Written 2026-10-06 from repo read (v1.1 shipped state, SCHEMA_VERSION 6).
Next step: turn §5 into phased GSD plans for the v1.2 milestone.*
