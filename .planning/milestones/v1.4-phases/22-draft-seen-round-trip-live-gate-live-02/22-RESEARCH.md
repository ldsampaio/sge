# Phase 22: Draft & Seen Round-Trip — Live Gate (Live-02, SANDBOXED) — Research

**Researched:** 2026-10-13
**Domain:** Live IMAP verification harness (Rust bin + shell wrapper) over existing SyncSession/manager verbs
**Confidence:** HIGH (behavior under test read verbatim from source this session; IMAP dialect from vendored async-imap call sites in-repo)

## Summary

Phase 22 is verification-only: prove the Phase 6/10/12 machinery works against the real `mail.utfpr.edu.br:993/SSL` inside a per-run `SGE-Test-<timestamp>` sandbox. No new capabilities, no new IMAP verbs, no new tables. The entire live run can be driven through the existing public surfaces: `SessionManager::{create_mailbox_in, save_draft_copy_in, discard_server_copy_in, set_seen_in, uid_expunge_in, move_message_in, delete_mailbox_in, lease_for}` plus `SyncWorker::{sync_with_borrowed, replay_outbox, replay_imap_outbox, replay_dirty_drafts}` — all reachable from a new `src-tauri/src/bin/live_gate_22.rs` binary that links `sge_lib`, wrapped by a small shell script that sources the env file and runs the secret scan.

**Primary recommendation:** Build a Rust bin (`live_gate_22`) that owns one `SessionManager` + one temp SQLite `Store` for the whole run (no second IMAP connection, leases only), takes all config from env vars, prints `STEP <n>/<N> <name>: PASS|FAIL <detail>` lines, and exits nonzero on first FAIL; wrap it in `scripts/live-22.sh` that does `set -a; . src-tauri/.live-test.env; set +a`, seeds the keyring once, runs the bin, greps logs + SQLite dump for credential substrings, and retries cleanup with backoff. Offline is induced by an in-harness transport gate (a `SyncSession` decorator that fails writes while "offline"), which exercises the real `set_seen`/`delete_message`/`save_draft` replay entries without touching the OS network stack.

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Sandbox folder is `SGE-Test-<timestamp>` at account root, deleted post-run (no persistent parent).
- **D-02:** Pre-flight guard = name-prefix (`SGE-Test-`) + exists-and-empty check before first APPEND, PLUS a LIST snapshot taken before the run; the run aborts if LIST changes unexpectedly mid-run.
- **D-03:** Cleanup failure: retry N times with backoff, then print exact manual-cleanup steps (folder name, what to delete) — never auto-retry destructive ops blindly forever.
- **D-04:** Test drafts APPEND into the sandbox folder instead of the real Drafts folder (test-only role override); the one-copy logic is asserted there. Real drafts are never at risk.
- **D-05:** Pass = after N saves + reconnect, server SEARCH in the sandbox returns exactly ONE UID for the session AND it equals the tracked `draft_uid` (count + identity).
- **D-06:** Failure mode covered LIVE: kill the session between APPEND-new and expunge-old, reconnect, assert reconcile collapses to 1 copy (sandbox-safe by D-01/D-04).
- **D-07:** Edit-offline-then-reconnect is in scope: toggle offline mid-edit, queue, reconnect, assert single copy with latest content winning.
- **D-08:** Content asserted: FETCH the surviving copy and compare subject/body to the last save (catches silent content loss).
- **D-09:** Offline is induced via an app-level offline toggle in the test harness (deterministic; no OS/network fiddling); reconnect is a clean re-run of sync.
- **D-10:** Replay order asserted in logs + end-state: pre-sweep `imap_outbox` replay runs BEFORE the backfill sweep, with a pending indicator visible until acked (proves the Phase 10 discipline live).
- **D-11:** Resurrection proof = delete offline → reconnect → SECOND full sync asserts the UID stays gone (tombstoned, not re-fetched). The double-sync is the direct resurrection test.
- **D-12:** Seen round-trip asserted in BOTH directions across reconnect (mark read → assert `\Seen`; mark unread → assert absent), proving pending-wins reconcile both ways.
- **D-13:** Scripts source the env file (`set -a; . file; set +a`); values travel as env vars to the test binary, never as CLI args (no `/proc`/history leakage).
- **D-14:** Env password seeds the OS keyring ONCE; all IMAP/SMTP auth reads from keyring — the live run exercises the real Phase 1/5 credential path.
- **D-15:** Post-run automated secret scan: grep logs + SQLite dump for credential substrings, FAIL the run if found (constraint becomes a gate, not a review step).
- **D-16:** Test mail carries a fixed `SGE-TEST-` subject prefix + unique per-run Message-ID; the run audits outbox/logs to assert no mail WITHOUT the prefix was sent (proves "only test mail, only to self").

