---
phase: "22"
slug: "draft-seen-round-trip-live-gate-live-02"
# status lifecycle: draft (seeded by plan-phase) → validated (set by validate-phase §6)
# audit-milestone §5.5 distinguishes NOT-VALIDATED (draft) from PARTIAL (validated + nyquist_compliant: false) (#2117)
status: draft
nyquist_compliant: false
wave_0_complete: false
created: "2026-10-13"
---

# Phase 22 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Verification-only live gate (LIVE-02, SANDBOXED): no new capabilities — HOW to prove live and safe.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust) + jest (frontend, minimal — pending indicators only) |
| **Config file** | src-tauri/Cargo.toml / package.json |
| **Quick run command** | `cargo test --manifest-path src-tauri/Cargo.toml` |
| **Full suite command** | `cargo test --manifest-path src-tauri/Cargo.toml && npx tsc --noEmit` |
| **Estimated runtime** | ~120 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --manifest-path src-tauri/Cargo.toml`
- **After every plan wave:** Run full suite command
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 120 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 22-01-01 | 01 | 1 | LIVE-02 | T-22-01 | Pre-flight guard refuses out-of-sandbox target; no existing mail touched | unit | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_22::preflight` | ❌ W0 | ⬜ pending |
| 22-01-02 | 01 | 1 | LIVE-02 | T-22-02 | One-copy draft: SEARCH returns exactly 1 UID == draft_uid; BODY.PEEK-only | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_22::one_copy` | ❌ W0 | ⬜ pending |
| 22-01-03 | 01 | 1 | LIVE-02 | T-22-03 | Kill-session reconcile collapses to 1 copy; offline edit latest-wins | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_22::reconcile` | ❌ W0 | ⬜ pending |
| 22-02-01 | 02 | 1 | LIVE-02 | T-22-04 | Offline delete/move replays pre-sweep; double-sync UID stays gone; Seen both directions | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_22::replay` | ❌ W0 | ⬜ pending |
| 22-02-02 | 02 | 1 | LIVE-02 | T-22-05 | Secret scan FAILs on credential hit; SGE-TEST- prefix audit; cleanup retry-then-manual | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_22::safety` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

Threat refs (T-22-0x) are placeholders the planner expands into the PLAN.md `<threat_model>` block (out-of-sandbox write, silent resurrection, \Seen side-effect FETCH, credential leak via logs/args, unscoped expunge).

---

## Wave 0 Requirements

- [ ] `src-tauri/src/bin/live_gate_22.rs` — harness skeleton (STEP PASS/FAIL lines, env-sourced creds, keyring-seed path)
- [ ] `src-tauri/tests/live_gate_22_safety.rs` — offline doubles for pre-flight, reconcile, replay-order, resurrection, secret-scan (no live creds)
- [ ] Existing infrastructure covers all phase requirements beyond the above (drafts.rs, commands/sync.rs, send_queue.rs, FakeSession/MockSession doubles).

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Live run vs real account under sandbox contract | LIVE-02 | Requires user-supplied git-ignored `src-tauri/.live-test.env` + real server; never runs in CI | 1. Copy `.live-test.env.example` → `.live-test.env`, fill creds. 2. Run wrapper script; assert all STEPs PASS. 3. Verify webmail shows no non-`SGE-TEST-` mail, sandbox folder removed post-run. |
| Kill-session between APPEND-new and expunge-old | LIVE-02 (D-06) | Live failure-mode injection timing | Follow harness prompt; assert reconcile collapses to 1 copy with latest content. |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 120s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
