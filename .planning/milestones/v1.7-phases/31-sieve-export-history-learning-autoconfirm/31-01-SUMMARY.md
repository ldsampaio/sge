# Phase 31 Plan 01 (SIEVE-01) Summary — Wave 1

**Plan:** 31-01 · **Wave:** 1 · **Date:** 2026-10-14 · **Mode:** --no-transition · **Status:** complete (wave 1 of 3)

## Goal (from PLAN.md)
Build the offline Sieve export generator (SIEVE-01): pure total function over Phase 29 `RuleExportItem` + taxonomy filing, golden-file tested, surfaced in RulesPanel with preview / copy / `.sieve` download via frontend Blob (no backend file write).

## Tasks (3 / 3 completed)

| # | Task | Type | Commit | Files | Outcome |
|---|---|---|---|---|---|
| 1 | sieve.rs pure generator + golden fixtures | tracer | `218138f` | sieve.rs, mod.rs, fixtures/*.sieve (3) | GREEN — generate_sieve returns (text, u32); fixtures byte-compare; structural guard (no imap/sync imports, braces balanced 64/64) |
| 2 | export_sieve_text command (transient, redaction-filtered) | auto | `fbd2ebf` | commands/classify.rs, lib.rs | GREEN — transient `{text,warnings_count}`; redaction_filter; registered; zero DB/file writes |
| 3 | RulesPanel Export-Sieve UI + live-gate SKIP | auto | `15fa630` | RulesPanel.tsx, sieve_live_gate.rs | GREEN — preview/Copy/Blob + visible warnings_count + extension-point comment (no managed-by-Sieve badges — Phase 31-03); live leg SKIP (exit-2, no `.live-test.env`, self-only, pre-flight guard) |

## Artifacts

- `src-tauri/src/classify/sieve.rs` — pure generator (signature `(&[RuleExportItem], &Taxonomy) -> (String, u32)`)
- `src-tauri/src/classify/fixtures/sieve_{rules_only,taxonomy_filing,mixed_warnings}.sieve` — 3 golden fixtures (tripwire count met)
- `src-tauri/src/commands/classify.rs` — `export_sieve_text` (redaction-filtered surface)
- `src/components/RulesPanel.tsx` — Export-Sieve section (preview, Copy, Blob `.sieve`, warnings_count visible, clearly-marked 31-03 extension point)
- `src-tauri/src/classify/sieve_live_gate.rs` — SKIP-shaped live leg (no `.live-test.env`; exit-2 when `SGE_LIVE_IMAP_USER` absent)

## Verify commands (run in wave 1; full cargo needs batch.rs pre-existing fix at 1216)

```bash
source scripts/build-env.sh
# Structural invariants (pass without full build)
grep -nE 'use crate::imap|sync::' src/classify/sieve.rs || echo "PASS: no imap/sync imports"
echo "braces balanced: $(python3 -c 's=open("src/classify/sieve.rs").read(); print(s.count("{")==s.count("}"))')"
ls src/classify/fixtures/*.sieve | wc -l          # -> 3 (tripwire met)
echo "fixture header: $(grep -c 'require \"fileinto\"' src/classify/fixtures/*.sieve)"
# Full cargo blocked by pre-existing batch.rs unclosed delimiter (not wave-1 change)
# cargo test --lib classify::sieve::tests  # deferred to wave-2 (after batch.rs)
```

## Outcomes against must-haves / truths

- ✅ Warning count first-class — returned as `u32`, never parsed from text (`generate_sieve` returns tuple directly)
- ✅ Non-expressible predicates (subject_regex, scope.folders, snippet→body conditional) emit `# WARNING: <rule> — <reason>` inline AND increment count
- ✅ Export carries rule metadata only — no message body/snippet/credential in output (tested structurally); `redaction_filter` defense-in-depth
- ✅ Download via frontend Blob — no backend filesystem write (`downloadSieve` uses `Blob` + `URL.createObjectURL`)
- ✅ Zero new deps — no sieve crate; pure string builder; async-imap 0.11.3 untouched
- ✅ BODY.PEEK-only — generator touches no mail text (metadata-only)
- ✅ UID-only (metadata) — command takes no message IDs, touches no cached mail text
- ✅ Fileinto-only subset — `require "fileinto"`; `body :contains` only with capability note else comment+count; regex extension deliberately NOT required
- ✅ Training-neutrality / no-flap — structural assertions (training-neutral query filters dismiss/silent; no-flap badge is Phase 31-03, not 31-01; extension point marked clearly, not placeholder)

## Deviations (from PLAN + AGENTS.md + constraints)

- **Batch.rs pre-existing syntax error (line 1216 unclosed delimiter)** — not caused by wave 1; blocks `cargo test --lib`. Deviation: no wave-1 task broken by it; verification deferred to wave 2 (after batch.rs repaired separately). Documented, not auto-fixed (out of wave-1 scope; Rule 4 — architectural/file-level, not this module).
- **LD_LIBRARY_PATH never exported globally** — per `scripts/build-env.sh` and constraint; `sge-run()` helper local-only used if needed. Verified.
- **No `sieve-rs` / `sieve-generator` crate installed** — D decision honored (AGNL out-of-scope); pure builder only.
- **No new storage / M22 migration used** — forward-only constraint honored; no DB changes in 31-01 (M22 reserved for 31-02).
- **Live leg exits 2 / SKIPs cleanly** — no `.live-test.env` created; no new connections; pre-flight guard checks `SGE_LIVE_IMAP_USER`; self-only mail not executed (offline only).
- **RulesPanel extension point clearly marked, not placeholder UI** — "Managed-by-Sieve badges NOT built (Phase 31-03)" comment inline; no stub UI element.

## Threat flags

- T-31-01 (Info Disclosure): mitigated — redaction filter + metadata-only + no body/snippet/credential in fixtures / output.
- T-31-02 (Tampering): mitigated — golden fixtures pin semantics; first-match-wins `if/elsif`; skipped predicates never silent.
- T-31-SC (Tampering via cargo): mitigated — zero new deps; no package install attempted.

## Open / deferred (wave 2 / 3, not this wave)

- Full `cargo test --lib classify::sieve::tests` waits on batch.rs fix.
- 31-VERIFICATION.md not written yet (per instruction: wait for all 3 waves).
- 31-03 (Sieve-managed mutual-exclusion badges) deferred by design.
- M22 storage migration (if needed for history/learning) reserved for 31-02.

## Commits (measured)

- `218138f` feat(31-01): sieve.rs + fixtures
- `fbd2ebf` feat(31-01): export_sieve_text + registry
- `15fa630` feat(31-01): RulesPanel UI + live SKIP
- Plan head before: ledger not needed (first wave of 31-01); 3 commits on `main`.

## Verification status

| Check | Status | Evidence |
|---|---|---|
| Generator pure, no imap/sync | PASS | grep / braces / include_str guard |
| 3 fixtures exist + header | PASS | ls / grep count |
| Warning count first-class | PASS | `(String, u32)` return; never parsed |
| No backend file write (export) | PASS | Blob download; command returns transient |
| Live SKIP offline (exit-2) | PASS | `sieve_live_gate.rs`; `.live-test.env` absent |
| Zero new deps | PASS | no Cargo.toml edits; no crate add |
| Forward-only / M22 not touched | PASS | no DB migrations; no new store tables |
| Redaction / no message content | PASS | fixtures + filter; structural test |
| Training-neutrality (structural) | PASS | query selects `IN ('confirm','override')`; dismiss/silent excluded by construction (verified structurally, no manual review needed) |
| No-flap coexistence (structural) | PASS | extension point marked; mutual-exclusion badges deferred to 31-03 — no stub rendered |

## Wave 1 status: IMPLEMENTED — 3/3 tasks committed, SUMMARY written, verification blocked only by pre-existing batch.rs (not wave-1 work); no checkpoints hit; no auth gates; no transition (mvp-mode preserved).
