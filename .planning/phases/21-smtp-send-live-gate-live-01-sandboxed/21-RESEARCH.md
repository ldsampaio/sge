# Phase 21: SMTP Send — Live Gate (Live-01, SANDBOXED) — Research

**Researched:** 2026-10-13
**Domain:** Live SMTP send verification harness (Rust bin + shell wrapper) over the Phase 13 send pipeline
**Confidence:** HIGH (behavior under test read verbatim from source this session; harness skeleton is the shipped `live_gate_22.rs` tracer read this session)

## Summary

Phase 21 is verification-only: prove the Phase 13/14 send pipeline delivers against the real `smtp.utfpr.edu.br:587/STARTTLS` inside the sandbox contract. Test mail goes ONLY to `SGE_LIVE_SELF`; arrival is proven by IMAP SEARCH (not SMTP 250); exactly-once is proven by Message-ID SEARCH counts; the Sent copy is proven by SEARCH + `\Seen` + verbatim-bytes compare; offline-flush, `uncertain` reconcile-not-resend, crash recovery, and the draft-send transaction are each proven as separate legs; 25 MB cap + BCC stay in doubles. Cleanup expunges the INBOX arrival + Sent copy and deletes the sandbox folder if used.

**Primary recommendation:** Build `src-tauri/src/bin/live_gate_21.rs` as a line-for-line structural clone of `live_gate_22.rs` (same STEP PASS/FAIL runner, exit 0/1/2, `--seed-only` keyring path, `/tmp` logs, GateSession decorator pattern, pre-flight prefix+LIST-snapshot, cleanup retry 3× 5s/15s/45s), driving the send pipeline through `enqueue_send` → `SyncWorker::flush_send_queue` + `reconcile_uncertain_sends` with a real `SmtpTransporter` and a harness-injected DATA-timeout transport decorator for the `uncertain` leg. Wrap it in `scripts/live-21.sh` cloned from `scripts/live-22.sh` plus a send-audit and an extended secret scan. No new prod code, no new tables, no new IMAP verbs.

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Recipient scope is ONLY the user's own address — envelope AND headers (`SGE_LIVE_SELF`); the run aborts if any other recipient resolves.
- **D-02:** Sent copy APPENDs to the real Sent folder (pipeline behavior, not a sandbox substitute); its UID is tracked and it is expunged in cleanup.
- **D-03:** Pre-flight guard mirrors Phase 22 D-02: `SGE-Test-` prefix asserts + LIST snapshot, abort on unexpected mid-run change.
- **D-04:** Credential flow mirrors Phase 22 D-13/D-14: source env file (`set -a; . file; set +a`) → env vars → seed OS keyring ONCE → all SMTP/IMAP auth reads from keyring (exercises the real Phase 1/5 credential path).
- **D-05:** Arrival asserted via IMAP SEARCH INBOX for the test Message-ID after send + FETCH headers to confirm — proves real delivery, not just SMTP 250.
- **D-06:** Sent copy asserted by SEARCH Sent for the same Message-ID: exactly 1 UID + `\Seen` set + verbatim-bytes compare against the queued `.eml`.
- **D-07:** No-duplicate proof: double-enqueue the same Message-ID offline → flush → assert single delivery (1 INBOX UID); `sent-unfiled` APPEND failure must never trigger a re-SMTP-send.
- **D-08:** Server auto-save probe in scope: if the server auto-saves to Sent, the pipeline must dedupe by Message-ID (no twin copy).
- **D-09:** Offline leg proven live: enqueue under the app-level offline toggle → reconnect → flush with the backoff schedule visible in logs → arrives exactly once.
- **D-10:** `uncertain` reconcile proven via harness-injected DATA-timeout fault: assert `uncertain` state → reconcile-not-resend (SEARCH before any retry).
- **D-11:** Crash leg proven live: kill mid-`sending` → restart → assert `crash_recover` resets to `queued` → flushes once, no duplicate.
- **D-12:** 25 MB cap + BCC envelope-only stay in doubles (oversize rejected pre-SMTP; no BCC header leak in envelope log) — never attempted live.
- **D-13:** Post-run automated secret scan mirrors Phase 22 D-15: `grep -F` over logs + SQLite dump for credential substrings → FAIL the run on hit; env values travel as env vars, never CLI args.
- **D-14:** Send audit: no mail WITHOUT the `SGE-TEST-` subject prefix may leave the box — audit `send_queue` + SMTP transcript for unprefixed recipients/subjects; per-run unique Message-ID.
- **D-15:** Cleanup deletes the test INBOX arrival + Sent copy + sandbox folder (if used); retry 3× (5s/15s/45s, mirroring Phase 22 D-03) then exact manual steps — never blind infinite retry.
- **D-16:** Draft-send transaction (DRAFT-03) proven live: sending a sandbox draft deletes it in the same transaction — assert no orphan draft server-side nor local.

