---
gsd_state_version: "1.0"
milestone: v1.9
current_phase: 42
current_phase_name: first pending phase
status: planning
stopped_at: Phase 42 context gathered
last_updated: "2026-10-20T04:07:34.651Z"
last_activity: 2026-10-20
last_activity_desc: v1.9 roadmap created (phases 42–46)
state_head: c6d55ee9b51d58d61783eff46173f66c8702e1b1
progress:
  total_phases: 5
  completed_phases: 0
  total_plans: 0
  completed_plans: 0
milestone_name: Email Threading & Interface Revamp
---

# SGE — State (v1.9 Email Threading & Interface Revamp)

**Milestone:** v1.9 (roadmap: phases 42–46, written 2026-10-20)
**Status:** Roadmap written — Phase 42 ready to plan
**Prev milestone:** v1.8 LLM-Assisted Replies (closed 2026-10-19 with human UAT deferred; authoritative record `.planning/milestones/v1.8-closeout/v1.8-CLOSEOUT-ADDENDUM.md`). Repair context: the 2026-10-18 `dc7f8c7` pass made Reply/Reply-All real after Phase 38's over-claim ("stub compiles" precedent — Phase 46 gates exist to block this class).

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-20)
**Core value:** Connect to an IMAP server on Linux and read mail locally in a fast Gmail-like UI.
**Current focus:** Phase 42 — Threading Backend Lane

## Current Position

Phase: 42 of 5 v1.9 phases (Threading Backend Lane) — first pending phase
Plan: — (none planned yet)
Status: Ready to plan (`/gsd-discuss-phase 42` → `/gsd-plan-phase 42`)
Last activity: 2026-10-20 — v1.9 roadmap created (phases 42–46)

Progress: [░░░░░░░░░░] 0%

## Roadmap Summary (v1.9)

- Phase 42: Threading Backend Lane (THR-01..04) — M26, References fetch lane (BODY.PEEK-only), per-folder JWZ `thread_id`, extended payload + `get_thread` — **not started**
- Phase 43: UI Foundation & MessageListRow Extraction (REV-01..03 + roving readiness) — fonts vendored, primitives, ONCE-only row extraction w/ a11y fix (parallel-safe with 42) — **not started**
- Phase 44: Conversation List (CNV-01..08) — grouped rows on frozen `list_messages` contract — **not started**
- Phase 45: Conversation Reader + Message-vs-Conversation Actions (READ-01..10, ACT-01..04) — **not started**
- Phase 46: Revamp Sweep + Verification (REV-04..06, VER-01..02) — consumption-only sweep + full gates — **not started**

Same-commit invariants to enforce at plan time: peek_audit path-5 with the lane (42); SCHEMA_VERSION 25→26 with M26 (42); TS+Rust payload sides in one commit (42/44/45); keyboard roving paired with `:focus-visible` final form (46, post-REV-03 Open question: batch-undo backend contract confirmed at plan review (Phase 45); `mail-threading` 0.1.3 dep confirmation at plan review (Phase 42).

## Performance Metrics

**Velocity:**

- Total v1.9 plans completed: 0
- Average duration: —
- Total execution time: —

**By Phase:** (none yet)

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 42 | 0 | — | — |

## Accumulated Context

### Decisions

Full log in PROJECT.md Key Decisions. Recent ones affecting v1.9:

- 2026-10-20: v1.8 closed with human UAT **deferred** (preceito v1.5) — deferred items are backlog, never claims
- 2026-10-20: v1.9 threading = backend References lane (user-approved); subject-only client grouping rejected as unreliable
- 2026-10-20: keep clay identity + embed Baloo 2/Nunito woff2 locally (no CDN), normalize fallback stack
- 2026-10-18: reply threading reads the local BODY.PEEK cache; LLM single-flight tracker is shared `AppState` state

### Pending Todos

- [2026-10-20] Human UAT of v1.8 reply/send/LLM path + LLM-10 regenerate (deferred from v1.8 closeout) — carry forward; contact `milestones/v1.8-closeout/v1.8-CLOSEOUT-ADDENDUM.md`

### Blockers/Concerns

- 42: first live `HEADER.FIELDS (REFERENCES)` round-trip vs UTFPR Zimbra never verified — capture as-received bytes; Zimbra-refusal fallback (References=In-Reply-To) pre-declared as Known Gap; live leg deferred-with-reason (no creds in this env)
- 44: `list_threads` COUNT/OFFSET pagination-within-page contract — decide at plan review, not discovered
- 45: batch-undo is a backend contract change — needs explicit plan-review approval or honest N labeled toasts
- 42: `mail-threading` 0.1.3 dep (only approved Rust dep) + in-house ~250 LOC fallback pre-costed

## Deferred Items

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| UAT | v1.8 human UAT: reply threading on the wire, non-ASCII attachment, regenerate, cancel, privacy audit | Open (backlog) | 2026-10-19 | v1.8 |
| Live gates | LIVE-01/02/03 SMTP-send/draft/Seen round-trips | exit-2 SKIP contracts | v1.6 era | carried |

## Session Continuity

Last session: 2026-10-20T04:07:34.642Z
Stopped at: Phase 42 context gathered
Resume file: .planning/phases/42-threading-backend-lane/42-CONTEXT.md
