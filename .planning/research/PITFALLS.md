# Pitfalls Research: v1.1 Triage & Folders (adding writes, folders, polling, backfill to a read-only IMAP client)

**Domain:** IMAP email client — incremental v1.1 capabilities on an existing read-only single-session client
**Researched:** 2026-10-04
**Confidence:** HIGH (RFC 3501/4549/4551/7162/4315/2177 semantics + Dovecot/Evolution implementation history)

## Critical Pitfalls

### Pitfall 1: Treating \Seen toggle as "just a STORE" without a same-source-of-truth flag-reconcile pass

**What goes wrong:**
User taps read/unread, app fires `UID STORE +FLAGS \Seen`, updates local DB optimistically — but the next header sync overwrites the local flag with stale server data (or vice versa), so the toggle visibly flaps: read → unread → read. Worse, if the FETCH during sync uses non-UID sequence numbers, an intervening EXPUNGE shifts sequence numbers and the flag lands on the wrong message.

**Why it happens:**
M1 was read-only, so the sync path was designed as "server is truth, overwrite local." The first write feature breaks that assumption, but developers bolt STORE onto the side without changing sync to reconcile (compare-and-merge) instead of overwrite. Sync also commonly fetches flags by sequence number rather than UID, which is only safe inside a single SELECT with no expunge in between.

**How to avoid:**
- Always address flag writes by UID (`UID STORE <uid> ±FLAGS (\Seen)`), never by sequence number.
- Sync must FETCH `UID + FLAGS` together and merge: apply server flags except for UIDs with a locally-pending (unacked) toggle; once STORE returns OK, clear pending and accept server state.
- Never use plain `FETCH BODY[]`/`FETCH FLAGS` (implicit `\Seen` side effect on some servers for BODY[] — use BODY.PEEK discipline from M1; for flags-only FETCH there is no Seen side effect, but keep the habit of explicit `.SILENT` or UID STORE to suppress noisy untagged FETCH storms).
- If server advertises CONDSTORE (RFC 7162), prefer `UID STORE … UNCHANGEDSINCE <modseq>` for toggles so a concurrent change from another client/phone returns `MODIFIED` instead of silently clobbering; on MODIFIED, re-fetch flags and re-apply user intent or surface conflict.

**Warning signs:**
- Flag state visibly flaps after toggle + sync.
- "Wrong message marked read" bug reports (sequence-number addressing + expunge shift).
- STORE issued with sequence numbers anywhere in the codebase.

**Phase to address:**
Flag-sync phase (first v1.1 phase) — reconcile logic must land in the same phase as the first STORE, not "later."

---

### Pitfall 2: Single global UIDVALIDITY / UIDNEXT / high-water-mark applied to all folders

**What goes wrong:**
App stores one `uidvalidity` / `last_uid` in SQLite, then adds Sent/Drafts/custom folders reusing the same row. Opening a second folder either (a) compares the wrong UIDVALIDITY and needlessly wipes/re-downloads, or (b) misses a real UIDVALIDITY change and shows stale/wrong messages matched to recycled UIDs. Folder rename on another client looks like "folder vanished + unknown folder appeared" and orphans local rows.

**Why it happens:**
M1 only knew INBOX, so per-mailbox state got modeled as global app state. RFC 3501/9051 UIDVALIDITY is per-mailbox, and UIDNEXT/high-water-mark are meaningless across mailboxes. Developers also forget that LIST must be re-queried: folder set is dynamic.

**How to avoid:**
- Schema rule: every piece of sync state is keyed by folder — `(mailbox_name, uidvalidity, uidnext, highest_uid_seen)` at minimum; messages table gets a `folder` column (or folder id FK) in the same migration that adds folder browsing.
- On every SELECT: compare returned UIDVALIDITY to stored per-folder value; on mismatch, purge only that folder's cached messages + drop its queued flag actions (RFC 4549 §3-d-1), then full re-fetch that folder.
- Re-run LIST on each sync/poll cycle (cheap) and handle: renamed folder = delete-old-rows + fresh sync under new name; deleted folder = purge local rows (or tombstone) rather than showing a ghost folder forever.
- Remember the existing stack constraint: imap-proto 0.16 cannot parse NAMESPACE — keep profiling folders from CAPABILITY + LIST-EXTENDED responses only; never branch on NAMESPACE data.

**Warning signs:**
- Switching folders shows INBOX messages, or unread counts leak across folders.
- One `sync_state` row without a folder column in code review.
- Folder list is fetched once at login and never refreshed.