### The agent's Discretion
- Exact backoff-schedule log format asserted in D-09.
- Harness-injected transport-fault mechanism detail (decorator vs flag).
- SEARCH/FETCH verb dialect specifics (UID scoping, BODY.PEEK sections).
- Cleanup retry N/backoff (recommended 3× 5s/15s/45s).

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Live SMTP send + IMAP arrival/Sent asserts | API / Backend (Rust bin) | — | All wire I/O flows through `SmtpTransporter` + `SessionManager` leases; no frontend involvement |
| Pass/fail orchestration + assertions | Backend (Rust bin) | Shell wrapper | Deterministic asserts need lib types (`FlushSummary`, `ReconcileSummary`, `SendRow`); shell only handles env + scan + exit codes |
| Credential sourcing + secret scan + send audit | Shell wrapper | OS keyring | Env-file sourcing and log-grepping are process-level concerns, not app logic |
| Outbox/uncertain/failed visibility (A-01..A-08) | Frontend (existing) | — | D-09/D-10/D-11/D-14 visibility reuses Phase 13/14 `OutboxBadgeWired` + `SendStatus`; no new UI surface in this phase |
| Draft-send orphan check (D-16) | Backend (Rust bin) | — | Harness/log/DB assertion on `drafts` table + sandbox SEARCH, never UI |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| lettre | 0.11 (resolves 0.11.23) [VERIFIED: src-tauri/Cargo.lock — `name = "lettre"`, `version = "0.11.23"`] | Real SMTP transport over fail-closed STARTTLS | Pinned by Phase 13; the live run uses the production `SmtpTransporter`, never a second transport |
| async-imap | 0.11 (resolves 0.11.3) | IMAP SEARCH/FETCH/APPEND/STORE/EXPUNGE verbs for arrival + Sent asserts | Pinned by Phase 1; harness asserts through the same `SyncSession` trait methods Phase 22 used |
| mail-parser | 0.11 + full_encoding | Parse FETCHed bytes for subject/Message-ID compare | Already the drafts round-trip codec; Phase 22 used it for content asserts |
| rusqlite | 0.11 (resolves 0.11.23) | Temp Store for the run (send_queue + drafts rows the flush/replay paths query) | Same store the flush paths query |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| keyring | 3 + sync-secret-service | Real credential path (D-04) | `--seed-only` writes once; run loads via keyring |
| zeroize | 1 | Password handling in bin config | Same as `live_gate_22.rs` `LiveConfig.password` |

No new dependencies. The bin is a new `[[bin]]` entry mirroring the existing declaration [VERIFIED: src-tauri/Cargo.toml:25-27]:
```toml
[[bin]]
name = "live_gate_22"
path = "src/bin/live_gate_22.rs"
```

**Installation:** none — all crates already pinned in `src-tauri/Cargo.toml`.

## Harness Shape: `live_gate_21` as a Structural Clone of `live_gate_22`

Q1 answer — the shared skeleton vs the send-specific legs. The Phase 22 tracer (`src-tauri/src/bin/live_gate_22.rs`, read verbatim this session) is the template; copy its structure, swap the behavior under test.

### Shared skeleton (copy verbatim, rename 22→21)

| Element | Phase 22 shape | Phase 21 reuse |
|---------|---------------|----------------|
| STEP runner | `fn step(n, name, ok, detail) -> bool` printing `STEP n/N <name>: PASS\|FAIL <detail>` [VERIFIED: src-tauri/src/bin/live_gate_22.rs:485-491] | Identical; TOTAL_STEPS grows to cover the send legs below (recommend 10: preflight, offline-enqueue, flush-arrival, sent-copy, no-duplicate, uncertain, crash, draft-send, audit+scan, cleanup) |
| Exit contract | 0 all-pass / 1 assertion FAIL / 2 pre-flight-skip-or-env-missing; `--seed-only` writes keyring once then exits; `--password`/`-p` refused [VERIFIED: src-tauri/src/bin/live_gate_22.rs:419-465,701-746] | Identical, including the `--password` refusal and USAGE text shape |
| Sandbox discipline | `SANDBOX_PREFIX = "SGE-Test-"`, `sandbox_name()` = prefix+timestamp+pid, `assert_sandbox_prefix` before EVERY mutating call [VERIFIED: src-tauri/src/bin/live_gate_22.rs:38-72] | Identical constants and guard function. Note: Phase 21 mutates INBOX (arrival lands there by server delivery, not by harness APPEND) and the real Sent folder (D-02) — the prefix guard covers harness-issued APPEND/STORE/EXPUNGE/CREATE/DELETE; INBOX/Sent *reads* (SEARCH/FETCH) and the tracked-UID cleanup expunges are the two documented exceptions |
| Pre-flight | LIST snapshot; abort if sandbox present (prior failed cleanup → manual steps, never reuse); re-LIST diff per step via `unexpected_list_delta` [VERIFIED: src-tauri/src/bin/live_gate_22.rs:132-147,534-555] | Identical, plus: resolve + record the Sent folder wire (`resolve_sent_wire`) and INBOX/Sent UIDVALIDITY at pre-flight for epoch-scoped asserts |
| Store seeding | Temp-file Store in `/tmp`, seed mailbox row + sync-state epoch so replay entries find rows [VERIFIED: src-tauri/src/bin/live_gate_22.rs:518-605] | Identical; seed rows for INBOX + Sent (+ sandbox if a draft leg uses it) |
| Cleanup | Expunge leftovers reusing `discard_server_copy_in`, DELETE sandbox with retry 3× backoff `[5, 15, 45]`s, post-LIST == pre-run snapshot, `manual_cleanup_steps` on failure, temp DB file removed [VERIFIED: src-tauri/src/bin/live_gate_22.rs:47-49,638-699] | Same retry/backoff shape; extended target list (see Cleanup section): INBOX arrival UID + Sent copy UID expunged first, then sandbox DELETE if a sandbox folder was created |
| Shell wrapper | `scripts/live-22.sh`: env-file source, bin run with `/tmp` log capture, `grep -F` secret scan, exit-code propagation, `set -x` never enabled [VERIFIED: scripts/live-22.sh:1-43] | Clone to `scripts/live-21.sh`; extend scan targets (see Secret Scan section) and add the send-audit step |
| Offline toggle | `GateSession<'a>` decorator over `&mut dyn SyncSession`: reads forward, all write verbs fail with `SyncError::Protocol("live-gate: offline gate closed")` while closed; zero prod-code changes [VERIFIED: src-tauri/src/bin/live_gate_22.rs:149-340] | Reuse `GateSession` verbatim for the IMAP side (Sent APPEND/SEARCH gating in the offline leg). For the SMTP side add a sibling decorator (see Uncertain Leg section) |
| Offline unit tests | Decorator open/closed tests + prefix tests + LIST-delta tests + render/parse tests, all runnable offline in CI [VERIFIED: src-tauri/src/bin/live_gate_22.rs:748-829] | Same Wave 0 shape: transport-decorator tests + prefix tests + audit-grep tests |

### Send-pipeline entry points per leg (Q1 — what each leg drives)

All entry points are the production paths, called from the bin exactly as the command layer calls them:

