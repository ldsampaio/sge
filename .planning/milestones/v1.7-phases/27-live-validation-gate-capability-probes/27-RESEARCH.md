# Phase 27: Live-Validation Gate + Capability Probes - Research

**Researched:** 2026-10-13
**Domain:** Live-validation closure + unauthenticated protocol capability probes (IMAP/SMTP/ManageSieve) on a Rust + Tauri v2 + React + SQLite Linux IMAP client
**Confidence:** HIGH (harness/probe source reads); MEDIUM (wire-probe command shapes, server behavior); LOW where marked

## Summary

Phase 27 closes every open live-validation thread from v1.4 against the real UTFPR servers and records the capability evidence that gates all downstream v1.5 protocol work. The phase is operator-executed for credentialed legs: the agent verifies offline green plus exit-2 SKIP without creds, and the human operator runs `scripts/live-21.sh`, `scripts/live-22.sh`, `scripts/live-26.sh` with their own git-ignored `src-tauri/.live-test.env` under the sandbox contract. No new application code ships except small harness/probe deltas (capability capture, draft-send STEP, Node 20+ build check, scrubber hardening); the binding rule from research is probes-first — no IDLE/CONDSTORE/upload code before recorded capability evidence.

The probe surface is three credential-free transcripts: IMAP `CAPABILITY` over implicit TLS (`mail.utfpr.edu.br:993`), SMTP `EHLO` capability list over STARTTLS (`smtp.utfpr.edu.br:587`), and a TCP + ManageSieve greeting probe on port 4190. The existing in-tree `imap_probe --pre-auth-only` binary already does the IMAP pre-auth leg with the same TLS discipline as the app and must be preferred over raw openssl where they overlap; raw `openssl s_client` covers the SMTP STARTTLS leg and the 4190 leg, which have no in-tree equivalent. Every transcript lands in `.planning/probes/` (new dir, does not exist yet — verified 2026-10-13) after passing through the scrubber, and the per-account capability JSON derived from them is the contract Phases 28/29/31/33 plan against.

**Primary recommendation:** Reuse the v1.4 harness conventions verbatim, add a thin `scripts/probe-27.sh` wrapper (pre-auth IMAP via `imap_probe`, SMTP-EHLO + 4190 via openssl, scrub + secret-grep + capability JSON emit), re-run the three live harnesses, resolve the draft-send FAIL BY DESIGN verdict either way, verify `npm run build` on a Node 20+ host, and record UAT + BUNDLE-REPORT as markdown tables — every leg green or deferred-with-reason, never silent.

## User Constraints (from CONTEXT.md)

### Locked Decisions
- Credentialed legs are operator-only — agent never touches creds; agent verifies offline green + scripts exit-2 SKIP without creds
- Draft-send gap: fix if small, else explicit defer-with-reason (LIVE-05 must resolve either way — no orphan drafts, or documented deferral)
- Secret-scan strictness: transcript scrubber (LOGIN/AUTHENTICATE/env) + post-run secret grep; transcripts secret-clean or leg marked failed
- Post-run cleanup: full cleanup verified (SGE-Test-* folders removed, /tmp traces removed, no creds file created)
- Probe method: raw openssl/STARTTLS CAPABILITY + port-4190 TCP probe; transcripts recorded in `.planning/probes/`, creds git-ignored
- No-capability baseline: silent degrade to UID range-diff / poll fallback, never hard-fail (binding per research)
- Phase 33 trigger: port-4190 open + ManageSieve greeting = run Phase 33; else export-only stands and no uploader code is written
- Capability profile: per-account JSON (IDLE/CONDSTORE/QRESYNC/SIEVE atoms) consumed by Ph28/29/31/33 planners
- Node 20+ build: verified on a Node 20+ host with version documented; env Node 18 SKIP is environmental, not a failure (UAT-01)
- Bundle artifact: Linux bundle smoke (install + health probe) recorded in BUNDLE-REPORT following the v1.4 pattern
- Human UAT: click-through checklist (scoped search/filter restore, archive rule save → Arquivar agora → progress → report → undo, rules live end-to-end tracer) recorded as a markdown table with pass/fail + notes (UAT-02)
- Deferred live items: any still-blocked leg gets deferred-with-reason + resume script, never silent (audit honesty)

