---
gsd_state_version: "1.0"
milestone: v1.5
milestone_name: Live Validation + Compose Hardening + Deferred Features
status: planning
last_updated: "2026-10-13T22:31:45.083Z"
last_activity: 2026-10-13
progress:
  total_phases: 7
  completed_phases: 0
  total_plans: 0
  completed_plans: 0
  percent: 0
---

---
# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-13 — Milestone v1.5 defined)

**Core value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.
**Current focus:** v1.5 roadmap complete (Phases 27–33) — next: discuss/plan Phase 27

## Milestone Status

**v1.5 Live Validation + Compose Hardening + Deferred Features: PLANNING** — roadmap `.planning/ROADMAP.md` (Phases 27–33, 18 requirements mapped 18/18; Phase 33 conditional on the port-4190/ManageSieve probe). Research: `.planning/research/SUMMARY.md` (confidence MEDIUM-HIGH, buildable; unknowns are probe-answered).
**v1.4 SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules: SHIPPED 2026-10-13** — 6/6 phases (21–26), 13/13 plans, audit `.planning/v1.4-MILESTONE-AUDIT.md` PASSED 11/11. Archives: `.planning/milestones/v1.4-ROADMAP.md`, `.planning/milestones/v1.4-REQUIREMENTS.md`. Live runs deferred-with-reason (no creds file); vite bundle needs Node 20+ host).
**v1.3 Auto-Classify: PASSED** — 17/17 requirements, 6/6 phases (15–20), audit `.planning/v1.3-MILESTONE-AUDIT.md` (PASSED, deferred live gates documented).

- Milestone audit: `.planning/v1.3-MILESTONE-AUDIT.md` — status: passed, scores: requirements 17/17, phases 6/6, integration 18/18 paths wired (1 blocker found + fixed in-audit)
- All phase summaries verified and complete

## Current Position

Phase: 27 (Live-Validation Gate + Capability Probes — next to discuss/plan)
Plan: —
Status: Roadmap complete, awaiting planning
Last activity: 2026-10-13 — v1.5 roadmap created (Phases 27–33)

## Performance Metrics

**Velocity:**

- v1.4 shipped 2026-10-13: 6 phases (21-26), 13 plans
- v1.3 shipped 2026-10-10: 6 phases (15-20)
- v1.2 shipped 2026-10-08: 5 phases (10-14), 252+ tests green
- v1.1 shipped 2026-10-05: 4 phases (6-9), 8 plans
- v1.0 shipped 2026-10-03: 5 phases (1-5)

**By Phase:** (v1.5 not started)

| Phase | Plans | Status |
|-------|-------|--------|
| 27. Live-Validation Gate + Capability Probes | 0/? | Not started |
| 28. Override-Event Log + CONDSTORE Fast Path | 0/? | Not started |
| 29. Rule Import/Export + IDLE Watcher | 0/? | Not started |
| 30. Batch Trust Pair | 0/? | Not started |
| 31. Sieve Export + History + Learning v1 + Auto-Confirm | 0/? | Not started |
| 32. Compose Hardening | 0/? | Not started (parallelizable after Phase 27) |
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

- v1.5: discuss/plan Phase 27 (live-validation gate + capability probes) — entry criteria (scrubber + pre-flight as code) before first credentialed run
- Validate live when convenient: `/gsd-verify-work 6/7/8` (v1.1) + `/gsd-verify-work 12/13` (v1.2) — standing deferred items, now folded into Phase 27 scope

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

**Stopped at:** v1.5 roadmap created (Phases 27–33) — next: `/gsd-discuss-phase 27` then `/gsd-plan-phase 27` (2026-10-13)

**Operator repairs 2026-10-13 (autonomous run pre-flight):**
- `38bd940` restored 6 product files wrongly deleted by `04fe8ca` cleanup (`sync.rs` 2773 lines, `filing.rs`, MailboxView, TaxonomyEditor, package.json/lock) — tsc clean after restore
- `fc2cd74` added `scripts/build-env.sh` (REQUIRED source before any cargo cmd: flatpak-SDK pkg-config + /tmp/opencode stub .pc/.so + host-only linking; never export LD_LIBRARY_PATH globally — segfaults cargo; sidecar binary path holds Phase-15 dev shim, weights dir empty stub, both git-ignored)

Last session: 2026-10-13T14:18:40.036Z
Milestone: v1.5 Live Validation + Compose Hardening + Deferred Features (roadmap complete)
Resume file: scripts/live-21.sh

## Operator Next Steps

- v1.5 roadmap complete — next agent action: `/gsd-discuss-phase 27` (live-validation gate + capability probes)
- Phase 32 (compose hardening) parallelizable anytime after Phase 27 sandbox pre-flight is proven
- Phase 33 executes only if the Phase 27 port-4190/ManageSieve probe succeeds; otherwise export-only stands with documented deferral