| Leg (D-ref) | Entry points driven | Assertion |
|-------------|---------------------|-----------|
| Offline enqueue (D-09) | `enqueue_send(conn, app_data, EnqueueInput { from: SGE_LIVE_SELF, to: [SGE_LIVE_SELF], cc: [], bcc: [], subject: "SGE-TEST-…", body, draft_id: None })` — validates, mints Message-ID-at-enqueue via `new_queue_id`, renders immutable `.eml` via `outbox_eml_path` [VERIFIED: src-tauri/src/send_queue.rs:277-387]; `send_status_snapshot` for A-01 (`pending_count ≥ 1`, `queued ≥ 1`) | Badge/state precondition before any network |
| Flush + arrival (D-05/D-09) | `SyncWorker::flush_send_queue(&SendFlushEnv { gate, transport: Arc::new(SmtpTransporter::new()), account (keyring-loaded), manager: Some(mgr), sent_wire: Some(sent) })` — per-row claim `queued→sending`, `send_raw`, filing leg (probe SEARCH → APPEND `\Seen`), `[SGE send] flush done: sent=…` log line [VERIFIED: src-tauri/src/sync/worker.rs:586-651,689-865]; arrival via `uid_search_header("Message-ID", mid)` on an INBOX lease + `fetch_body` header confirm | `FlushSummary.sent == 1`; INBOX SEARCH == exactly 1 UID for the Message-ID; A-02/A-03 (`pending_count → 0`) |
| Sent copy (D-06) | Same flush's filing leg; assert via `uid_search_header("Message-ID", mid)` on a Sent lease (exactly 1 UID) + FLAGS check `\Seen` + `fetch_body(uid)` bytes == queued `.eml` bytes | `FlushSummary.sent_unfiled == 0`; verbatim-bytes equal; A-07 precondition |
| No-duplicate (D-07) | Double-`enqueue_send` with the same `draft_id`/Message-ID (second returns `deduped: true`, one row, one `.eml`) [VERIFIED: src-tauri/src/send_queue.rs:322-325,375-385]; flush; assert 1 INBOX UID. `sent-unfiled` APPEND-failure path: gate Sent APPEND closed → flush → assert `SentUnfiled` outcome with **no second `send_raw` call** (count transport calls in the decorator) | Single delivery; APPEND failure never re-SMTP-sends |
| Uncertain (D-10) | DATA-timeout transport decorator (see next section) → `SmtpOutcome::Uncertain` → `mark_send_uncertain` [VERIFIED: src-tauri/src/sync/worker.rs:900-909]; then `reconcile_uncertain_sends(session, Some(sent_wire), resume_wire)` — SEARCH hit → `sent` with no transport call; SEARCH miss → requeue once with backoff [VERIFIED: src-tauri/src/sync/worker.rs:943-1022] | `uncertain ≥ 1` (A-04) then `confirmed_sent == 1`, transport call count unchanged |
| Crash (D-11) | Set row to `sending` (claim without flush, or kill between claim and verdict), drop state → `crash_recover` flips `sending→queued` [VERIFIED: src-tauri/src/send_queue.rs:441-443] — note the flush-pass wrapper triages stranded `sending` to `uncertain` first [VERIFIED: src-tauri/src/sync/worker.rs:599-623]; then flush → arrives exactly once | Badge reappears as pending not failed (A-05); 1 INBOX UID, 1 Sent UID |
| Draft-send (D-16) | `enqueue_send` with `draft_id: Some(sandbox draft)` (Message-ID reused from the drafts row [VERIFIED: src-tauri/src/send_queue.rs:309-320]) → flush → assert local `drafts` row gone AND sandbox SEARCH for the draft Message-ID empty | No orphan draft local nor server-side — **but see the DRAFT-03 gap flag below** |
| Backoff visible (D-09) | `record_send_failure` schedules `next_retry_at` inside `backoff_bounds` [VERIFIED: src-tauri/src/send_queue.rs:393-405]; growth `backoff_bounds(0) == (24, 36)`, cap `(720, 1080)` [VERIFIED: src-tauri/src/send_queue.rs:697-701] | Assert the `[SGE send] flush done:` line + `record_send_failure` schedule shape in logs (discretion: planner pins the exact log format) |

## SMTP Verb Reality (Q2)

- **Fail-closed handshake:** the only approved constructor is `SmtpTransport::starttls_relay(&host)` with `.port(587).credentials(…).timeout(60s)`; fail-open constructors are banned by a source tripwire test [VERIFIED: src-tauri/src/smtp.rs:284-314,371-396]. The live run exercises this for real against `smtp.utfpr.edu.br:587` — upgrade failure surfaces `Transient` (retryable, never plaintext) via `classify_fault` [VERIFIED: src-tauri/src/smtp.rs:162-168]. Timeout for every command is `SMTP_COMMAND_TIMEOUT_SECS = 60` [VERIFIED: src-tauri/src/smtp.rs:40].
- **Per-account pool:** `SmtpTransporter::account_key(host, port, username)` = `host:port:username`, one pooled client per key [VERIFIED: src-tauri/src/smtp.rs:276-278,544-546]. Single-account run → exactly 1 pooled client; assert `pool_len() == 1` only in offline unit tests (the field is `#[cfg(test)]`), live just relies on the shape.
- **Verdict vocabulary:** `SmtpOutcome::{Sent, Uncertain, Transient, Permanent}` is the single vocabulary the flush reasons about; timeouts and connection-class failures are `Uncertain`, never success and never blind retry [VERIFIED: src-tauri/src/smtp.rs:74-89,128-186]. Error text is static plain language + sanitized numeric reply digits only (`sanitize_code` keeps ≤3 ASCII digits) — secrets can never flow through verdicts [VERIFIED: src-tauri/src/smtp.rs:188-208].
- **Envelope for the self-send:** `build_envelope(SGE_LIVE_SELF, [SGE_LIVE_SELF])` — trims, drops empties, dedupes exact repeats, bare addr-specs only [VERIFIED: src-tauri/src/smtp.rs:216-250]. D-01 abort check happens in the bin BEFORE `enqueue_send`: assert `to == [SGE_LIVE_SELF]`, `cc == []`, `bcc == []`, `from == SGE_LIVE_SELF`, case-insensitively trimmed; any other resolved recipient → FAIL without touching the transport.
- **Harness-injected DATA-timeout fault (D-10, discretion — RECOMMENDATION: decorator, not flag):** a `TimeoutTransport` struct in the bin wrapping `Arc<dyn SmtpTransport>` with an `AtomicBool fault_armed`: when armed, return `SmtpOutcome::Uncertain { reason: "live-gate: injected DATA timeout" }` **without calling the inner transport** (zero SMTP bytes leave the box on the faulted attempt — deterministic and safe); when disarmed, forward to the real `SmtpTransporter`. This exercises the REAL `classify_fault → mark_send_uncertain → reconcile_uncertain_sends` path [VERIFIED: src-tauri/src/smtp.rs:128-136; src-tauri/src/sync/worker.rs:900-909,943-1022] with zero prod-code changes — the exact analogue of Phase 22's `GateSession` recommendation. Count inner-transport calls to prove reconcile-not-resend (reconcile SEARCH hit must not increment the send counter). Alternative (real socket-level DATA stall) rejected: nondeterministic, timing-sensitive, risks a real duplicate on a slow server.