### The agent's Discretion
- Exact retry count N and backoff schedule for cleanup (D-03).
- App-level offline toggle implementation detail (feature flag vs harness-injected transport failure).
- SEARCH/FETCH verb dialect details for the one-copy and content assertions (UID SEARCH scoping, BODY.PEEK sections).

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Live IMAP run (CREATE/APPEND/STORE/EXPUNGE/DELETE) | API / Backend (Rust bin) | — | All wire I/O flows through `SessionManager` leases; no frontend involvement |
| Pass/fail orchestration + assertions | Backend (Rust bin) | Shell wrapper | Deterministic asserts need the lib types; shell only handles env + scan + exit codes |
| Credential sourcing + secret scan | Shell wrapper | OS keyring | Env-file sourcing and log-grepping are process-level concerns, not app logic |
| Pending indicator until acked | Frontend (existing) | — | D-10 visibility reuses Phase 10/12 UI; no new UI surface in this phase |

## Approaches Compared

### A. Harness shape: Rust bin + thin shell wrapper (RECOMMENDED)

- **Bin** `src-tauri/src/bin/live_gate_22.rs` links `sge_lib` (same pattern as `sync_diag.rs` [VERIFIED: src-tauri/src/bin/sync_diag.rs:24-27] and `sync_demo.rs` with its local `SyncSession` impl [VERIFIED: src-tauri/src/bin/sync_demo.rs:31-140]). Owns one `SessionManager`, one temp-file `Store`, runs the scripted steps, prints `STEP` lines, exits nonzero on FAIL.
- **Shell** `scripts/live-22.sh` (or `src-tauri/scripts/`): env-file sourcing, keyring seed, bin invocation with log capture, secret scan, cleanup retry loop.
- **Why:** step assertions need lib types (`MailboxSummary.uid_validity`, `ReplaySummary`, `DraftRow`) — a pure-shell harness would re-implement IMAP in curl/openssl and bypass the very code under test. A pure-Rust harness can't source env files idiomatically (D-13 mandates `set -a; . file; set +a`). Split at that seam.

### B. Alternative: shell + existing `sync_diag`/`sync_demo` binaries (REJECTED)

- `sync_diag` is read-only by design (SELECT/SEARCH/FETCH sample, env-only password, refuses `--password` [VERIFIED: src-tauri/src/bin/sync_diag.rs:88-91]) and `sync_demo` runs against a deterministic local `MockSession`, never the network [VERIFIED: src-tauri/src/bin/sync_demo.rs:2,20-31]. Neither can APPEND/STORE/EXPUNGE, and extending them would tangle diagnostic tools with mutating test logic. Reuse them as **skeleton candidates only** (arg-parsing shape, exit-code convention 0/1/2 [VERIFIED: src-tauri/src/bin/sync_diag.rs:17], env-only password discipline).

### C. Alternative: Tauri IPC-driven run via the real app (REJECTED)

- Driving `save_draft`/`set_seen` commands through a running Tauri app would exercise the command layer, but requires a display/webview, is non-deterministic in CI/headless, and the lease/manager wiring is identical underneath (`manager_for` + `SessionManager` [VERIFIED: src-tauri/src/commands/sync.rs:84-98]). The bin calls the **same manager methods the commands call** (`save_draft_copy_in`, `set_seen_in`), so coverage of the wire discipline is equivalent with far less flakiness. Frontend pending-indicator visibility (D-10) is asserted structurally (pending_count > 0 pre-replay, 0 post-replay) rather than via pixels.

### Offline toggle: three options (discretion item — RECOMMENDATION: option 2)

