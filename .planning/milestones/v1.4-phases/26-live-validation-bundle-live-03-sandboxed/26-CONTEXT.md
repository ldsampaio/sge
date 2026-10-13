# Phase 26: Live Validation Bundle (Live-03, SANDBOXED) - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous; all recommended answers accepted)

<domain>
## Phase Boundary

Execute every still-deferred `/gsd-verify-work` behavior against the user's
real account (`mail.utfpr.edu.br:993/SSL` + `smtp.utfpr.edu.br:587/STARTTLS`,
creds in git-ignored `src-tauri/.live-test.env`) under the verbatim Phase
21/22 sandbox safety contract — one `SGE-Test-<run-id>` sandbox per run, ALL
mutating ops scoped to it, test mail ONLY to the user's own address, existing
mail/folders never touched, pre-flight guard aborts on out-of-sandbox
targets, BODY.PEEK-only reads, post-run cleanup + secret scan, exit-2 clean
SKIP without creds — and record per-gate PASS/FAIL plus any remaining
documented deferrals in a single bundle REPORT.

Verification only — HOW to prove the deferred gates live and safe, never new
capabilities. UI: none.

</domain>

<decisions>
## Implementation Decisions

### Bundle scope (which deferred gates, new vs reused harness)
- **Scope is the full deferred set:** v1.1 gates 6 (Seen round-trip in webmail + offline-replay convergence), 7 (sidebar tree matches server + per-folder browse + badges match webmail), 8 (arrival on 5-min tick + stale-session reconnect recovery) — per `.planning/v1.1-MILESTONE-AUDIT.md`; v1.2 gates 12 (draft round-trip) + 13 (SMTP 587 send) — per `.planning/v1.2-MILESTONE-AUDIT.md`; v1.3 gates confirm→MOVE→undo, RENAME/DELETE, large-folder batch/chunk gate — per `.planning/v1.3-MILESTONE-AUDIT.md` `deferred_live_gates`. No gate is silently dropped; each gets a PASS line or a documented deferral with reason.
- **Reuse live_gate_21/22 legs verbatim for the 12/13-derived behaviors** (draft one-copy + SMTP send + Seen round-trip already have STEP runners there); the bundle does NOT re-implement them — it re-runs or cites their REPORT lines. Rationale: Phase 21/22 CONTEXTs lock those STEP semantics; duplication risks divergence.
- **One new harness bin covers the genuinely new legs** (6/7/8 read-side asserts + v1.3 confirm→MOVE→undo + batch/chunk probe), structured as a clone of the `live_gate_21`/`live_gate_22` STEP-runner pattern (STEP n/N PASS/FAIL lines, exit 0/1/2, `--seed-only` keyring path, `/tmp` logs, GateSession-style sandbox asserts). Rationale: scout shows the STEP-runner + exit-contract is the established, reviewable shape (`src-tauri/src/bin/live_gate_21.rs`, `live_gate_22.rs`).
- **No new product code paths are exercised for the first time here:** every gate drives already-shipped verbs (STORE/APPEND/MOVE/CREATE/RENAME/DELETE/poll-tick/backfill-sweep via existing manager/lease entry points). If a leg needs a new verb, that is a scope-creep signal — defer, don't build. Alternative rejected: bundling feature fixes into the live run (would corrupt the verification-only boundary).

### Sandbox reuse (one sandbox, verbatim guards, single REPORT)
- **One shared `SGE-Test-<run-id>` sandbox folder at account root per bundle run**, created once at start, reused by every gate leg, deleted once at end. Rationale: matches the Phase 22 D-01 naming (`SGE-Test-<timestamp>-<pid>`, unique per run so a present name always means a prior failed cleanup) and keeps cleanup reasoning single-point. Alternative rejected: per-gate sandboxes (N create/delete cycles multiply flakiness and LIST-churn against the real server).
- **Pre-flight guard + prefix asserts reused verbatim from Phase 21/22:** `SGE-Test-` prefix assert before EVERY mutating call site in code, LIST snapshot at start, abort on unexpected mid-run change (21 D-03 / 22 D-02). Sent-copy APPEND to the real Sent folder is the sole sanctioned exception (21 D-02 pipeline behavior), UID-tracked + expunged in cleanup. Alternative rejected: weakening the guard for "read-side" gates (LIST drift detection is what makes 7/8 trustworthy).
- **Cleanup discipline mirrors Phase 22 D-03 / 21 D-15:** delete test INBOX arrivals + Sent copy + sandbox folder, retry 3× (5s/15s/45s), then print exact manual-cleanup steps — never blind infinite retry of destructive ops. Alternative rejected: per-leg cleanup (leaves partial state when an early leg fails).
- **Single combined bundle REPORT at end of run** (per-gate PASS/FAIL lines + DUMP path + SCAN lines), same `/tmp/sge-live26-<pid>.log` capture shape as `scripts/live-21.sh` / `scripts/live-22.sh`. Alternative rejected: one report per gate (harder to audit LIVE-03 as a single requirement).

