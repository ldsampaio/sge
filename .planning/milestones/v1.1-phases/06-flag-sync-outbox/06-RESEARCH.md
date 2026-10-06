# Phase 6: Flag Sync + Outbox - Research

**Researched:** 2026-10-04
**Domain:** IMAP flag writes (UID STORE \Seen), durable offline outbox, sync reconcile
**Confidence:** HIGH

## Summary

Phase 6 ends the read-only era: the first IMAP write verb (`UID STORE ±FLAGS.SILENT (\Seen)`) ships alongside a durable SQLite outbox and a pending-wins reconcile, plus the SessionManager single-session foundation later phases build on. No new external crates are needed — everything rides on the pinned stack (async-imap 0.11.3, rusqlite 0.37, rusqlite_migration 2.3.0, Tauri 2.12.1, all confirmed in Cargo.lock this session).

The exact `uid_store` spelling is verified against the locked async-imap source: `session.uid_store(uid_set, query)` emits `UID STORE <uidset> <query>` on the wire, and the flag argument must be `+FLAGS.SILENT (\Seen)` / `-FLAGS.SILENT (\Seen)` (`.SILENT` suffix suppresses untagged FETCH; flag list parenthesized). The critical async-imap gotcha is that `uid_store` returns a *Stream* — the STORE only completes if the caller drains it to completion.

Reconcile hooks into one precise location: the `upsert_message` loop in `sync/worker.rs` step 5, which today overwrites local flags wholesale from server FETCH data. Pending-wins means skipping (or merging) the flags column for UIDs that have unacknowledged outbox ops. The outbox itself is a second rusqlite_migration (`M::up` CREATE TABLE, schema version 1→2) following the existing single-writer `queries.rs` convention.

**Primary recommendation:** Extend `SyncSession` with `set_seen(uid, seen)`, add a `flag_outbox` table via migration M2, route all flag writes through a lease-holding SessionManager, and gate the worker's flag write-back on pending outbox ops — with `.SILENT` STORE args and full stream drain as non-negotiable details.

## User Constraints (from CONTEXT.md)

### Locked Decisions
- Auto-mark read when a message is opened in the reader (Thunderbird-style Seen-on-open), plus explicit toggle for mark-unread
- Toggle affordance: clickable unread dot in message list rows + mark read/unread button in reader header
- Optimistic UI: toggle applies instantly locally, per-message pending state until server acknowledges
- Pending indicator: subtle pending style on the row + aggregate state in SyncStatus; never blocks browsing
- Automatic replay on reconnect (no manual flush button)
- Pending-wins reconcile: unacknowledged local toggles beat server FLAGS diffs on merge
- Durable SQLite outbox table with RFC 4549 playback rules (drop op when message missing, drop folder queue on UIDVALIDITY change)
- Failed ops surface in sync status text and retry on the next sync naturally
- UID-only STORE addressing (`uid_store` + `±FLAGS.SILENT (\Seen)`); never sequence numbers
- BODY.PEEK audit over every fetch path (imap/headers.rs, imap/bodies.rs, sync/bodies.rs, commands/sync.rs fetch_message) — no path may set \Seen as a side effect
- SessionManager owns the single session; mailbox-scoped leases from day one (INBOX is the `mailbox="INBOX"` case)
- Scope guard: no delete/expunge/move, no \Flagged keywords, no bulk multi-select — read/unread triage only

### the agent's Discretion
- Exact outbox table shape and replay batching; poll-interval-independent reconnect detection details are deferred to Phase 8 — Phase 6 only needs "replay on next successful sync/session open"

### Deferred Ideas (OUT OF SCOPE)
- Seen-on-open delay tuning (mark after N seconds of reading vs instant) — ship instant, tune later if needed
- \Flagged/star support — later milestone, not triage-minimal
- Session idle-timeout empirical test (1-hour-open) — Phase 8 poll concern, noted in research gaps

## Project Constraints (from AGENTS.md)