**Phase to address:**
Folder-browsing phase — the per-folder sync-state migration is a prerequisite task in that phase, before any multi-folder FETCH.

---

### Pitfall 3: Overlapping syncs on one connection — poll timer fires while a sync (or STORE) is still in flight

**What goes wrong:**
Poll interval elapses mid-sync; app issues a second SELECT/FETCH on the same single `async-imap` session (or sends a command while IDLE is active without DONE). Responses interleave, untagged FETCH/EXPUNGE lines get attributed to the wrong operation, UI shows duplicates or drops messages. With `async-imap 0.11`'s single-session SyncEngine there is exactly one command pipeline — concurrent use is a data race even if it compiles (requires `&mut` juggling or panics/deadlocks on a shared session).

**Why it happens:**
Polling looks trivially easy (`tokio::time::interval` + `sync()`), so it gets added without a sync mutex/queue. Developers also mix IDLE-style expectations with poll code, or forget RFC 2177's rule that nothing may be sent while the server waits for DONE.

**How to avoid:**
- Single-flight guard: one async Mutex (or command queue / actor) around the entire SyncEngine session; poll tick that finds sync-in-progress either skips (and records "dirty → sync again after") or coalesces — never runs concurrently.
- Decide poll vs IDLE explicitly: v1.1 scope is poll + manual refresh (NOOP/SELECT-based). If IDLE is ever added, it needs its own dedicated connection; the poll timer must DONE/close IDLE before issuing any command.
- Manual refresh button must go through the same single-flight gate as the timer (shared `request_sync()` entry point).
- Re-issue logic: RFC 2177 advises re-issuing IDLE ≥ every 29 min; for poll, pick a conservative default (e.g. 60–120 s, user-configurable) and add jitter so reconnect storms don't hammer the server.

**Warning signs:**
- Two tasks holding the IMAP session handle; `select`/`fetch` called from timer callback directly.
- Intermittent duplicate or missing messages that only reproduce under slow networks (long sync overlapping next tick).
- IDLE `DONE` never sent before next command (protocol error / server BYE).

**Phase to address:**
Poll-refresh phase — single-flight + shared entry point are acceptance criteria of that phase.

---

### Pitfall 4: Treating every UID gap as "missing, fetch it" — confusing expunged UIDs with never-seen UIDs, and racing UIDNEXT

**What goes wrong:**
Backfill logic computes `missing = (max_seen+1..UIDNEXT) − present` naively and FETCHes each gap UID. For UIDs that were expunged (deleted elsewhere) the server returns nothing, so the gap is "still missing" next cycle → app re-requests forever (sync never converges, log spam, battery drain). Conversely, sampling UIDNEXT, then FETCHing, then assuming contiguity misses arrivals between the two commands (UIDNEXT race) — new mail silently skipped until next full poll.

**Why it happens:**
UIDs are dense-in-practice so gaps feel like errors; developers forget expunge creates permanent holes (RFC 3501: UIDs are never reused within a UIDVALIDITY epoch, holes are normal). UIDNEXT is a prediction, not a snapshot — it can advance between any two commands.

**How to avoid:**
- Backfill protocol per folder: `SELECT` → record `UIDNEXT_u1` → `UID FETCH known_range` (or `UID SEARCH ALL`) to learn the true present-set → missing = expected_range − present − known_expunged; fetch only those; then re-check UIDNEXT (`u2`); if `u2 > u1`, fetch `u1..u2` arrivals explicitly. Never loop forever on a UID the server repeatedly returns nothing for — after N (e.g. 2) empty results, record it as expunged/tombstoned in SQLite and stop asking.
- Distinguish three states per UID in local store: `present`, `expunged` (server confirmed gone or repeatedly empty), `unknown` (never observed). Backfill only targets `unknown`.
- Do NOT use `1:*` sequence FETCH to "fill gaps" — sequence numbers shift under expunge; always UID-addressed FETCH.
- Cap backfill range size per cycle (e.g. fetch in chunks of 50–100) so a huge gap after offline weeks doesn't block the UI or OOM the session parser.

**Warning signs:**
- Sync loop never reports "up to date"; same UIDs re-fetched every poll.
- New mail intermittently missed right after backfill runs (UIDNEXT sampled too early).
- `FETCH 1:*` or sequence-range FETCH in backfill code.

**Phase to address:**
UID-backfill phase — convergence test (poll twice, second poll issues zero FETCHes) belongs in that phase's success criteria.

---

