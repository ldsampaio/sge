---
phase: 13-send-pipeline
plan: 13-02
subsystem: send-transport
tags: [smtp, lettre, starttls, send-queue, backoff, exactly-once, reconcile]
requires:
  - phase: 13-01
    provides: M10 send_queue table + SendGate + backoff + crash-reset + frozen command contract
provides:
  - lettre 0.11 SMTP transport (fail-closed STARTTLS, per-account pool, pure verdict mapping)
  - flush_send_queue + reconcile_uncertain_sends (step 4d wired in both pass branches)
  - SentUnfiled outcome definition for the 13-03 APPEND leg
affects: [13-03 (APPEND leg + M11 migration + SendFlushEnv plumbing from commands), 14-compose (queued-send consumer)]
tech-stack:
  added: [lettre 0.11.23]
  patterns: [fail-closed-starttls-constructor-tripwire, pure-verdict-classifier, reconcile-not-resend, gate-serialized-flush, crash-triage-to-uncertain]
key-files:
  created:
    - src-tauri/src/smtp.rs
  modified:
    - src-tauri/Cargo.toml
    - src-tauri/Cargo.lock
    - src-tauri/src/lib.rs
    - src-tauri/src/sync/worker.rs
    - src-tauri/src/store/queries.rs
    - src-tauri/src/send_queue.rs
key-decisions:
  - "starttls_relay (port 587, upgrade-required) — matches smtp.utfpr.edu.br:587/STARTTLS; the implicit-TLS 465 constructor is never used"
  - "Stranded sending rows triage to uncertain (reconcile-not-resend), not blind requeue — closes the crash-after-accept duplicate window"
  - "sent_unfiled CHECK migration deferred to 13-03 M11 with the APPEND leg; 13-02 defines the SentUnfiled outcome only"
  - "MAX_SEND_ATTEMPTS=10 transient cap, 60 s per-command SMTP timeout, backoff constants inherited from 13-01 (30 s base, 15 min cap, ±20 % jitter)"
  - "Flush is sync (blocking-thread discipline); reconcile resolves Sent via LIST+roles only when uncertain rows exist"
decisions: []
metrics:
  duration: ~55min
  completed: 2026-10-08
status: complete
actuals:
  tokens: 19016
  tasks: 3
  commits: 2
  plan_head_before: e75cd188e2b27711b68cec6fa56e5c78bb9c95a6
commits:
  - 84b2df5
  - 3066d64
requirements-completed: [SEND-04]
coverage:
  - id: D1
    description: "Queued mail flushes over SMTP with fail-closed STARTTLS and keyring credentials, never blocking the async runtime"
    requirement: "SEND-04"
    verification:
      - kind: unit
        ref: "src-tauri/src/smtp.rs#fail_closed_starttls_never_plaintext"
        status: pass
      - kind: unit
        ref: "src-tauri/src/smtp.rs#pool_caches_one_client_per_account_key"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#flush_sent_marks_row_sent_with_identical_bytes"
        status: pass
    human_judgment: false
  - id: D2
    description: "DATA-timeout or connection-drop-after-send yields uncertain, reconciled by Sent SEARCH before any re-send"
    requirement: "SEND-04"
    verification:
      - kind: unit
        ref: "src-tauri/src/smtp.rs#data_timeout_and_mid_send_break_map_to_uncertain"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#flush_uncertain_reconciles_before_any_resend"
        status: pass
    human_judgment: false
  - id: D3
    description: "Backoff retries eventually deliver; failed stays terminal-with-manual-retry and is never auto-dropped"
    requirement: "SEND-04"
    verification:
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#flush_transient_backs_off_failed_terminal_and_skips_failed"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#flush_serializes_on_send_gate_one_delivery_per_row"
        status: pass
      - kind: unit
        ref: "src-tauri/src/sync/worker.rs#crash_reset_runs_before_first_flush"
        status: pass
    human_judgment: false
  - id: D4
    description: "Live SMTP delivery against smtp.utfpr.edu.br:587 with real credentials"
    requirement: "SEND-04"
    verification: []
    human_judgment: true
    rationale: "No live send attempted — needs real UTFPR credentials and would deliver real mail; deferred per the phase live-gate precedent (unit proof via FakeTransport only)"
---

# Phase 13 Plan 02: SMTP transport + flush/reconcile Summary

