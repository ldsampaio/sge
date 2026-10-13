---
phase: 21-smtp-send-live-gate-live-01-sandboxed
plan: "01"
subsystem: live-gate-smtp-send
tags: [live-gate, smtp, tracer, sandboxed, LIVE-01, verification-only]
requires: ["phase-13-send-pipeline", "phase-22-harness-pattern"]
provides: ["live_gate_21-bin", "live-21-wrapper", "live_gate_21_safety-doubles"]
affects: ["21-02-expansion"]
tech-stack:
  added: []
  patterns: ["structural-clone-of-live_gate_22", "counting-transport-decorator", "shared-run-wide-call-counter", "exit-0-1-2-contract", "tracked-uid-only-cleanup"]
key-files:
  created:
    - src-tauri/src/bin/live_gate_21.rs
    - scripts/live-21.sh
    - src-tauri/tests/live_gate_21_safety.rs
  modified:
    - src-tauri/Cargo.toml
decisions:
  - "Sent-unfiled live leg uses an unresolvable Sent wire (lease fails → production SentUnfiled path) instead of GateSession injection — the flush drives IMAP via manager leases internally, so no decorator can gate its APPEND"
  - "One run-wide SharedCountingTransport counter spans arrival/no-duplicate/sent-unfiled flushes — per-env counters would hide a second send_raw"
  - "Cleanup expunges Sent UIDs via the Sent wire (UIDs are per-folder; INBOX expunge of a Sent UID would delete the wrong mail)"
  - "Same-account SMTP password reuses the IMAP keyring slot; an explicitly set SGE_LIVE_SMTP_PASS flows through keyring-or-env"
  - "A-07 asserted via FlushOutcome::SentUnfiled + transport call counts, never via send_status_snapshot.sent_unfiled (hard-codes 0 — recorded gap, RESEARCH Pitfall 5)"
metrics:
  duration: "~90 min"
  completed: "2026-10-13"
commits: 3
plan_head_before: eddac02
actuals:
  tokens: 23704
  tasks: 3
  commits: 3
status: complete
---

# Phase 21 Plan 01: SMTP Send Live Gate Tracer Summary

Tracer: prove the Phase 13 send pipeline delivers one self-addressed mail end-to-end against the real server, exactly once, with a verbatim Sent copy — inside the sandbox contract (`live_gate_21` bin + `scripts/live-21.sh` wrapper + offline safety doubles + `[[bin]]` registration).

## One-liner

Self-send live-gate tracer: recipient-guarded enqueue → production flush → IMAP-proven arrival + verbatim `\Seen` Sent copy + double-enqueue single-delivery + SentUnfiled never-resends, all offline-green with exit-2 clean SKIP when creds are absent.

## Tasks Completed

| # | Name | Commit | Files |
|---|------|--------|-------|
| 1 | Harness skeleton + wrapper + recipient guard (offline green) | fe25e49 | live_gate_21.rs, Cargo.toml, live-21.sh, live_gate_21_safety.rs |
| 2 | Self-send arrival + verbatim Sent copy + auto-save probe | d96945a | live_gate_21.rs, live_gate_21_safety.rs |
| 3 | Double-enqueue single-delivery + sent-unfiled never-resends | 4aa907e | live_gate_21.rs, live_gate_21_safety.rs |

## What Was Built

**`src-tauri/src/bin/live_gate_21.rs`** (1641 lines) — 8-STEP runner (`STEP n/8 <name>: PASS|FAIL`), exit 0/1/2, `--seed-only` keyring path, `--password`/`-p` argv refusal:
- STEP 1 preflight — LIST snapshot, Sent wire resolve (`resolve_sent_wire`), INBOX/Sent UIDVALIDITY epoch capture.
- STEP 2 recipient-guard — `assert_self_only` (case-insensitive, trimmed; to == exactly [self], cc/bcc empty, from == self) aborts BEFORE `enqueue_send`.
- STEP 3 arrival — `enqueue_send` self-send → A-01 snapshot (`pending_count ≥ 1`, `queued ≥ 1`) → production `flush_send_queue` (one pooled `SmtpTransporter`, manager leases) → `sent == 1` → INBOX `uid_search_header` exactly 1 UID (bounded 3×10s poll) → PEEK-only header confirm (subject prefix + Message-ID) → A-02/A-03 clean snapshot.
- STEP 4 sent-copy — Sent SEARCH exactly 1 UID + `\Seen` via envelope flags + `fetch_body` bytes byte-equal the queued `.eml`.
- STEP 5 no-duplicate — draft-anchored double `enqueue_send` (second `deduped: true`, one row, one `.eml`) → flush `sent == 1` → exactly 1 INBOX UID + 1 Sent UID.
- STEP 6 sent-unfiled — bogus Sent wire drives the production lease-failure → `SentUnfiled` path: outcome `sent_unfiled == 1`, exactly +1 transport call, row parked `sent` (never failed); re-flush empty with +0 calls (never-resend). A-07 gap logged in the STEP line.
- STEP 7 autosave-probe — end state exactly 1 Sent UID per filed leg (0|1 tolerated for the unfiled leg as the autosave signal), never a twin; branch named via `[SGE send]` flush lines in the wrapper log.
- STEP 8 cleanup — tracked-UID-only `UID EXPUNGE` (INBOX UIDs via INBOX, Sent UIDs via Sent wire), re-LIST == snapshot, temp DB + outbox dir removed, 3× backoff then manual steps.