### the agent's Discretion
- Probe script language (bash vs rust harness), transcript file naming, and exact scrubber regex set — follow existing `scripts/live-*.sh` conventions
- BUNDLE-REPORT/UAT table placement within phase dir — planner's choice

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope. Phase 33 execution itself is conditional (deferred to its own phase on probe failure, export-only stands).

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| LIVE-04 | Live-21/22/26 harnesses run green against the real account under the sandbox contract (self-only mail, pre-flight guard, secret scan, post-run cleanup) with user-supplied git-ignored creds | Harness delta § (STEP/REPORT/SCAN-DB conventions), sandbox contract, exit-2 SKIP verification |
| LIVE-05 | Draft-send gap closed — FAIL BY DESIGN fixed (no orphan drafts) or explicitly deferred with reason | Draft-send gap analysis (live_gate_21.rs STEP 10 verdict lines) |
| UAT-01 | `npm run build` succeeds on a Node 20+ host (rolldown-vite `node:util` styleText requirement) | Environment Availability (env Node is 18.19.1 → SKIP, needs 20+ host) |
| UAT-02 | Human click-through UAT executed and recorded (scoped search/filter restore, archive rule save → Arquivar agora → progress → report → undo, rules live end-to-end tracer) | UAT markdown-table recording pattern |

## Project Constraints (from AGENTS.md)