**lettre 0.11 send path with fail-closed STARTTLS, gated flush with backoff, and reconcile-not-resend — 253 backend tests green**

## Performance

- **Duration:** ~55min wall
- **Started:** 2026-10-08
- **Completed:** 2026-10-08
- **Tasks:** 3 / 3
- **Files modified:** 7 (1 created, 6 modified)

## Accomplishments

- `smtp.rs`: `SmtpTransport` trait (`Sent | Uncertain | Transient | Permanent`) plus lettre-backed `SmtpTransporter` — sync transport, per-account pool keyed `host:port:username` (SessionManager shape), built exclusively via `starttls_relay` (upgrade failure is a hard error, never plaintext)
- Pure `classify_fault` verdict mapping: timeouts and mid-send breaks → `Uncertain`; 4xx → `Transient`; 5xx → `Permanent`; TLS failure → `Transient`-never-plaintext; static plain-language messages with sanitized numeric codes only (no secret, address, or body content)
- `flush_send_queue` on `SyncWorker`: SendGate single-flight (busy → skip), crash-reset triage at entry, per-row `queued → sending → verdict`, envelope from To+Cc+BCC, immutable `.eml` bytes re-sent identically; `Transient` backs off via 13-01 schedule, parks `failed` after 10 consecutive tries, `Permanent` parks immediately, `failed` rows never auto-retried
- `reconcile_uncertain_sends`: zero IMAP when nothing uncertain or no Sent resolved; else SELECT Sent + Message-ID SEARCH — hit marks `sent` with no second transport call, miss requeues exactly once with attempts counted + backoff scheduled; restores the pass mailbox selection
- Step 4d wired after `replay_dirty_drafts` in both the normal and empty-mailbox branches (order: crash reset → imap_outbox replay → dirty drafts → flush); `SentUnfiled` outcome defined for the 13-03 APPEND leg
- Full backend suite green: **253 passed, 0 failed, 1 ignored** (239 plan-13-01 baseline + 14 new; the ignore is the pre-existing live-keyring test)

## Task Commits

1. **Task 1: lettre 0.11 transport over spawn_blocking with fail-closed STARTTLS** - `84b2df5` (feat: smtp.rs + Cargo dep/lock + lib registration; 6 unit tests)
2. **Task 2: Flush pass with backoff, uncertain reconcile-not-resend, sent-unfiled** - `3066d64` (feat: worker flush + reconcile + step-4d wiring + 3 new queries; 8 worker tests)
3. **Task 3: Full backend suite green, no IMAP regressions** - verification only, no code change (suite 253 green; BODY.PEEK/UID-only paths untouched)

## Files Created/Modified

- `src-tauri/src/smtp.rs` (created) — transport trait, verdict classifier, envelope builder, pooled lettre backend, 6 unit tests
- `src-tauri/src/sync/worker.rs` (modified) — flush/reconcile/pass-wiring + `SendFlushEnv`/`FlushSummary`/`ReconcileSummary`/`FlushOutcome` + 8 tests
- `src-tauri/src/store/queries.rs` (modified) — `list_sending_send_ids`, `list_uncertain_sends`, `mark_send_uncertain`
- `src-tauri/src/send_queue.rs` (modified) — contract comments only (`sent_unfiled` ownership → 13-03 + M11)
- `src-tauri/Cargo.toml` + `src-tauri/Cargo.lock` (modified) — `lettre = "0.11"` → resolved **0.11.23**
- `src-tauri/src/lib.rs` (modified) — `pub mod smtp` registration

## Decisions Made

