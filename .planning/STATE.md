---
gsd_state_version: "1.0"
milestone: v1.3
milestone_name: Auto-Classify
status: planning
last_updated: "2026-10-10T00:00:00.000Z"
last_activity: 2026-10-10
progress:
  total_phases: 6
  completed_phases: 0
  total_plans: 0
  completed_plans: 0
  percent: 0
---

---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-10 — Milestone v1.3 started)

**Core value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.
**Current focus:** v1.3 Auto-Classify (Phases 15-20, roadmap created, not started)

## Current Position

Phase: 19 (Taxonomy Editor) — next; 15 freeze+bundle pending, 16+17+18 COMPLETE
Plan: 19-01 backend next
Status: 4/6 phases complete; executing 19→20 autonomously
Last activity: 2026-10-10 — Phase 18 verified+closed (318 tests green, tsc clean)

## Performance Metrics

**Velocity:**

- v1.2 shipped 2026-10-08: 5 phases (10-14), 252+ tests green
- v1.1 shipped 2026-10-05: 4 phases (6-9), 8 plans
- v1.0 shipped 2026-10-03: 5 phases (1-5)

**By Phase:**

| Phase | Plans | Status |
|-------|-------|--------|
| 15. Sidecar Packaging Spike | 0/0 | Not started |
| 16. Taxonomy + Store | 0/0 | Not started |
| 17. Classify Engine (No Moves) | 0/0 | Not started |
| 18. Confirm + Trust UX | 0/0 | Not started |
| 19. Taxonomy Editor + Import | 0/0 | Not started |
| 20. Batch Reorganization | 0/0 | Not started |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table. v1.3 structural decisions (from research SUMMARY.md, FINAL):
- Sidecar: PyInstaller-frozen `laya[serve]` 0.4.2 speaking `POST /v1/systemone` (+ `/batch`), CPU-only torch, multilingual checkpoint only, weights as Tauri `resources`, binary `sge-laya-<target-triple>` via `externalBin`. Lean ONNX/IPC is a deferred fallback with an explicit trigger (size/RAM blowout), never parallel.
- Build order: packaging spike → taxonomy/store → suggestions-without-moves → confirm/MOVE → editor/import → batch.
- Trust: single-email always user-confirmed (IPC-enforced); batch unconfirmed with journal + undo; worst case `A Classificar`, never delete; secondary label local-only.
- Privacy: redaction sanitizer designed once in Phase 16, reused everywhere; labels store pointers, never content.

### Pending Todos

- Plan Phase 15 (packaging spike — measure, don't research: size, cold-start, lifecycle in dev AND built bundle).
- Research flags for later planning: Phase 17 needs Laya 0.4.2 question-shaping re-verification (`--research-phase`); Phase 20 needs chunk/journal/UIDVALIDITY + live throttling verification.
- Live validation deferred from earlier milestones: `/gsd-verify-work 6/7/8` (v1.1), `/gsd-verify-work 12/13` (v1.2 send/draft round-trips).

### Blockers/Concerns

- Bundle size (~1 GB est.) and cold-start seconds unknown until the Phase 15 spike measures them — fallback trigger armed if packaging limits break.
- Confidence threshold default must be tuned on representative pt-BR mail in Phase 17 (no universal value).
- `Auto`-root collision (user already owns `Auto`) → confirm-then-nest dialog decided at Phase 18 planning.

## Deferred Items

Items acknowledged and deferred at milestone close, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| Validation | SMTP 587 live send + draft round-trip | Deferred | v1.2 close | v1.2 |
| Validation | Seen round-trip + offline replay vs live server | Deferred | v1.1 close | v1.1 |
| Validation | Folder tree/browse/badges vs webmail | Deferred | v1.1 close | v1.1 |
| Validation | Poll arrival + reconnect recovery live | Deferred | v1.1 close | v1.1 |
| Feature | IDLE push (poll stays fallback) | Deferred | M1 close | v1.1 |
| Feature | CONDSTORE/QRESYNC fast path | Deferred | M1 close | v1.1 |
| Feature | Learning/fine-tuning Laya from overrides (log from day one) | Deferred | v1.3 scoping | v1.3 |
| Feature | Batch dry-run + retry-failed | Deferred | v1.3 scoping | v1.3 |
| Feature | Override history view | Deferred | v1.3 scoping | v1.3 |

## Session Continuity

Last session: 2026-10-10
Stopped at: v1.3 roadmap created, ready to plan Phase 15
Resume file: None

## Operator Next Steps

- Plan Phase 15: `/gsd-plan-phase 15` (after `/gsd-discuss-phase 15` per workflow)
- Validate live when convenient: `/gsd-verify-work 6/7/8` (v1.1) + `/gsd-verify-work 12/13` (v1.2)
