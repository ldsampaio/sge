# Phase 27, Plan 01 — SUMMARY

**Phase:** 27-live-validation-gate-capability-probes
**Plan:** 27-01-PLAN.md (tracer, wave 1)
**Status:** ✅ COMPLETE
**Date:** 2026-10-13

## Tasks Completed

| Task | Status | Verify Result |
|------|--------|---------------|
| Tracer: unauthenticated probes to scrubbed transcripts + capability-profile JSON | ✅ Done | All verify commands pass |
| Entry-criteria proof: exit-2 SKIP, scrubber tripwire, pre-flight hygiene | ✅ Done | All verify commands pass |

## Artifacts Produced

- `scripts/probe-27.sh` — executable probe wrapper (STEP 1/3 IMAP pre-auth, 2/3 SMTP EHLO, 3/3 port-4190)
- `.planning/probes/27-imap-capability.txt` — scrubbed IMAP pre-auth CAPABILITY transcript
- `.planning/probes/27-smtp-ehlo.txt` — scrubbed SMTP EHLO STARTTLS transcript
- `.planning/probes/27-managesieve-4190.txt` — scrubbed ManageSieve probe transcript (filtered/timeout)
- `.planning/probes/capability-profile.json` — per-account capability profile with atoms.*, managesieve.verdict, phase33_armed

## Verify Commands (all passed)

```bash
# Syntax check
bash -n scripts/probe-27.sh                    # → exit 0

# End-to-end probe run
source scripts/build-env.sh && bash scripts/probe-27.sh  # → exit 0, 3 transcripts + JSON emitted

# Exit-2 SKIP proof (v1.4 harnesses without creds)
./scripts/live-21.sh; test $? -eq 2            # → exit 2
./scripts/live-22.sh; test $? -eq 2            # → exit 2
./scripts/live-26.sh; test $? -eq 2            # → exit 2

# Secret tripwire over committed artifacts
grep -rF ... .planning/probes/                 # → exit 1 (no hits)

# Git hygiene
git check-ignore -q src-tauri/.live-test.env   # → exit 0 (git-ignored)

# Lib test suite unchanged
source scripts/build-env.sh && cargo test --manifest-path src-tauri/Cargo.toml --lib  # → 392 passed
```

## Capability Profile (emitted)

```json
{
  "schema_version": "1",
  "probe_tool": "probe-27.sh",
  "accounts": [{
    "host": "mail.utfpr.edu.br",
    "port": 993,
    "mode": "implicit_tls",
    "capability_preauth": [],
    "capability_postlogin": "SKIP: operator-only credentialed leg",
    "atoms": { "IDLE": false, "CONDSTORE": false, "QRESYNC": false },
    "smtp": { "host": "smtp.utfpr.edu.br", "port": 587, "mode": "starttls",
              "ehlo": ["mail.utfpr.edu.br","PIPELINING","SIZE 31465824","VRFY","ETRN","ENHANCEDSTATUSCODES","8BITMIME","DSN"] },
    "managesieve": { "port": null, "reachable": false, "verdict": "filtered/timeout" },
    "phase33_armed": false
  }]
}
```

**Key findings:**
- IMAP pre-auth CAPABILITY returns minimal response (server defers full capabilities to post-login)
- SMTP STARTTLS EHLO works, advertises PIPELINING, SIZE 31MB, ENHANCEDSTATUSCODES, DSN, etc.
- Port 4190 (ManageSieve) is filtered/timeout on both mail.utfpr.edu.br and smtp.utfpr.edu.br
- Phase 33 (ManageSieve upload) is **not armed** — export-only path will ship
- All atoms default false per contract (no verbatim transcript evidence)

## Commits

- `a190a32` — docs(27): tracer probe-27.sh + scrubbed transcripts + capability-profile.json

## Deviations

None. All tasks completed per plan.

## Deferred

- Credentialed-leg deltas (LIVE-04 full, LIVE-05, UAT-01, UAT-02) → Plan 27-02
- Phase 33 execution conditional on positive ManageSieve probe (not met)

## Next

Execute Plan 27-02 (wave 2, depends on 27-01): credentialed-leg harness deltas, draft-send gap fix-or-defer, Node 20+ build check + BUNDLE-REPORT-27, UAT-27 checklist + deferred ledger.