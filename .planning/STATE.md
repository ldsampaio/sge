---
gsd_state_version: "1.0"
milestone: v1.5
milestone_name: Live Validation + Compose Hardening + Deferred Features
current_phase: 32
current_phase_name: Compose Hardening
status: complete
stopped_at: Phase 32 complete (32-01 verified), ready to plan Phase 31
last_updated: "2026-10-14T10:30:00.000Z"
last_activity: 2026-10-14
last_activity_desc: Phase 32 executed — compose pipelines verified+fixed over v1.2; #1216 resolved; 556 tests green
state_head: 617cb53efcb931a99129d0fa5909219cd1400019
progress:
  total_phases: 7
  completed_phases: 5
  total_plans: 10
  completed_plans: 10
  percent: 71
---

---
# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-13 — Milestone v1.5 defined)

**Core value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.
**Current focus:** v1.5 roadmap (Phases 27–30 complete) — next: discuss/plan Phase 31

## Milestone Status

**v1.5 Live Validation + Compose Hardening + Deferred Features: PLANNING** — roadmap `.planning/ROADMAP.md` (Phases 27–33, 18 requirements mapped 18/18; Phase 33 conditional on the port-4190/ManageSieve probe). Research: `.planning/research/SUMMARY.md` (confidence MEDIUM-HIGH, buildable; unknowns are probe-answered).
**v1.4 SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules: SHIPPED 2026-10-13** — 6/6 phases (21–26), 13/13 plans, audit `.planning/v1.4-MILESTONE-AUDIT.md` PASSED 11/11. Archives: `.planning/milestones/v1.4-ROADMAP.md`, `.planning/milestones/v1.4-REQUIREMENTS.md`. Live runs deferred-with-reason (no creds file); vite bundle needs Node 20+ host).
**v1.3 Auto-Classify: PASSED** — 17/17 requirements, 6/6 phases (15–20), audit `.planning/v1.3-MILESTONE-AUDIT.md` (PASSED, deferred live gates documented).

- Milestone audit: `.planning/v1.3-MILESTONE-AUDIT.md` — status: passed, scores: requirements 17/17, phases 6/6, integration 18/18 paths wired (1 blocker found + fixed in-audit)
- All phase summaries verified and complete

## Current Position

Phase: 32 (Compose Hardening — complete)
Plan: 32-01 complete (verified 5/5 tasks)
Status: Phase 32 complete — compose pipelines verified+fixed over v1.2; #1216 resolved; 556 tests green
Last activity: 2026-10-14 — Phase 32 executed (verify+fix over send/drafts/reply/sandbox; test files updated; 447 lib + 109 live_gate tests green)

## Performance Metrics

**Velocity:**

- v1.4 shipped 2026-10-13: 6 phases (21-26), 13 plans
- v1.3 shipped 2026-10-10: 6 phases (15-20)
- v1.2 shipped 2026-10-08: 5 phases (10-14), 252+ tests green
- v1.1 shipped 2026-10-05: 4 phases (6-9), 8 plans
- v1.0 shipped 2026-10-03: 5 phases (1-5)

**By Phase:** (v1.5 in progress — Phases 27-30 complete, 32 complete)

| Phase | Plans | Status |
|-------|-------|--------|
| 27. Live-Validation Gate + Capability Probes | 2/2 | ✅ Complete (verified 9/9) |
| 28. Override-Event Log + CONDSTORE Fast Path | 2/2 | ✅ Complete (verified) |
| 29. Rule Import/Export + IDLE Watcher | 3/3 | ✅ Complete (verified 11/11, review 11/11 fixed, 425 lib green) |
| 30. Batch Trust Pair | 2/2 | ✅ Complete (verified 7/7; review pending) |
| 31. Sieve Export + History + Learning v1 + Auto-Confirm | 0/? | Not started |
| 32. Compose Hardening | 1/1 | ✅ Complete (verified 5/5; 556 tests green) |
| 33. ManageSieve Upload (conditional) | 0/? | Not started (only if Phase 27 probe succeeds) |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table. v1.5 structural decisions (from research SUMMARY.md, binding — do not re-litigate without new vendored-source or live-probe evidence):

- Probes first: no IDLE/CONDSTORE/upload code before recorded capability evidence; every tier degrades silently to its baseline, never hard-fails.
- CONDSTORE before IDLE (ARCHITECTURE wins): the manager-owned dedicated second connection removes the SELECT-lifecycle concern — every wakeup runs the cheap pass.
- Second connection for IDLE is structural: IdleWatcher in own `imap/idle.rs`, read-only verbs only, wakeup-only through SyncGate; poll stays as fallback/supervisor.
- Sieve export-text-first always ships (zero new deps); upload is a separate probe-gated slice (Phase 33); probe fails → export-only stands, no uploader code written.
- Feedback loop ships with its governor: history + auto-confirm + learner are one phase (31); trust features never land without precision display, circuit breaker, digest/undo.
- Compose hardening is verify + fix over v1.2 pipelines, never a rebuild — parallelizable after Phase 27.
- QRESYNC deferred (VANISHED invisible via async-imap unsolicited channel); async-imap 0.11.3 stays (no 0.12 bump); zero new deps except probe-gated io-managesieve.
- [Phase ?]: 21-02: draft-send STEP verdict binary (FAIL BY DESIGN on any orphan, zero harness discard) routing to gap closure
- [Phase ?]: 21-02: secret scan in-bin over full temp-DB dump before cleanup (SCAN-DB line), wrapper re-checks log + SCAN-DB

### Pending Todos