### Pitfall 5: Offline flag toggle lost or double-applied — no durable outbox for writes made while disconnected

**What goes wrong:**
User toggles read/unread with no connection (or session expired mid-STORE). App either drops the intent (toggle silently reverts on next sync — user thinks app is broken) or retries blindly on reconnect and double-applies / applies to a stale UID after a UIDVALIDITY change (flag lands on a different message or errors confusingly).

**Why it happens:**
M1 never needed a write queue — everything was a read. First-write feature inherits "fire and forget STORE; on error, show toast." RFC 4549 §5 explicitly calls out playback error recovery as the trickiest part: operations on no-longer-existing messages, and pending actions invalidated by UIDVALIDITY change.

**How to avoid:**
- Durable pending-ops table in SQLite: `(folder, uid, op=±Seen, created_at, attempts)` written before the optimistic UI update; a replay worker drains it on reconnect in order.
- Playback rules (RFC 4549 §5.1): on per-op failure because message no longer exists → silently drop that op (not abort whole queue); on UIDVALIDITY mismatch for the folder → drop all queued ops for that folder (they reference dead UIDs) and notify once; cap retries with backoff, then surface "N changes couldn't sync" with a retry button.
- Optimistic UI must mark toggled rows as "pending" (subtle indicator) until STORE ACK clears the queue entry — so revert-on-failure reads as honest state, not a glitch.

**Warning signs:**
- No table/queue for pending writes; STORE errors only toasted.
- Toggle-while-offline silently does nothing (no outbox row).
- After UIDVALIDITY regeneration, queued STOREs applied to recycled UIDs.

**Phase to address:**
Flag-sync phase (outbox + replay designed alongside first STORE); poll phase adds "drain outbox on reconnect" trigger.

---

### Pitfall 6: Session expiry treated as fatal / login replayed from plaintext instead of keyring + clean re-SELECT

**What goes wrong:**
Server closes idle connection (common: 30-min inactivity timeout; explicitly noted in RFC 2177 for IDLE, and aggressive on some providers). App shows "disconnected" permanently, or crashes on use-after-close, or — worst — caches the password in memory/plaintext to "reconnect quickly," violating the keyring-only constraint. After reconnect, app resumes FETCHing with pre-disconnect sequence numbers or cached UIDNEXT, missing everything that arrived during the gap.

**Why it happens:**
M1's happy path keeps one long-lived session from login; expiry paths were never exercised. Reconnect feels like an edge case until polling keeps a client connected for hours.

**How to avoid:**
- Health-check each poll tick: cheap `NOOP` (RFC 3501 §6.1.2 — explicitly designed as periodic poll/keepalive) before sync; on failure, full reconnect: re-auth using keyring credentials (never cached plaintext), re-SELECT folder, re-read UIDVALIDITY + UIDNEXT, then incremental sync — not resume-mid-stream.
- Session wrapper: `ensure_connected()` that every sync/poll/STORE path calls; exponential backoff on repeated failures; surface "reconnecting…" state in UI rather than error toast per tick.
- Clear in-memory auth material on disconnect; re-read from keyring per reconnect (keeps the M1 keyring decision intact).

**Warning signs:**
- Password held in a global/static for reconnect; any plaintext credential file.
- Sync-after-reconnect uses stale sequence numbers or skips UIDVALIDITY check.
- First manual "leave app open 1 hour" test never performed.