**`scripts/live-21.sh`** — clone of `live-22.sh`: env-file source (`set -a`, no `set -x`), `/tmp/sge-live21-$$.log`, `grep -F` password scan → `SECRET-LEAK` FAIL, password unset, exit-code propagation.

**`src-tauri/tests/live_gate_21_safety.rs`** (22 tests) — drives REAL bin helpers via `#[path]` + REAL prod `enqueue_send`/`flush_send_queue` against in-memory doubles: guard accept/refuse/case variants, prefix + LIST-delta, GateSession open/closed, arrival SEARCH-count + verbatim shapes, offline enqueue-to-sent flush (A-01→A-03), draft-anchored dedupe (one row/one `.eml`), twin detection (2 hits refuse), sent-unfiled park shape + gap pin, D-12 oversize-reject (zero transport calls) + no-`Bcc:`-line.

## Verification

- `cargo test --bin live_gate_21`: 10/10 green, no creds, no network.
- `cargo test --test live_gate_21_safety`: 22/22 green, no creds, no network.
- Full `cargo test`: 328 lib + all targets green — no regressions.
- Bin without creds in env: `SKIP live-gate offline` exit 2, reached before any IMAP/SMTP traffic; `--password x` refused exit 2.
- Live `scripts/live-21.sh` (STEPs PASS vs real server): NOT run here — requires the user-supplied git-ignored `src-tauri/.live-test.env` + human execution (manual-only verification per 21-VALIDATION.md). The wrapper sources the env file; the file was never read, printed, or committed by this run.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Cleanup expunged Sent UIDs via the INBOX wire**
- **Found during:** Task 3 (wiring tracked cleanup for the new legs)
- **Issue:** The Task 1/2 cleanup loop ran `discard_server_copy_in("INBOX", uid)` for both INBOX and Sent UIDs. UIDs are per-folder — an INBOX expunge of a Sent UID would delete the wrong mail (T-21-05 tampering risk).
- **Fix:** `TrackedMail.sent_wire` set at preflight; cleanup expunges INBOX UIDs via INBOX and Sent UIDs via the Sent wire through a shared `expunge_tracked` retry helper.
- **Files modified:** src-tauri/src/bin/live_gate_21.rs
- **Commit:** 4aa907e

**2. [Rule 3 - Blocking] Flush drives IMAP via manager leases — GateSession cannot gate its APPEND**
- **Found during:** Task 3 (sent-unfiled leg design)
- **Issue:** The plan's "close the GateSession gate around the Sent APPEND" assumed the bin controls the session the flush uses, but `flush_one_row` leases IMAP sessions internally from `SendFlushEnv.manager`. No decorator injection point exists without prod-code changes (out of scope — verification-only).
- **Fix:** Drive the production APPEND-failure path instead: flush with an unresolvable Sent wire → lease failure → `FlushOutcome::SentUnfiled` (APPEND-only retry, never re-SMTP-send). Same code path, zero prod changes.
- **Files modified:** src-tauri/src/bin/live_gate_21.rs
- **Commit:** 4aa907e

**3. [Rule 2 - Missing critical] Per-leg transport counters would hide a re-send**
- **Found during:** Task 3 (never-resend proof design)
- **Issue:** Each flush leg built its own `CountingTransport` with a private counter, so a second `send_raw` in a later leg could not be distinguished from the leg's own first send.
- **Fix:** New module-level `SharedCountingTransport` forwarding into one run-wide `Arc<AtomicUsize>` shared by the arrival, no-duplicate and sent-unfiled flushes; arrival leg switched to it.
- **Files modified:** src-tauri/src/bin/live_gate_21.rs
- **Commit:** 4aa907e

Or otherwise: plan executed as written (all must-have truths hold offline; live STEPs implemented for the human-run gate).

## Known Stubs

None — every helper is wired into the live path or the offline doubles; no placeholder values, no mock data flows, no `TODO`/`FIXME` in the new files. The Task 2/3 middle legs are fully implemented (not scaffolded); only the live *execution* awaits the human-run gate with real creds.

## Decisions Made

- Sent-unfiled via unresolvable Sent wire (production lease-failure path), not decorator injection — no prod-code changes.
- Run-wide shared SMTP call counter for the never-resend proof.
- Per-folder-wire tracked cleanup (T-21-05).
- A-07 via flush outcomes + call counts; `send_status_snapshot.sent_unfiled == 0` pinned as a recorded gap (RESEARCH Pitfall 5) in both the STEP-6 detail line and an offline test.
- Same-account SMTP password reuses the IMAP keyring slot; explicit `SGE_LIVE_SMTP_PASS` flows through keyring-or-env.

## Auth Gates

None — no authenticated operation was performed. The bin's keyring/env credential path type-checks and SKIP-paths correctly; `--seed-only` and any live run await the human with the git-ignored env file.

## Self-Check: PASSED

- All 4 asserted paths git-tracked (`git ls-files --error-unmatch` OK); `scripts/live-21.sh` executable.
- Commits fe25e49, d96945a, 4aa907e present in log; no unintended deletions (`git diff --diff-filter=D` clean for plan files; unrelated pre-existing dirty files untouched).
- Bin + safety tests green post-commit state; full suite green; zero build warnings.
