# Stack Research: v1.1 Triage & Folders

**Domain:** IMAP flag sync, multi-folder sync, poll refresh, UID backfill (incremental on existing SGE codebase)
**Researched:** 2026-10-04
**Confidence:** HIGH

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| async-imap (existing, no change) | 0.11.3 (locked) | UID STORE flag writes, per-folder SELECT/STATUS, UID SEARCH diff | Verified on docs.rs 0.11.3 (Sept 2026, current): `Session::uid_store(uid_set, query)` and `Session::store(seq_set, query)` both exist with `+FLAGS.SILENT` / `-FLAGS.SILENT` semantics. `select`, `status`, `list`, `noop` already cover every v1.1 IMAP verb. Zero new IMAP surface to learn; the M1 read-only invariant lifts by *extending* the existing `SyncSession` trait, not by adding a crate. |
| tauri::async_runtime (existing re-export, no new dep) | via tauri 2.12.1 (locked) | Poll-loop spawning if a backend loop is ever needed | `commands/sync.rs` already drives all IMAP work as `spawn_blocking` + `async_std::task::block_on` on a dedicated thread, never on Tokio threads. A backend poll loop (if chosen over frontend polling) fits the identical pattern — `spawn_blocking` with an `async_std::task::sleep` loop. No direct `tokio` dependency needed; tokio 1.53.1 is already transitive via Tauri. |
| rusqlite_migration (existing, no change) | 2.x (locked) | Schema migration v2: per-folder sync state | Folder support is a schema problem, not a library problem. `M::up` migration v2 generalizes the `mailboxes`/`sync_state` rows from hardcoded `INBOX` to one row per folder (name, uid_validity, uid_next, last_sync_at). The v1 pattern (WAL + single-writer `Arc<Mutex<Store>>`) carries over unchanged. |
| React setInterval + existing Channel events (no new npm dep) | react 19.1, @tauri-apps/api v2 (locked) | Poll + manual refresh trigger | `start_sync` already streams `SyncEvent` progress over a Tauri `Channel`. Polling is just a timer calling the command that already exists. No scheduler library, no new IPC shape. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| (none — no new crates) | — | — | All four v1.1 features compose from locked dependencies. This is the headline finding: **v1.1 adds zero Rust dependencies and zero npm dependencies.** |
| imap-proto 0.16.7 (transitive, unchanged) | 0.16.7 (locked) | Flag parsing on FETCH responses | FLAGS already flow through `fetch_envelopes` into the local `flags` column; flag-write round-trips reuse the same parser. The NAMESPACE gap is untouched (LIST-derived profiles still apply per folder). |

## Installation

```bash
# No new dependencies for v1.1. Locked versions already cover everything:
#   async-imap 0.11.3, imap-proto 0.16.7, rusqlite 0.37, rusqlite_migration 2.x,
#   tauri 2.12.1, async-std 1.13, futures 0.3
# After extending SyncSession, just:
cargo check -p sge
```

## Integration Points (where each feature lands)

| Feature | IMAP call (existing crate) | Trait change | Store change | UI trigger |
|---------|---------------------------|--------------|--------------|------------|
| Read/unread sync | `uid_store("<uid>", "+FLAGS.SILENT (\\Seen)")` to mark read; `-FLAGS.SILENT (\\Seen)` to mark unread (docs.rs 0.11.3, verified) | Add `set_seen(uid, seen)` to `SyncSession` in `imap/mod.rs`; implement for `BoxedSession` + `MockSession` | Optimistic local `flags` column update, reconcile on next sync | New `toggle_seen` Tauri command; use `.SILENT` so the server skips untagged FETCH floods |
| Folder browsing | Existing `list(Some(""), Some("*"))` for discovery; `select("<folder>")` + `status("<folder>", "(MESSAGES UIDVALIDITY UIDNEXT)")` per folder | Generalize `select_inbox()` → `select_folder(name)`; `search_uids`/`fetch_envelopes` already folder-agnostic (they operate on the selected mailbox) | Migration v2: one `mailboxes` + sync-state row per folder; reuse the 7-step worker loop per selected folder | Sidebar folder list from local `mailboxes` table; SELECT only the opened folder |
| Poll + manual refresh | Reuse full `sync_with_session` pass; optional lightweight `NOOP` keepalive between passes | None (reuse worker as-is) | None (existing `sync_status` row already feeds "Up-to-date \<timestamp\>") | Frontend `setInterval` calling `start_sync` (default 60 s, only when focused); manual refresh button calls the same command; backend skips a pass if a sync is already in flight (reuse the existing cancel-sync flag pattern) |
| UID backfill | `uid_search` with range (`UID <lo>:*`) + existing `uid_fetch` in 200-UID batches (`BATCH_SIZE` unchanged) | None — gap fill is a caller-side range computation | Gap query: compare local UID set vs `UID SEARCH ALL` result; fetch only missing ranges instead of full re-sweep | Automatic inside each sync pass (Step 4.5 of the worker); no separate UI |

### Cross-cutting rules