### Offline-first structure (CI green without creds, agents never create creds)
- **Every new live leg ships a MockSession/FakeSession double leg** (patterns in `src-tauri/src/sync/worker.rs` / `src-tauri/src/imap/manager.rs`, already cited by 22-CONTEXT) so `cargo test` is fully green with no creds and no network. Rationale: Phase 21/22 precedent — offline doubles are the CI fallback, live legs are additive. Alternative rejected: live-only legs (CI would go red/fragile without creds).
- **Live legs clean-SKIP (exit 2, zero IMAP/SMTP traffic) when `src-tauri/.live-test.env` is missing or unreadable** — same contract as both prior bins and both wrapper scripts. Rationale: locked sandbox contract; the Phase 23 executor is concurrently working and must never be blocked by a hanging live attempt.
- **No agent ever creates the creds file:** scripts read it if the human provided it; otherwise SKIP. The `.example` file is the only template. Rationale: explicit task constraint + secret-safety; prevents accidental credential-shaped commits. Alternative rejected: generating a placeholder creds file (risks real-secret confusion + git accidents).
- **Credential flow mirrors 21 D-04 / 22 D-13+D-14 verbatim:** `set -a; . file; set +a` → env vars → seed OS keyring ONCE via `--seed-only` → all auth reads from keyring; values never travel as CLI args (no `/proc`/history leakage); tracing stays OFF while secrets are in scope. Alternative rejected: per-leg keyring seeding (redundant writes, wider secret window).

### Reporting & deferral criteria (what counts as done vs deferred)
- **Per-gate verdict lines are mandatory:** each deferred gate emits `STEP n/N <gate-name>: PASS|FAIL <detail>` in the log plus a one-line row in the bundle REPORT (gate, verdict, evidence pointer). A gate without a line is an incomplete run, not a silent pass. Rationale: LIVE-03 requires "results recorded" — the line is the record. Alternative rejected: narrative-only summary (unauditable).
- **A deferral is documentable only with (a) gate name, (b) concrete reason** (no creds / server limitation e.g. 993-only for STARTTLS-shape legs / flaky-server evidence / out-of-scope verb), **(c) resume pointer** (`/gsd-verify-work N` or next-run command). Bare "deferred" with no reason is a FAIL, not a deferral. Rationale: matches the v1.1/v1.2/v1.3 audit deferral-row shape.
- **The bundle report lives in two places:** the run log under `/tmp` (ephemeral, may contain redacted transcripts) plus a committed phase doc recording per-gate verdicts and deferral rows with reasons — never secrets, never credential substrings, never raw transcripts. Rationale: secret-scan gates (21 D-13 / 22 D-15 `grep -F` over logs + SQLite dump → FAIL on hit) apply unchanged; only the verdict table enters the repo. Alternative rejected: committing full logs (secret-leak surface).
- **Secret-scan + PEEK-gate + order-evidence checks from the wrappers carry over:** in-bin full-DB dump scan (`SCAN-DB` line) + wrapper re-check of log + dump, no non-PEEK `BODY[` sections in any FETCH transcript, and replay-before-sweep order evidence where backfill legs run. Rationale: scout shows these are load-bearing lines in `live_gate_21/22` + wrappers; dropping them silently weakens the safety case.