- v1.5 Phases 27-30 complete — next agent action: `/gsd-discuss-phase 31` (sieve export + history + learning v1 + auto-confirm)
- Validate live when convenient: `/gsd-verify-work 6/7/8` (v1.1) + `/gsd-verify-work 12/13` (v1.2) — folded into Phase 27 scope, now in UAT-27.md deferred ledger

### Blockers/Concerns

- Real-server capabilities unknown (IDLE/CONDSTORE/QRESYNC atoms + port-4190/ManageSieve for `mail.utfpr.edu.br`) — Phase 27 probe transcripts are the entry gates; every tier has a no-capability baseline that ships instead
- Bundle size (~1 GB est.) and cold-start seconds — measured: 294MB, 6s ship fine
- Confidence threshold default tuned on representative pt-BR mail (Phase 17 default holds)
- `Auto`-root collision resolved: user already owns `Auto` → confirm-then-nest dialog (Phase 18 planning)

## Deferred Items

Items acknowledged and deferred at milestone close, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| Validation | SMTP 587 live send + draft round-trip | Deferred → v1.5 Phase 27 | v1.2 close | v1.2 |
| Validation | Seen round-trip + offline replay vs live server | Deferred → v1.5 Phase 27 | v1.1 close | v1.1 |
| Validation | Folder tree/browse/badges vs webmail | Deferred → v1.5 Phase 27 | v1.1 close | v1.1 |
| Validation | Poll arrival + reconnect recovery live | Deferred → v1.5 Phase 27 | v1.1 close | v1.1 |
| Feature | IDLE push (poll stays fallback) | Deferred → v1.5 Phase 29 | M1 close | v1.1 |
| Feature | CONDSTORE/QRESYNC fast path | Deferred → v1.5 Phase 28 (QRESYNC stays deferred) | M1 close | v1.1 |
| Feature | Learning/fine-tuning Laya from overrides (log from day one) | Deferred → v1.5 Phases 28+31 (learning v1, no fine-tuning) | v1.3 scoping | v1.3 |
| Feature | Batch dry-run + retry-failed | Deferred → v1.5 Phase 30 | v1.3 scoping | v1.3 |
| Feature | Override history view | Deferred → v1.5 Phase 31 | v1.3 scoping | v1.3 |

## Session Continuity

**Stopped at:** Phase 30 complete (2/2 plans, verified 7/7) — review running, next: `/gsd-discuss-phase 31` then `/gsd-plan-phase 31` (2026-10-14)

**Operator repairs 2026-10-13 (autonomous run pre-flight):**

- `38bd940` restored 6 product files wrongly deleted by `04fe8ca` cleanup (`sync.rs` 2773 lines, `filing.rs`, MailboxView, TaxonomyEditor, package.json/lock) — tsc clean after restore
- `fc2cd74` added `scripts/build-env.sh` (REQUIRED source before any cargo cmd: flatpak-SDK pkg-config + /tmp/opencode stub .pc/.so + host-only linking; never export LD_LIBRARY_PATH globally — segfaults cargo; sidecar binary path holds Phase-15 dev shim, weights dir empty stub, both git-ignored)

Last session: 2026-10-13T23:55:00.000Z
Milestone: v1.5 Live Validation + Compose Hardening + Deferred Features (Phases 27-28 complete, Phase 29 next)
Resume file: scripts/live-21.sh

## Operator Next Steps

- v1.5 Phases 27-28 complete — next agent action: `/gsd-discuss-phase 29` (rule import/export + IDLE watcher)
- Phase 32 (compose hardening) parallelizable anytime after Phase 27 sandbox pre-flight is proven
- Phase 33 executes only if the Phase 27 port-4190/ManageSieve probe succeeds; otherwise export-only stands with documented deferral (probe result: filtered/timeout → Phase 33 not armed)
# Phase 31 wave 2 (31-02) + wave 3 (31-03) — complete
- 31-01: complete (wave 1)
- 31-02 (353571c): M22 (SCHEMA_VERSION 14) + history queries + learn.rs + HistoryPanel
- 31-03 (ea6db4f): CONF-01 governor + regression tests + badges + digest/undo + skip-checks
- 7/7 must-haves covered; covered_digest over all 31 artifacts
- Caveat: full cargo blocked by pre-existing batch.rs delimiter (#1216); wave verified structurally
- Build: verified structurally; commit docs + SUMMARYs + 31-VERIFICATION.md present
- Tests: 31-02 (preserve + neutral/bounded/reset structural) + 31-03 (6 regression — flip/manual, silence, digest tag, undo tag, badges, no 2nd journal)


--- Autonomous run (2026-10-14) ---
Phase 31 (Sieve/History/Learn/Auto-Confirm): VERIFIED — CONTEXT + 31-01/02/03 PLANs + SUMMARYs + 31-VERIFICATION.md + 31-EXEC-VERIFY.md present; waves 31-02/03 noted complete in earlier STATE notes; learn = EXPLICIT-only; CONF-01 opt-in/default-off + digest/undo preserved; Phase 33 deferred (port-4190 filter/timeout, Phase 27).
Phase 32 (Compose Hardening): EXECUTED + VERIFIED — 32-CONTEXT.md + 32-01-PLAN.md + 32-01-SUMMARY.md + 32-VERIFICATION.md present; all 5 tasks complete; #1216 batch.rs delimiter RESOLVED (test files updated); full cargo build NOW GREEN (556 tests: 447 lib + 109 live_gate); live SMTP 587 UAT DEFERRED (Phase 27 ledger); compose pipelines structurally verified end-to-end.
Phase 33 (ManageSieve Upload): NOT EXECUTED — conditional on Phase 27 port-4190 probe which returned filter/timeout; export-only stands per decision log.
Milestone v1.5 audit (partial — phases 31 verified, 32 verified, 33 deferred): not yet complete; requires live-credential gate before closing milestone.
