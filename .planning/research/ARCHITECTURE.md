# Architecture Research

**Domain:** Desktop IMAP email client — Rust (Tauri v2 backend) + React frontend + SQLite local cache, Linux-only M1
**Researched:** 2026-10-03
**Confidence:** HIGH (Tauri v2 bridge semantics from official docs; IMAP sync rules from RFC 3501 + imap/async-imap crate docs; mail parsing from mail-parser/stalwart docs; frontend offline-first from TanStack ecosystem docs)

## Standard Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    React Frontend (WebView)                  │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐  │
│  │ Sidebar │  │ MsgList │  │ Reader  │  │ SyncStatusBar │  │
│  │ (static │  │(virtual-│  │(sanitized│  │(progress via  │  │
│  │ M1 INBOX│  │ ized)   │  │  HTML)  │  │ Channel)      │  │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────────┬────────┘  │
│       │ invoke()   │ invoke()   │ invoke()       │ onmessage │
├───────┴────────────┴────────────┴────────────────┴───────────┤
│              Tauri IPC Bridge (invoke ↔ commands)             │
│   commands (request/response) │ Channel (sync progress stream)│
├──────────────────────────────────────────────────────────────┤
│                  Rust Backend (src-tauri)                     │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  SyncWorker (tokio task, owns IMAP session; emits   │    │
│  │  SyncProgress via tauri::ipc::Channel, NOT events)  │    │
│  └──────────────────┬──────────────────────────────────┘    │
│  ┌──────────┐  ┌────┴─────┐  ┌──────────┐  ┌────────────┐    │
│  │ ImapConn │  │ MailStore│  │ BodyFetch│  │ Credential │    │
│  │ (connect │  │ (rusqlite│  │ (on-     │  │ Vault      │    │
│  │ +SELECT) │  │ +WAL)    │  │ demand)  │  │ (keyring)  │    │
│  └──────────┘  └──────────┘  └──────────┘  └────────────┘    │
├──────────────────────────────────────────────────────────────┤
│                    Local Persistence                          │
│  ┌──────────┐  ┌──────────┐  ┌────────────────────────────┐  │
│  │ SQLite   │  │ OS       │  │ FS attachment blob dir     │  │
│  │ (WAL msg │  │ keyring  │  │ (~/.local/share/sge/att/)  │  │
│  │ cache)   │  │ (secret) │  │                            │  │
│  └──────────┘  └──────────┘  └────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| SyncWorker | Owns the single IMAP session; runs header sweep + incremental poll; streams progress; writes to SQLite | `tokio::spawn` task in Tauri `State<Mutex<SyncHandle>>`; `Channel<SyncEvent>` param on `start_sync` command; cancellable via `CancellationToken` |
| ImapConn | TLS/STARTTLS connect, LOGIN, SELECT INBOX, FETCH, LOGOUT; reconnect with backoff | `async-imap` + `async-native-tls` (or `tokio-rustls`); connection timeout 10s, read timeout 30s; one session per worker, never pooled across threads |
| MailStore | All SQLite access; schema migrations; UIDVALIDITY-guarded upserts; FTS index | `rusqlite` bundled SQLite, WAL mode, single writer behind `Mutex<Connection>` in Tauri managed state |
| BodyFetch | On-demand `UID FETCH BODY[]` / `BODYSTRUCTURE` for selected message; attachment part fetch + save to disk | Tauri command `fetch_body(uid)` → cache in `message_bodies`; `save_attachment` streams bytes via `Channel` for large parts |
| CredentialVault | Store/retrieve IMAP password in OS keyring; never in SQLite or logs | `keyring` crate (Secret Service on Linux); service=`sge`, account=username |
| React query layer | All reads go to Rust via `invoke`, cached in TanStack Query; sync progress invalidates queries | `@tanstack/react-query` + `invoke`; `Channel` callback calls `queryClient.invalidateQueries(['messages'])` |
| Reader sanitizer | Render HTML safely: sanitize in Rust, render in iframe/sandbox in React | Rust `ammonia` allowlist sanitize before storing/display; React renders via `srcDoc` in sandboxed `iframe` (`sandbox=""`) |
| MIME parser | Parse headers, multipart bodies, attachment metadata from raw RFC822 | `mail-parser` crate (stalwartlabs, 100% safe Rust, no deps) for headers + body parts |

## Recommended Project Structure

```
sge/
├── src/                          # React frontend
│   ├── components/
│   │   ├── Sidebar.tsx           # M1: static INBOX entry + account status
│   │   ├── MessageList.tsx       # virtualized list, selection model
│   │   ├── ReaderPane.tsx        # sanitized HTML via sandboxed iframe
│   │   └── SyncStatus.tsx        # progress bar bound to Channel events
│   ├── hooks/
│   │   ├── useMessages.ts        # TanStack Query: list_messages(uid range/search)
│   │   ├── useMessageBody.ts     # TanStack Query: fetch_body(uid), on-demand
│   │   └── useSync.ts            # invoke start_sync + Channel wiring
│   ├── lib/
│   │   └── tauri.ts              # typed invoke wrappers + SyncEvent types
│   └── App.tsx                   # three-pane shell layout
├── src-tauri/
│   ├── src/
│   │   ├── main.rs               # Tauri builder, plugin registration
│   │   ├── lib.rs                # command registration, managed State
│   │   ├── commands/             # thin invoke adapters (no IMAP logic here)
│   │   │   ├── auth.rs           # login, logout, stored-credential check
│   │   │   ├── sync.rs           # start_sync(Channel), sync_status, cancel
│   │   │   ├── messages.rs       # list_messages, search, fetch_body
│   │   │   └── attachments.rs    # list_parts, save_attachment
│   │   ├── imap/
│   │   │   ├── session.rs        # connect + SELECT, TLS/STARTTLS modes
│   │   │   ├── headers.rs        # header sweep FETCH logic
│   │   │   └── bodies.rs         # on-demand BODY[] / BODYSTRUCTURE fetch
│   │   ├── store/
│   │   │   ├── mod.rs            # Connection setup, WAL, migrations
│   │   │   ├── schema.sql        # canonical DDL (see sketch below)
│   │   │   └── queries.rs        # all SQL in one reviewable module
│   │   ├── sanitize.rs           # ammonia pipeline for HTML bodies
│   │   └── creds.rs              # keyring get/set/delete
│   ├── Cargo.toml
│   └── tauri.conf.json           # Linux bundle targets (deb/appimage)
└── .planning/research/           # this research
```

### Structure Rationale

- **commands/ stays thin:** every command is argument validation + delegate to `imap/` or `store/`. This is the testing seam — unit-test `imap/` and `store/` without Tauri, integration-test commands with a fake store trait.
- **store/queries.rs centralizes SQL:** all queries in one file means the UIDVALIDITY guard and FTS triggers get reviewed once, not scattered.
- **imap/ split by fetch type:** headers sweep vs body fetch have different performance shapes (batched small FETCH vs single large FETCH); separating them keeps timeout/retry policy per operation.
- **Frontend mirrors backend boundaries:** `hooks/useMessages` ↔ `commands/messages.rs`, `hooks/useSync` ↔ `commands/sync.rs`. One hook per command module prevents prop-drilling sync state through the pane tree.

## Architectural Patterns

### Pattern 1: Command for request/response, Channel for sync progress

**What:** Tauri commands (`invoke`) return a single value — use them for login, list, search, fetch_body. For the header sweep, pass a `tauri::ipc::Channel<SyncEvent>` argument; the Rust worker calls `channel.send(...)` per batch and the frontend's `onmessage` updates progress + invalidates queries.
**When to use:** Any operation longer than ~200ms or emitting N progress updates (sync sweep, large attachment download).
**Trade-offs:** Channels are ordered and typed (better than `emit` events, which are untyped JSON strings and unsuitable for high-throughput per official docs). Cost: caller must construct a `Channel` per invocation; don't reuse one channel across syncs.

**Example:**
```rust
// src-tauri/src/commands/sync.rs
#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
enum SyncEvent { Started { total: u32 }, Progress { fetched: u32, total: u32 }, Finished { added: u32 }, Failed { reason: String } }

#[tauri::command]
async fn start_sync(state: State<'_, AppState>, on_event: Channel<SyncEvent>) -> Result<(), String> {
    let worker = SyncWorker::new(state.inner().clone(), on_event);
    worker.run_incremental().await.map_err(|e| e.to_string())
}
```
```typescript
// src/hooks/useSync.ts
import { invoke, Channel } from '@tauri-apps/api/core';
const onEvent = new Channel<SyncEvent>();
onEvent.onmessage = (m) => {
  if (m.event === 'progress') setProgress(m.data);
  if (m.event === 'finished') queryClient.invalidateQueries({ queryKey: ['messages'] });
};
await invoke('start_sync', { onEvent });
```

### Pattern 2: UIDVALIDITY-guarded incremental sync with sync_state table

**What:** Persist `(mailbox, uid_validity, uid_next, highest_modseq)` per mailbox. Every sync compares stored UIDVALIDITY against SELECT response; mismatch → full resync (delete + re-sweep). Match → fetch only UIDs ≥ stored `uid_next`. Bodies never fetched in the sweep.
**When to use:** Always — this is the core sync algorithm for M1 and the multi-folder future.
**Trade-offs:** UID-based (not sequence numbers) survives expunges. Cost: must handle UIDVALIDITY resets explicitly or users see duplicated/stale mail; must store ENVELOPE+FLAGS (not full RFC822) in sweep to keep it fast.

**Example (algorithm steps — quality-gate normative):**
```
1. CONNECT with 10s timeout → LOGIN → SELECT INBOX
   → server returns (uid_validity, uid_next, highest_modseq?, exists)
2. READ sync_state WHERE mailbox='INBOX'
3. IF no row OR stored.uid_validity != server.uid_validity:
     DELETE FROM messages WHERE mailbox_id=INBOX
     DELETE FROM message_bodies for those uids
     SET fetch_from = 1, full_resync = true
   ELSE:
     SET fetch_from = stored.uid_next   // incremental window
     IF fetch_from >= server.uid_next: nothing new → still run expunge check (step 5), then DONE
4. HEADER SWEEP in batches of 200 UIDs:
     FETCH <batch> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)
     // BODY.PEEK[HEADER] equivalent: never set \Seen — read-only M1 must not mutate flags
     FOR each msg: parse with mail-parser → UPSERT messages row → FTS index update
     SEND Channel Progress { fetched, total } per batch
     COMMIT per batch (keeps UI responsive, bounds WAL)
5. EXPUNGE CHECK (cheap, every sync): FETCH 1:* (UID) or UID SEARCH ALL → diff UID set
   vs local → DELETE missing rows (server-side deletes)
6. WRITE sync_state SET uid_validity=server.uid_validity, uid_next=server.uid_next,
   highest_modseq=server.highest_modseq, last_sync=now()
7. SEND Channel Finished { added } → frontend invalidates ['messages']
```

On-demand body fetch (separate path, not part of sweep):
```
ON select message uid:
  IF message_bodies.body_complete for uid → render from SQLite (offline OK)
  ELSE FETCH uid (BODY[] + BODYSTRUCTURE) → parse parts via mail-parser
       → sanitize text/html via ammonia → INSERT message_bodies
       → INSERT attachment_parts rows → render
```

### Pattern 3: SQLite as the single source of truth; React never talks IMAP

**What:** React renders exclusively from SQLite-via-commands. IMAP is write-into-store only. TanStack Query caches command results with `staleTime` ~30s; sync completion invalidates. Offline = queries still resolve from cache/store; `start_sync` failure surfaces as banner, not empty list.
**When to use:** Entire M1; extends to multi-folder later by adding mailbox_id filter to the same queries.
**Trade-offs:** Pro: offline-capable after first sync (explicit requirement), fast list paint, trivially testable UI with seeded DB. Con: search must be implemented in SQLite FTS (not server SEARCH) — acceptable for M1 INBOX scope.

**Example:**
```typescript
// src/hooks/useMessages.ts
export function useMessages(search: string) {
  return useQuery({
    queryKey: ['messages', search],
    queryFn: () => invoke<MessageRow[]>('list_messages', { mailbox: 'INBOX', search, limit: 200 }),
    staleTime: 30_000, placeholderData: keepPreviousData,
  });
}
```

### Pattern 4: Rust-side HTML sanitization pipeline

**What:** Raw `text/html` MIME part → `mail-parser` decode (charset/base64/quoted-printable) → `ammonia` allowlist sanitize (no `<script>`, no `on*`, no `javascript:` URLs; allow `a[href]`, `img[src=cid/http]`, basic tables) → store sanitized HTML in `message_bodies.body_html` → React renders in `<iframe sandbox="" srcDoc>`.
**When to use:** Every HTML body before storage, never sanitize only at render time (stored copy must already be safe so FTS preview and exports can't leak unsanitized content).
**Trade-offs:** Double protection (Rust sanitize + iframe sandbox) costs one extra render hop but closes the #1 desktop-mail XSS vector: remote content + credential-bearing WebView.

## Concrete SQLite Table Sketch (normative for roadmap Phase: local store)

```sql
-- WAL for concurrent reader (UI) + writer (sync worker)
PRAGMA journal_mode = WAL;

CREATE TABLE mailboxes (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,          -- 'INBOX' (M1); others later
  uid_validity  INTEGER NOT NULL DEFAULT 0,
  uid_next      INTEGER NOT NULL DEFAULT 0,
  highest_modseq INTEGER,                      -- NULL if server lacks CONDSTORE
  last_sync_at  TEXT                           -- ISO8601 UTC
);
-- sync_state folds into mailboxes (one row per folder); no separate table needed at M1 scale.

CREATE TABLE messages (
  id            INTEGER PRIMARY KEY,
  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,              -- IMAP UID, unique per mailbox
  message_id    TEXT,                          -- RFC822 Message-ID (nullable, not trusted as key)
  subject       TEXT NOT NULL DEFAULT '',
  from_addr     TEXT NOT NULL DEFAULT '',
  to_addrs      TEXT NOT NULL DEFAULT '',      -- JSON array (M1-simple; normalize later)
  cc_addrs      TEXT NOT NULL DEFAULT '[]',
  date_utc      TEXT NOT NULL,                 -- INTERNALDATE preferred over Date: header
  flags         TEXT NOT NULL DEFAULT '[]',    -- JSON: Seen, Flagged, Answered...
  has_attachments INTEGER NOT NULL DEFAULT 0,  -- from BODYSTRUCTURE, drives paperclip icon
  preview       TEXT NOT NULL DEFAULT '',      -- first ~200 chars of text/plain, for list + FTS
  UNIQUE (mailbox_id, uid)
);
CREATE INDEX idx_messages_mailbox_uid ON messages(mailbox_id, uid);
CREATE INDEX idx_messages_date ON messages(mailbox_id, date_utc DESC);

CREATE TABLE message_bodies (
  message_id    INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  body_text     TEXT,                          -- text/plain decoded
  body_html     TEXT,                          -- SANITIZED html only (never raw)
  body_complete INTEGER NOT NULL DEFAULT 0,    -- 1 when fetched; 0 = header-only row
  fetched_at    TEXT
);

CREATE TABLE attachment_parts (
  id            INTEGER PRIMARY KEY,
  message_id    INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  part_number   TEXT NOT NULL,                 -- MIME part path e.g. '2', '1.2'
  filename      TEXT NOT NULL DEFAULT '',
  mime_type     TEXT NOT NULL DEFAULT 'application/octet-stream',
  size_bytes    INTEGER NOT NULL DEFAULT 0,
  local_path    TEXT,                          -- set after save_attachment; NULL = not downloaded
  UNIQUE (message_id, part_number)
);

-- Full-text search over cached headers+preview (M1 local search requirement)
CREATE VIRTUAL TABLE messages_fts USING fts5(subject, from_addr, preview,
  content='messages', content_rowid='id');
CREATE TRIGGER msg_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_addr, preview)
  VALUES (new.id, new.subject, new.from_addr, new.preview);
END;
CREATE TRIGGER msg_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, preview)
  VALUES ('delete', old.id, old.subject, old.from_addr, old.preview);
END;
```

Sizing note: headers ~1–2KB/row → 50k-message UTFPR INBOX ≈ 100MB SQLite, fine for local. Bodies/attachments excluded from DB growth except fetched-on-demand rows; attachment bytes live on disk, only metadata in `attachment_parts`.

## Data Flow

### Request Flow

```
[Select message in list]
    ↓
[ReaderPane] → invoke('fetch_body', {uid}) → [commands/messages.rs] → [store: SELECT body_complete?]
    ↓ HIT                                              ↓ MISS
[render from SQLite]                    [imap/bodies.rs: UID FETCH] → parse → sanitize → INSERT
                                                        ↓
                                          [return row] → TanStack caches → render
```

### State Management

```
[SQLite] ──invoke list/search──▶ [TanStack Query cache] ──subscribe──▶ [Sidebar/List/Reader]
    ▲                                    │
    │ Channel Finished → invalidate ─────┘
[SyncWorker writes batches; UI never writes IMAP state in M1]
```

### Key Data Flows

1. **Login → first sweep:** `login` stores creds in keyring → `start_sync(Channel)` full sweep (no sync_state row) → batched progress → list paints incrementally via per-batch invalidation (throttle: invalidate at most 1×/500ms).
2. **Incremental poll (app focus / 5-min timer):** `start_sync` with stored `uid_next` → typically 0–few rows → cheap; expunge diff keeps deletes correct.
3. **Body on demand:** selection → `fetch_body` → cache-or-fetch → sanitized render; attachment click → `save_attachment` (dialog path from frontend) → stream bytes via Channel for >1MB parts.

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| M1: single INBOX, ~10–50k msgs | This design as-is. Batch 200, per-batch commit, FTS5. No changes needed. |
| Multi-folder (post-M1) | Add rows to `mailboxes`; sync loops folders; `mailbox_id` filter already in schema. SyncWorker gains per-folder Channel events. |
| 100k+ msgs / huge attachments | Paginate `list_messages` with cursor (`date_utc, uid` keyset, not OFFSET); cap FTS `preview` length; attachment streaming already via Channel; consider `VACUUM INTO` backup. |

### Scaling Priorities

1. **First bottleneck:** header sweep latency on slow university IMAP (mail.utfpr.edu.br). Fix: batch size 100–200, per-batch commit + progressive render, 30s read timeout with 1 retry, then surface partial results (never block list on full sweep).
2. **Second bottleneck:** FTS index write amplification during full resync. Fix: wrap full resync in single transaction with `defer_foreign_keys`, rebuild FTS once at end instead of per-row triggers for the initial load path.

## Anti-Patterns

### Anti-Pattern 1: Sequence-number-based sync

**What people do:** `FETCH 1:*` by sequence number and assume stability.
**Why it's wrong:** Sequence numbers shift on every expunge; UIDVALIDITY+UID is the only stable identity (RFC 3501 §2.3.1.1). Sequence-based caches silently show wrong messages after deletes.
**Do this instead:** Key everything on `(mailbox_id, uid)`; use sequence numbers only transiently inside one SELECT session.

### Anti-Pattern 2: IMAP logic inside Tauri commands / frontend-driven FETCH loops

**What people do:** Frontend calls `fetch_batch` in a loop, or each command opens its own IMAP connection.
**Why it's wrong:** N connections → server throttling/lockouts; UI thread orchestrates retries; impossible to cancel cleanly; credentials cross the bridge repeatedly.
**Do this instead:** One SyncWorker owns one session; commands are thin; cancellation via token; frontend only observes via Channel.

### Anti-Pattern 3: Rendering unsanitized HTML / `dangerouslySetInnerHTML` on raw parts

**What people do:** Display `BODY[TEXT]` directly for fidelity.
**Why it's wrong:** Tracking pixels, `javascript:` URIs, and CSS exfiltration run in the same WebView that holds the keyring-backed session — stored XSS with credential access.
**Do this instead:** `ammonia` in Rust before storage + sandboxed `iframe srcDoc` at render. Block remote images by default (M1-safe choice; add per-sender allowlist post-M1).

### Anti-Pattern 4: Storing credentials in SQLite / Tauri Store plugin

**What people do:** Persist password next to mail for convenience.
**Why it's wrong:** DB file is backed up/copied; plaintext secret violates the project's explicit security constraint.
**Do this instead:** `keyring` crate only; SQLite holds `account_username`, never the secret.

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| IMAP server (mail.utfpr.edu.br) | `async-imap` over TLS (`async-native-tls`); STARTTLS vs SSL/TLS user-configurable per PROJECT.md | University servers often have slow/old TLS: allow port 993 SSL + 143 STARTTLS; 10s connect / 30s read timeouts; single session; never `STORE` flags in M1 (read-only) |
| OS keyring (Secret Service) | `keyring` crate; service `sge` | Linux Secret Service may be locked headless — surface clear error, don't fall back to plaintext |
| Linux bundle | Tauri v2 bundler: `.deb` + `.AppImage` | Sign nothing for M1; test install on clean VM (missing webkit deps is the classic Linux failure) |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| React ↔ Rust | `invoke` (typed commands) + `Channel<SyncEvent>` (progress) | Commands return `Result<T, String>`; frontend maps Err to toast. No `emit` for progress (untyped, unordered). |
| SyncWorker ↔ MailStore | `Mutex<rusqlite::Connection>` via Tauri `State` | Single writer; readers take short locks; WAL lets list queries run during sweep. Testing seam: `Store` trait with in-memory SQLite impl. |
| MIME parse ↔ sanitize | `mail-parser` output → `ammonia::clean` → store | Sanitize-before-store invariant enforced in `store/queries.rs` insert path (can't bypass by calling parse directly). |

## Phase-Shaped Component Boundaries (for downstream roadmap)

| Roadmap phase | Owns | Depends on | Done when |
|---------------|------|------------|-----------|
| 1. Backend sync core | `imap/session.rs`, `headers.rs`, sync algorithm, timeouts/reconnect | Nothing (mock FETCH against local fixture RFC822 files) | Incremental algorithm steps 1–7 pass against a fake IMAP transcript; UIDVALIDITY-reset test passes |
| 2. Local store | `schema.sql`, `queries.rs`, FTS triggers, MailStore state | Phase 1 types (MessageRow) | Header sweep persists 10k fixture rows; `list/search` <50ms; resync test green |
| 3. UI shell | Three-pane layout, virtualized list, TanStack hooks, sync progress bar | Phase 2 commands | List paints from seeded DB offline; progress bar reflects Channel events |
| 4. Reader + attachments | `bodies.rs`, sanitize pipeline, iframe reader, save_attachment | Phases 2–3 | HTML renders sanitized (XSS fixture test); attachment downloads byte-identical |
| 5. Auth + packaging | keyring vault, auto-login, Linux bundles | Phases 1–4 | Cold start auto-logs-in; `.deb` installs on clean VM |

Order rationale: sync-core → store → shell → reader → packaging is dependency order; each phase is demoable (CLI sweep → sqlite3 inspect → offline UI → full read → installed app).

## Sources

- Tauri v2 official docs — Calling the Frontend from Rust (Events vs Channels guidance; Channel ordered/typed recommendation): https://v2.tauri.app/develop/calling-frontend/
- Tauri v2 official docs — Calling Rust from the Frontend (async commands preferred): https://v2.tauri.app/develop/calling-rust/
- `imap` / `async-imap` crate docs — Session, UID/UIDNEXT/UIDVALIDITY semantics (docs.rs/imap, lib.rs/crates/async-imap)
- RFC 3501 §2.3.1.1 + nickb.dev Introduction to IMAP — UIDVALIDITY/UID immutability contract; BODY.PEEK non-mutating fetch
- stalwartlabs `mail-parser` (crates.io / context7) — header/MIME parsing choice
- TanStack DB / Query offline-first docs (tanstack.com, powersync.com, expo local-first) — SQLite-as-truth + query-invalidation pattern

---
*Architecture research for: SGE Linux IMAP desktop client (Tauri v2 + React + SQLite)*
*Researched: 2026-10-03*