- IMAP must leave mail on server (BODY.PEEK only, never set \Seen in M1 — holds for all new fetch paths in v1.5). No SMTP/send in M1 (historical; send pipeline exists since v1.2, live-gated).
- INBOX only (v1.5 IDLE leg is INBOX-only); credentials only in OS keyring (never plaintext); Linux-only bundle.
- Headers-first sync, UIDVALIDITY-guarded incremental; sanitize HTML (ammonia) + sandboxed iframe before render.
- Stack pins: Tauri v2.12, async-imap 0.11, mail-parser 0.11 + full_encoding, rusqlite 0.37 bundled + rusqlite_migration 2.x, keyring 3 + sync-secret-service.
- Workflow: research + plan-check + verifier enabled (do not skip); phases run in numeric ROADMAP order; parallel execution allowed for independent plans.
- `source scripts/build-env.sh` is REQUIRED before any cargo command (flatpak-SDK pkg-config + /tmp/opencode stub .pc/.so + host-only linking). [VERIFIED: scripts/build-env.sh:1-20, read this session]
- Never export LD_LIBRARY_PATH globally (shadows cargo's libssl/curl, segfaults cargo); use `sge-run` wrapper only for running test/harness binaries if linked against SDK libs. [VERIFIED: scripts/build-env.sh:15-20]

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Live IMAP legs (21/22/26 bins) | API / Backend (Rust bins) | — | Harness bins speak IMAP directly via SessionManager verbs; no UI involvement |
| Capability probes (pre-auth) | API / Backend (CLI/bin) | — | `imap_probe`/openssl hit the wire; transcripts are files, not UI state |
| Transcript scrubber + secret-grep | API / Backend (bash wrapper) | — | Runs in `scripts/` at log time, before any artifact lands in repo |
| Capability profile JSON | Database / Storage (file contract) | API / Backend | Static JSON in `.planning/probes/` consumed by downstream planners |
| `npm run build` + bundle smoke | CDN / Static (build) | — | Frontend toolchain + Linux bundle install/health probe |
| Human click-through UAT | Browser / Client | — | Human drives the React UI; agent only provides checklist + records table |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `openssl` (system CLI) | 3.0.13 [VERIFIED: `openssl version` run this session] | SMTP STARTTLS EHLO probe + port-4190 TCP/ManageSieve probe | Only tool for the two legs with no in-tree equivalent; IMAP leg prefers `imap_probe` |
| `imap_probe` bin (in-tree) | pinned workspace (async-imap 0.11.3) [CITED: src-tauri/src/bin/imap_probe.rs:1-40] | Pre-auth IMAP CAPABILITY probe with app-identical TLS discipline | Same `sge_lib::imap::probe` core as the `connect_account` Tauri command; password env-only, `--password` flag refused |
| `live_gate_21/22/26` bins (in-tree) | workspace | Credentialed live legs (STEP/REPORT/SCAN-DB) | v1.4-proven harnesses; Phase 27 re-runs them, does not replace them |
| Node.js | 20+ (required); env has 18.19.1 [VERIFIED: `node --version` this session] | `npm run build` (`tsc && vite build`, vite ^8.0.16) [VERIFIED: package.json read] | rolldown-vite `node:util` styleText needs Node 20+; Node 18 SKIP is environmental |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `bash` + `grep -F` + `mktemp` | system | Probe wrapper, fixed-substring secret scan, /tmp log sinks | All probe/harness scripting; `-F` mandatory (passwords contain regex metachars) |
| `git check-ignore` | system | Pre-flight creds-file hygiene | Assert `src-tauri/.live-test.env` stays git-ignored before any credentialed run |
| `timeout`/`nc` (system) | system | Bound the 4190 TCP probe | Prevents a hung probe when the port is filtered (no response ≠ closed) |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `imap_probe --pre-auth-only` | raw `openssl s_client -connect mail.utfpr.edu.br:993` + manual `a0 CAPABILITY` | openssl works but bypasses app TLS/cert-exception semantics; use openssl as cross-check only, `imap_probe` as record |
| bash `probe-27.sh` | Rust harness bin for probes | Rust gives typed parsing but is heavier; CONTEXT leaves language to agent discretion — bash matches `live-*.sh` conventions and is recommended |
| In-repo capability JSON | Tauri command surfacing caps | No UI needs caps in Phase 27; a static JSON file is the planner-consumed contract |

**Installation:**
```bash
# No new packages. Phase 27 installs nothing: openssl/bash/git are system tools (verified present),
# imap_probe + live_gate_* build from the workspace. io-managesieve is EXPLICITLY NOT added
# in this phase — it ships only in Phase 33 and only on probe success.
```

## Package Legitimacy Audit

No external packages are installed in Phase 27. `io-managesieve 0.2.0` (the sole v1.5 candidate dep) is probe-gated to Phase 33 and must not appear in any Phase 27 task. No `cargo add`, no `npm install`.

| Package | Registry | Verdict | Disposition |
|---------|----------|---------|-------------|
| (none) | — | — | No installs; audit trivially clean |

**Packages removed due to SLOP verdict:** none
**Packages flagged as suspicious SUS:** none

## Architecture Patterns

### System Architecture Diagram

```
Operator creds (git-ignored src-tauri/.live-test.env, never committed)
        │
        ▼
scripts/live-{21,22,26}.sh ── env-vars-only sourcing (never argv) ──► live_gate_{21,22,26} bins
        │                                                                    │
        │ STEP/REPORT lines                                                  │ IMAP/SMTP vs mail/smtp.utfpr.edu.br
        ▼                                                                    ▼
/tmp/sge-live*.log ──► AUDIT stage + SCAN stage (grep -F secrets) ──► exit 0/1/2
        │
        │ (parallel, credential-free)
scripts/probe-27.sh ──┬──► imap_probe --pre-auth-only ──► CAPABILITY transcript (IDLE/CONDSTORE/QRESYNC atoms)
                      ├──► openssl s_client -starttls smtp ──► EHLO capability transcript
                      └──► openssl/tcp 4190 ──► ManageSieve greeting transcript (or connection refused/timed out)
        │
        ▼
scrubber ──► .planning/probes/*.txt (secret-clean) + capability-profile.json ──► gates Ph28/29/31/33
        │
        ▼ (separate host)
Node 20+ host: npm run build ──► Linux bundle ──► install + health smoke ──► BUNDLE-REPORT
        │
        ▼
Human click-through ──► UAT markdown table (pass/fail + notes)
```

### Recommended Project Structure (deltas only)

```
scripts/
├── live-21.sh / live-22.sh / live-26.sh   # v1.4 harnesses — small delta, not rewrite
├── build-env.sh                            # source before ANY cargo cmd (unchanged)
└── probe-27.sh                             # NEW: capability probes + scrub + profile JSON emit
.planning/
├── probes/                                 # NEW dir: 27-imap-capability.txt, 27-smtp-ehlo.txt,
│                                           # 27-managesieve-4190.txt, capability-profile.json
└── phases/27-live-validation-gate-capability-probes/
    ├── 27-CONTEXT.md                       # exists
    ├── 27-RESEARCH.md                      # this file
    └── UAT-27.md / BUNDLE-REPORT-27.md     # planner's choice of placement
/tmp/                                        # ALL raw logs/dumps stay here, never the repo
```

### Pattern 1: v1.4 live-harness conventions (reuse verbatim)
**What:** Bins print `STEP n/m <leg>: PASS|FAIL|SKIP`, one-line `REPORT gate=<g> verdict=<v> [reason=… resume=…]`, `SCAN-DB … PASS`, `DUMP <path>`, `BUNDLE-REPORT end` / `BUNDLE-ROW …`; wrappers re-verify REPORT rows, secret-grep the /tmp log + dump, retry `--cleanup-only` on 5s/15s/45s, propagate exit 0/1/2. [VERIFIED: scripts/live-21.sh:1-120, scripts/live-22.sh:1-125, scripts/live-26.sh:1-184 — read this session]
**When to use:** Every Phase 27 leg. Transcripts stay comparable across milestones.
**Example:** live-26 wrapper REPORT-row check:
```bash
# Source: scripts/live-26.sh:102-115
for GATE in seen-roundtrip tree-browse-badges arrival-tick reconnect-recovery confirm-move-undo rename-delete batch-probe cite-draft-onecopy cite-smtp-send cite-seen-22; do
    if grep -q "STEP .*/15 $GATE: PASS" "$LOG" 2>/dev/null; then
        if ! grep -q "^REPORT gate=$GATE verdict=PASS" "$LOG" 2>/dev/null; then
            echo "REPORT-FAIL: STEP $GATE passed but its REPORT row is missing"
            CODE=1
        fi
    fi
```

### Pattern 2: Pre-auth probe before credentialed probe
**What:** `imap_probe --host mail.utfpr.edu.br --pre-auth-only` (greeting + CAPABILITY, no login) runs first and is committable evidence on its own; the authenticated CAPABILITY (post-login atoms, which can differ — e.g. LOGIN- versus SASL-gated advertisements) is captured only inside the operator's credentialed run. [VERIFIED: src-tauri/src/bin/imap_probe.rs:141-163; src-tauri/src/imap/probe.rs:64-88]
**When to use:** Always — pre-auth is agent-runnable today; post-login waits for the operator.

### Pattern 3: Deferred-with-reason (audit honesty)
**What:** Any still-blocked leg emits `REPORT gate=<g> verdict=SKIP reason=<non-empty> resume=<script-or-command>`; wrapper treats reason-less SKIP as FAIL (live-26 pattern). Never a silent pass, never NOT-RUN rows in the bundle. [VERIFIED: scripts/live-26.sh:109-134]

### Anti-Patterns to Avoid
- **Credentialed probing by the agent:** agent never sources `.live-test.env`, never passes passwords on argv (`--password` is refused by design [VERIFIED: src-tauri/src/bin/imap_probe.rs:80-85]). Agent verifies offline green + exit-2 SKIP only.
- **Recording raw transcripts in the repo:** raw logs may carry secrets; only scrubbed transcripts land in `.planning/probes/`, raw stays in /tmp and is deleted post-run.
- **Writing uploader/IDLE/CONDSTORE code in Phase 27:** probes-first is binding; Phase 27 emits evidence + JSON, not protocol code.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| IMAP pre-auth CAPABILITY | openssl transcript parser in bash | `imap_probe --pre-auth-only` | App-identical TLS/cert semantics + built-in scrub; openssl is the cross-check |
| Secret redaction | Custom regex engine | `Transcript::render` (in-tree) + wrapper `grep -F` scan | In-tree render already replaces password ≥3 chars with `***` and never echoes LOGIN [VERIFIED: src-tauri/src/imap/mod.rs:883-905]; wrapper fixed-substring grep covers env/log/dump |
| Sandbox guard | Ad-hoc folder checks | Existing pre-flight LIST-snapshot + `SANDBOX_PREFIX="SGE-Test-"` + `unexpected_list_delta` | Proven in live_gate_21 [VERIFIED: src-tauri/src/bin/live_gate_21.rs:54-55]; aborts on ANY unexpected delta |
| Sieve upload client | Hand-rolled ManageSieve | Nothing in Phase 27 (io-managesieve only in Phase 33 on probe success) | Out of scope by CONTEXT decision |
| Build toolchain | Vite downgrade to satisfy Node 18 | Node 20+ host for the build leg | Env Node is 18.19.1; downgrading vite to dodge the requirement breaks the pinned stack |

**Key insight:** The scrubber's strength is layered: LOGIN is never echoed by construction (auth issued inside async-imap), `render()` redacts residual password occurrences, and the wrapper's `grep -F` over log + dump is the tripwire. No single layer is trusted alone.

## Probe Commands (credential-free)

> All three run WITHOUT credentials. The operator's credentialed runs add the post-login CAPABILITY leg. Commands below are standard `openssl s_client` / `imap_probe` usage **[ASSUMED — not executed against the real server this session]**; option semantics verified against local tool presence (`openssl 3.0.13` present [VERIFIED: shell run this session]) and in-tree `--help` text [VERIFIED: src-tauri/src/bin/imap_probe.rs:18-40].

### Probe 1 — IMAP pre-auth CAPABILITY (preferred: in-tree binary)
```bash
source scripts/build-env.sh
cargo run --manifest-path src-tauri/Cargo.toml --bin imap_probe -- \
  --host mail.utfpr.edu.br --pre-auth-only | tee /tmp/sge-probe27-imap-preauth.raw
# Expected: greeting `S: * OK ...` then `S: * CAPABILITY IMAP4rev1 ... <atoms> ...`
# Record which of IDLE / CONDSTORE(NOTE: advertised as CONDSTORE) / QRESYNC appear.
```
Cross-check (raw openssl, same facts):
```bash
printf 'a0 CAPABILITY\r\na1 LOGOUT\r\n' | \
  openssl s_client -connect mail.utfpr.edu.br:993 -crlf -quiet 2>/tmp/sge-probe27-tls.err
# No creds sent. Scrub /tmp/sge-probe27-tls.err (may contain cert subject lines — safe, keep).
```

### Probe 2 — SMTP EHLO capabilities over STARTTLS
```bash
printf 'EHLO sge-probe\r\nQUIT\r\n' | \
  openssl s_client -connect smtp.utfpr.edu.br:587 -starttls smtp -crlf -ign_eof 2>/tmp/sge-probe27-smtp-tls.err
# Expected: `250-...` EHLO list (8BITMIME/SIZE/DSN/AUTH atoms of interest) after `220 ... Ready to start TLS`.
# NOTE: some servers require a second EHLO post-STARTTLS; -ign_eof keeps the pipe open for it.
# No AUTH is attempted — capability listing only.
```

### Probe 3 — Port 4190 / ManageSieve greeting
```bash
timeout 15 openssl s_client -connect mail.utfpr.edu.br:4190 -crlf -quiet 2>&1 | head -5
# SUCCESS signal: `"IMPLEMENTATION" ... "SIEVE" ...` greeting line (RFC 5804) → Phase 33 arms.
# `connection refused` → server speaks no ManageSieve → export-only stands, no uploader code.
# hang until `timeout` kills it → port filtered; record as FILTERED (treat as probe-failed, re-probe later milestone).
# ALSO try smtp host: timeout 15 openssl s_client -connect smtp.utfpr.edu.br:4190 -crlf -quiet
# Plaintext variant if TLS handshake fails (ManageSieve is STARTTLS-capable, may greet in clear):
printf 'LOGOUT\r\n' | timeout 15 nc -v mail.utfpr.edu.br 4190 2>&1 | head -5   # [ASSUMED nc flags; fallback: /dev/tcp]
```

### Post-login CAPABILITY (operator-only, inside credentialed run)
The authenticated `CAPABILITY` (post-login atoms) is captured by the existing probe core (`open_inbox` issues post-login `CAPABILITY` [VERIFIED: src-tauri/src/imap/session.rs:392-396]) and by `imap_probe` full mode (`SGE_IMAP_PASSWORD` env-only). The operator runs it; the agent never does. Both pre- and post-login atom sets go into the profile JSON with their source labeled.

## Transcript Scrubber Design

Existing layers to reuse (do not reinvent):
1. **Construction:** LOGIN/AUTHENTICATE lines are never echoed — auth is issued inside async-imap; only CAPABILITY/NAMESPACE/LIST/STATUS/SELECT exchanges enter the transcript. [VERIFIED: src-tauri/src/imap/mod.rs:848-870 comments; src-tauri/src/bin/imap_probe.rs:9-11]
2. **Render:** `Transcript::render(cfg)` replaces password occurrences (len ≥ 3) with `***`; username intentionally NOT scrubbed (short names would annihilate text; username already appears in error strings by design). [VERIFIED: src-tauri/src/imap/mod.rs:883-905]
3. **Wrapper tripwire:** `grep -F` (fixed-substring, never regex) over the /tmp log and the SQLite dump for each of `SGE_LIVE_IMAP_PASS`, `SGE_LIVE_SMTP_PASS` (+ `SGE_LIVE_IMAP_USER` in live-22/26); any hit names WHICH artifact, never the secret; secrets `unset` before further output; tracing stays OFF while secrets are in scope. [VERIFIED: scripts/live-22.sh:46-65; scripts/live-26.sh:49-68; scripts/live-21.sh:19-24]

Phase 27 delta (small, in `probe-27.sh`):
- Extend the secret set with the probe-relevant env names actually present (`SGE_IMAP_PASSWORD` for `imap_probe` full mode) — scan every name the script sources.
- Scrub targets: `/tmp/sge-probe27-*.raw` → scrubbed copies land in `.planning/probes/`; raw files deleted after verification (`rm -f /tmp/sge-probe27-*` + assert gone).
- Post-run grep that must pass before commit: `grep -rF -f <(env-sourced secrets are NEVER written to a file; scan in-memory loop as live-*.sh do)` over `.planning/probes/` — any hit fails the leg. Also `git status --porcelain` must show no `.live-test.env` and `git check-ignore -q src-tauri/.live-test.env` must succeed ([VERIFIED: .gitignore:74 ignores it]).
- Trace sinks, all /tmp-only: raw probe outputs, `sge-live{21,22,26}-*` logs/dumps, openssl stderr files. Final STEP of probe-27.sh lists + removes them and asserts removal.

## live-21/22/26.sh Delta for v1.5

Keep the scripts structurally identical (STEP numbering, REPORT, SCAN-DB, exit 0/1/2, cleanup-retry 5s/15s/45s). Required deltas:

1. **Capability capture (all three):** after the gate runs, emit `CAPABILITY-IMAP-PREAUTH atoms=<from probe transcript or SKIP reason>` and (operator runs) `CAPABILITY-IMAP-POSTLOGIN atoms=…` lines into the log so the profile JSON can be derived without re-parsing IMAP. Smallest change: wrapper greps the committed probe transcript files; no bin change needed. [ASSUMED: exact atom sets unknown until probed]
2. **Draft-send gap STEP (live-21):** STEP 10 `draft-send` currently reports `FAIL BY DESIGN` with the D-16 gap note [VERIFIED: src-tauri/src/bin/live_gate_21.rs:1132-1137]. Phase 27 resolves LIVE-05 either by (a) small fix: `delete_draft`-in-flush so the flush removes the local drafts row AND the server copy (bin already asserts zero-harness-discard on the pass path), or (b) explicit defer-with-reason REPORT row. Planner sizes (a); if it exceeds a small fix, (b) with reason + resume pointer is the honest pre-agreed outcome.
3. **Node 20+ build check:** new wrapper-level STEP (or probe-27.sh section): record `node --version` + `npm run build` result; env Node 18.19.1 documents the SKIP, a 20+ host documents the PASS. Never downgrade vite to fit Node 18.
4. **UAT checklist pointer:** scripts print the UAT checklist path at close so the human run is traceable to the recorded table (scoped search/filter restore, archive rule save → Arquivar agora → progress → report → undo, rules live end-to-end tracer).
5. **Full-cleanup assert:** extend the closing tail to assert: no `SGE-Test-*` folders remain (LIST check), `/tmp/sge-{live,probe}*` removed, no creds file created (`git status --porcelain` clean of `.live-test.env`).

## Per-Account Capability Profile JSON Schema

File: `.planning/probes/capability-profile.json` (one object per account/host; UTFPR account first). Downstream planners (Ph28 CONDSTORE atom, Ph29 IDLE atom, Ph31 probe note, Ph33 4190 transcript) read this file — field names are the contract:

```json
{
  "schema_version": 1,
  "probed_at": "2026-10-13T00:00:00Z",
  "probe_tool": "imap_probe --pre-auth-only + openssl s_client 3.0.13",
  "accounts": [
    {
      "host": "mail.utfpr.edu.br",
      "port": 993,
      "mode": "implicit_tls",
      "greeting": "* OK ... (first line, secrets scrubbed)",
      "capability_preauth": ["IMAP4rev1", "IDLE", "CONDSTORE", "..."],
      "capability_postlogin": ["IMAP4rev1", "IDLE", "..."],
      "atoms": { "IDLE": true, "CONDSTORE": false, "QRESYNC": false },
      "smtp": { "host": "smtp.utfpr.edu.br", "port": 587, "ehlo": ["8BITMIME", "SIZE", "..."] },
      "managesieve": { "port": 4190, "reachable": false, "greeting": null, "verdict": "closed|filtered|open-greeting" },
      "transcripts": ["27-imap-capability.txt", "27-smtp-ehlo.txt", "27-managesieve-4190.txt"],
      "phase33_armed": false
    }
  ]
}
```

Rules: `atoms.*` default `false` unless the atom string appears verbatim in the recorded transcript (silent-degrade baselines apply); `phase33_armed` is true ONLY if `managesieve.verdict == "open-greeting"`; `QRESYNC: true` additionally requires a VANISHED-capable path (deferred regardless — profile records the atom, Phase 28 still ships CONDSTORE-only per binding ruling). [ASSUMED: atom values pending real probe]

## Common Pitfalls

### Pitfall 1: Secret leakage via argv / set -x / raw transcript commit
**What goes wrong:** Password on a CLI flag is world-readable via `ps`; `set -x` with secrets in scope prints them into the tee'd log; committing a raw transcript leaks creds into git history forever.
**Why it happens:** Convenience shortcuts under time pressure.
**How to avoid:** Env-only passwords (`SGE_IMAP_PASSWORD`, never `--password`); tracing OFF while secrets in scope; scrub-then-commit (only scrubbed copies enter `.planning/probes/`); post-run `grep -F` tripwire + `git check-ignore` assert. All three are existing v1.4 convention — follow them.
**Warning signs:** Any new `--password` flag, any `set -x` near sourced env, any raw /tmp path added to git.

### Pitfall 2: Writing protocol code before probe evidence
**What goes wrong:** IDLE/CONDSTORE/upload code built against assumed atoms, then the real server disagrees.
**Why it happens:** Skipping the boring probe step.
**How to avoid:** Probes-first is binding (SUMMARY + CONTEXT). Phase 27 writes zero protocol code; `phase33_armed` gates Phase 33's existence.
**Warning signs:** A plan task touching `imap/` or adding io-managesieve inside Phase 27.

### Pitfall 3: Node 18 vs 20 confusion
**What goes wrong:** `npm run build` fails on env Node 18.19.1 and gets recorded as a product failure, or vite gets downgraded to fit.
**Why it happens:** Env host is Node 18 [VERIFIED this session]; the pinned vite 8 requires Node 20+.
**How to avoid:** Record env-Node-18 SKIP as environmental; run the real build leg on a Node 20+ host with version documented. Never touch the vite pin.
**Warning signs:** `package.json` diff touching vite, or a FAIL verdict without a host Node version beside it.

### Pitfall 4: Probe-before-cleanup ordering gaps
**What goes wrong:** /tmp traces or SGE-Test-* folders survive the run; next run's pre-flight LIST-snapshot sees deltas and aborts.
**Why it happens:** Cleanup asserted only on the happy path.
**How to avoid:** Cleanup-retry (5s/15s/45s) + closing asserts (no SGE-Test-* folders, /tmp traces removed, no creds file) on ALL exit paths.

### Pitfall 5: Treating filtered port 4190 as open/closed
**What goes wrong:** A filtered port (silent drop, timeout) is misread as closed (fast refuse) or open, arming/disarming Phase 33 wrongly.
**Why it happens:** No `timeout` bound, no verdict taxonomy.
**How to avoid:** Always `timeout 15`; record three-valued verdict (closed/refused vs filtered/timeout vs open-greeting); only open-greeting arms Phase 33.

## Code Examples

### Pre-flight SKIP verification (agent-runnable today)
```bash
# Source: scripts/live-21.sh:13-17 (same shape in live-22.sh, live-26.sh)
source scripts/build-env.sh
mv src-tauri/.live-test.env /tmp/sge-keep-env 2>/dev/null  # ensure absent; restore after
./scripts/live-21.sh; echo "exit=$? (expect 2)"
./scripts/live-22.sh; echo "exit=$? (expect 2)"
./scripts/live-26.sh; echo "exit=$? (expect 2)"
cargo run --manifest-path src-tauri/Cargo.toml --bin imap_probe -- --host mail.utfpr.edu.br --pre-auth-only
```

### Offline green (agent-runnable today)
```bash
source scripts/build-env.sh
cargo test --manifest-path src-tauri/Cargo.toml --lib   # v1.4 closed at 345/472/392 lib green per milestone
```

### Capability atom extraction (probe-27.sh core)
```bash
# Fixed-substring atom check over the scrubbed transcript (atoms are exact tokens):
for ATOM in IDLE CONDSTORE QRESYNC; do
  if grep -qw "$ATOM" .planning/probes/27-imap-capability.txt; then echo "atoms.$ATOM=true";
  else echo "atoms.$ATOM=false (baseline: poll / UID range-diff)"; fi
done
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Assume IDLE/CONDSTORE present | Probe-first, silent-degrade baselines | v1.5 research (binding) | Every protocol phase gated on recorded atoms |
| Sieve upload as same-phase work | Export-text always ships; upload a separate probe-gated slice | v1.5 synthesis ruling | Phase 33 exists only on open-greeting |
| Async-imap bump for features | Stay on 0.11.3 (0.12 re-opens parser risk) | REQUIREMENTS out-of-scope | Zero needed features from 0.12 |
| QRESYNC as CONDSTORE-phase work | QRESYNC deliberately deferred (VANISHED invisible via async-imap) | v1.5 ruling | Phase 28 is CONDSTORE-only; profile still records the atom |

**Deprecated/outdated:**
- `sieve-rs` / `sieve-generator` crates: AGPL-tainted / dead since 2022 — never use (REQUIREMENTS out-of-scope).
- Blind Sieve overwrite: backup-then-diff mandated; export-only if no ManageSieve.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | openssl SMTP STARTTLS + 4190 command shapes work as written against UTFPR servers | Probe Commands | Low — standard flags; operator reports actual output, commands adjusted |
| A2 | `nc` present as 4190 fallback (else /dev/tcp) | Probe Commands | Low — fallback documented inline |
| A3 | Capability atom sets (IDLE/CONDSTORE/QRESYNC presence) | Profile JSON, live delta | None if handled right — profile defaults false + silent-degrade baselines cover every combination |
| A4 | Draft-send fix is small (delete_draft-in-flush) | live delta | Low — explicit defer-with-reason is the pre-agreed fallback |
| A5 | A Node 20+ host is available to the operator | Environment | Medium — if unavailable, UAT-01 stays deferred-with-reason with resume pointer |

## Open Questions

1. **Real-server capability atoms**
   - What we know: Code paths for every tier exist; baselines defined.
   - What's unclear: Actual IDLE/CONDSTORE/QRESYNC advertisement + 4190 state on UTFPR servers.
   - Recommendation: Operator runs probe-27.sh; planners of Ph28/29/31/33 consume the committed JSON, not assumptions.
2. **Draft-send fix size**
   - What we know: Gap precisely located (flush performs no delete_draft; bin verdict text names it).
   - What's unclear: Fix size until the planner sizes it.
   - Recommendation: Planner timeboxes; overflow → defer-with-reason, never scope creep.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| openssl | SMTP + 4190 probes | ✓ | 3.0.13 | — |
| Node (env host) | `npm run build` | ✓ but 18.19.1 (need 20+) | 18.19.1 | Node 20+ host for the build leg; env-18 SKIP is environmental |
| cargo + build-env.sh | bins (imap_probe, live_gate_*) | ✓ (convention) | — | — |
| Network to mail/smtp.utfpr.edu.br | probes + live legs | operator-side | — | Pre-auth probe agent-runnable; credentialed legs operator-only |
| nc / timeout | 4190 probe bound | to verify at plan time | — | bash /dev/tcp fallback |

**Missing dependencies with no fallback:**
- Node 20+ host access (if operator has none, UAT-01 defers with resume pointer)

**Missing dependencies with fallback:**
- None — every protocol leg has a no-capability baseline that ships instead

## Validation Architecture

| Property | Value |
|----------|-------|
| Framework | cargo test (Rust bins) + bash exit-code contract (0 pass / 1 fail / 2 SKIP) + human UAT table |
| Config file | none — see conventions in scripts/live-*.sh |
| Quick run command | `source scripts/build-env.sh && ./scripts/live-21.sh` (exit 2 without creds) |
| Full suite command | `source scripts/build-env.sh && cargo test --manifest-path src-tauri/Cargo.toml` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LIVE-04 | Harnesses green under sandbox contract | live (operator) + offline SKIP (agent) | `./scripts/live-{21,22,26}.sh` | ✅ scripts exist; `.planning/probes/` ❌ Wave 0 (create dir + probe-27.sh) |
| LIVE-05 | No orphan drafts or documented deferral | live STEP 10 (operator) | live-21 STEP 10 draft-send | ✅ bin exists; verdict currently FAIL BY DESIGN |
| UAT-01 | Build green on Node 20+ | manual (20+ host) | `npm run build` | ✅ script exists; needs 20+ host |
| UAT-02 | Click-through recorded | manual | checklist → markdown table | ❌ Wave 0 (checklist + table file) |

### Sampling Rate
- **Per task commit:** offline SKIP re-verified (`live-*.sh` exit 2) + `cargo test` untouched-green
- **Per wave merge:** full offline suite green
- **Phase gate:** Operator transcripts committed + secret-grep clean + cleanup asserted before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `.planning/probes/` dir + `scripts/probe-27.sh` (probes, scrub, profile JSON emit)
- [ ] UAT checklist + `UAT-27.md` table scaffold (planner's placement)
- [ ] BUNDLE-REPORT-27 scaffold following the v1.4 pattern

## Security Domain

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes (operator creds) | OS keyring + git-ignored env file; agent never touches creds |
| V3 Session Management | no | No session surface in this phase |
| V4 Access Control | yes (sandbox) | Pre-flight LIST-snapshot + SANDBOX_PREFIX guard + self-only mail |
| V5 Input Validation | yes | Probe transcript atom matching is exact-token (`grep -w`); env never on argv |
| V6 Cryptography | yes | Implicit-TLS 993 / STARTTLS 587 discipline unchanged; `--allow-untrusted` logged + refused-silent-never |

### Known Threat Patterns for this phase

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Credential leak via ps/argv | Information disclosure | Env-only passwords; `--password` refused in-tree |
| Secret leak via transcript/log commit | Information disclosure | 3-layer scrub (construct + render + grep -F tripwire); raw stays in /tmp |
| Live run touching real user mail | Tampering | Sandbox contract (SGE-Test-* only, self-only, pre-flight abort, post-run cleanup) |
| \Seen side-effect from wrong FETCH | Tampering | PEEK gate in wrappers (any non-PEEK `BODY[` fails the run) |

## Sources

### Primary (HIGH confidence)
- scripts/live-21.sh, live-22.sh, live-26.sh, build-env.sh — read this session, full files
- src-tauri/src/bin/imap_probe.rs (1-195), src-tauri/src/imap/probe.rs (1-88) — read this session
- src-tauri/src/imap/mod.rs Transcript (render/scrub comments) — read this session
- src-tauri/src/imap/session.rs:267-335, 392-396, 494-511 (manual_capability, post-login CAPABILITY) — grepped + partially read
- src-tauri/src/bin/live_gate_21.rs draft-send verdict (1132-1137), SANDBOX_PREFIX (54-55) — grepped this session
- src-tauri/.live-test.env.example, .gitignore:74, package.json build script, `node --version` (18.19.1), `openssl version` (3.0.13) — verified this session

### Secondary (MEDIUM confidence)
- .planning/research/SUMMARY.md (binding probes-first, silent-degrade, Phase 33 gating) + 27-CONTEXT.md + REQUIREMENTS.md + ROADMAP.md Phase 27 section

### Tertiary (LOW confidence)
- Raw openssl SMTP/4190 command shapes — standard usage, not executed against the real server this session (marked [ASSUMED] where relevant)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - harness/probe sources read verbatim; tool versions run locally
- Architecture: HIGH - v1.4 conventions + CONTEXT decisions directly mapped
- Pitfalls: HIGH - grounded in wrapper/bin source (PEEK gate, scrub layers, exit-2 SKIP)

**Research date:** 2026-10-13
**Valid until:** 30 days (stable domain; re-probe if server behavior changes)
