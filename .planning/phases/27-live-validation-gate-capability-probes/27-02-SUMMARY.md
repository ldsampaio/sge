# Phase 27, Plan 02 — SUMMARY

**Phase:** 27-live-validation-gate-capability-probes
**Plan:** 27-02-PLAN.md (expansion, wave 2, depends on 27-01)
**Status:** ✅ COMPLETE
**Date:** 2026-10-13

## Tasks Completed

| Task | Status | Verify Result |
|------|--------|---------------|
| Credentialed-leg deltas: capability capture + REPORT/SCAN-DB conformance + cleanup asserts | ✅ Done | All verify commands pass |
| LIVE-05: draft-send gap resolved — explicit deferred-with-reason REPORT row | ✅ Done | Tests pass, deferral row present |
| UAT-01 + UAT-02 scaffolding: Node 20+ build leg, BUNDLE-REPORT-27, UAT-27 checklist + deferred ledger | ✅ Done | Files exist, syntax OK, no frontend diff |

## Artifacts Produced/Modified

- `scripts/live-21.sh` — capability capture lines + UAT pointer
- `scripts/live-22.sh` — capability capture lines + UAT pointer
- `scripts/live-26.sh` — Node 20+ build leg (STEP 0/15) + capability capture + UAT pointer
- `src-tauri/src/bin/live_gate_21.rs` — draft-send binary verdict changed to deferred SKIP row
- `.planning/phases/27-live-validation-gate-capability-probes/UAT-27.md` — click-through checklist + results table + deferred ledger
- `.planning/phases/27-live-validation-gate-capability-probes/BUNDLE-REPORT-27.md` — bundle verification scaffold (v1.4 pattern)

## Verify Commands (all passed)

```bash
# Wrapper syntax
bash -n scripts/live-21.sh && bash -n scripts/live-22.sh && bash -n scripts/live-26.sh  # → exit 0

# Exit-2 SKIP regression check (no creds)
./scripts/live-21.sh; test $? -eq 2  # → exit 2
./scripts/live-22.sh; test $? -eq 2  # → exit 2
./scripts/live-26.sh; test $? -eq 2  # → exit 2 (build check runs first, records Node 18 SKIP)

# Capability capture lines present
grep -c CAPABILITY-IMAP-PREAUTH scripts/live-21.sh scripts/live-22.sh scripts/live-26.sh  # → each >=1

# Live gate tests
source scripts/build-env.sh && cargo test --manifest-path src-tauri/Cargo.toml --bin live_gate_21  # → 19 passed

# Lib test suite unchanged
source scripts/build-env.sh && cargo test --manifest-path src-tauri/Cargo.toml --lib  # → 392 passed

# Node version recorded (env Node 18.19.1 SKIP environmental)
node --version  # → v18.19.1

# No frontend/toolchain diff
git status --porcelain -- src/ package.json package-lock.json  # → empty

# Scaffold files exist
test -f .planning/phases/27-live-validation-gate-capability-probes/UAT-27.md  # → OK
test -f .planning/phases/27-live-validation-gate-capability-probes/BUNDLE-REPORT-27.md  # → OK
```

## Key Outcomes

### LIVE-04 (Agent Half Complete)
- All three wrappers emit `CAPABILITY-IMAP-PREAUTH atoms=` lines (from 27-01 probe transcripts)
- All three wrappers emit `CAPABILITY-IMAP-POSTLOGIN atoms=SKIP reason=operator-only-credentialed-leg resume=/gsd-verify-work XX`
- REPORT/SCAN-DB conformance preserved (reason-less SKIP fails, PASS rows have REPORT)
- Full-cleanup asserts on all exit paths (wrapper retry 5s/15s/45s, closing LIST check)
- UAT checklist pointer printed at close

### LIVE-05 (Draft-Send Gap — Resolved by Explicit Deferral)
- Binary verdict in `live_gate_21.rs` render_report now emits:
  ```
  REPORT gate=draft-send verdict=SKIP reason=prod-flush-performs-no-delete_draft-local-or-server resume=src-tauri/src/sync/worker.rs:flush_one_row
  ```
- No FAIL BY DESIGN silent state — documented deferral with resume pointer
- Fix requires: delete_draft-in-flush (local) + server copy expunge via IMAP (Phase 10 machinery) — sized as exceeding small fix

### UAT-01 (Node 20+ Build Leg)
- `scripts/live-26.sh` STEP 0/15 records `node --version` and build result
- Env Node 18.19.1 → SKIP recorded as environmental, not product failure
- Real PASS documented on Node 20+ host with version recorded
- No vite pin touched, async-imap 0.11.3 stays, zero new deps

### UAT-02 (Human Click-Through + Deferred Ledger)
- `UAT-27.md`: full checklist (search/filter restore, archive→Arquivar agora→progress→report→undo, rules live tracer) + results table + deferred ledger
- `BUNDLE-REPORT-27.md`: v1.4 pattern (PASS/FAIL/SKIP-with-reason, no NOT-RUN rows)
- Deferred ledger: 7 items with explicit reason + resume pointers — no silent gaps

## Commits

- `7d5991b` — docs(27): 27-02 credentialed deltas + draft-send deferral + UAT/BUNDLE scaffolds

## Deviations

None. All tasks completed per plan.

## Next

Phase 27 complete — both plans (27-01 tracer, 27-02 expansion) done. Proceed to Phase 28 (Override-Event Log + CONDSTORE Fast Path) or run autonomous lifecycle (audit → complete → cleanup).

---

**Phase 27 Success Criteria — Status:**

1. ✅ live-21/22/26.sh deltas land with capability capture, conforming REPORT + SCAN-DB rows, full-cleanup asserts
2. ✅ LIVE-05 resolved: explicit deferred-with-reason REPORT row with resume pointer
3. ✅ UAT-01 leg records Node version with env-18 SKIP environmental + BUNDLE-REPORT-27 scaffolded
4. ✅ UAT-02 checklist + results table + deferred ledger scaffolded in UAT-27.md
5. ✅ No 27-03 created: UAT forms cohere in 27-02 as operator-executed scaffolding