1. **Feature flag in prod code** (e.g. `SGE_FORCE_OFFLINE` checked in `SessionManager`): most "real" but pollutes the shipping binary with test-only branches; risk of flag leaking into release behavior. [ASSUMED — no such flag exists in-repo; none found in manager.rs]
2. **Harness-injected transport failure (RECOMMENDED):** a `SyncSession` decorator struct in the bin (`GateSession<S: SyncSession>` with an `AtomicBool offline`) that forwards reads and fails writes (`set_seen`, `store_deleted`, `append_message`, `uid_expunge`, `uid_copy_to`, `uid_move_to`) with `SyncError::Protocol("live-gate: offline gate closed")` while closed. The queued-then-replay flow goes through the **real** `SyncWorker::replay_*` and the **real** manager `*_in` methods on reconnect — no prod code changes at all, deterministic, per-op granularity (can fail exactly the expunge-old leg for D-06 by closing the gate between APPEND and expunge via direct `save_draft_on_session` leg calls).
3. **Kill-session (drop lease + `reconnect()`) only:** exercises the reconnect-retry path but not the offline-queue path (ops fail only if attempted while dead; nothing queues deterministically). Useful as a *complement* for D-06 (kill between APPEND-new and expunge-old), not as the offline mechanism. The manager already exposes `reconnect()` used by every `*_in` retry path [VERIFIED: src-tauri/src/imap/manager.rs:222-228].

## IMAP Verb Dialect (discretion item — PRESCRIPTIVE)

All forms below are the exact wire shapes the code under test already emits; the harness asserts through the same `SyncSession` trait methods, never raw sockets.

| Step | Verb | Exact form | Source |
|------|------|-----------|--------|
| Sandbox create | `CREATE SGE-Test-<ts>` | `session.create_mailbox(&wire)` via `create_mailbox_in` (INBOX lease; one reconnect-retry; `Refused` never retried) | [VERIFIED: src-tauri/src/imap/manager.rs:291-305] |
| Sandbox delete | `DELETE SGE-Test-<ts>` | `session.delete_mailbox(name)` via `delete_mailbox_in` (lease SELECTs the target first) | [VERIFIED: src-tauri/src/imap/manager.rs:375-388] |
| Folder discovery / pre-flight + LIST snapshot | `LIST "" "*"` | `session.list_mailboxes()` → `Vec<MailboxInfo>` (raw wire names) | [VERIFIED: src-tauri/src/imap/mod.rs:384-386] |
| Draft persist | `APPEND <sandbox> (\Draft \Seen) <bytes>` | `append_message(mailbox, DRAFT_FLAGS, bytes)`; `DRAFT_FLAGS = "(\Draft \Seen)"` | [VERIFIED: src-tauri/src/imap/mod.rs:444], [VERIFIED: src-tauri/src/imap/manager.rs:772-774] |
| One-copy reconcile | `UID SEARCH HEADER Message-ID "<id>"` | `uid_search_header("Message-ID", msg_id)` builds `HEADER Message-ID "<value>"` via `uid_search`; rule: exactly-one → it; zero → loud `Refused` (keep dirty, never blind-retry); multiple → max | [VERIFIED: src-tauri/src/imap/mod.rs:766-779], [VERIFIED: src-tauri/src/imap/manager.rs:764-793] |
| Expunge-old (UIDPLUS path) | `UID STORE <old> +FLAGS.SILENT (\Deleted)` then `UID EXPUNGE <old>` | `store_deleted(old, true)` + `expunge_single_on_session` (UIDPLUS → `UID EXPUNGE`; else single-UID unmark dance; **never bare `expunge()`** on this path) | [VERIFIED: src-tauri/src/imap/manager.rs:786-792], [VERIFIED: src-tauri/src/imap/manager.rs:858-869] |
| Content assert | `UID FETCH <uid> BODY.PEEK[]` | `fetch_body(uid)` — full RFC822 bytes; **never sets `\Seen`** (M1 invariant; D-08 MUST use this, not any other FETCH section) | [VERIFIED: src-tauri/src/imap/mod.rs:591-605] |
| Seen toggle | `UID STORE <uid> +FLAGS.SILENT (\Seen)` / `-FLAGS.SILENT (\Seen)` | `set_seen(uid, seen)` via `seen_store_arg(seen)` — exactly two canonical variants, UID as `u32` formatted by caller; stream drained to completion = the ack | [VERIFIED: src-tauri/src/imap/mod.rs:607-622], [VERIFIED: src-tauri/src/imap/mod.rs:422-427] |
| Read/unread assert | `UID FETCH <uid> (FLAGS)` or ENVELOPE sweep | Cheapest correct: `fetch_envelopes("<uid>")` returns `MessageHeader` (check `.seen` / flags) — but NOTE the sweep attrs are `UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE` [VERIFIED: src-tauri/src/bin/sync_diag.rs:42-43]; a FLAGS-only fetch is fine if done via `run_command`/`uid_fetch` directly. Simpler and fully sufficient: re-`SELECT` + `search_uids` + envelope check, or track via `mailbox_status` UNSEEN datum. **Never use `BODY[]` (non-PEEK) anywhere.** | [VERIFIED: src-tauri/src/imap/mod.rs:570-589] |
| Draft discard / delete test | `UID STORE <uid> +FLAGS.SILENT (\Deleted)` + scoped removal | `discard_server_copy_in(wire, uid, epoch)` — epoch-gated (MJ-04): pass the SELECT-time UIDVALIDITY; stale epoch skips the server leg | [VERIFIED: src-tauri/src/imap/manager.rs:552-592] |
| Delete/move replay | `UID MOVE <uid> <dest>` preferred; fallback COPY→STORE→scoped-expunge | `move_message_in` / `run_move_on_session` with capability gating; ~200-UID chunks (irrelevant at live-gate volumes — single UIDs) | [VERIFIED: src-tauri/src/imap/manager.rs:653-660], [VERIFIED: src-tauri/src/sync/worker.rs:389-412] |
| Seen replay entry | `replay_outbox(session, mailbox_id, epoch, live_uids)` | Epoch-mismatch → whole-mailbox drop; absent-UID → single drop; per-op failure stays queued | [VERIFIED: src-tauri/src/sync/worker.rs:222-295] |
| Delete/move replay entry | `replay_imap_outbox(session, mailbox_id, name, epoch, live_uids)` | **PRE-SWEEP position is non-negotiable** (before header sweep, else same-pass resurrection); `Refused` → drop, never re-COPY | [VERIFIED: src-tauri/src/sync/worker.rs:297-322], [VERIFIED: src-tauri/src/sync/worker.rs:1373-1393] |
| Full reconnect pass | `sync_with_borrowed(session, mailbox, cb)` | Order inside: SELECT → STATUS → UIDVALIDITY guard → SEARCH ALL → **4b pre-sweep imap replay → 4c dirty-draft flush → 4d send flush** → sweep → post-sync flag replay | [VERIFIED: src-tauri/src/sync/worker.rs:1221-1247], [VERIFIED: src-tauri/src/sync/worker.rs:1373-1419] |

