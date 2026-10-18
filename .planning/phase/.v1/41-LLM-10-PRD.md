# SGE — LLM-10: Regenerate (Phase 41 deliverable)

**Status:** Implemented and unit-tested; awaiting Phase 41 human UAT against a configured endpoint.

## Context

LLM-05–LLM-09 focus on one-shot generation with a configurable prompt/endpoint. LLM-10 lets users regenerate a reply when the first result doesn't match their tone/facts: re-call `generate_reply` with an existing draft as input, replace the body in-place with the new result while preserving threading headers (`In-Reply-To` / `References`).

**Technical note:** The implementation is trivial for two reasons. First, threading headers are regenerated from current message attributes; second, the same prompt + endpoint call just needs a different payload (the previous draft as context). Privacy guard stays intact: same chokepoint, no extra network calls.

## UI

A single button or keyboard shortcut in any compose window (Reply/Reply-All) that re-invokes `generate_reply` with the current draft as "context" (or empty if no previous attempt), replacing the body with new text while preserving headers and recipients. The existing "Generate" button is reused; a label variant "Gerar novamente" or an icon next to it signals regeneration.

**Wiring code:** exists in `ComposeWindow.tsx` alongside the original Generate UI (commit 50c517b + follow-ups). This PRD documents the feature for UAT.

## Acceptance Criteria

- **AC-01** User edits an existing generated draft; clicks "Gerar novamente" (regenerate); the body is replaced with a new result while `In-Reply-To`, `References` headers remain attached to the message.
- **AC-02** The original error prompt template from LLM-03 remains usable if regeneration fails again (user can edit manually).
- **AC-03** Regeneration respects rate limits and timeouts; same retry logic as initial generation applies.

## Privacy Guardrail (LLM-11)

Regenerate is a re-cube: same HTTP request, same payload, same network path, same audit trail (endpoint, latency, tokens). No bodies beyond what already exists in the compose window leave except on Generate click. Keys never logged or displayed.  

**Regression check:** no changes to `generate_reply` I/O interface; existing tests validate signature continuity after adding regeneration semantics.

## Test Cases

| # | Description | Expected |
|---|---|---|
| 41-10-01 | User regenerates a failed draft; receives new text with same threading headers | PASS (headers preserved, body replaced) |
| 41-10-02 | Regeneration fails twice; user clears the body manually | PASS (no crash, manual editing continues to work) |
| 41-10-03 | Regeneration rate-limit hit from provider | "Servidor limitado. Aguarde um momento e tente novamente." |

## References

- `.planning/PROJECT.md` Phase 41 section  
- `.planning/v1.8-research/privacy.md` guardrail implementation  
- LLM-07: generation handler (commit 3925918 + follow-ups) for payload reuse

---

*Author:* GSD session 2026-10-18  
*Related commits:* dc7f8c7, 50c517b  
*Next:* UAT + Phase 41 completion