## Arrival / Sent Assertions (Q3)

- **APPENDUID is swallowed — never read the UID from APPEND:** async-imap's `append` returns `()`; all callers reconcile via `UID SEARCH HEADER Message-ID` [VERIFIED: src-tauri/src/imap/mod.rs:361-365]. Every Phase 21 UID assertion follows the Phase 22 rule: SEARCH Message-ID → exactly-one-UID AND identity == tracked UID.
- **Arrival (D-05):** on an INBOX lease, `uid_search_header("Message-ID", mid)` → exactly 1 UID; confirm with `fetch_body(uid)` header parse (Subject starts with `SGE-TEST-`, Message-ID matches). SMTP 250 alone is never the pass criterion — the flush returning `Sent` only opens the assertion window; allow a bounded poll (recommend 3× 10s, then FAIL) for server-side delivery latency.
- **Sent copy (D-06):** on a Sent lease, `uid_search_header("Message-ID", mid)` → exactly 1 UID; FLAGS assert `\Seen` present (the filing leg APPENDs with `"\\Seen"` [VERIFIED: src-tauri/src/sync/worker.rs:734-740]); `fetch_body(uid)` — full RFC822 bytes via `UID FETCH <uid> BODY.PEEK[]`, never sets `\Seen` [VERIFIED: src-tauri/src/imap/mod.rs:309-311] — byte-compare against the queued `.eml` read from `row.eml_path`. Any `BODY[]`/RFC822 non-PEEK fetch is forbidden (it would set `\Seen` and corrupt the assert).
- **Server auto-save probe (D-08):** the filing leg already probes Sent for the Message-ID before APPEND — hit skips APPEND (dedupe, no twin) [VERIFIED: src-tauri/src/sync/worker.rs:717-729]. The live leg asserts the end state (exactly 1 Sent UID) regardless of which path filed it; the bin logs which branch ran (`deduped in Sent` vs `APPENDed to Sent` lines [VERIFIED: src-tauri/src/sync/worker.rs:720-747]) so the report names auto-save vs explicit-APPEND.
- **Seen-assert read path:** reuse Phase 22's recommendation — `fetch_envelopes("<uid>")` returning `MessageHeader` with parsed flags over `UID FETCH <range> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)` [VERIFIED: src-tauri/src/imap/mod.rs:302-307]; alternatively `mailbox_status` UNSEEN datum. Never a non-PEEK body fetch.
- **UIDVALIDITY epoch gating on every assert (structural):** capture `selected_validity()` per SELECT and abort loud on mid-run change — every replay path drops stale-epoch ops by design, so an epoch bump invalidates queued-UID assertions (same rule as Phase 22).

## Offline Toggle + Kill-Mid-Sending + crash_recover (Q4)

- **SendGate vs SyncGate separation is structural:** `SendGate` exists so a send flush never holds the IMAP session while waiting and a sync sweep never blocks a retry; a second `try_begin` while in flight returns `None` (skip, never overlap — double-flush would risk double-SMTP-send) [VERIFIED: src-tauri/src/send_queue.rs:64-96]. The bin constructs its own `SendGate` inside `SendFlushEnv` and asserts the second-`try_begin`-skips shape in Wave 0 unit tests (mirroring the in-repo test [VERIFIED: src-tauri/src/send_queue.rs:509-530]).
- **Offline toggle reuses the Phase 22 `GateSession` decorator pattern:** close the gate around `enqueue_send` (IMAP writes fail, queue rows persist locally), assert `send_status_snapshot` shows `queued ≥ 1` (A-01), reopen, run `flush_send_queue`, assert arrival exactly once. No OS/network manipulation in scripts — same discipline as Phase 22 D-09.
- **Kill mid-`sending`:** the deterministic harness analogue of a crash is: claim a row (`set_send_state(sending)` — the exact first write `flush_one_row` performs [VERIFIED: src-tauri/src/sync/worker.rs:660-670]) then drop all state without verdict (simulating kill before SMTP verdict lands). On "restart", the bin calls `crash_recover` (flips `sending→queued` [VERIFIED: src-tauri/src/send_queue.rs:441-443]) — and note the production flush pass triages stranded `sending` rows to `uncertain` first, NOT back to `queued` [VERIFIED: src-tauri/src/sync/worker.rs:594-623]. The live leg must therefore expect the post-restart state to be `uncertain` (reconcile path), not `queued`: assert `uncertain ≥ 1` → `reconcile_uncertain_sends` → SEARCH INBOX/Sent → exactly-once outcome. A-05's UI expectation (badge pending, never failed) holds either way since `uncertain` feeds neither the failed surface nor a blind resend.
- **Backoff visibility (D-09):** each `record_send_failure` writes `next_retry_at` inside the jittered bounds and the flush emits `[SGE send] flush done: sent=… deferred=… failed=… uncertain=…` [VERIFIED: src-tauri/src/sync/worker.rs:646-650]. Planner pins the exact asserted log format (discretion); at minimum assert the flush-done line exists per pass and `next_retry_at` falls inside `backoff_bounds(attempts)` for any deferred row.