**UIDVALIDITY epoch gating on every assertion (structural):** capture `select_mailbox(sandbox).uid_validity` at run start and before each assert block; abort the run (FAIL, loud) if it ever changes mid-run — every replay path drops/ignores stale-epoch ops by design [VERIFIED: src-tauri/src/sync/worker.rs:240-250], so a mid-run epoch bump invalidates all queued-UID assertions. The inconsistency guard (`EXISTS>0` + empty SEARCH → loud error, never wipe) [VERIFIED: src-tauri/src/sync/worker.rs:1301-1310] is the model for harness fail-loud behavior.

**Test-only folder override mechanics (D-04):** `save_draft_copy_in(&sandbox_wire, msg_id, bytes, old_uid, epoch)` and `search_draft_uid` take the folder as a plain `&str` — passing the sandbox wire name instead of the Drafts wire name needs **zero prod changes**; the one-copy sequence (APPEND→SEARCH→expunge-old) is folder-agnostic under one held lease [VERIFIED: src-tauri/src/imap/manager.rs:463-488]. Same for `discard_server_copy_in` and `set_seen_in` (which already takes any `mailbox`) [VERIFIED: src-tauri/src/imap/manager.rs:212-229]. The bin must ALSO seed the temp Store's mailbox row + sync-state epoch for the sandbox (`ensure_mailbox` + `set_sync_state`) so `replay_dirty_drafts`/`replay_*` find their rows — mirroring what `sync_with_borrowed` Step 1 does [VERIFIED: src-tauri/src/sync/worker.rs:1227-1240].

## Harness Skeleton

```
scripts/live-22.sh                      # D-13/D-14/D-15/D-03: env source → keyring seed → run bin → scan → cleanup retry
src-tauri/src/bin/live_gate_22.rs       # owns SessionManager + temp Store; STEP lines; exit 0/1/2
src-tauri/.live-test.env                # git-ignored (already ignored [VERIFIED: .gitignore:73-74]); NEVER committed
```

**Bin step order (each prints `STEP n/N <name>: PASS|FAIL <detail>`, abort on first FAIL except cleanup which always runs):**