### the agent's Discretion
- Exact STEP numbering/total and bin name (`live_gate_26` recommended for symmetry).
- Whether the bundle bin shells out to / links against 21/22 leg functions vs re-running their binaries and citing REPORT lines.
- Chunk-size / large-folder probe thresholds for the v1.3 batch leg (server-throttle etiquette).
- REPORT markdown table exact columns beyond gate/verdict/evidence.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src-tauri/src/bin/live_gate_21.rs` — STEP PASS/FAIL runner, exit 0/1/2, `--seed-only` keyring path, GateSession decorator, pre-flight LIST snapshot, 3× cleanup backoff, in-bin secret scan + DUMP + REPORT; structural template for the bundle bin, and its legs cover the 13-derived gates.
- `src-tauri/src/bin/live_gate_22.rs` — same runner shape for draft one-copy + replay-order + double-sync resurrection + Seen both-ways + prefix audit; its legs cover the 12/6-derived gates.
- `scripts/live-21.sh` + `scripts/live-22.sh` — wrapper pattern to clone: env-file sourcing as env vars, `/tmp` log capture, `grep -F` secret scan over log + dump, PEEK-gate, order-evidence check, exit-code propagation.
- `src-tauri/src/sync/worker.rs` (MockSession) + `src-tauri/src/imap/manager.rs` (FakeSession) — double patterns for the offline-first legs.
- Deferred-gate specs: `.planning/v1.1-MILESTONE-AUDIT.md` (6/7/8 resume lines), `.planning/v1.2-MILESTONE-AUDIT.md` (12/13 rows), `.planning/v1.3-MILESTONE-AUDIT.md` (`deferred_live_gates` list), `.planning/21-LIVE-RUN.md` if present (prior run evidence shape).

### Established Patterns
- Sandbox safety contract (locked, Phases 21/22): `SGE-Test-<run-id>` per run, prefix assert before every mutating call, LIST snapshot + abort on drift, self-only mail (`SGE_LIVE_SELF` + `SGE-TEST-` subject prefix + per-run Message-ID + send-audit), BODY.PEEK-only reads, cleanup retry 3× 5s/15s/45s then manual steps, secret scan fails the run, exit-2 SKIP without creds.
- Credential flow: env file → env vars → keyring seed ONCE → all auth from keyring; never CLI args; no `set -x` in secret scope.
- Product verbs under test are all shipped: `drafts.rs` one-copy save, `send_queue.rs` enqueue/guard/reconcile/recover, `commands/sync.rs` `set_seen`/`replay_outbox`, `classify/filing.rs` confirm→MOVE replay-mirror, Phase 10/11 MOVE/CREATE/RENAME/DELETE + UIDVALIDITY gating, Phase 8 poll-tick + Phase 9 backfill sweep.

### Integration Points
- New bundle harness connects via existing `SessionManager`/lease + `SyncWorker` entry points (no second IMAP connection, no second SMTP transport); creds flow keyring-first.
- No frontend surface: bundle asserts backend behavior + log evidence only (ROADMAP UI: no).

</code>

<specifics>
## Specific Ideas

- User's hard constraints (verbatim, carried from Phases 21/22): test script "must not tamper with the mail that is in the account nor the folders"; "must create new mails (sending them to myself)"; "never send email to other accounts, just to my own account".
- `SGE-TEST-` subject prefix + per-run Message-ID make test mail visually distinguishable in webmail during the run; per-run unique sandbox name means a present name always signals a prior failed cleanup.
- REQUIREMENTS.md LIVE-03 text is the acceptance anchor: "All deferred `/gsd-verify-work` gates (6/7/8 for v1.1, 12/13 for v1.2, confirm→MOVE for v1.3) executed against the real server under the sandbox contract; results recorded; any remaining deferrals documented with reason." Requires all phases + creds file.

</specifics>

<deferred>
## Deferred Ideas

- STARTTLS live proof against a 143 server (mail.utfpr.edu.br is 993-only; stub + unit coverage stand — carried from v1.1 audit).
- IDLE push / CONDSTORE/QRESYNC fast paths (deferred since M1; not live gates, not in bundle scope).
- Batch dry-run + retry-failed, override-history view, Laya fine-tuning (v1.3 future work; not verification gates).

</deferred>