- IMAP must leave mail on server (BODY.PEEK only for reads; Phase 6 lifts only the \Seen-write rule — still no delete/expunge/move, no SMTP/send).
- INBOX only; credentials only in OS keyring (never plaintext); Linux-only bundle.
- Headers-first sync, UIDVALIDITY-guarded incremental; sanitize HTML (ammonia) + sandboxed iframe before render (unchanged by this phase).
- Stack pins: Tauri v2.12, async-imap 0.11, mail-parser 0.11 + full_encoding, rusqlite 0.37 bundled + rusqlite_migration 2.x, keyring 3 + sync-secret-service.
- GSD workflow: research → plan-check → execute → verify; phases run in numeric order.

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| FLAG-01 | Mark read/unread with Seen synced to server (optimistic UI, reconciled on sync) | `SyncSession::set_seen` via `uid_store` + `.SILENT` args; optimistic local write + pending flag; pending-wins merge in worker write-back |
| FLAG-02 | Offline toggles queue durably in SQLite and replay on reconnect | `flag_outbox` table (migration M2) + RFC 4549 replay on session open / post-sync; failures surface in sync status, retry next sync |

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Seen toggle + optimistic UI + pending wash | Frontend (React) | — | Instant local state, aria-label flip, accent-soft row wash per UI-SPEC; no server round-trip before paint |
| Pending count + replay-failure text | Frontend SyncStatus | Backend `sync_status` query | Aggregate "N alterações aguardando envio" reads outbox count via existing `sync_status` command path |
| `set_seen` command + outbox enqueue | API / Backend (Tauri commands) | — | Single entry point for MessageList dot + ReadingPane button + Seen-on-open |
| UID STORE execution + outbox replay | API / Backend (SessionManager) | — | Single session ownership; mailbox-scoped leases; replay on session open / post-sync |
| Pending-wins reconcile | API / Backend (sync worker) | — | Write-back merge in worker step 5 is the only place server FLAGS meet local pending ops |
| Flag persistence + outbox durability | Database / Storage (SQLite) | — | `messages.flags` JSON + `flag_outbox` table under rusqlite_migration versioning |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| async-imap | 0.11.3 [VERIFIED: Cargo.lock:127-129] | `uid_store` for Seen writes; `Flag::Seen` | Already pinned; `uid_store(uid_set, query)` is the UID-only STORE path [VERIFIED: client.rs:845-866] |
| rusqlite | 0.37.0 bundled [VERIFIED: Cargo.lock:3216-3218] | Outbox CRUD, flag column updates | Already pinned; single-writer `queries.rs` convention |
| rusqlite_migration | 2.3.0 [VERIFIED: Cargo.lock:3230-3232] | M2 migration for outbox table | Already pinned; `Migrations::new(vec![M::up(...)])` pattern in store/mod.rs:80 |
| Tauri | 2.12.1 [VERIFIED: Cargo.lock:3911-3913] | `set_seen` command, extended `sync_status` | Already pinned; `Result<_, String>` command convention |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| futures::TryStreamExt | (transitive via async-imap) | Drain the STORE response stream (`try_collect`) | Mandatory — STORE completes only when its stream is consumed |
| serde_json | (already used in queries.rs) | `parse_flags` / flags JSON merge | Reuse `parse_flags` + `is_unread` helpers (queries.rs:28-37) |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `+FLAGS.SILENT` | `+FLAGS` (non-silent) | Returns untagged FETCH per message — extra parse burden, no benefit since client already knows target state; locked decision is `.SILENT` |
| `uid_store` | `store` (sequence numbers) | Sequence numbers shift under concurrent expunge → mislanding; locked decision forbids them |
| SQLite outbox | In-memory queue | Lost on restart — violates FLAG-02 durability; no reason to diverge |

**Installation:**
```bash
# No new packages — all dependencies already pinned in src-tauri/Cargo.toml / Cargo.lock.
cargo build --manifest-path src-tauri/Cargo.toml
```

