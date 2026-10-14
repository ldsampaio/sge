# Requirements: SGE — v1.5 Live Validation + Compose Hardening + Deferred Features

**Defined:** 2026-10-13
**Core Value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.

## v1.5 Requirements

### Live Validation (real creds, sandboxed)

- [ ] **LIVE-04**: Live-21/22/26 harnesses run green against the real account under the sandbox contract (self-only mail, pre-flight guard, secret scan, post-run cleanup) with user-supplied git-ignored creds
- [ ] **LIVE-05**: Draft-send gap closed — the expected FAIL BY DESIGN verdict is fixed (no orphan drafts) or explicitly deferred with reason

### Bundle & UAT

- [ ] **UAT-01**: `npm run build` succeeds on a Node 20+ host (rolldown-vite `node:util` styleText requirement)
- [ ] **UAT-02**: Human click-through UAT executed and recorded — scoped search/filter restore, archive rule save → Arquivar agora → progress → report → undo, rules live end-to-end tracer

### Sync Upgrades (capability-gated, poll stays fallback)

- [x] **IDLE-01**: User receives new INBOX mail via IDLE push through a manager-owned watcher connection with advisory wakeups funneled through SyncGate; poll remains as fallback + multi-folder sweeper
- [ ] **SYNC-01**: User's reconnects resync fast via CONDSTORE (MODSEQ deltas, UIDVALIDITY-scoped highestmodseq); UID range-diff remains as fallback

### Classifier Trust

- [ ] **HIST-01**: User can browse override history with per-category stats (the dataset future learning consumes)
- [ ] **LEARN-01**: App adapts keyword weights + confidence thresholds from explicit override signals only (no fine-tuning, no silence-as-approval)
- [ ] **CONF-01**: User can enable per-category auto-confirm on earned precision with visible precision display and auto-disengage on override
- [x] **IMP-01**: User can import/export smart-folder rules as validated JSON (no empty patterns, no reserved names)

### Batch Trust (one run record)

- [ ] **BATCH-03**: User can dry-run a batch/archive operation and review the per-item plan with writes off
- [ ] **BATCH-04**: User can retry only the failed items of a batch run with writes re-on for failures

### Sieve Export (upload probe-gated)

- [ ] **SIEVE-01**: User can export smart-folder rules + taxonomy filing as Sieve text (RFC 5228, offline string generation, golden-file tested)
- [ ] **SIEVE-02**: App uploads Sieve text via ManageSieve only if the port-4190/SIEVE probe succeeds (backup-then-diff, never blind overwrite); otherwise export-only stands

### Compose Hardening (verify-and-fix over v1.2 pipelines)

- [ ] **COMP-01**: User can send a new email end-to-end (queue → flush → arrival + Sent copy, no duplicate)
- [ ] **COMP-02**: User can save and resume drafts with exactly one server copy per session
- [ ] **COMP-03**: User can reply with correct threading headers and quote
- [ ] **COMP-04**: User can reply-all honoring Reply-To and excluding own identity

## Future Requirements (deferred)

- QRESYNC VANISHED support (deliberately deferred — Gmail-class servers lack it; UID tombstoning covers the gap)
- Laya checkpoint fine-tuning on user mail (log from day one continues; no training runtime in v1.5)
- Multi-account support
- Sieve upload if the v1.5 probe fails (re-probe in a later milestone)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Cloud AI fallback | Mail content never leaves the machine (structural) |
| Windows/macOS builds | Linux only (structural) |
| OAuth / 2FA flows | User+password auth only |
| GPU sidecar builds | CPU-only for now |
| async-imap 0.12 bump | imap-proto 0.17 re-opens parser-poisoning risk for zero needed features |
| sieve-rs / sieve-generator crates | AGPL-tainted / dead since 2022 |
| Blind Sieve overwrite | Backup-then-diff mandated; export-only if no ManageSieve |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| LIVE-04 | Phase 27 | Pending |
| LIVE-05 | Phase 27 | Pending |
| UAT-01 | Phase 27 | Pending |
| UAT-02 | Phase 27 | Pending |
| SYNC-01 | Phase 28 | Pending |
| IMP-01 | Phase 29 | Complete |
| IDLE-01 | Phase 29 | Complete |
| BATCH-03 | Phase 30 | Pending |
| BATCH-04 | Phase 30 | Pending |
| SIEVE-01 | Phase 31 | Pending |
| HIST-01 | Phase 31 | Pending |
| LEARN-01 | Phase 31 | Pending |
| CONF-01 | Phase 31 | Pending |
| COMP-01 | Phase 32 | Pending |
| COMP-02 | Phase 32 | Pending |
| COMP-03 | Phase 32 | Pending |
| COMP-04 | Phase 32 | Pending |
| SIEVE-02 | Phase 33 (conditional — only if Phase 27 port-4190 probe succeeds) | Pending |

**Coverage:**

- v1.5 requirements: 18 total
- Mapped to phases: 18
- Unmapped: 0

---
*Requirements defined: 2026-10-13*
*Last updated: 2026-10-13 after initial definition*
