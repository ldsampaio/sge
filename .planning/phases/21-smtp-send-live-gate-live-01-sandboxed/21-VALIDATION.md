---
phase: "21"
slug: "smtp-send-live-gate-live-01-sandboxed"
# status lifecycle: draft (seeded by plan-phase) → validated (set by validate-phase §6)
# audit-milestone §5.5 distinguishes NOT-VALIDATED (draft) from PARTIAL (validated + nyquist_compliant: false) (#2117)
status: draft
nyquist_compliant: false
wave_0_complete: false
created: "2026-10-13"
---

# Phase 21 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Verification-only live gate (LIVE-01, SANDBOXED): no new capabilities — HOW to prove live and safe.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust) + jest (frontend, idle — no new UI) |
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
| 21-01-01 | 01 | 1 | LIVE-01 | T-21-01 | Harness skeleton + abort-on-foreign-recipient; env-only creds, keyring-seed path | unit | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_21::skeleton` | ❌ W0 | ⬜ pending |
| 21-01-02 | 01 | 1 | LIVE-01 | T-21-02 | Self-send arrives (SEARCH INBOX Message-ID) + Sent copy 1 UID + `\Seen` + verbatim bytes, PEEK-only | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_21::arrival` | ❌ W0 | ⬜ pending |
| 21-01-03 | 01 | 1 | LIVE-01 | T-21-03 | Double-enqueue same Message-ID → single delivery; `sent-unfiled` never re-SMTP-sends; auto-save dedupe | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_21::dedupe` | ❌ W0 | ⬜ pending |
| 21-02-01 | 02 | 2 | LIVE-01 | T-21-04 | Offline enqueue → flush with backoff, exactly once; DATA-timeout → `uncertain` → reconcile-not-resend; kill-mid-sending → recover → once | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_21::retry` | ❌ W0 | ⬜ pending |
| 21-02-02 | 02 | 2 | LIVE-01 | T-21-05 | Secret scan FAILs on credential hit; no-unprefixed-send audit; cleanup (arrival + Sent copy + sandbox) + draft-send transaction no-orphan | unit + live | `cargo test --manifest-path src-tauri/Cargo.toml live_gate_21::safety` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

Threat refs (T-21-0x) are placeholders the planner expands into the PLAN.md `<threat_model>` block (foreign-recipient send, duplicate delivery, re-send on unfiled, credential leak via logs/args/transcript, unscoped cleanup delete).

---

## Wave 0 Requirements

- [ ] `src-tauri/src/bin/live_gate_21.rs` — harness skeleton (STEP PASS/FAIL lines, env-sourced creds, keyring-seed path; structural clone of `live_gate_22.rs`)
- [ ] `src-tauri/tests/live_gate_21_safety.rs` — offline doubles for skeleton, arrival-shape, dedupe, retry/uncertain/crash, secret-scan, send-audit (no live creds)
- [ ] Existing infrastructure covers all phase requirements beyond the above (send_queue.rs, smtp.rs, drafts.rs DRAFT-03 path).

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Live run vs real account under sandbox contract | LIVE-01 | Requires user-supplied git-ignored `src-tauri/.live-test.env` + real server; never runs in CI | 1. `.live-test.env` filled (present). 2. Run `scripts/live-21.sh`; assert all STEPs PASS. 3. Verify webmail shows no non-`SGE-TEST-` mail, INBOX arrival + Sent copy removed post-run. |
| DATA-timeout → `uncertain` + kill-mid-`sending` | LIVE-01 (D-10/D-11) | Live failure-mode injection timing | Follow harness prompts; assert reconcile-not-resend and recover-then-once. |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 120s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
