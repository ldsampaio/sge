---
phase: "6"
slug: "flag-sync-outbox"
# status lifecycle: draft (seeded by plan-phase) → validated (set by validate-phase §6)
# audit-milestone §5.5 distinguishes NOT-VALIDATED (draft) from PARTIAL (validated + nyquist_compliant: false) (#2117)
status: draft
nyquist_compliant: false
wave_0_complete: false
created: "2026-10-04"
---

# Phase 6 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust) + tsc/eslint/vite (frontend) |
| **Config file** | `src-tauri/Cargo.toml` / `package.json` |
| **Quick run command** | `cargo test --manifest-path src-tauri/Cargo.toml` |
| **Full suite command** | `cargo test --manifest-path src-tauri/Cargo.toml && npx tsc --noEmit && npx eslint src/ && npm run build` |
| **Estimated runtime** | ~120 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --manifest-path src-tauri/Cargo.toml`
- **After every plan wave:** Run full suite command
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 180 seconds

---

## Per-Task Verification Map

Seeded at plan time; planner fills Task ID rows per plan. Existing infrastructure covers all phase requirements (63 Rust tests green at M1 close).

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| (planner fills) | | | FLAG-01 | T-6-01 | STORE uses UID addressing; no sequence-number writes | unit | `cargo test --manifest-path src-tauri/Cargo.toml flag_outbox` | ❌ W0 | ⬜ pending |
| (planner fills) | | | FLAG-01 | — | No fetch path sets \Seen as side effect (BODY.PEEK audit) | unit | `cargo test --manifest-path src-tauri/Cargo.toml peek_audit` | ❌ W0 | ⬜ pending |
| (planner fills) | | | FLAG-02 | T-6-02 | Outbox survives restart; replay drops ops on missing message / UIDVALIDITY change | unit | `cargo test --manifest-path src-tauri/Cargo.toml outbox_replay` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `src-tauri/src/store/migrations` — M2 `flag_outbox` migration covered by migration test
- [ ] `src-tauri/src/imap/manager.rs` — session manager unit tests (single-flight lease)
- [ ] Existing 63-test Rust suite stays green throughout

*Wave 0 deltas only — existing infrastructure covers all phase requirements.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Seen flag visible on real server (webmail cross-check) | FLAG-01 | Requires live UTFPR round-trip | Toggle read in app → confirm Seen in UTFPR webmail; toggle unread → confirm unseen |
| Offline toggle replay | FLAG-02 | Requires disconnect simulation | Disconnect network → toggle → reconnect → confirm server flag converges |

*Live-server checks need the UTFPR account; stub-server + unit coverage otherwise.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 180s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
