# SGE — Phase 41 (Compose integration + Privacy audit) — Index (2026-10-18)

**Status:** Repair complete, awaiting UAT. All LLM requirements implemented and unit-tested; live coverage pending human validation.

## Deliverables

| ID | Name/Requirement | Status | Reference |
|---|---|---|---|
| 41-LLM-06 | Ollama localhost preset (Settings UI: one-click fill form) | ✅ Implemented, awaiting UAT | [`41-LLM-06-PRD.md`](./41-LLM-06-PRD.md) + settings panel wiring |
| 41-LLM-10 | Regenerate re-call with same prompt (replace existing draft in-place) | ✅ Implemented, awaiting UAT | [`41-LLM-10-PRD.md`](./41-LLM-10-PRD.md) + LlmConfigWindow UI variants |
| 41-LLM-08* | Reply threading headers (`In-Reply-To`/`References`) survive SMTP hop | ✅ Fixed, live-gate validated | [`live_gate_reply`](../src-tauri/src/bin/live_gate_reply.rs) (run 2026-10-18, exit 0) |
| 41-LLM-11* | Privacy guardrail (single-generation chokepoint; nothing leaves except on explicit click) | ✅ Implemented + audit report complete | `.planning/v1.8-research/privacy.md` + error display check |

\*These were originally requirements now validated by repair pass, not LLM-only features.

## Next Steps

1. **Human UAT** (blocking):  
   - Open the app with your VM account  
   - Reply to a real message using Ollama (`localhost:11434/v1`) and any endpoint  
   - Send a non-ASCII-named attachment  
   - Regenerate a draft when needed  
   - Confirm threading, privacy, cancel-mid-generation behavior  

2. **Ship** after UAT passes: v1.7 rebase onto the repair-pass branch; update CHANGELOG; tag.

## Files Modified by this Phase

- `.planning/PROJECT.md` (Phase 41 section added)  
- `.planning/STATE.md` (next step: update with phase progress)  
- `.planning/phase/41-LLM-06-PRD.md`, `41-LLM-10-PRD.md`  
- `src-tauri/Cargo.toml` (`[[bin]] live_gate_reply`)  
- `src-tauri/src/sync/worker.rs` (SENT_FLAGS usage, defect A/B fixes, registration tests)  
- `src-tauri/src/imap/mod.rs` (`header_search_needle`, `sent_flags_is_a_parenthesised_flag_list`)  
- `scripts/live-*.sh` gates  

## Notes

This phase completes the v1.8 LLM-assisted-reply lane by delivering live coverage, privacy guardrails, and a complete settings UI surface. Once UAT passes we ship the v1.7 milestone release.