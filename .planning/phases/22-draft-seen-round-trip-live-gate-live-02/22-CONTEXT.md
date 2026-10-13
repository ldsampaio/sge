# Phase 22: Draft & Seen Round-Trip — Live Gate (Live-02, SANDBOXED) - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Text-mode discuss (interactive prompts unavailable; numbered-list Q&A)

<domain>
## Phase Boundary

Prove against the user's real account (`mail.utfpr.edu.br` / `smtp.utfpr.edu.br:587`,
creds in git-ignored `src-tauri/.live-test.env`) what Phases 6/10/12 built: draft
APPEND leaves exactly one server copy per compose session, offline edits survive
reconnect, `imap_outbox` delete/move replays with no silent resurrection by backfill,
and Seen-flag round-trips survive reconnect. ALL mutating ops are scoped to a per-run
sandbox folder `SGE-Test-<timestamp>` at account root; test mail is sent ONLY to the
user's own address; existing mail and existing folders are never touched.

Verification only — HOW to prove live and safe, never new capabilities.

</domain>

<decisions>
## Implementation Decisions

### Sandbox mechanics
- **D-01:** Sandbox folder is `SGE-Test-<timestamp>` at account root, deleted post-run (no persistent parent).
- **D-02:** Pre-flight guard = name-prefix (`SGE-Test-`) + exists-and-empty check before first APPEND, PLUS a LIST snapshot taken before the run; the run aborts if LIST changes unexpectedly mid-run.
- **D-03:** Cleanup failure: retry N times with backoff, then print exact manual-cleanup steps (folder name, what to delete) — never auto-retry destructive ops blindly forever.
- **D-04:** Test drafts APPEND into the sandbox folder instead of the real Drafts folder (test-only role override); the one-copy logic is asserted there. Real drafts are never at risk.

### One-copy draft proof
- **D-05:** Pass = after N saves + reconnect, server SEARCH in the sandbox returns exactly ONE UID for the session AND it equals the tracked `draft_uid` (count + identity).
- **D-06:** Failure mode covered LIVE: kill the session between APPEND-new and expunge-old, reconnect, assert reconcile collapses to 1 copy (sandbox-safe by D-01/D-04).
- **D-07:** Edit-offline-then-reconnect is in scope: toggle offline mid-edit, queue, reconnect, assert single copy with latest content winning.
- **D-08:** Content asserted: FETCH the surviving copy and compare subject/body to the last save (catches silent content loss).

### Offline + resurrection proof
- **D-09:** Offline is induced via an app-level offline toggle in the test harness (deterministic; no OS/network fiddling); reconnect is a clean re-run of sync.
- **D-10:** Replay order asserted in logs + end-state: pre-sweep `imap_outbox` replay runs BEFORE the backfill sweep, with a pending indicator visible until acked (proves the Phase 10 discipline live).
- **D-11:** Resurrection proof = delete offline → reconnect → SECOND full sync asserts the UID stays gone (tombstoned, not re-fetched). The double-sync is the direct resurrection test.
- **D-12:** Seen round-trip asserted in BOTH directions across reconnect (mark read → assert `\Seen`; mark unread → assert absent), proving pending-wins reconcile both ways.

### Credential handling in scripts
- **D-13:** Scripts source the env file (`set -a; . file; set +a`); values travel as env vars to the test binary, never as CLI args (no `/proc`/history leakage).
- **D-14:** Env password seeds the OS keyring ONCE; all IMAP/SMTP auth reads from keyring — the live run exercises the real Phase 1/5 credential path.
- **D-15:** Post-run automated secret scan: grep logs + SQLite dump for credential substrings, FAIL the run if found (constraint becomes a gate, not a review step).
- **D-16:** Test mail carries a fixed `SGE-TEST-` subject prefix + unique per-run Message-ID; the run audits outbox/logs to assert no mail WITHOUT the prefix was sent (proves "only test mail, only to self").

### the agent's Discretion
- Exact retry count N and backoff schedule for cleanup (D-03).
- App-level offline toggle implementation detail (feature flag vs harness-injected transport failure).
- SEARCH/FETCH verb dialect details for the one-copy and content assertions (UID SEARCH scoping, BODY.PEEK sections).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements & milestone scope
- `.planning/REQUIREMENTS.md` — LIVE-02 requirement text + sandbox safety contract (v1.4 revision 2026-10-13)
- `.planning/ROADMAP.md` — Phase 22 goal, requirements (LIVE-02), UI hint (minimal)
- `.planning/PROJECT.md` — Current Milestone section: goal, sandbox safety contract, scope guard
- `src-tauri/.live-test.env.example` — credential fields + safety contract summary (the real `.live-test.env` is git-ignored; NEVER commit or print it)

### Prior-phase behavior under test (do NOT re-derive, reuse verbatim)
- `src-tauri/src/drafts.rs` — one-copy save (APPEND-new + expunge-old, `draft_uid` tracking)
- `src-tauri/src/send_queue.rs` — pre-sweep replay discipline reference
- `src-tauri/src/commands/sync.rs` — `set_seen` replay, `replay_outbox`, optimistic `pending_delete` flow
- `src-tauri/src/classify/filing.rs` — outbox replay-mirror pattern (`confirm_suggestion` reconnect replay)
- `src-tauri/src/imap/manager.rs` — FakeSession test double pattern (for harness design)
- `src-tauri/src/sync/worker.rs` — MockSession test double pattern (for harness design)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `draft_uid`-tracked save path in `src-tauri/src/drafts.rs`: reuse for sandbox-scoped APPEND with test-only folder override.
- `replay_outbox` + `set_seen` replay in `src-tauri/src/commands/sync.rs`: the exact replay entry points the live run triggers after the offline toggle flips back.
- `sync_demo` / `sync_diag` binaries in `src-tauri/src/bin/`: candidate harness skeleton for the scripted live run (deterministic, stdout-observable).
- FakeSession/MockSession doubles: patterns for the app-level offline toggle + verb-order log assertions.

### Established Patterns
- BODY.PEEK-only reads hold on all fetch paths (M1 invariant) — content assertion (D-08) must FETCH with PEEK, never set `\Seen` as a side effect.
- UID-only addressing with UIDVALIDITY epoch gating — replay and SEARCH assertions must be epoch-scoped.
- `flag_outbox` (Seen) vs `imap_outbox` (delete/move) are separate contracts — assert each queue independently.
- Plain-language errors, no secret leakage — extended live by D-15's automated scan.

### Integration Points
- New live-test harness connects via existing `SyncSession`/manager leases (no second IMAP connection); creds flow keyring-first per D-14.
- Frontend: minimal — pending indicators until acked (D-10); no new UI surface beyond what Phases 6/12 shipped.

</code>

<specifics>
## Specific Ideas

- User's hard constraints (verbatim): test script "must not tamper with the mail that is in the account nor the folders"; "must create new mails (sending them to myself)"; "never send email to other accounts, just to my own account".
- User supplies real credentials via the git-ignored file; the `SGE-TEST-` subject prefix makes test mail visually distinguishable in webmail during the run.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 22-Draft & Seen Round-Trip — Live Gate (Live-02, SANDBOXED)*
*Context gathered: 2026-10-13*