**Version verification:** async-imap 0.11.3, rusqlite 0.37.0, rusqlite_migration 2.3.0, tauri 2.12.1 — all confirmed via `grep Cargo.lock` this session (see table).

## Package Legitimacy Audit

No external packages are installed in this phase — all crates are pre-pinned M1 dependencies verified in Cargo.lock. Audit not applicable; nothing to add, remove, or flag.

## Architecture Patterns

### System Architecture Diagram

```
MessageList dot / ReadingPane button / Seen-on-open
        │  optimistic local write (flags JSON + outbox enqueue)
        ▼
┌──────────────┐   lease (mailbox="INBOX")   ┌────────────────┐
│  set_seen    │ ───────────────────────────▶ │ SessionManager │── single BoxedSession
│  Tauri cmd   │                              │  (new, owns    │   (replaces per-command
└──────────────┘                              │   session)     │    connect_sync calls)
        │                                     └───────┬────────┘
        │ offline / error                     UID STORE ±FLAGS.SILENT (\Seen)
        ▼                                             │
┌──────────────┐  replay on session open /     ┌──────▼─────────┐
│ flag_outbox  │  post-sync (RFC 4549 rules)   │ mail.utfpr.    │
│ (SQLite M2)  │◀──────────────────────────────│  edu.br:993    │
└──────────────┘                               └────────────────┘
        │
        │ pending-wins merge
        ▼
┌──────────────┐  step-5 write-back skips      ┌───────────────┐
│ sync/worker  │  flags for pending UIDs       │ messages.flags│
│ (reconcile)  │──────────────────────────────▶│ (SQLite)      │
└──────────────┘                               └───────────────┘
```

### Recommended Project Structure
```
src-tauri/src/
├── imap/
│   ├── mod.rs           # SyncSession += set_seen (PinBox signature)
│   └── session.rs       # SessionManager (owns BoxedSession, mailbox leases)
├── store/
│   ├── schema.sql       # UNCHANGED (M1 baseline stays v1 text)
│   ├── migrations.rs    # (or mod.rs) M2: CREATE TABLE flag_outbox; SCHEMA_VERSION 1→2
│   └── queries.rs       # outbox CRUD + set_local_seen + pending_uids (single-writer rule)
├── sync/
│   └── worker.rs        # reconcile: skip/merge flags for pending UIDs in step-5 loop
└── commands/
    └── sync.rs          # set_seen cmd; sync_status += pending count; replay hook post-sync
```

### Pattern 1: Object-safe trait extension with PinBox
**What:** `SyncSession::set_seen` follows the existing `PinBox<'_, Result<…, SyncError>>` convention so `Box<dyn SyncSession>` keeps working and `MockSession` in worker tests gains a trivial impl.
**When to use:** Every new session capability in this codebase.
**Example:**
```rust
// Source: existing trait shape, src-tauri/src/imap/mod.rs:253-274
fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>>;
// Impl drains the STORE stream — uid_store returns a Stream, the command
// completes only when it is consumed:
let arg = if seen { "+FLAGS.SILENT (\\Seen)" } else { "-FLAGS.SILENT (\\Seen)" };
let stream = self.uid_store(uid.to_string(), arg).await?;
let _updates: Vec<_> = stream.try_collect().await?;
```

### Pattern 2: Mailbox-scoped session lease
**What:** SessionManager owns one `BoxedSession`; callers check out a short-lived guard bound to `mailbox="INBOX"` (SELECTs INBOX on first lease, re-SELECTs after reconnect). Later phases add more mailbox names without changing the shape.
**When to use:** Every IMAP operation from Phase 6 on — retire per-command `connect_sync` calls toward the manager.
**Example:**
```rust
// Shape to fit connect_sync (src-tauri/src/imap/session.rs:755) + AppState
// (src-tauri/src/lib.rs:27-29: Arc<Mutex<Store>> + Mutex<Option<ActiveAccount>>):
let _lease = manager.mailbox("INBOX").await?; // SELECTs INBOX, single-flight
manager.set_seen(uid, seen).await?;           // UID STORE on the owned session
```