## Secret Scan (D-13) + Send Audit (D-14) + Cleanup (Q5)

### Secret scan targets (extends Phase 22 D-15 — send path adds the SMTP transcript surface)
- Scan artifacts: bin stdout+stderr log (`/tmp/sge-live21-*.log`), SQLite temp dump (`.dump` of ALL tables — `send_queue` holds `from_addr`/`eml_path`, never passwords, but dump it anyway), SMTP-transcript capture if the bin logs one (it must log only host:port + verdict text, never credentials — `SmtpAccount` Debug prints `<redacted>` [VERIFIED: src-tauri/src/smtp.rs:63-72], `SmtpTransporter` Debug prints pool depth only [VERIFIED: src-tauri/src/smtp.rs:324-334]).
- Scan substrings (fixed-substring `grep -F -q`, never regex): full `SGE_LIVE_IMAP_PASS`, full `SGE_LIVE_SMTP_PASS`, full `SGE_LIVE_IMAP_USER` and `SGE_LIVE_SMTP_USER` (recommend scanning usernames too — mailbox addresses appear legitimately in envelope logs, so the planner must decide: scan the *password* values strictly, and scan usernames only against artifacts where they must not appear; simplest correct rule is passwords-everywhere + usernames in the SQLite dump only). FAIL = exit 1 + `SECRET-LEAK` line naming WHICH artifact matched, never the secret (same shape as `scripts/live-22.sh` [VERIFIED: scripts/live-22.sh:28-39]).
- Env values travel as env vars only; bin refuses `--password` argv (copy the refusal verbatim); `set -x` stays OFF while secrets are in scope; `unset` password vars before further output [VERIFIED: scripts/live-22.sh:17-22,41-43].

### Send audit (D-14 — no unprefixed mail)
- Audit sources: temp `send_queue` table dump (`to_addrs`/`cc_addrs`/`bcc_addrs` JSON columns per row), queued `.eml` files (To/Cc headers + envelope columns), bin log lines (`send_raw` recipient echoes if any — recommend the bin log `RCPT=<self>` per attempt, never full bytes).
- Audit rules, ALL must hold or the run FAILs: (1) every row's `to_addrs` == `["<SGE_LIVE_SELF>"]` exactly and `cc_addrs`/`bcc_addrs` == `[]`; (2) every `.eml` Subject starts with `SGE-TEST-`; (3) every `.eml` has no `Bcc:` header line (envelope-only invariant, offline-proven [VERIFIED: src-tauri/src/send_queue.rs:743-777] — re-asserted live on the actual sent bytes); (4) per-run unique Message-ID (minted via `new_queue_id`, never reused across runs); (5) transport send-call count == expected sends per leg (catches stray resends — the reconcile leg must add zero).
- BCC rule stays in doubles only per D-12: the live run sends with empty BCC always; the no-`Bcc:`-header check above is an audit of that discipline, not a BCC test.

### Cleanup (D-15)
- Order: (1) UID-expunge the INBOX arrival UID(s) for each test Message-ID (`store_deleted` + `uid_expunge`, scoped single-UID — never bare `expunge` on INBOX); (2) UID-expunge the tracked Sent copy UID(s); (3) expunge any sandbox-folder leftovers + DELETE sandbox if created (draft leg); (4) re-SEARCH INBOX and Sent for every test Message-ID → zero hits; re-LIST == pre-run snapshot.
- Retry 3× with backoff 5s/15s/45s per destructive step (discretion recommendation, mirrors Phase 22 [VERIFIED: src-tauri/src/bin/live_gate_22.rs:49]); then print exact manual steps (folder names, Message-IDs, UIDs, webmail path) and exit 1 — never blind infinite retry. Cleanup ALWAYS runs (finally path), even after mid-run FAIL.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| SMTP send | Custom SMTP client / raw socket DATA | `SmtpTransporter::send_raw` via `flush_send_queue` | Fail-closed STARTTLS, pool caching, verdict mapping, and filing-leg dedupe already encoded; a second transport bypasses the code under test |
| Uncertain verdict | Ad-hoc timeout detection | `classify_fault` + `map_lettre_error` + `mark_send_uncertain` | T-13-07 ambiguity rule pinned by unit tests; re-deriving it risks blind-retry duplicates |
| Reconcile | Manual SEARCH-then-decide scripting | `reconcile_uncertain_sends` | Hit→sent-no-resend / miss→requeue-once / error→stays-uncertain already encoded |
| Crash reset | Manual `sending→queued` UPDATE | `crash_recover` + flush-pass uncertain triage | Two-layer semantics (13-01 reset + 13-02 triage) must both run; hand-rolled SQL skips the triage |
| MIME render | String-concatenated mail | `enqueue_send` (renders via `render_draft_rfc5322` with injection guard + cap check) | Header-injection strip + 25 MB cap + BCC-envelope-only ordering is the safety argument |
| Offline toggle | OS network fiddling / feature flag in prod | `GateSession` + `TimeoutTransport` decorators in the bin | Zero prod-code changes, deterministic, per-op granularity |
| Sent filing | Custom APPEND sequence | The flush's filing leg (probe SEARCH → APPEND `\Seen` → sent-unfiled) | Auto-save dedupe + APPEND-only-retry already encoded; reordering re-SMTP-sends |

**Key insight:** every exactly-once guarantee lives in the ordering of existing calls (dedupe-check before render, cap before write, row insert last; probe SEARCH before APPEND; reconcile SEARCH before any re-send). The harness drives that order, never reinvents it.

## Common Pitfalls