- **SELECT (read-write), never EXAMINE, for any folder the user can triage.** EXAMINE opens read-only and silently drops flag writes — the exact class of silent-failure bug M1's worker guards already reject. Keep the worker's fail-loud style: a STORE that returns no confirmation is an error, not a success.
- **Delimiter handling per folder:** reuse the LIST-derived delimiter profile from the probe when passing folder names to `select`/`status` (names with spaces or non-ASCII need quoting — construct via the `Name::name()` value verbatim).
- **UIDVALIDITY guard stays per folder:** the M1 bump-and-wipe logic generalizes to each folder row independently; one folder's renumbering must not wipe the others.

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| `uid_store` on async-imap 0.11.3 | Sync `imap` 2.x crate on `std::thread` | Only if 0.11 async bridging hurts — the documented fallback in `Cargo.toml`. Flag writes do **not** trigger it; `uid_store` is a normal async method on the already-owned session. |
| Frontend `setInterval` poll | IMAP IDLE push (`Session::idle`, exists in 0.11.3) | Defer IDLE past v1.1. IDLE holds one connection open per selected folder, fights the one-shot `connect_sync`/`logout` lifecycle, and university servers routinely time out idle connections. Poll matches the milestone spec and the existing command shape. Revisit when real-time push is a stated requirement. |
| `tauri::async_runtime` re-export | Direct `tokio` dependency with `time` feature | Never for v1.1 — tokio 1.53.1 is already transitive via Tauri and `async-std` drives the IMAP futures. A direct dep buys version-resolution risk for zero capability. |
| Plain `UID SEARCH ALL` diff | CONDSTORE / QRESYNC (`select_condstore` exists in 0.11.3) | Only after probing `CAPABILITY` for CONDSTORE on the target server. mail.utfpr.edu.br support is unverified; the plain SEARCH diff is robust everywhere and the mailbox sizes don't justify the complexity yet. |
| Timer + existing command | A scheduler crate (e.g. `tokio-cron`, `job_scheduler`) | Never for one interval. A crate is justified at 3+ schedules with persistence; v1.1 has exactly one ("refresh every N seconds while open"). |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| Sync `imap` 2.x crate now | Swaps the entire async session model for zero v1.1 gain; `uid_store` already exists on the locked 0.11.3 | Stay on async-imap 0.11.3; keep the 2.x fallback documented-only |
| `Session::examine` for triage folders | Read-only open: STOREs fail or silently no-op, producing "toggled but server disagrees" state | `select_folder(name)` (read-write SELECT) for any user-triageable folder |
| Non-`.SILENT` STORE queries | Server returns an untagged FETCH per stored message — a 200-flag batch becomes a 200-FETCH flood the stream must drain | `+FLAGS.SILENT` / `-FLAGS.SILENT`; reconcile from the local optimistic write + next sync pass |
| IMAP IDLE in v1.1 | Conflicts with one-shot session lifecycle; server idle timeouts create phantom-disconnect bugs; milestone explicitly scopes poll | Frontend interval + manual refresh; IDLE is a post-v1.1 enhancement |
| Direct `tokio` dependency | Transitive tokio already present; mixing runtimes with `async_std::task::block_on` on the IMAP thread invites subtle executor bugs | `tauri::async_runtime::spawn_blocking` (already the codebase pattern) |
| `EXPUNGE` / `CLOSE` anywhere in v1.1 | v1.1 has no delete feature; expunge permanently destroys server mail and widens every flag-write code path into a data-loss path | Flag writes only; expunge arrives with a delete feature and its own confirmation UX |
| New npm scheduler/state library | Poll state (interval handle, last-sync timestamp, in-flight flag) is three `useRef`/`useState` values around an existing command | Plain `setInterval` in the existing sync hook |

## Stack Patterns by Variant

**If the UTFPR server rejects `.SILENT` STORE variants:**
- Fall back to non-SILENT `+FLAGS (\\Seen)` and drain the returned FETCH stream (same `try_next` loop as `fetch_envelopes`)
- Because some older servers accept only RFC 3501 base syntax; behavior is identical, just chattier

**If a folder name contains the hierarchy delimiter or non-ASCII (e.g. `INBOX.Enviadas`, `Rascunhos & Arquivos`):**
- Pass the LIST-returned name verbatim as the single `select`/`status` argument; never split or re-encode
- Because the server canonicalizes hierarchy — client-side splitting corrupts names on servers with `.` vs `/` delimiters

**If poll fires while a sync is in flight:**
- Skip the tick (log + update "last checked" timestamp), never queue a second session
- Because two concurrent sessions SELECTing different folders invalidate each other's EXISTS/UIDNEXT views and double-logins can trip server connection limits

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| async-imap@0.11.3 | imap-proto@0.16.7 | Locked pair in Cargo.lock; `uid_store`/`store` signatures verified against 0.11.3 docs (Sept 2026, latest in the 0.11 line) |
| tauri@2.12.1 | tokio@1.53.1 (transitive) | `async_runtime::spawn_blocking` is the supported bridge; do not add direct tokio |
| rusqlite@0.37 + rusqlite_migration@2.x | Schema v1 → v2 migration | v2 must be additive (new tables/columns only); v1 messages/bodies/FTS triggers untouched so M1 databases upgrade in place |
| @tauri-apps/api@v2 | Channel<T> event streaming | Existing `SyncEvent` Channel carries poll results with no IPC change |

## Sources

- docs.rs async-imap 0.11.3 `Session` API — `store`, `uid_store`, `select`, `status`, `list`, `noop`, `idle` signatures verified (HIGH confidence, fetched 2026-10-04)
- `src-tauri/Cargo.lock` — async-imap 0.11.3, tauri 2.12.1, tokio 1.53.1, imap-proto 0.16.7 (HIGH, local lockfile)
- `src-tauri/src/commands/sync.rs` — `spawn_blocking` + `block_on` threading pattern, Channel progress events (HIGH, local source)
- `src-tauri/src/imap/mod.rs` + `session.rs` — `SyncSession` trait surface, LIST-derived namespace profile, 30 s timeout, read-only invariant scope (HIGH, local source)
- `src-tauri/src/sync/worker.rs` — 7-step sweep, BATCH_SIZE=200, UIDVALIDITY guard, fail-loud invariants (HIGH, local source)

---
*Stack research for: SGE v1.1 Triage & Folders (incremental — validated M1 stack unchanged)*
*Researched: 2026-10-04*