### Pattern 3: Pending-wins reconcile at write-back
**What:** In the worker step-5 `upsert_message` loop (worker.rs:176-213), fetch the pending-UID set once per sync pass; for pending UIDs, write every column *except* `flags` (keep the optimistic local value). Non-pending UIDs take server FLAGS verbatim.
**When to use:** Any server→local merge while unacknowledged local ops exist — this is what prevents flap/mislanding under concurrent sync.

### Anti-Patterns to Avoid
- **Sequence-number STORE:** `session.store(...)` shifts under concurrent expunge → flags land on the wrong message. Always `uid_store`.
- **Non-SILENT STORE without draining:** Extra untagged FETCH traffic nobody parses; and an undrained stream means the STORE may never complete. Always `.SILENT` + `try_collect`.
- **Replacing schema.sql for M2:** `include_str!("schema.sql")` is migration v1; M2 must be an *additional* `M::up(CREATE TABLE …)` so existing databases migrate forward instead of failing checksum/ordering.
- **Upserting server FLAGS over pending rows:** Wholesale `upsert_message` with server flags clobbers optimistic toggles → visible flap. Gate on the pending set.
- **Bare `BODY[]` anywhere:** Any fetch path using `BODY[]` instead of `BODY.PEEK[]` sets \Seen as a side effect, silently defeating the outbox. Audit all four paths.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| IMAP STORE wire format | Manual `run_command("STORE…")` strings | `session.uid_store(uid, "+FLAGS.SILENT (\\Seen)")` | Validates UID-set encoding, parses FETCH responses, handles untagged data; hand-rolled strings risk sequence/UID confusion |
| Flag name spelling | Custom `\seen` / `Seen` literals | `async_imap::types::Flag::Seen` ↔ `"\\Seen"` mapping [VERIFIED: types/mod.rs:160-170] | `"\\Seen" => Some(Flag::Seen)` is the canonical wire spelling [VERIFIED: types/mod.rs:162]; case/exactness matters on strict servers |
| Migration versioning | Ad-hoc `CREATE TABLE IF NOT EXISTS` | rusqlite_migration M2 (`SCHEMA_VERSION` 1→2) | Ordering, atomicity, and fresh-vs-upgrade parity; ad-hoc DDL diverges test (in-memory) from prod databases |
| Offline retry timing | Custom backoff scheduler | Replay on next successful sync/session open | Poll-timer ownership lands in Phase 8; a second trigger source now means double-STORE races later |

**Key insight:** The IMAP protocol punishes hand-rolled wire code (silent FETCH truncation, sequence renumbering) — every write goes through `uid_store` and every schema change through migrations, no exceptions.

## Common Pitfalls

### Pitfall 1: Undrained STORE stream (no-op write)
**What goes wrong:** `uid_store` returns a `Stream`; if never polled, the command may not complete and the flag never lands on the server while local UI already shows read.
**Why it happens:** async-imap models command completion as stream exhaustion (`parse_fetches` in client.rs); fire-and-forget looks type-correct.
**How to avoid:** Always `try_collect().await` the returned stream inside `set_seen`; assert in tests that MockSession records the call.
**Warning signs:** UI shows read, server still unread after sync; STORE missing from protocol logs.

### Pitfall 2: Non-UID addressing (mislanding)
**What goes wrong:** Flags applied to the wrong message after an expunge renumbered the mailbox.
**Why it happens:** Using `store()` (sequence) instead of `uid_store`, or building the uid-set string from row positions.
**How to avoid:** Locked rule — `uid_store` with the message's IMAP UID only; never row index, never sequence.
**Warning signs:** Wrong row flips read/unread after a sync that also deleted messages.

### Pitfall 3: Server FLAGS clobbering optimistic state (flap)
**What goes wrong:** Toggle → row shows read → next sync sweep rewrites flags from server FETCH → rowflaps back to unread until replay finishes.
**Why it happens:** Step-5 upsert writes server FLAGS unconditionally.
**How to avoid:** Pending-wins gate: skip the flags column for UIDs present in the outbox during write-back; clear pending only on STORE ack.
**Warning signs:** Dot flickers on every sync; toggle "loses" then reappears.