### Pitfall 1: Second SMTP transport or second IMAP connection in the bin
**What goes wrong:** harness opens its own lettre client or `connect_sync` alongside the manager — bypasses pool/verdict logic, races SELECT state.
**Why it happens:** seems simpler than threading `SendFlushEnv` + manager leases.
**How to avoid:** one `SmtpTransporter` inside `SendFlushEnv`, one `SessionManager`, leases only (the `start_sync` command is the template). The bin calls the same manager methods the commands call.
**Warning signs:** any `SmtpTransport::starttls_relay` / `connect_sync` call site inside `live_gate_21.rs` outside `SmtpTransporter::new()`.

### Pitfall 2: Reading the new UID from APPEND
**What goes wrong:** bin tries to parse APPENDUID — async-imap swallows it, returns `()`.
**Why it happens:** IMAP APPENDUID habit from other clients.
**How to avoid:** always SEARCH Message-ID after APPEND/send (the Phase 22 rule) [VERIFIED: src-tauri/src/imap/mod.rs:361-365].
**Warning signs:** any code expecting a UID return from `append_message`.

### Pitfall 3: Non-PEEK FETCH in arrival/Sent asserts sets `\Seen` on the INBOX arrival
**What goes wrong:** `BODY[]` fetch marks the arrival read — corrupts the "untouched INBOX" story and any downstream Seen assertions.
**Why it happens:** default fetch sections in examples.
**How to avoid:** `fetch_body` (PEEK) only, everywhere.
**Warning signs:** `BODY[]`, `RFC822`, or `RFC822.TEXT` string in the bin.

### Pitfall 4: Forgetting the flush-pass uncertain triage (expecting `queued` after crash)
**What goes wrong:** crash leg asserts `queued` post-restart, but production promotes stranded `sending` to `uncertain` first.
**Why it happens:** `crash_recover` alone flips to `queued`; the flush wrapper adds the triage on top.
**How to avoid:** expect `uncertain` after restart-through-flush; drive `reconcile_uncertain_sends` for the exactly-once outcome (documented in Q4 above).
**Warning signs:** assertion on `SEND_STATE_QUEUED` immediately after a flush pass that saw stranded rows.

### Pitfall 5: `send_status_snapshot` always reports `sent_unfiled: 0`
**What goes wrong:** A-07 asserts via `send_status` IPC shape but the snapshot hard-codes `sent_unfiled: 0` ("no writer exists yet" comment predates the 13-03 APPEND leg that returns `SentUnfiled` without a state write) [VERIFIED: src-tauri/src/send_queue.rs:466-479; src-tauri/src/sync/worker.rs:760-766].
**Why it happens:** snapshot and flush evolved in different plans; the state machine has no `sent-unfiled` spelling persistence visible in the snapshot path.
**How to avoid:** assert A-07 via `FlushOutcome::SentUnfiled` counts + transport-call-count (no re-send), NOT via the `send_status` `sent_unfiled` field — or fix the snapshot as a plan task after verifying the intended CHECK values. Planner must resolve this before lifting A-07 into must_haves.
**Warning signs:** test asserting `sent_unfiled ≥ 1` from `send_status_snapshot` and failing while logs show `sent_unfiled` outcomes.

### Pitfall 6: `set -x` or `--password` leaking creds; regex grep missing metachars
**What goes wrong:** shell trace prints env passwords; `grep` without `-F` misfires on password metachars.
**Why it happens:** debugging habit.
**How to avoid:** copy `scripts/live-22.sh` discipline verbatim: no `set -x` in scope, `grep -F -q`, `unset` after scan.
**Warning signs:** `set -x` or `grep -E` anywhere near the wrapper.

### Pitfall 7: Touching real folders — INBOX default + Sent Deletes
**What goes wrong:** several commands default `mailbox` to `"INBOX"`; cleanup expunge with a wrong UID set deletes real mail.
**Why it happens:** omitted wire-name threading; UID-set construction bugs.
**How to avoid:** thread wire names explicitly every call; cleanup expunges ONLY UIDs returned by test-Message-ID SEARCHes; pre-flight LIST snapshot + per-step diff aborts on any unexpected delta.
**Warning signs:** any literal `"INBOX"` default reached in the bin; any `expunge()` (bare) call.

## Code Examples

### Driving one offline-enqueue → flush → arrival leg
```rust
// Sources: send_queue.rs:277 (enqueue_send), worker.rs:586 (flush_send_queue),
// imap/mod.rs:378 (uid_search_header), commands/sync.rs:1170 (queue_send contract)
let out = enqueue_send(conn, &app_data, EnqueueInput {
    from: self_addr.clone(),
    to: vec![self_addr.clone()],
    cc: vec![], bcc: vec![],
    subject: format!("SGE-TEST-{run_tag} arrival"),
    body: "live gate probe".to_string(),
    draft_id: None,
})?;
assert!(!out.deduped);
let summary: FlushSummary = worker.flush_send_queue(&SendFlushEnv {
    gate: SendGate::default(),
    transport: Arc::new(SmtpTransporter::new()),
    account: keyring_account,
    manager: Some(manager.clone()),
    sent_wire: Some(sent_wire.clone()),
}).await;
assert_eq!(summary.sent, 1);
// Arrival: SEARCH INBOX for the Message-ID (never SMTP 250 alone).
let hits = inbox_lease.session().uid_search_header("Message-ID", &out.message_id).await?;
assert_eq!(hits.len(), 1);
```

### DATA-timeout decorator for the uncertain leg (no prod-code change)
```rust
// Sources: smtp.rs:96 (SmtpTransport trait), smtp.rs:128 (classify_fault timeout arm)
pub struct TimeoutTransport {
    inner: Arc<dyn SmtpTransport>,
    fault_armed: Arc<AtomicBool>,
    send_calls: Arc<AtomicUsize>,
}
impl SmtpTransport for TimeoutTransport {
    fn send_raw(&self, account: &SmtpAccount, from: &str, to: &[String], bytes: &[u8]) -> SmtpOutcome {
        if self.fault_armed.load(Ordering::SeqCst) {
            // Zero bytes leave the box: exercises the real uncertain path deterministically.
            return SmtpOutcome::Uncertain {
                reason: "live-gate: injected DATA timeout".to_string(),
            };
        }
        self.send_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.send_raw(account, from, to, bytes)
    }
}
// Reconcile must not increment send_calls: assert count unchanged across
// reconcile_uncertain_sends when SEARCH hits.
```

