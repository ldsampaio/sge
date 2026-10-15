---
gsd_state_version: "1.0"
milestone: v1.8
milestone_name: Roadmap — LLM-Assisted Replies
current_phase: 39
current_phase_name: LLM Settings Foundation
status: executing
last_updated: "2026-10-15T14:41:18.358Z"
last_activity: 2026-10-15
last_activity_desc: Phase 39 plan 39-01 complete (M23 store + validation, 17 tests green)
state_head: e208140
progress:
  total_phases: 3
  completed_phases: 0
  total_plans: 3
  completed_plans: 1
  percent: 33
---

# SGE — State (v1.8 LLM-Assisted Replies)

**Milestone:** v1.8 LLM-Assisted Replies (roadmap: phases 39–41, written 2026-10-15)
**Status:** Executing Phase 39
**Prev milestone:** v1.7 (phases 37–38 complete: New Message button + Compose window, Reply/Reply-All UX)

## Current Position

Phase: 39 (LLM Settings Foundation) — EXECUTING
Plan: 2 of 3 (39-01 complete)
Status: Executing Phase 39
Last activity: 2026-10-15 — Phase 39 plan 39-01 complete (M23 llm_settings + validate, 17 llm tests + 70 store tests green)

## Roadmap Summary

- Phase 39: LLM Settings Foundation (LLM-01..04) — store + validation + prompt render
- Phase 40: Generation Flow (LLM-05, LLM-07, LLM-09) — client + IPC chokepoint
- Phase 41: Compose Integration + Hardening/UAT (LLM-06, LLM-08, LLM-10, LLM-11) — UI + privacy audit

## Next

- **Phase 39 Plan 39-02:** keyring API-key entry + fenced prompt renderer on the 39-01 LlmSettings foundation
- **Phase 39 Plan 39-03:** Tauri settings commands + stale SCHEMA_VERSION (14-vs-23) fix + reqwest feature widening

## Decisions (accumulated)

- 39-01: `LlmSettingsError` variants are payload-free unit variants — secrets cannot be interpolated into errors by construction (T-39-01)
- 39-01: `DEFAULT_TEMPLATE` single-line literal shared byte-identically with the M23 seed; drift guarded by `seed_defaults_equal_consts` test
- 39-01: `pub mod llm;` one-line declaration in lib.rs required for compilation (plan's don't-touch-lib.rs honored otherwise); SCHEMA_VERSION fix deferred to 39-03