**Phase to address:**
Poll-refresh phase (reconnect + NOOP health check are core poll-phase tasks).

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Poll only INBOX, backfill/folders reuse INBOX code path with a folder-name parameter but shared sync-state row | Folders "work" fast | Cross-folder state corruption (Pitfall 2) | Never — per-folder state from the start |
| Optimistic flag toggle with no pending-ops table ("add queue later") | Flag sync ships in days | Silent loss of offline toggles; no conflict story (Pitfalls 1, 5) | Never for writes — queue is part of the write feature |
| Fire-and-forget STORE without checking MODIFIED / re-fetch | Simpler toggle code | Lost updates when phone + desktop both flag (Pitfall 1) | Only if CONDSTORE absent AND single-client assumption documented; revisit when multi-client reports appear |
| `FETCH 1:*` full re-download as "backfill" | No gap logic to write | Bandwidth/CPU blowup on large UTFPR mailboxes; UI jank | Only as a manual "repair" button, never the automatic path |
| Fixed 10 s poll interval for "real-time feel" | Feels instant in demo | Server throttling/BYE, battery drain, hammering on metered links | Never as default; 60–120 s default + manual refresh; sub-30 s only user-opt-in |
| Single global "last sync" timestamp instead of per-folder UIDNEXT/modseq | Less schema churn | Cannot resume per folder; one slow folder stalls all | Never — schema cost is one migration |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|---------------|------------------|
| Dovecot / generic IMAP server (flag writes) | Assuming STORE +FLAGS always succeeds; ignoring `MODIFIED` / `NO` responses | Check tagged response; handle MODIFIED by re-fetching flags; treat `NO [UIDNOTSTICKY]`/read-only SELECT as "flags not persisted" (re-open read-write or warn) |
| Server without CONDSTORE/QRESYNC (e.g. minimal/older servers) | Requiring HIGHESTMODSEQ/CHANGEDSINCE unconditionally → sync breaks on capable-poor servers | Capability-gate: use CONDSTORE fast path when advertised, fall back to UID+FLAGS full-range FETCH diff otherwise |
| async-imap 0.11 session | Sharing `&mut Session` across timer + UI tasks; sending while IDLE active | Single owner + async Mutex/actor; commands only when no IDLE outstanding (DONE first); dedicated connection if IDLE ever adopted |
| imap-proto 0.16 parser | Parsing NAMESPACE / exotic LIST-EXTENDED responses; unhandled untagged responses crash sync loop | Keep CAPABILITY+LIST profiling (existing M1 decision); tolerate-and-ignore unknown untagged data; keep regression tripwire test |
| OS keyring (reconnect auth) | Caching password in memory indefinitely for fast reconnect | Re-read from keyring on each reconnect; zero in-memory copies after disconnect |
| SQLite store | One `sync_state` row; messages without folder FK; no pending-ops table | Migrate: `folders(name PK, uidvalidity, uidnext, highest_seen)` + `messages(folder, uid, …, UNIQUE(folder,uid))` + `pending_ops(id, folder, uid, op, attempts)` |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Full-folder UID+FLAGS FETCH every poll | Poll latency grows linearly; UI stalls on big INBOX | Incremental: `UID FETCH last_seen:UIDNEXT` + flag-diff only for known range; full FETCH only on UIDVALIDITY change | Breaks at ~5–10k messages per folder on poll cadence |
| Unbounded backfill range in one FETCH | Multi-second hangs, large allocations in imap-proto parse | Chunk backfill (50–100 UIDs/command), yield to UI between chunks | Breaks after offline weeks / first sync of huge Sent folder |
| Poll + backfill + flag-replay all firing on reconnect | Thundering-herd: 3 full syncs back-to-back | Single `request_sync(reason)` coalescing entry; reconnect = one ordered pass: connect → per-folder incremental → drain outbox → backfill | Breaks on every laptop-wake/reconnect |
| Re-LIST + re-SELECT every folder every tick | N folders × round trips per poll; slow on high-latency links | LIST refresh at slower cadence (e.g. every N polls or on manual refresh); per-tick only sync visible/selected folder + INBOX | Breaks with 20+ folders on slow links |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Storing password outside keyring for reconnect convenience | Credential theft from disk/memory dump | Keyring-only (M1 constraint restated); re-read per reconnect; no plaintext fallback |
| Using sequence-number FETCH/STORE after reconnect | Flags/bodies attributed to wrong messages (integrity, not just cosmetic — could mark wrong mail read in a shared mailbox) | UID-only addressing for all post-connect operations |
| Applying queued STOREs after UIDVALIDITY change | Mutating wrong messages on recycled UIDs | Drop folder's queue on UIDVALIDITY mismatch (RFC 4549) + user-visible notice |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Toggle with no pending indicator; silent revert on failure | "App ignores me / is broken" | Optimistic toggle + subtle pending dot; honest revert + inline retry on failure |
| Full-folder spinner on every poll | App feels frozen every minute | Background incremental sync; only badge/list-update, no modal spinner; spinner only on manual refresh |
| Ghost folders after server-side rename/delete | Confusion, taps into empty dead folder | Refresh LIST regularly; remove/rename local folder rows with a one-time notice |
| Backfill progress invisible on huge gaps | "Is it stuck?" after offline weeks | Determinate progress ("Syncing 1,200 of 3,400") with cancel; backfill yields to interaction |
| Poll failure toasts every 60 s on bad network | Notification spam | Quiet reconnecting state; toast only after sustained failure (e.g. 3+ failed polls) |

## "Looks Done But Isn't" Checklist