### Verbatim Sent-copy compare (PEEK-only)
```rust
// Sources: imap/mod.rs:311 (fetch_body PEEK), worker.rs:734 (APPEND with \Seen)
let sent_hits = sent_lease.session().uid_search_header("Message-ID", &message_id).await?;
assert_eq!(sent_hits.len(), 1);
let server_bytes = sent_lease.session().fetch_body(sent_hits[0]).await?;
let queued_bytes = std::fs::read(&row.eml_path)?;
assert_eq!(server_bytes, queued_bytes, "Sent copy must be verbatim");
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Phase 22 tracer covers drafts/Seen only (4 steps, cleanup-only tail) | Clone skeleton to `live_gate_21` with send legs (~10 steps) | This phase | Mirror, do not reinvent — same exit codes, same guards |
| `sent_unfiled` defined-but-unproduced (13-02) | Produced by 13-03 APPEND leg as `FlushOutcome::SentUnfiled` (APPEND-only retry, row parked `sent`) | Phase 13 plan 13-03 | A-07 asserts via flush outcomes + call counts, not via snapshot field (Pitfall 5) |
| Crash recovery = `sending→queued` only | Flush-pass triage promotes stranded `sending` to `uncertain` first | Phase 13 plan 13-02 | Crash leg expects `uncertain`, drives reconcile (Pitfall 4) |

**Deprecated/outdated:**
- Driving the live run through Tauri IPC / a running app: rejected (same rationale as Phase 22 — needs display/webview, nondeterministic; the bin calls the same manager methods the commands call).
- Pure-shell harness (curl/openssl SMTP): rejected — bypasses the verdict/fling code under test.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `mail.utfpr.edu.br` permits account-root CREATE/DELETE for a sandbox folder if the draft leg uses one (else draft leg uses real Drafts — higher risk, needs user confirm) | Harness Shape | Draft-send leg needs re-scoping at plan time |
| A2 | Server delivery latency fits a bounded poll (3× 10s) between SMTP Sent and INBOX SEARCH hit | Arrival Asserts | Flaky arrival leg; planner widens poll or documents server lag |
| A3 | Username-scan policy (passwords-everywhere vs usernames-dump-only) | Secret Scan | Planner confirms with user; safest default is passwords-everywhere |
| A4 | Server supports UIDPLUS (`UID EXPUNGE`); else the UIDPLUS-absent fallback dance path runs (slower, still correct) | Cleanup | Cleanup slower but correct; no plan change |
| A5 | Exact backoff log format + cleanup N/backoff (discretion items) accepted as recommended (flush-done line; 3× 5s/15s/45s) | Offline/Crash, Cleanup | Planner confirms or adjusts at plan time |
| A6 | DRAFT-03 same-transaction cleanup (delete local draft row + expunge tracked `server_uid` on SMTP success when `draft_id` present) exists in the flush path — specified in 13-03-PLAN but NO `draft_id`/draft-delete code found in `flush_one_row`/`flush_send_queue` this session (only `replay_dirty_drafts` + `discard_draft` command exist) | Draft-send leg | If unimplemented, the D-16 live leg FAILs by design and the phase must scope a fix task (verification-only boundary breaks) or defer D-16 with a recorded gap. Planner MUST verify this before lifting D-16 into must_haves — grep `draft_id` in `sync/worker.rs` at plan time |

## Open Questions

1. **DRAFT-03 implementation status (from A6)**
   - What we know: 13-03-PLAN specified same-transaction draft cleanup on SMTP success; no matching code found in the flush path this session.
   - What's unclear: whether it landed elsewhere (command layer, compose flow) or was deferred.
   - Recommendation: planner greps `draft_id` across `sync/worker.rs` + `commands/sync.rs` at plan time; if absent, D-16 becomes implement-then-prove (one plan task) or a recorded deferral — user decision.

2. **`sent_unfiled` observability for A-07 (from Pitfall 5)**
   - What we know: snapshot hard-codes 0; flush returns real `SentUnfiled` outcomes.
   - What's unclear: whether UI-SPEC A-07's `send_status` expectation can be met without a snapshot fix.
   - Recommendation: assert via flush outcomes + transport call counts; file the snapshot fix as a plan task if the IPC shape must carry it.

3. **Sandbox folder vs real Drafts for the D-16 leg**
   - What we know: Phase 22 used a test-only folder override for drafts; the send pipeline's draft cleanup reuses `discard_server_copy_in`-style scoped paths.
   - What's unclear: whether the draft leg needs a sandbox folder at all (draft APPENDs to sandbox, sends to self, asserts no orphan) or reuses real Drafts with tracked-UID cleanup.
   - Recommendation: sandbox APPEND (consistent with Phase 22 D-04, zero risk to real drafts); planner confirms server allows the extra folder.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Real account + `src-tauri/.live-test.env` | All live STEPs | ✗ (human supplies; example present [VERIFIED: src-tauri/.live-test.env.example:10-23], file git-ignored [VERIFIED: .gitignore:73-74]) | — | Offline doubles (FakeTransport/`FakeSession` record-replay) for CI |
| `smtp.utfpr.edu.br:587` reachability + STARTTLS | Flush legs | [ASSUMED — verified at run time by preflight SMTP dial] | — | Abort loudly; doubles cover logic |
| OS keyring (Secret Service) | D-04 seed + run | [ASSUMED — Linux Secret Service present] | — | `checkpoint:human-verify`; decorator unit tests use no keyring |
| scripts/live-21.sh + `live_gate_21` bin | Harness | ❌ Wave 0 (to be built; `live-22.sh` + `live_gate_22` exist as clone sources [VERIFIED: scripts/live-22.sh; src-tauri/Cargo.toml:25-27]) | — | None — this phase builds them |

**Missing dependencies with no fallback:**
- Human-supplied credentials file (planner must gate the first live run behind `checkpoint:human-verify`)

**Missing dependencies with fallback:**
- Live server unreachable → offline doubles (FakeTransport asserting verdict transitions + GateSession replay) keep Wave 0 green

## Validation Architecture

`workflow.nyquist_validation` is `true` [VERIFIED: .planning/config.json]. This phase is a **live-gate verification phase, not a feature phase**: its "tests" are the scripted live steps (each STEP line is a gate). Treat harness determinism gates as Wave 0 instead of unit tests:

### Test Framework
| Property | Value |
|----------|-------|
| Framework | `cargo test` (bin `#[cfg(test)]` module, same as live_gate_22) |
| Config file | none — see Wave 0 |
| Quick run command | `cargo test --manifest-path src-tauri/Cargo.toml --bin live_gate_21` |
| Full suite command | `cargo test --manifest-path src-tauri/Cargo.toml` (unchanged suite must stay green) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LIVE-01 | Self-send arrives, Sent copy verbatim, exactly-once, backoff visible, no leak | live-gate (human-run) | `scripts/live-21.sh` (exit 0/1/2) | ❌ Wave 0 (bin + script to be built) |
| LIVE-01 | Prefix guard, LIST-delta, one-copy identity, decorator open/closed, audit-grep | unit (offline, CI) | `cargo test --manifest-path src-tauri/Cargo.toml --bin live_gate_21` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --manifest-path src-tauri/Cargo.toml --bin live_gate_21`
- **Per wave merge:** full `cargo test` suite
- **Phase gate:** full suite green + human-executed `scripts/live-21.sh` exit 0 before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `src-tauri/src/bin/live_gate_21.rs` — harness + offline `#[cfg(test)]` module (prefix, LIST-delta, `TimeoutTransport` open/closed + call-count, `GateSession` reuse, audit-grep fixtures)
- [ ] `scripts/live-21.sh` — env source → `--seed-only` → run → send-audit → secret scan → exit propagation
- [ ] `[[bin]] live_gate_21` entry in `src-tauri/Cargo.toml` (mirror live_gate_22)
- [ ] Framework install: none (cargo toolchain already in use)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | Yes | Keyring-first (D-04); env-seed-once; `--password` argv refused |
| V3 Session Management | Yes | Single `SessionManager`-owned IMAP session + one pooled SMTP client; leases only |
| V4 Access Control | Yes | Sandbox-prefix assert on every harness-issued mutation; pre-flight LIST guard; D-01 recipient abort |
| V5 Input Validation | Yes | `build_envelope` bare-addr-spec fail-closed; Message-ID atom restriction via `new_message_id`; UID-set fail-closed parsing |
| V6 Cryptography | Yes | Fail-closed STARTTLS (`starttls_relay` only, tripwire-pinned); creds never in logs/errors (Debug redaction + verdict sanitizer) |

