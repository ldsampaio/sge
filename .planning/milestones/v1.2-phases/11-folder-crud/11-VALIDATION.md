---
phase: "11"
slug: "folder-crud"
# status lifecycle: draft (seeded by plan-phase) → validated (set by validate-phase §6)
# audit-milestone §5.5 distinguishes NOT-VALIDATED (draft) from PARTIAL (validated + nyquist_compliant: false) (#2117)
status: draft
nyquist_compliant: false
wave_0_complete: false
created: "2026-10-06"
---

# Phase 11 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust `cargo test` (backend) + `tsc --noEmit` (frontend) |
| **Config file** | `src-tauri/Cargo.toml` / `tsconfig` (existing — no Wave 0 installs) |
| **Quick run command** | `cargo test -p sge <module>::` (module touched by the task) |
| **Full suite command** | `cargo test -p sge && tsc --noEmit` |
| **Estimated runtime** | ~180 seconds (executor records actual on first wave) |

Existing infrastructure covers all phase requirements — no Wave 0 needed.

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p sge <module>::` (+ `tsc --noEmit` for frontend tasks)
- **After every plan wave:** Run `cargo test -p sge && tsc --noEmit`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 180 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 11-01-01 | 01 | 1 | FOLD-04 | T-11-01 | Non-ASCII folder names encode to valid modified-UTF-7; delimiter bytes never encoded | unit | `cargo test -p sge imap::mutf7::` | ✅ | ⬜ pending |
| 11-01-02 | 01 | 1 | FOLD-04 | T-11-02 | Empty/delimiter-bearing/INBOX-variant names rejected before any wire verb | unit | `cargo test -p sge folder_validate` | ✅ (new) | ⬜ pending |
| 11-01-03 | 01 | 1 | FOLD-04 | T-11-01 | CREATE goes out only under manager lease with one reconnect-retry; raw wire name only | unit | `cargo test -p sge imap::manager::` | ✅ | ⬜ pending |
| 11-01-04 | 01 | 1 | FOLD-04 | — | `create_folder` returns fresh tree; offline fails loudly, never queues | unit | `cargo test -p sge commands::` | ✅ | ⬜ pending |
| 11-01-05 | 01 | 1 | FOLD-04 | — | Dialog sends raw wire name; LIST refresh selects new folder | typecheck | `tsc --noEmit` | ✅ | ⬜ pending |
| 11-02-01 | 02 | 2 | FOLD-05, FOLD-06 | T-11-04 | RENAME/DELETE verbs are name-only, UID-space untouched, errors mapped | unit | `cargo test -p sge imap::` | ✅ | ⬜ pending |
| 11-02-02 | 02 | 2 | FOLD-05, FOLD-06 | T-11-05 | Rename keeps mailbox_id+messages; delete cascades + drops both outboxes | unit | `cargo test -p sge store::` | ✅ | ⬜ pending |
| 11-02-03 | 02 | 2 | FOLD-05, FOLD-06 | T-11-06 | INBOX/\Noselect/children/non-empty guards refuse before wire; selection migrates | unit | `cargo test -p sge commands::` | ✅ | ⬜ pending |
| 11-03-01 | 03 | 3 | FOLD-04, FOLD-05, FOLD-06 | T-11-07 | M8 forward-only; existing rows preserved with role/attributes defaults | unit | `cargo test -p sge store::` | ✅ | ⬜ pending |
| 11-03-02 | 03 | 3 | FOLD-04 | T-11-08 | Single role resolver; `detect_trash` delegates; Missing still needs confirm | unit | `cargo test -p sge imap::trash:: imap::roles::` | ✅ | ⬜ pending |
| 11-03-03 | 03 | 3 | FOLD-05, FOLD-06 | T-11-09 | Context menu + delete modal: destructive never default-focused; INBOX never offered | typecheck | `tsc --noEmit` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- Existing infrastructure covers all phase requirements.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| CREATE → re-LIST → select new folder on real server | FOLD-04 | Needs live IMAP credentials + network | Plan 11-01 live gate: CREATE `SGE-Test-<ts>`, assert in tree, DELETE cleanup |
| RENAME preserves UIDs, migrates selection, re-LISTs | FOLD-05 | Needs live server (subtree + UIDVALIDITY behavior is server-specific) | Plan 11-02 live gate: rename test folder, verify STATUS UIDVALIDITY unchanged, delete cleanup |
| DELETE guards + cascade + INBOX fallback on real server | FOLD-06 | Needs live server (non-empty refusal varies by server) | Plan 11-02 live gate: nested create/delete sequence, INBOX negative probes |
| Trash auto-detection on real LIST | Roadmap-4 | Needs live LIST data | Plan 11-03 live gate: log resolved Trash role, no CREATE unless Missing + confirmed |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 180s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
