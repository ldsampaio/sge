---
phase: "39"
slug: "llm-settings-foundation"
# status lifecycle: draft (seeded by plan-phase) → validated (set by validate-phase §6)
# audit-milestone §5.5 distinguishes NOT-VALIDATED (draft) from PARTIAL (validated + nyquist_compliant: false) (#2117)
status: draft
nyquist_compliant: false
wave_0_complete: false
created: "2026-10-15"
---

# Phase 39 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust, unit + integration in-crate) |
| **Config file** | src-tauri/Cargo.toml (no separate config) |
| **Quick run command** | `cargo test -p sge --lib llm` |
| **Full suite command** | `cargo test` (workspace root runs src-tauri lib + bins; full suite ~454+ lib tests per v1.6 baseline) |
| **Estimated runtime** | ~120 seconds (full); ~20 seconds (llm-filtered) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p sge --lib llm`
- **After every plan wave:** Run `cargo test`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 180 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 39-01-01 | 01 | 1 | LLM-01/04 | — | M23 table holds non-secrets only; restart-persistence round-trip | unit | `cargo test -p sge --lib llm::settings` | ❌ W0 | ⬜ pending |
| 39-01-02 | 01 | 1 | LLM-02 | T-39-01 | API key absent from SQLite file; presence-bool only over command boundary | unit | `cargo test -p sge --lib llm::keyring` | ❌ W0 | ⬜ pending |
| 39-02-01 | 02 | 1 | LLM-01/04 | — | https-only + loopback exception; clamped temp/tokens; field-named errors | unit | `cargo test -p sge --lib llm::validate` | ❌ W0 | ⬜ pending |
| 39-02-02 | 02 | 1 | LLM-03 | T-39-02 | Body fenced as data; hidden HTML stripped; ~8k cap + marker; unknown placeholders literal | unit | `cargo test -p sge --lib llm::prompt` | ❌ W0 | ⬜ pending |
| 39-03-01 | 03 | 2 | LLM-01..04 | T-39-01 | Settings commands never return/log key or bodies; no network I/O in this phase | unit | `cargo test -p sge --lib llm::commands` | ❌ W0 | ⬜ pending |
| 39-03-02 | 03 | 2 | phase gate | — | `cargo tree` shows no rustls/ring (native-tls only) | unit | `cargo tree -i rustls 2>&1` (expect empty) + `cargo tree -i hyper-tls` (expect present) | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- Existing infrastructure covers all phase requirements — `cargo test` + in-memory SQLite (`Store::open_in_memory`) + `MemoryStore` keyring double already exist. No new harness needed.
- First post-widening `cargo fetch` needs crates.io network (hyper-tls/tokio-native-tls not in current lockfile) — executor runs `cargo fetch` once before Wave 1.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| OS keyring round-trip on a real Linux desktop session | LLM-02 | Secret Service availability varies by environment; CI uses MemoryStore | 1. Launch app under GNOME/KDE session 2. Save LLM key via settings command 3. Restart app 4. Confirm presence-bool true and key absent from SQLite file (strings/grep on db) |

*All other phase behaviors have automated verification.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 180s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