### Pitfall 4: UIDVALIDITY bump orphaning the outbox
**What goes wrong:** Server reassigns UIDs; replayed ops address recycled UIDs belonging to different messages.
**Why it happens:** Replay keyed on UID alone without checking the validity epoch.
**How to avoid:** RFC 4549 rule (locked): store `uid_validity` per op; on bump, drop the whole mailbox queue before any replay; drop single ops whose UID is absent from `UID SEARCH ALL`.
**Warning signs:** After "mailbox recreated" server events, wrong messages marked read.

### Pitfall 5: Read-path side effect setting \Seen
**What goes wrong:** Merely opening or syncing a message marks it read on the server, creating phantom outbox conflicts.
**Why it happens:** A fetch path uses bare `BODY[]` / `RFC822` instead of `BODY.PEEK[]`, or FETCH_ATTRS gains a body part.
**How to avoid:** Phase-6 BODY.PEEK audit over all four paths (headers.rs FETCH_ATTRS has no BODY part [VERIFIED: headers.rs:21]; bodies go through `fetch_body` → `BODY.PEEK[]` [VERIFIED: imap/mod.rs:327-341]); add the existing FETCH_ATTRS unit-test style assertion to CI thinking for any new fetch.
**Warning signs:** Messages become read on the server without any toggle; outbox grows spontaneously.

## Code Examples

Verified patterns from locked local sources:

### UID STORE Seen on/off
```rust
// Source: async-imap 0.11.3 client.rs:770-798 (STORE data items) + 845-866 (uid_store wire format)
use futures::TryStreamExt;
// Mark read:
let mut s = session.uid_store("1234", "+FLAGS.SILENT (\\Seen)").await?;
let _upd: Vec<_> = s.try_collect().await?;  // drain to completion
// Mark unread:
let mut s = session.uid_store("1234", "-FLAGS.SILENT (\\Seen)").await?;
let _upd: Vec<_> = s.try_collect().await?;
```

### Outbox migration M2 (fits existing convention)
```rust
// Source: existing convention src-tauri/src/store/mod.rs:77-83
fn apply_migrations(conn: &mut Connection) -> StoreResult<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    let migrations = Migrations::new(vec![
        M::up(include_str!("schema.sql")),          // v1 — UNCHANGED M1 baseline
        M::up("CREATE TABLE flag_outbox (
                 id INTEGER PRIMARY KEY,
                 mailbox_id INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
                 uid INTEGER NOT NULL,
                 seen INTEGER NOT NULL,             -- 1 = mark read, 0 = mark unread
                 uid_validity INTEGER NOT NULL,     -- RFC 4549 epoch guard
                 created_at TEXT NOT NULL,          -- ISO8601 UTC, FIFO replay order
                 attempts INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 UNIQUE (mailbox_id, uid)           -- one pending op per message: latest wins
               );
               CREATE INDEX idx_outbox_mailbox ON flag_outbox(mailbox_id);"),
    ]);
    migrations.to_latest(conn)?;
    Ok(())
}
```