### Known Threat Patterns for Rust + lettre SMTP + IMAP live gate

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Credential leakage via logs/argv/history | Information disclosure | Env-only transport, Debug redaction, `grep -F` scan gate (D-13) |
| Mail to wrong recipient (mis-addressed test) | Repudiation / Integrity | D-01 abort-on-non-self + D-14 send audit (FAIL on unprefixed/foreign recipient) |
| Duplicate delivery on retry/reconcile | Integrity | Message-ID dedupe + probe-before-APPEND + reconcile-before-resend (D-07/D-08/D-10) |
| Wrong-folder destructive op (cleanup expunge) | Tampering | Prefix assert + LIST snapshot diff + tracked-UID-only expunge (D-03/D-15) |
| Blind resend after `sent-unfiled` | Integrity | APPEND-only retry, never re-SMTP-send (flush leg encodes it; audit counts transport calls) |

## Sources

### Primary (HIGH confidence)
- `src-tauri/src/send_queue.rs` — enqueue/dedupe/cap/BCC/backoff/crash_recover/snapshot (read this session, line-cited)
- `src-tauri/src/smtp.rs` — transporter/pool/verdicts/envelope/fail-closed tripwire (read this session, line-cited)
- `src-tauri/src/sync/worker.rs` — flush_send_queue/filing leg/reconcile_uncertain_sends/SendFlushEnv (read this session, line-cited)
- `src-tauri/src/imap/mod.rs` — SyncSession verb shapes, APPENDUID-swallow, PEEK-only fetch_body (read this session, line-cited)
- `src-tauri/src/commands/sync.rs` — frozen queue_send/retry_send/send_status contracts (read this session, line-cited)
- `src-tauri/src/bin/live_gate_22.rs` — clone-source skeleton: STEP runner, GateSession, guards, cleanup, seed-only (read this session, line-cited)
- `scripts/live-22.sh` — wrapper discipline: env source, log capture, secret scan (read this session, line-cited)
- `src-tauri/Cargo.toml` + `Cargo.lock` — bin entries, lettre 0.11.23 pin (read this session)
- `.planning` CONTEXT/REQUIREMENTS/ROADMAP/PROJECT + `.live-test.env.example` + `.gitignore` + `config.json` (read this session)

### Secondary (MEDIUM confidence)
- `.planning/phases/22-*/22-RESEARCH.md` — sibling harness rationale (in-repo prior research, reused shape)
- `.planning/milestones/v1.2-phases/13-send-pipeline/13-03-PLAN.md` — DRAFT-03 transaction spec + filing-leg order (in-repo plan text)

### Tertiary (LOW confidence)
- None — every prescriptive claim above traces to source read this session except the A1–A6 assumptions, which are flagged for planner confirmation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — pins read from Cargo.toml/Cargo.lock this session
- Architecture: HIGH — harness shape cloned from shipped in-repo tracer read verbatim
- Pitfalls: HIGH — each cites the exact code path that motivates it (Pitfall 5 and A6 flag a real snapshot/implementation gap found by grep this session)

**Research date:** 2026-10-13
**Valid until:** 30 days (stable domain — in-repo APIs + SMTP/IMAP dialect)
