# Phase 21: SMTP Send — Live Gate (Live-01, SANDBOXED) - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous; all recommended answers accepted)

<domain>
## Phase Boundary

Prove against the user's real account (`smtp.utfpr.edu.br:587/STARTTLS`,
creds in git-ignored `src-tauri/.live-test.env`) what Phases 12/13/14 built:
the durable send pipeline delivers test mail ONLY to the user's own address,
it arrives (IMAP-verified, not just SMTP 250), exactly once per Message-ID,
with a verbatim Sent copy, visible retry/backoff, and zero secret leakage.
Offline queue → flush, `uncertain` reconcile-not-resend, crash recovery, and
the draft-send transaction are all proven live; oversize/BCC rules stay in
doubles. Cleanup removes every test artifact post-run.

Verification only — HOW to prove live and safe, never new capabilities.

</domain>

<decisions>
## Implementation Decisions

### Send scope & sandbox mechanics
- **D-01:** Recipient scope is ONLY the user's own address — envelope AND headers
  (`SGE_LIVE_SELF`); the run aborts if any other recipient resolves.
- **D-02:** Sent copy APPENDs to the real Sent folder (pipeline behavior, not a
  sandbox substitute); its UID is tracked and it is expunged in cleanup.
- **D-03:** Pre-flight guard mirrors Phase 22 D-02: `SGE-Test-` prefix asserts +
  LIST snapshot, abort on unexpected mid-run change.
- **D-04:** Credential flow mirrors Phase 22 D-13/D-14: source env file
  (`set -a; . file; set +a`) → env vars → seed OS keyring ONCE → all SMTP/IMAP
  auth reads from keyring (exercises the real Phase 1/5 credential path).

### Arrival, Sent-copy & no-duplicate proof
- **D-05:** Arrival asserted via IMAP SEARCH INBOX for the test Message-ID after
  send + FETCH headers to confirm — proves real delivery, not just SMTP 250.
- **D-06:** Sent copy asserted by SEARCH Sent for the same Message-ID: exactly 1
  UID + `\Seen` set + verbatim-bytes compare against the queued `.eml`.
- **D-07:** No-duplicate proof: double-enqueue the same Message-ID offline →
  flush → assert single delivery (1 INBOX UID); `sent-unfiled` APPEND failure
  must never trigger a re-SMTP-send.
- **D-08:** Server auto-save probe in scope: if the server auto-saves to Sent,
  the pipeline must dedupe by Message-ID (no twin copy).

### Retry, backoff & failure-mode coverage
- **D-09:** Offline leg proven live: enqueue under the app-level offline toggle
  → reconnect → flush with the backoff schedule visible in logs → arrives
  exactly once.
- **D-10:** `uncertain` reconcile proven via harness-injected DATA-timeout fault:
  assert `uncertain` state → reconcile-not-resend (SEARCH before any retry).
- **D-11:** Crash leg proven live: kill mid-`sending` → restart → assert
  `crash_recover` resets to `queued` → flushes once, no duplicate.
- **D-12:** 25 MB cap + BCC envelope-only stay in doubles (oversize rejected
  pre-SMTP; no BCC header leak in envelope log) — never attempted live.

### Safety gates, secret scan & cleanup
- **D-13:** Post-run automated secret scan mirrors Phase 22 D-15: `grep -F` over
  logs + SQLite dump for credential substrings → FAIL the run on hit; env
  values travel as env vars, never CLI args.
- **D-14:** Send audit: no mail WITHOUT the `SGE-TEST-` subject prefix may leave
  the box — audit `send_queue` + SMTP transcript for unprefixed
  recipients/subjects; per-run unique Message-ID.
- **D-15:** Cleanup deletes the test INBOX arrival + Sent copy + sandbox folder
  (if used); retry 3× (5s/15s/45s, mirroring Phase 22 D-03) then exact manual
  steps — never blind infinite retry.
- **D-16:** Draft-send transaction (DRAFT-03) proven live: sending a sandbox
  draft deletes it in the same transaction — assert no orphan draft server-side
  nor local.