1. `preflight` — env present; sandbox name matches `SGE-Test-` prefix (assert in code, not just shell); `LIST` snapshot taken; sandbox absent from LIST; `SELECT` sandbox impossible (not created yet). Store snapshot-UlDs for mid-run LIST-diff.
2. `create` — `create_mailbox_in(&sandbox)`; re-LIST: exactly one delta (the sandbox); record `uid_validity` from `lease_for(&sandbox)` SELECT.
3. `guard-empty` — `search_uids` on sandbox lease == empty (D-02 exists-and-empty).
4. `append-1` — `save_draft_copy_in(&sandbox, msg_id_run1, &bytes_v1, None, Some(epoch))` → `uid_a`; SEARCH Message-ID == `[uid_a]`.
5. `one-copy` — second save `save_draft_copy_in(&sandbox, msg, &bytes_v2, Some(uid_a), Some(epoch))` → `uid_b`; SEARCH == exactly `[uid_b]` (count 1 AND identity == tracked).
6. `kill-reconcile` (D-06) — direct leg calls on the session: `append_message` v3 → close gate/drop lease + `manager.reconnect()` → `uid_search_header` → max-wins reconcile → expunge all but max → SEARCH == 1 copy. (Drives the MJ-03 resume shape without a second connection.)
7. `offline-edit` (D-07) — close `GateSession` writes; dirty-draft row queued (`replay_dirty_drafts` would fail/skip); reopen; `sync_with_borrowed` full pass; SEARCH == 1 UID with latest content.
8. `content` (D-08) — `fetch_body(uid)` (BODY.PEEK[]); parse with `mail-parser`; assert subject == `SGE-TEST-…` last-save subject AND body == last-save body.
9. `replay-order` (D-10) — with gate closed, enqueue one `imap_outbox` op (delete/move within sandbox) + capture `pending_count>0`; reopen; run `sync_with_borrowed`; assert log order `imap replay:` line precedes sweep lines AND post-pass `imap_outbox_count==0` (grep the bin's own stderr transcript — the worker already emits these lines [VERIFIED: src-tauri/src/sync/worker.rs:1389-1393]).
10. `delete-double-sync` (D-11) — offline delete → reconnect sync #1 (op acked, row tombstoned/hidden) → sync #2 → assert UID absent from `search_uids` AND local row gone (no resurrection by backfill; tombstone join excludes it [VERIFIED: src-tauri/src/sync/worker.rs:1452-1482]).
11. `seen-both-ways` (D-12) — `set_seen_in(&sandbox, uid, true)` → FLAGS assert `\Seen`; `set_seen_in(&sandbox, uid, false)` → FLAGS assert absent; repeat with gate closed then `replay_outbox` (pending-wins both directions).
12. `no-unprefixed-send` (D-16) — audit temp send_queue/outbox tables + captured logs: every subject contains `SGE-TEST-`, every recipient == `SGE_LIVE_SELF`; FAIL otherwise. (This phase sends no SMTP; the audit covers drafts-rendered mail + proves the discipline.)
13. `cleanup` — `discard_server_copy_in`/expunge leftovers → `delete_mailbox_in(&sandbox)` → re-LIST equals pre-run snapshot; retry N=3 with backoff (5s, 15s, 45s — discretion recommendation), then print exact manual steps (folder name, webmail path) and exit 1. Cleanup ALWAYS runs (Drop guard / finally), even after mid-run FAIL.

**Shell wrapper (`scripts/live-22.sh`) responsibilities only:** `set -a; . src-tauri/.live-test.env; set +a` (D-13) → refuse if file missing/unreadable → keyring-seed-once via a tiny `seed_keyring` bin path or `KeyringStore::save` through the live bin's `--seed-only` mode (uses `KeyringStore::save` + `save_server_config` [VERIFIED: src-tauri/src/creds.rs:102-115,146-151]) → run bin capturing `live-22.log` → secret scan → propagate bin exit code.

## Credential Safety (D-13/D-14/D-15)

- **Transport:** env vars only (bin reads `std::env::var`; `sync_diag` already refuses `--password` argv [VERIFIED: src-tauri/src/bin/sync_diag.rs:88-91] — copy that refusal verbatim). Never print env values; errors are plain-language (existing `SyncError` Display carries no secrets by Phase 6/12 convention [ASSUMED — verify at plan time by grepping `SyncError` Display impls for password/host]).
- **Keyring-seed-once:** `--seed-only` writes `SavedCredentials{username, password}` via `KeyringStore::save` and `ServerConfig{host, port, security}` via `save_server_config`; the run itself loads via the keyring path (`load_account_config` prefers in-memory, falls back to keyring [VERIFIED: src-tauri/src/commands/sync.rs:36-79]) — to exercise the REAL path, the bin must NOT hold in-memory creds: construct `AccountConfig` from keyring-loaded values only.
- **Secret scan (D-15) — FAIL iff any match:**
  - Scan targets: `live-22.log` (bin stdout+stderr), `RUST_LOG`/eprintln transcript, SQLite temp dump (`.dump` of all tables), the harness's own shell trace (`set -x` output OFF — never enable it while env is sourced).
  - Scan substrings: full `SGE_LIVE_IMAP_PASS` value, full `SGE_LIVE_SMTP_PASS` value, `SGE_LIVE_IMAP_USER` if it contains a non-public local-part [ASSUMED policy — planner confirms: safest is to scan username too], and the keyring JSON blob prefix. Compare by fixed-substring grep (`grep -F -q`), never regex (passwords contain regex metachars).
  - Password must be read from env ONCE into a shell variable for the scan AFTER the bin exits; unset it before any `echo`. The bin itself holds the password in `zeroize::Zeroizing` (`AccountConfig.password` type [VERIFIED: src-tauri/src/commands/sync.rs:37-49]).
  - FAIL = nonzero exit + `SECRET-LEAK` line naming WHICH artifact matched (not the secret itself).

## Sandbox Safety Mechanics (D-01/D-02/D-03/D-04/D-16)

- **Name discipline:** `SGE-Test-<unix-timestamp>-<pid>`; a `const SANDBOX_PREFIX: &str = "SGE-Test-"` in the bin; EVERY mutating call asserts `wire.starts_with(SANDBOX_PREFIX)` before touching the session (defense in depth beyond the shell). Account-root level (no parent folder to create/delete).
- **Pre-flight guard (D-02):** (1) prefix check; (2) `LIST` must NOT contain the name (fresh timestamp → absent; present = previous failed cleanup → abort with manual steps, do NOT reuse); (3) after CREATE, `search_uids` must be empty before first APPEND; (4) LIST snapshot (sorted wire-name set) stored; before each subsequent mutating step, re-LIST and diff — any delta other than the sandbox itself aborts the run (detects external interference / wrong-folder writes).
- **Existing-mail protection:** the bin never SELECTs/APPENDs/STOREs/EXPUNGEs any folder except the sandbox; `INBOX` is touched only implicitly by `create_mailbox_in`'s stable INBOX lease (SELECT-only, read-only posture [VERIFIED: src-tauri/src/imap/manager.rs:291-305]) — document this as the single accepted exception. Test drafts never enter the real Drafts folder (override param, D-04).
- **Cleanup (D-03):** expunge-all-in-sandbox (STORE Deleted + UID EXPUNGE per UID, reusing `discard_server_copy_in`) → `delete_mailbox_in` → LIST-diff-against-snapshot assert. Retry 3× (5s/15s/45s backoff — discretion recommendation, matches send-queue backoff-base idiom of seconds-scale waits [VERIFIED: src-tauri/src/send_queue.rs:31-36]); then print: folder name, "delete via webmail or any client: folder list → SGE-Test-<ts> → delete", and "verify with LIST — rerun preflight". Never loop forever.

## Test Matrix (D-decision → concrete assertion)

| D | Assertion | How (verb / entry point) | Pass criterion |
|---|-----------|--------------------------|----------------|
| D-01 | Sandbox lifecycle | `CREATE` → run → `DELETE` + LIST-diff | Post-run LIST == pre-run snapshot |
| D-02 | Pre-flight guard | prefix assert + absent-in-LIST + empty-SEARCH + snapshot diff per step | Abort-before-mutation on any violation |
| D-03 | Cleanup discipline | retry 3× backoff → manual steps | Sandbox gone, or exact manual instructions printed |
| D-04 | Override, no real Drafts | pass sandbox wire to `save_draft_copy_in` | Real Drafts folder never SELECTed for write (assert via select-call log) |
| D-05 | One-copy count+identity | `UID SEARCH HEADER Message-ID` after N saves + reconnect | Exactly 1 UID AND == tracked `server_uid` |
| D-06 | Kill-session reconcile | APPEND → drop lease/reconnect → SEARCH-max → expunge-rest | Collapses to 1 copy, tracked UID updated |
| D-07 | Offline edit wins | gate closed → queue → reopen → `sync_with_borrowed` | 1 copy, content == latest save |
| D-08 | Content fidelity | `UID FETCH BODY.PEEK[]` + mail-parser compare | subject AND body == last save |
| D-09 | Deterministic offline | `GateSession` write-fail decorator | No OS/network manipulation in scripts |
| D-10 | Pre-sweep replay order | `replay_imap_outbox` before sweep; pending_count>0 → 0 | Log order + end-state depth 0 |
| D-11 | No resurrection | delete → sync#1 → sync#2 | UID absent both passes; local row gone |
| D-12 | Seen both directions | `UID STORE ±Seen` + FLAGS check, online AND via replay | `\Seen` present after read, absent after unread |
| D-13 | Env-only creds | shell sources file; bin refuses `--password` | No secret in argv/history/proc |
| D-14 | Keyring-first auth | `--seed-only` then keyring-load run | Run succeeds with env password unset for bin (only seed step sees it) |
| D-15 | Secret scan gate | `grep -F` log + SQLite dump | Zero matches or run FAILs |
| D-16 | Only-self test mail | audit subjects/recipients | All subjects prefixed, all recipients == self |

## Standard Stack

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| async-imap | 0.11 (resolves 0.11.3) | IMAP wire verbs | Pinned by Phase 1; vendored method names confirmed in-repo |
| mail-parser | 0.11 + full_encoding | Content-assert parsing | Already the drafts round-trip codec |
| rusqlite | 0.37 bundled + rusqlite_migration 2.x | Temp Store for the run | Same store the replay paths query |
| keyring | 3 + sync-secret-service | Real credential path | Phase 5 contract; D-14 exercises it |

No new dependencies. The bin is a new `[[bin]]` entry mirroring `sync_demo`'s declaration [VERIFIED: src-tauri/Cargo.toml:21-23].

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Draft APPEND+reconcile | Custom APPEND/SEARCH sequence in the bin | `save_draft_copy_in` / `save_draft_on_session` | Retry-resume (MJ-03), epoch-drop, zero-vs-multi hit rules already encoded |
| Replay ordering | Manual STORE-then-sweep scripting | `sync_with_borrowed` full pass | Pre-sweep/post-sync positions are load-bearing; reordering resurrects |
| SeenFlag/Delete queueing | Direct STORE for offline steps | `replay_outbox` / `replay_imap_outbox` with live_uids | Epoch-drop + absent-drop + Refused-drop rules prevent wrong-message writes |
| MIME render for drafts | String concatenation | `render_draft_rfc5322` + `new_message_id` | Injection guard + stable-ID discipline the SEARCH reconciles on |

## Common Pitfalls

1. **Second IMAP connection:** opening `connect_sync` alongside the manager breaks the one-connection invariant and races SELECT state. The bin must construct ONE `SessionManager` and use only `lease_for`/manager `*_in` verbs (the `start_sync` command is the template [VERIFIED: src-tauri/src/commands/sync.rs:120-147]).
2. **Non-PEEK FETCH in asserts:** any `BODY[]`/`RFC822` fetch sets `\Seen` and corrupts the D-12 assertions. `fetch_body` (PEEK) only.
3. **`INBOX` as default mailbox:** several commands default `mailbox` to `"INBOX"` [VERIFIED: src-tauri/src/commands/sync.rs:209,378]. The bin must thread the sandbox wire name through EVERY call explicitly — no defaults.
4. **UIDVALIDITY caching:** `lease.selected_validity()` vs store epoch — capture fresh per step; a stale cached epoch silently disables the guard (guard is `None`-means-proceed [VERIFIED: src-tauri/src/imap/manager.rs:835-840]).
5. **`set -x` with sourced secrets:** shell tracing prints env values. Keep tracing off, or trace only after unsetting password vars.
6. **APPENDUID assumption:** async-imap's `append` returns `()` and swallows APPENDUID — never try to read the new UID from APPEND; always SEARCH Message-ID [VERIFIED: src-tauri/src/imap/mod.rs:361-365].

## Validation Architecture

`workflow.nyquist_validation` is `true` [VERIFIED: .planning/config.json — `"nyquist_validation": true`], but this phase is a **live-gate verification phase, not a feature phase**: its "tests" are the scripted live steps themselves (each STEP line is a gate). Recommend the planner treat the harness determinism gates as Wave 0 instead of unit tests:
- Wave 0: `GateSession` decorator unit tests (offline-closed fails writes, open forwards) + sandbox-prefix assert tests + Message-ID/`new_message_id` stability tests — all runnable offline in CI.
- Live waves: the STEP sequence above, each a UAT-style gate executed by a human with creds (`checkpoint:human-verify` before first live run).

## Security Domain

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | Yes | Keyring-first (D-14); env-seed-once; no plaintext at rest |
| V3 Session Management | Yes | Single SessionManager-owned session; leases only |
| V4 Access Control | Yes | Sandbox-prefix assert on every mutating call; pre-flight LIST guard |
| V5 Input Validation | Yes | `new_message_id` atom restriction; UID-set fail-closed parsing |
| V6 Cryptography | Partial | TLS via existing `connect_sync` paths; no new crypto |

Threat patterns: credential leakage via logs/argv (mitigated: env-only + scan gate D-15); wrong-folder destructive op (mitigated: prefix assert + LIST diff D-02); cross-run interference (mitigated: unique timestamp+pid sandbox names).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | No `SGE_FORCE_OFFLINE`-style flag exists in prod code (grep found none) | Approaches Compared | If one exists, option analysis changes |
| A2 | `SyncError` Display never carries secrets (Phase 6/12 convention) | Credential Safety | Scan list must widen to cover error strings |
| A3 | Username sensitivity policy (scan it or not) | Credential Safety | Planner must confirm with user |
| A4 | Server supports UIDPLUS (`UID EXPUNGE`); fallback dance exists but untested live here | Verb Dialect | D-06/D-11 steps take the dance path — still correct, just slower |
| A5 | `mail.utfpr.edu.br` permits CREATE/DELETE at account root | Sandbox Safety | Preflight must surface server refusal loudly on first run |

## Open Questions

1. **Cleanup retry N/backoff (D-03 discretion):** recommended 3× (5s/15s/45s). Confirm with user at plan time or accept.
2. **Offline toggle shape (discretion):** recommended harness decorator (option 2). Confirm no objection to NOT touching prod code.
3. **FLAGS-assert read path:** `fetch_envelopes` sweep vs STATUS UNSEEN vs FLAGS-only fetch — planner picks; all are PEEK-safe. Recommendation: `fetch_envelopes("<uid>")` and read the parsed flags (simplest, reuses worker attrs).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Real account + `.live-test.env` | All live STEPS | ✗ (human supplies) | — | MockSession/FakeSession offline gates (CI fallback per sandbox contract) |
| OS keyring (Secret Service) | D-14 seed + run | [ASSUMED — Linux Secret Service present] | — | `checkpoint:human-verify`; MemoryStore for decorator unit tests only |
| Network to mail.utfpr.edu.br:993 | All live STEPS | [ASSUMED — verified at run time by preflight] | — | Abort loudly; offline doubles cover logic |

## Sources

### Primary (HIGH)
- `src-tauri/src/imap/mod.rs` (`SyncSession` trait, verb forms, DRAFT_FLAGS, fetch_body PEEK) — read this session
- `src-tauri/src/imap/manager.rs` (`save_draft_copy_in`/`save_once`/`search_draft_uid`/`expunge_old_only`, `set_seen_in`, `uid_expunge_in`, `create/delete_mailbox_in`, FakeSession pattern) — read this session
- `src-tauri/src/sync/worker.rs` (`replay_outbox`, `replay_imap_outbox`, `replay_dirty_drafts`, `sync_with_borrowed` order, tombstoning) — read this session
- `src-tauri/src/commands/sync.rs` (`save_draft`/`set_seen`/`delete_message` command shapes, `load_account_config`, `manager_for`) — read this session
- `src-tauri/src/bin/sync_diag.rs` / `sync_demo.rs` (harness skeleton, env-only password, exit codes) — read this session
- `src-tauri/src/creds.rs` (KeyringStore save/load_server_config) — read this session
- `.planning` CONTEXT/REQUIREMENTS/ROADMAP/PROJECT + `.live-test.env.example` + `.gitignore` + `config.json` — read this session

## Metadata

- Standard stack: HIGH — pins read from Cargo.toml + STACK references in code comments this session
- Architecture: HIGH — harness shape derived from in-repo binaries and manager/lease structure
- Pitfalls: HIGH — each cites the exact code path that motivates it
- Research date: 2026-10-13. Valid until: 30 days (stable domain — IMAP dialect + in-repo APIs).
