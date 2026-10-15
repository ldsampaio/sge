# Phase 27: Live-Validation Gate + Capability Probes - Context

**Gathered:** 2026-10-13
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous, batch proposals accepted)

<domain>
## Phase Boundary

Every open live-validation thread is closed against the real servers (`mail.utfpr.edu.br:993/SSL`, `smtp.utfpr.edu.br:587/STARTTLS`) and the app is human-verified end-to-end. Operator runs `live-21/22/26.sh` with own git-ignored creds under the sandbox contract; each leg is green or newly deferred with reason. Draft-send gap (FAIL BY DESIGN verdict) is resolved either way. `npm run build` succeeds on a Node 20+ host. Human click-through UAT is executed and recorded. Capability probes (CAPABILITY atoms IDLE/CONDSTORE/QRESYNC, per-account profile, port-4190/ManageSieve transcript) gate all downstream protocol work (Phases 28/29/31/33).

</domain>

<decisions>
## Implementation Decisions

### Live-Run Execution & Sandbox
- Credentialed legs are operator-only — agent never touches creds; agent verifies offline green + scripts exit-2 SKIP without creds
- Draft-send gap: fix if small, else explicit defer-with-reason (LIVE-05 must resolve either way — no orphan drafts, or documented deferral)
- Secret-scan strictness: transcript scrubber (LOGIN/AUTHENTICATE/env) + post-run secret grep; transcripts secret-clean or leg marked failed
- Post-run cleanup: full cleanup verified (SGE-Test-* folders removed, /tmp traces removed, no creds file created)

### Capability Probes & Gating
- Probe method: raw openssl/STARTTLS CAPABILITY + port-4190 TCP probe; transcripts recorded in `.planning/probes/`, creds git-ignored
- No-capability baseline: silent degrade to UID range-diff / poll fallback, never hard-fail (binding per research)
- Phase 33 trigger: port-4190 open + ManageSieve greeting = run Phase 33; else export-only stands and no uploader code is written
- Capability profile: per-account JSON (IDLE/CONDSTORE/QRESYNC/SIEVE atoms) consumed by Ph28/29/31/33 planners

### Bundle & UAT Recording
- Node 20+ build: verified on a Node 20+ host with version documented; env Node 18 SKIP is environmental, not a failure (UAT-01)
- Bundle artifact: Linux bundle smoke (install + health probe) recorded in BUNDLE-REPORT following the v1.4 pattern
- Human UAT: click-through checklist (scoped search/filter restore, archive rule save → Arquivar agora → progress → report → undo, rules live end-to-end tracer) recorded as a markdown table with pass/fail + notes (UAT-02)
- Deferred live items: any still-blocked leg gets deferred-with-reason + resume script, never silent (audit honesty)

### the agent's Discretion
- Probe script language (bash vs rust harness), transcript file naming, and exact scrubber regex set — follow existing `scripts/live-*.sh` conventions
- BUNDLE-REPORT/UAT table placement within phase dir — planner's choice

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `scripts/live-21.sh`, `scripts/live-22.sh`, `scripts/live-26.sh` harnesses + sandbox pre-flight guard + secret-scan pattern (v1.4 Phase 21/22/26)
- `scripts/build-env.sh` (REQUIRED source before any cargo cmd: flatpak-SDK pkg-config + /tmp/opencode stub .pc/.so + host-only linking)
- `src-tauri/src/imap/` session/manager surface (CAPABILITY path), `src-tauri/.live-test.env` git-ignored convention (never committed)

### Established Patterns
- Sandbox contract: dedicated `SGE-Test-<run-id>` folder per run, self-only mail, pre-flight abort on out-of-sandbox targets, post-run cleanup
- Exit-2 SKIP without creds (never create a creds file); live gates documented as deferred-with-reason in audits
- Forward-only migrations with preserve-rows tests; BODY.PEEK-only reads; UID-only addressing with UIDVALIDITY gating

### Integration Points
- Phase 28 consumes the CONDSTORE capability atom; Phase 29 consumes IDLE; Phase 31 consumes rule-envelope + probe note; Phase 33 consumes the port-4190 transcript
- STATE.md Deferred Verification + audit files record any remaining deferred legs

</code>

<specifics>
## Specific Ideas

- Reuse v1.4 live-harness conventions verbatim (STEP numbering, REPORT, SCAN-DB lines) so transcripts stay comparable across milestones
- Capability probe transcripts must be recorded before any IDLE/CONDSTORE/upload code — probes-first is binding

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope. Phase 33 execution itself is conditional (deferred to its own phase on probe failure, export-only stands).

</deferred>