- `SmtpTransport::starttls_relay` is the only client constructor (port 587, STARTTLS-required) — matches `smtp.utfpr.edu.br:587/STARTTLS`; the implicit-TLS 465 constructor is never used (pinned by the `fail_closed_starttls_never_plaintext` source-tripwire test)
- Crash-recovered `sending` rows become `uncertain`, not `queued` — a maybe-SMTP-accepted row must reconcile, never blindly re-send (strengthens the plan's reset-ordering requirement)
- `sent_unfiled` persistence (CHECK-extension → table rebuild) deferred to 13-03's M11 migration with the APPEND leg; 13-02 defines `FlushOutcome::SentUnfiled` only and `send_status().sent_unfiled` stays 0
- `MAX_SEND_ATTEMPTS = 10` consecutive transients → terminal `failed`; `SMTP_COMMAND_TIMEOUT_SECS = 60` per-command window; backoff base/cap/jitter inherited from 13-01
- Flush is a sync function (blocking-thread discipline like `creds.rs`; production path already runs inside `spawn_blocking`); reconcile is async (borrows the pass session, restores selection)
- Sent-wire resolution via `LIST` + Phase 11 roles runs only when uncertain rows exist; unresolvable Sent → wait untouched (13-03 owns create-behind-confirmation)
- 13-03 activation handoff: the 4d code path is wired but inert (`send_flush: None` default) until 13-03 plumbs `SendFlushEnv` from the command layer (keyring/in-memory account + shared transporter)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `?` applied to `push` return in `build_envelope`**
- **Found during:** Task 1 (first compile)
- **Issue:** `rcpts.push(...parse().map_err(...)?)` — the `?` bound to `push`'s `()` instead of the parse result
- **Fix:** Bound the parse to a typed `Address` local with `?`, then pushed
- **Files modified:** `src-tauri/src/smtp.rs`
- **Commit:** `84b2df5`

**2. [Rule 1 - Bug] lettre 0.11 address API differs from training knowledge**
- **Found during:** Task 1 (first compile)
- **Issue:** No `Mailbox` in `lettre::address` in 0.11 — envelope uses `Address` (addr-spec only); `Envelope::to` is a method, not a field (verified against docs.rs 0.11.23)
- **Fix:** Switched to `Address` + `to()` accessor via the documentation lookup; documented the bare-addr-spec fail-closed behavior
- **Files modified:** `src-tauri/src/smtp.rs`
- **Commit:** `84b2df5`

**3. [Rule 2 - Missing critical] Crash-recovered rows triage to `uncertain`**
- **Found during:** Task 2 (flush entry design)
- **Issue:** Plan required reset-before-flush ordering, but a plain `sending → queued` reset re-sends a row the server may already have accepted — the exact duplicate-send window this phase exists to close
- **Fix:** Flush entry lists stranded `sending` ids, runs the 13-01 reset (semantics preserved), then parks those ids `uncertain` for reconcile-not-resend
- **Files modified:** `src-tauri/src/sync/worker.rs`, `src-tauri/src/store/queries.rs` (`list_sending_send_ids`)
- **Commit:** `3066d64`

**4. [Rule 2/3 - Blocking] Three small queue queries the plan's file list omitted**
- **Found during:** Task 2 (flush/reconcile implementation)
- **Issue:** No existing query lists `sending`/`uncertain` rows or writes state+error without bumping attempts — all three are required by the flush entry, reconcile pass, and uncertain verdict
- **Fix:** Added `list_sending_send_ids`, `list_uncertain_sends`, `mark_send_uncertain` next to `list_due_sends`
- **Files modified:** `src-tauri/src/store/queries.rs`
- **Commit:** `3066d64`

**5. [Rule 2 - Missing critical] `sent_unfiled` contract ownership clarified**
- **Found during:** Task 2 (SentUnfiled outcome definition)
- **Issue:** 13-01 comments said 13-02 "owns the APPEND leg" — but the APPEND leg belongs to 13-03, and persisting `sent_unfiled` needs a CHECK-extension table rebuild that must land with it
- **Fix:** Defined `FlushOutcome::SentUnfiled` (never produced yet), updated `send_queue.rs` contract comments to assign the M11 migration + APPEND leg to 13-03
- **Files modified:** `src-tauri/src/sync/worker.rs`, `src-tauri/src/send_queue.rs`
- **Commit:** `3066d64`

**6. [Rule 1 - Bug] Crash-test summary expectation miscounted triage rows**
- **Found during:** Task 2 (test run — 48/49 green)
- **Issue:** Recovered rows triage outside the per-row loop (no transport call), so `summary.uncertain` counts only SMTP-uncertain verdicts this pass — the test asserted `(sent, uncertain) == (1, 1)`
- **Fix:** Corrected to `(1, 0)` with a comment; the state assertions (stranded → uncertain, queued → sent, 1 transport call) already prove the behavior
- **Files modified:** `src-tauri/src/sync/worker.rs` (test only)
- **Commit:** `3066d64`

---

**Total deviations:** 6 auto-fixed (3 Rule-1 bugs, 3 Rule-2 correctness)
**Impact on plan:** All required for correctness or compilation; no scope creep. Threat mitigations verified: T-13-05 (fail-closed constructor tripwire green), T-13-06 (no-secret battery green), T-13-07 (uncertain mapping + reconcile tests green), T-13-08 (backoff bounds + terminal-failed + gate tests green), T-13-SC (lettre legitimacy recorded below).

## Issues Encountered

- System pacman `rustc` remains ABI-broken (same as Phases 12–13-01); used the user-space rustup toolchain (`~/.cargo/bin`, rustc 1.99.0) for all verification — environment only, no project change
- HEAD is on `main`, but `.planning/config.json` sets `git.allow_default_branch_commits: true`, so per-task commits on main are the permitted path here (same as 13-01)
- Pre-existing `.planning/STATE.md` modification (from discuss) left untouched per dispatch instructions

## Known Stubs

None introduced. (`FlushOutcome::SentUnfiled` is a defined-but-unproduced variant awaiting 13-03's APPEND leg — an explicit cross-plan handoff documented in code, not a stub. `send_flush: None` default is the designed inert state until 13-03 plumbs the env.)

## Threat Flags

None beyond the plan's register — the new network surface (app → `smtp.utfpr.edu.br:587` over STARTTLS; transport → queue verdicts) is exactly the surface the plan's threat model covers, and every mitigation has a green test (see Deviations impact line).

## lettre Legitimacy Record (T-13-SC)

- Established crate: owners `amousset` + `github:lettre:release` team, MIT license, repo `github.com/lettre/lettre`, homepage `lettre.rs`
- Version: `0.11.23` — latest in the 0.11 line per the crates sparse index and docs.rs (3 Aug 2026), verified 2026-10-08; pinned in `Cargo.lock` (`registry+https://github.com/rust-lang/crates.io-index`)
- Default features already include `smtp-transport` + `pool` + `native-tls` (aligns with the project's `native-tls 0.2` pin); docs 93.07% coverage; no build-script concerns surfaced during compile

## Backoff Constants Record (for the plan-checker)

- Base 30 s / cap 15 min / ±20 % jitter inherited from 13-01 (`SEND_BACKOFF_BASE_SECS`, `SEND_BACKOFF_CAP_SECS`, `SEND_JITTER_PCT` — unchanged)
- `MAX_SEND_ATTEMPTS = 10` consecutive transient failures → terminal `failed` (manual retry only)
- `SMTP_COMMAND_TIMEOUT_SECS = 60` per-command window (connect/EHLO/STARTTLS/AUTH/MAIL/RCPT/DATA); DATA-timeout → `Uncertain`
- Pass ordering (both branches): crash reset → `imap_outbox` replay (4b) → `replay_dirty_drafts` (4c) → `flush_send_queue` + reconcile (4d); BODY.PEEK-only and UID-only IMAP invariants untouched (send path never FETCHes; SMTP leg issues zero IMAP verbs)

## User Setup Required

None - no external service configuration required. (Live SMTP delivery needs real UTFPR credentials and is deferred per the phase live-gate precedent.)

## Next Phase Readiness

- 13-03 builds on: `SmtpTransport`/`SmtpTransporter` (pool shared via `Arc`), `flush_send_queue` + `reconcile_uncertain_sends` (step-4d path wired, inert until `SendFlushEnv` is plumbed), `FlushOutcome::SentUnfiled` (produce it from the APPEND leg + M11 CHECK migration), `send_status_snapshot` shape (fill `sent_unfiled`), frozen `queue_send`/`retry_send`/`send_status` contract from 13-01
- Residual risk (accepted): a crash between server-accept and local `sent`-mark on a NON-stranded row is covered by reconcile only if the row was `uncertain`; the window is one pass's transport call, and crash leftovers now triage to `uncertain` rather than re-sending
- Do NOT touch: `commands/`, frontend — 13-03 owns the command layer and `SendFlushEnv` plumbing in `start_sync`

## Self-Check: PASSED

`smtp.rs` on disk; task commits `84b2df5` + `3066d64` present in history; full suite re-verified at Task 3: 253 passed, 0 failed.

---
*Phase: 13-send-pipeline*
*Completed: 2026-10-08*