- [ ] **Flag sync:** Often missing reconcile-on-sync — verify toggle → poll → flag stays; toggle on phone → poll → desktop reflects it
- [ ] **Flag sync:** Often missing expunged-during-STORE handling — verify toggle on a message deleted elsewhere fails silently without breaking the queue
- [ ] **Folders:** Often missing per-folder UIDVALIDITY check — verify change UIDVALIDITY for one folder (or simulate) purges only that folder
- [ ] **Folders:** Often missing LIST refresh — verify server-side create/rename/delete appears/disappears locally after poll
- [ ] **Poll:** Often missing single-flight — verify poll firing mid-sync coalesces instead of overlapping (instrument or slow-network test)
- [ ] **Poll:** Often missing reconnect path — verify kill connection / wait out timeout → next poll reconnects via keyring with no user action
- [ ] **Backfill:** Often missing convergence — verify two consecutive polls after backfill issue zero FETCHes
- [ ] **Backfill:** Often missing expunged-vs-missing distinction — verify deleted-elsewhere UID is tombstoned, not re-requested forever
- [ ] **Offline writes:** Often missing durable outbox — verify toggle offline → kill app → relaunch online → toggle replays

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Flag flap / wrong-message flags | MEDIUM | Stop sequence-addressed STOREs; add UID addressing + reconcile pass; one-time full flag re-FETCH per folder to heal |
| Global sync state corruption across folders | MEDIUM | Migrate schema to per-folder state; wipe + full resync all folders once (bounded, user-warned) |
| UIDVALIDITY miss (stale cache shown) | LOW | Add per-SELECT UIDVALIDITY compare; purge affected folder; re-fetch |
| Runaway backfill (infinite re-FETCH) | LOW | Add tombstone marking + empty-result counter; clear runaway loop; backfill converges next poll |
| Lost offline toggles | HIGH (data already lost) | Ship outbox; cannot recover past intents — apologize via release note; going forward all intents durable |
| Plaintext credential cache added for reconnect | MEDIUM | Remove cache; re-read keyring; rotate password if written to disk; add test asserting no credential file exists |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| 1 — Flag reconcile / UID STORE / CONDSTORE conflict | Flag-sync phase (ships with first STORE) | Toggle stays across poll; cross-client (phone) change reflected; no sequence-number STORE in review |
| 5 — Offline outbox + playback rules | Flag-sync phase (same phase as first write) | Offline toggle → restart → replay; UIDVALIDITY-change drops queue with notice |
| 2 — Per-folder sync state + LIST refresh + rename/delete | Folder-browsing phase (prerequisite migration) | Per-folder rows in schema; folder rename/delete handled; imap-proto NAMESPACE still untouched |
| 3 — Single-flight sync + shared entry for timer/manual | Poll-refresh phase | Overlapping-tick test coalesces; manual + timer share gate |
| 6 — NOOP health check + keyring reconnect + re-SELECT | Poll-refresh phase | Kill-connection test self-heals; no credential outside keyring |
| 4 — Backfill gap semantics + UIDNEXT race + convergence | UID-backfill phase | Double-poll zero-FETCH; expunged tombstoned; chunked FETCH |

Suggested phase order from these pitfalls: **flags (with outbox) → folders (with per-folder state) → poll (with single-flight + reconnect) → backfill (with convergence)**. Backfill last because it depends on per-folder high-water marks (folders phase) and must not fight the poll loop (poll phase).

## Sources

- RFC 3501 / RFC 9051 §2.3.1 — UIDs, UIDVALIDITY, UIDNEXT semantics (HIGH)
- RFC 4549 (Synchronization Operations for Disconnected IMAP4 Clients) §§3–5 — sync algorithm, UIDVALIDITY-mismatch purge, playback error recovery (HIGH)
- RFC 4551 / RFC 7162 (CONDSTORE/QRESYNC) — UNCHANGEDSINCE, MODIFIED response, mod-sequences (HIGH)
- RFC 4315 (UIDPLUS) — UID EXPUNGE semantics for disconnected clients (HIGH)
- RFC 2177 (IDLE) — DONE-before-command rule, 29-minute re-issue, server inactivity timeout (HIGH)
- Dovecot docs + Nylas UIDVALIDITY troubleshooting — signed-32-bit UID bugs, resync-stop on UIDVALIDITY flapping (MEDIUM)
- Evolution/camel-imapx history — NOOP-race crashes, IDLE-cancel SELECT races, DONE-timeout reconnect (MEDIUM)

---
*Pitfalls research for: SGE v1.1 Triage & Folders*
*Researched: 2026-10-04*