### the agent's Discretion
- Exact backoff-schedule log format asserted in D-09.
- Harness-injected transport-fault mechanism detail (decorator vs flag).
- SEARCH/FETCH verb dialect specifics (UID scoping, BODY.PEEK sections).
- Cleanup retry N/backoff (recommended 3× 5s/15s/45s).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements & milestone scope
- `.planning/REQUIREMENTS.md` — LIVE-01 requirement text + sandbox safety contract (v1.4 revision 2026-10-13)
- `.planning/ROADMAP.md` — Phase 21 goal, requirements (LIVE-01), UI hint (yes)
- `.planning/PROJECT.md` — Current Milestone section: goal, sandbox safety contract, scope guard
- `src-tauri/.live-test.env.example` — credential fields + safety contract summary (the real `.live-test.env` is git-ignored; NEVER commit or print it)

### Prior-phase behavior under test (do NOT re-derive, reuse verbatim)
- `src-tauri/src/send_queue.rs` — `enqueue_send`, `SendGuard`/`try_begin`, `record_send_failure`, `mark_send_failed`, `requeue_failed`, `crash_recover`, `backoff_bounds`/`backoff_delay_for_attempt`, `new_queue_id` (Message-ID-at-enqueue), `outbox_eml_path`
- `src-tauri/src/smtp.rs` — `SmtpPool`/`account_key` caching, `classify_fault` → `SmtpOutcome`, `map_lettre_error`, `build_envelope` (BCC envelope-only)
- `src-tauri/src/drafts.rs` — draft-send transaction (DRAFT-03, no orphan drafts)
- `src-tauri/src/commands/sync.rs` — `sent-unfiled` APPEND-retry path (never re-SMTP-send)
- `src-tauri/src/bin/sync_demo.rs` — candidate harness skeleton (cf. Phase 22 `live_gate_22` pattern)

### Sibling-phase pattern (mirror, do not reinvent)
- `.planning/phases/22-draft-seen-round-trip-live-gate-live-02/22-CONTEXT.md` — sandbox mechanics D-01..D-04, credential handling D-13..D-16, offline-toggle D-09, cleanup D-03

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `send_queue` API surface above: enqueue/guard/failure/requeue/recover/backoff are all implemented — the live run drives them, it does not rebuild them.
- `SmtpPool::account_key(host, port, username)` caching + `build_envelope` BCC handling: reuse for the single-recipient self-send.
- Phase 22 `live_gate_22` harness (planned, sibling): mirror its STEP PASS/FAIL runner, exit 0/1/2 contract, `--seed-only` keyring path, `/tmp` logs, no `set -x`.

### Established Patterns
- BODY.PEEK-only reads hold on all fetch paths (M1 invariant) — arrival/content assertions must FETCH with PEEK, never set `\Seen` as a side effect.
- Message-ID-at-enqueue dedupe; DATA-timeout → `uncertain` reconcile-not-resend; crash-recovery reset of `sending`→`queued` at launch.
- `SendGate` separate from `SyncGate`; per-account pool caching; 25 MB send cap.
- Plain-language errors, no secret leakage — extended live by D-13's automated scan.

### Integration Points
- New live-test harness connects via the existing send pipeline entry points (no second SMTP transport); creds flow keyring-first per D-04.
- Frontend: outbox badge + failed-retry surface already shipped (Phase 13/14) — assert visibility, build no new UI.

</code>

<specifics>
## Specific Ideas

- User's hard constraints (verbatim, carried from Phase 22): test script "must not tamper with the mail that is in the account nor the folders"; "must create new mails (sending them to myself)"; "never send email to other accounts, just to my own account".
- User supplies real credentials via the git-ignored file; the `SGE-TEST-` subject prefix makes test mail visually distinguishable in webmail during the run.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 21-SMTP Send — Live Gate (Live-01, SANDBOXED)*
*Context gathered: 2026-10-13*