### Optimistic toggle + enqueue (command sketch)
```rust
// Fits: queries.rs single-writer rule + commands/sync.rs spawn_blocking convention
// 1. Update messages.flags JSON locally (add/remove "\\Seen") — instant UI truth.
// 2. UPSERT flag_outbox (mailbox_id, uid, seen, uid_validity) — durable queue.
// 3. If session lease available: UID STORE now; on ack DELETE op. Else: leave queued.
// 4. sync_status returns pending count for "N alterações aguardando envio".
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| M1 read-only: no STORE/EXPUNGE/APPEND exposed (SyncSession docs, mod.rs:250-252) | First write verb `set_seen` via `uid_store` | This phase | Lifts only the D-flags rule; all other write verbs stay banned |
| Per-command `connect_sync` sessions (commands/sync.rs:89,271,407) | SessionManager single-session + leases | This phase | Foundation for poll timer (Phase 8) without connection storms |
| `messages.flags` display-only (`is_unread` reads, never writes back — queries.rs:35-37) | Optimistic local writes + server ack | This phase | READ-03 display becomes FLAG-01 triage |

**Deprecated/outdated:**
- The "client never writes \Seen" comment on `is_unread` (queries.rs:35-37) — update it when the write path lands; it becomes "writes go through SessionManager only".

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | UTFPR server accepts `.SILENT` STORE suffix and `+`/`-FLAGS` modifiers [ASSUMED] | Standard Stack | If rejected, fall back to non-SILENT `+FLAGS` and parse the untagged FETCH; probe CAPABILITY at plan start |
| A2 | UTFPR advertises UIDPLUS or at least tolerates multi-UID comma sets in `uid_store` [ASSUMED] | Architecture | Single-UID STORE per op always works; batching is an optimization, replay one-op-per-round-trip if needed |
| A3 | `SELECT` persists across `uid_store` on the manager-owned session (no intervening SELECT by sync worker on another connection) [ASSUMED] | Pitfalls | Manager must SELECT INBOX per lease; worker and flag paths sharing one session need lease serialization |
| A4 | RFC 4549 playback semantics apply cleanly to Seen-only ops (drop-on-missing, drop-queue-on-UIDVALIDITY-change) [ASSUMED] | Architecture | Seen ops are idempotent and non-destructive, so even a loose reading is safe; risk is low |
| A5 | Frontend `sync_status` polling cadence is sufficient for pending-count freshness (no push needed) [ASSUMED] | UI / Validation | Pending wash is per-row optimistic so staleness only affects the aggregate line; acceptable per UI-SPEC |

## Open Questions

1. **Session idle timeout on UTFPR (deferred to Phase 8 per CONTEXT)**
   - What we know: Manager holds one session open; Phase 6 replays "on next successful sync/session open".
   - What's unclear: How long the server keeps an idle session alive before BYE.
   - Recommendation: Manager must handle reconnect-on-failure transparently now (reconnect + re-SELECT + retry once); empirical timeout test stays deferred.

2. **Replay batching granularity**
   - What we know: Agent's discretion — exact batching is planner's choice.
   - What's unclear: One UID STORE per op vs comma-joined multi-UID STORE per seen-state.
   - Recommendation: Start one-STORE-per-op (simplest correct, per-op ack clears pending precisely); batch only if profiling demands it.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cargo/rustc | Build + unit tests | ✓ | 1.98.1 | — |
| SQLite (bundled via rusqlite) | Outbox + migrations | ✓ | bundled | — |
| UTFPR IMAP (mail.utfpr.edu.br:993) | Live STORE verification | ? | — | MockSession unit tests; live probe at plan start |
| OS keyring | Credential reuse in set_seen | ✓ (M1 Phase 5 shipped) | — | In-memory active_account |

**Missing dependencies with no fallback:**
- None blocking — full phase is implementable and unit-testable offline via MockSession.

**Missing dependencies with fallback:**
- Live UTFPR STORE acceptance (`.SILENT`, PERMANENTFLAGS including `\Seen`) — verify with a manual probe; code defensively (see A1).

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness (`cargo test`) + existing MockSession fixture |
| Config file | none — see Wave 0 |
| Quick run command | `cargo test -p sge flag_outbox` (targeted) |
| Full suite command | `cargo test --manifest-path src-tauri/Cargo.toml` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| FLAG-01 | set_seen emits UID STORE ±FLAGS.SILENT; mock records ack; local flags flip | unit | `cargo test -p sge set_seen` | ❌ Wave 0 |
| FLAG-01 | Pending-wins: worker write-back preserves optimistic flags for pending UIDs | unit | `cargo test -p sge pending_wins` | ❌ Wave 0 |
| FLAG-02 | Offline toggle enqueues durable op; replay on session open drains queue | unit | `cargo test -p sge outbox_replay` | ❌ Wave 0 |
| FLAG-02 | UIDVALIDITY bump drops mailbox queue; missing UID drops single op | unit | `cargo test -p sge outbox_rfc4549` | ❌ Wave 0 |
| FLAG-01/02 | No fetch path issues bare BODY[] (PEEK audit) | unit | `cargo test -p sge peek_audit` | ❌ Wave 0 (extend existing FETCH_ATTRS assertion pattern, headers.rs:417-428) |

### Sampling Rate
- **Per task commit:** `cargo test --manifest-path src-tauri/Cargo.toml` (suite is small, runs in seconds)
- **Per wave merge:** full suite
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `MockSession::set_seen` impl + recorded-calls assertion support (extend worker.rs test fixture)
- [ ] Outbox query tests (enqueue/dequeue/drop rules) in store/queries.rs tests module
- [ ] Reconcile + replay worker tests in sync/worker.rs tests module
- [ ] PEEK-audit test: grep-style assertion that no fetch-attr constant contains `BODY[]` without `PEEK`

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | Keyring-only credentials (M1 Phase 5); no new auth surface — set_seen reuses active_account/keyring path |
| V3 Session Management | yes | SessionManager single session; reconnect + re-SELECT; never log password (Transcript redaction precedent) |
| V4 Access Control | no | Single-user local client; mailbox lease is INBOX-only by scope guard |
| V5 Input Validation | yes | UID is u32 from local DB (never user-typed); flag arg is a fixed two-variant literal — no string interpolation of user input into STORE |
| V6 Cryptography | no | No change — existing TLS via connect_sync; no new crypto |

### Known Threat Patterns for IMAP flag-write stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Flag mislanding on wrong message (sequence reuse) | Tampering | UID-only STORE (locked); never sequence numbers |
| \Seen set as read side effect (phantom state change) | Tampering | BODY.PEEK audit on all fetch paths; FETCH_ATTRS assertion test |
| Stale-UID replay after mailbox recreation | Tampering | uid_validity epoch per op; drop queue on bump (RFC 4549) |
| Credential exposure in session logs | Information disclosure | Existing Transcript redaction + Debug redaction on AccountConfig; extend to any new STORE logging |

## Sources

### Primary (HIGH confidence)
- async-imap 0.11.3 locked source, `src/client.rs:770-798` (STORE data items incl. `.SILENT`) and `:845-866` (uid_store wire format) — read this session via Read tool
- async-imap 0.11.3 locked source, `src/types/mod.rs:108-170` (`Flag::Seen`, `"\\Seen"` canonical spelling) — read this session via Read tool
- In-repo: `src-tauri/src/imap/mod.rs:244-274` (SyncSession trait), `:327-341` (fetch_body BODY.PEEK[]); `src-tauri/src/imap/session.rs:755` (connect_sync); `src-tauri/src/store/mod.rs:77-83` (migration pattern); `src-tauri/src/store/queries.rs:28-37` (flags helpers); `src-tauri/src/sync/worker.rs:47-261` (sync pass + write-back loop); `src-tauri/src/lib.rs:27-29` (AppState); Cargo.lock (pinned versions)

### Secondary (MEDIUM confidence)
- RFC 4549 (synchronization operations for disconnected IMAP4 clients) — playback/drop rules as referenced by CONTEXT.md playback requirement [ASSUMED detail — standard knowledge, confirm drop-rule exactness against RFC text at plan time if planner needs verbatim semantics]

### Tertiary (LOW confidence)
- None — all load-bearing claims verified against locked local sources above.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - versions from Cargo.lock + API spelling from locked crate source read this session
- Architecture: HIGH - all integration points mapped to exact file/line in local code
- Pitfalls: HIGH - derived from observed code paths (undrained-stream risk follows directly from uid_store's Stream return; flap follows directly from unconditional upsert)

**Research date:** 2026-10-04
**Valid until:** 2026-11-03 (stable domain: pinned deps, RFC-defined protocol, local codebase)
