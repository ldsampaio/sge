---
phase: 29-rule-import-export-idle-watcher
verified: 2026-10-14T05:36:40Z
status: passed
score: 11/11 must-haves verified
covered_files: [".planning/phases/29-rule-import-export-idle-watcher/29-01-PLAN.md", ".planning/phases/29-rule-import-export-idle-watcher/29-01-SUMMARY.md", ".planning/phases/29-rule-import-export-idle-watcher/29-02-PLAN.md", ".planning/phases/29-rule-import-export-idle-watcher/29-02-SUMMARY.md", ".planning/phases/29-rule-import-export-idle-watcher/29-03-PLAN.md", ".planning/phases/29-rule-import-export-idle-watcher/29-03-SUMMARY.md", "src-tauri/src/classify/mod.rs", "src-tauri/src/classify/rules_io.rs", "src-tauri/src/commands/classify.rs", "src-tauri/src/commands/sync.rs", "src-tauri/src/imap/idle.rs", "src-tauri/src/imap/mod.rs", "src-tauri/src/lib.rs", "src-tauri/src/store/mod.rs", "src-tauri/src/store/queries.rs", "src-tauri/src/sync/mod.rs", "src/components/RulesPanel.tsx", "src/components/Sidebar.tsx", "src/components/SyncStatus.tsx", "src/hooks/useIdleStatus.ts"]
covered_digest: "v1:sha256:d7e3051c61b1caa31161b20e2b5cd9acc1603f4660fcea3ecfdff45d6e154292"
behavior_unverified: 0
overrides_applied: 1
overrides:
  - must_have: "Capability gating: IMPORT_EXPORT=false → RulesPanel import/export buttons disabled with tooltip"
    reason: "IMPORT_EXPORT atom absent from Phase-27 capability profile (only IDLE/CONDSTORE/QRESYNC atoms exist); rule portability is local-file so backend validation is authoritative and an ungated UI loses nothing. Matches 29-03 plan tracer T-29-12."
    accepted_by: "phase-29-orchestrator"
    accepted_at: "2026-10-14T06:00:00Z"
re_verification:
  previous_status: passed
  previous_score: 11/11
  gaps_closed:
    - "CR-01: import_rules validate-all-then-apply in ONE rusqlite transaction (atomic commit, rollback on mid-apply failure)"
    - "WR-01: 29-min Timeout re-IDLE silent (no wakeup/counter)"
    - "WR-02: 1 MiB / 500-rule import caps (backend + frontend)"
    - "WR-03: IDLE gate consults every account's atom (fail-closed abstain)"
    - "WR-04: Accept enabled when imported+updated>0 (partial-valid-subset)"
    - "WR-05: begin_idle releases handle (DONE+LOGOUT) on init failure"
    - "WR-06: store Mutex only for snapshot read + commit tx"
  gaps_remaining: []
  regressions: []
---

# Phase 29: Rule Import/Export + IDLE Watcher Verification Report

**Phase Goal:** Users carry their rules across machines as validated JSON and receive new INBOX mail instantly via push (ROADMAP.md Phase 29: IMP-01 + IDLE-01)
**Verified:** 2026-10-14T05:36:40Z
**Status:** passed
**Re-verification:** Yes — second re-verification after review-fix round (5 fix commits: d50331b CR-01/WR-02/WR-03/WR-06/IN-01/IN-04, 39837b9 WR-01/WR-05/IN-03, 2407d99 WR-02/WR-04, d27cf6f IN-02, plus b04da56 warning cleanup). All 11/11 must-haves re-checked against current file state; digest recomputed via `query verification.fingerprint` (canonical seam, never hand-written).

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Export versioned JSON (envelope version 1) over real folder_rules rows | ✓ VERIFIED | `EXPORT_VERSION: u32 = 1` + `RuleExport { version, rules }` in `rules_io.rs:42,65,95`; `export_rules` Tauri command (`commands/classify.rs:2222`) serializes every row |
| 2 | Invalid files rejected with plain-language reasons (empty patterns, reserved system/auto, dup names, priority 1–999, dest_kind enum) | ✓ VERIFIED | 6 named tests green: `validation_empty_pattern`, `validation_reserved_names`, `validation_duplicate_names`, `validation_priority_range`, `validation_invalid_dest_kind`, plus `validation_unknown_folder_warns_but_imports`; NEW `import_rejects_oversize_and_overcount` enforces 1 MiB / 500-rule caps (`rules_io.rs:49-53,261-274,710-718`) |
| 3 | Round-trip export→import→export byte-identical (fixpoint) | ✓ VERIFIED | `round_trip_fixpoint` passes; NEW `import_commit_counts_match_dry_run` proves preview≡commit counts (single shared validate path, CR-01) |
| 4 | Re-import bumps rule_version (existing rule_id → update) | ✓ VERIFIED | `import_existing_id_updates_and_bumps_version` passes; NEW `import_same_file_rename_frees_name` proves rename-frees-name; M21 `rule_version` column, preserve-rows test `m21_adds_rule_version_preserving_rows` |
| 5 | RulesPanel Export/Import + preview modal (Accept-disabled-on-errors, Reject discards) | ✓ VERIFIED | WR-04: Accept enabled when `imported+updated>0` (`RulesPanel.tsx:339`) — partial-valid-subset works; WR-02 frontend 1 MiB pre-check (`RulesPanel.tsx:300-306`); `tsc --noEmit` clean |
| 6 | M21 forward-only migration with preserve-rows test; export_rules/import_rules Tauri commands | ✓ VERIFIED | `M21_RULE_VERSION_SQL` forward-only, `schema_version_is_13_with_m21_rule_version`; CR-01: validate-all-then-apply in ONE `transaction()` (`rules_io.rs:401`) with in-tx re-verify for concurrent-writer race (`:395-401`); single duplicate map; Mutex held only for snapshot read + commit tx (WR-06, `:252,276-285`) |
| 7 | Dedicated manager-free connection: SELECT→IDLE→DONE on ALL exits, 29-min re-IDLE, capped backoff, live CAPABILITY re-check, stop-interruptible | ✓ VERIFIED | WR-01: NEW `reidle_cycles_never_fire_on_wake` proves Timeout re-IDLE is silent (no wakeup/counter); WR-05: init failure releases handle best-effort DONE+LOGOUT (`idle.rs:431-452`); IN-03 poison-safe snapshot locks; `drive_cycle` unconditional `end_idle`; transcript tripwires `done_reconnect_backoff`, `wakeup_coalescing`, `poll_fallback` green |
| 8 | Advisory-only wakeup via SyncWake dirty-flag → SyncGate::try_begin (skip-if-busy, coalesce) | ✓ VERIFIED | `SyncWake` in `sync/mod.rs`; `wake_quiet_idle_never_dispatches` + burst-coalescing tests; UI `idle-wakeup` → `sge-idle-wakeup` CustomEvent (`useIdleStatus.ts:105-106`) → `requestSync("poll")` gated on `syncingRef` (`SyncStatus.tsx:226-231`); IN-02 single `idle_status` fetch on mount |
| 9 | spawn_if_allowed refuses when profile IDLE=false (repo profile has IDLE:false → poll-only) | ✓ VERIFIED | WR-03: NEW `idle_gate_consults_every_account` — gate consults every account's atom, any IDLE:true → spawn, absent abstains fail-closed; `spawn_if_allowed` IDLE=false → None; repo `.planning/probes/capability-profile.json` has `"IDLE": false`; IN-04 `SGE_CAPABILITY_PROFILE` is dev/test escape hatch only (`rules_io.rs:514-520`) |
| 10 | Kill-mid-IDLE sends DONE (+backoff, no duplicate sync, no stall) | ✓ VERIFIED | `idle_edge_kill_mid_idle_sends_done` green; 1s stop-poll race sends DONE first (`idle.rs:372-373,438`) |
| 11 | Sidebar Push/Polling/Offline via useIdleStatus + idle_status command | ✓ VERIFIED | `useIdleStatus.ts:30-54` maps Push/Polling/Offline (unknown → Polling, never phantom Push); `Sidebar.tsx:153-154,265-266`; `idle_status_command` + `idle_edge_status_mapping` tests green |
| 12 | IMPORT_EXPORT=false → rules UI disabled | ✓ VERIFIED (override) | Deviation accepted (see overrides frontmatter): atom absent from profile, backend validation authoritative → buttons always enabled |

**Score:** 11/11 truths verified (12 rows; row 12 carried by recorded override) + 1 override applied

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src-tauri/src/classify/rules_io.rs` | Envelope, validation, atomic import, caps | ✓ VERIFIED | Substantive; ONE-tx commit path; wired via commands + queries |
| `src-tauri/src/imap/idle.rs` | IdleWatcher, DONE-exits, silent re-IDLE, gate | ✓ VERIFIED | Substantive; no manager import; dead helper removed; wired via mod.rs + sync.rs + lib.rs |
| `src-tauri/src/commands/classify.rs` | export_rules/import_rules commands | ✓ VERIFIED | Present, rule-IO refs intact |
| `src-tauri/src/commands/sync.rs` | idle_status + idle_start commands | ✓ VERIFIED | `idle_status_command`, `app_state_starts_without_idle_watcher` green |
| `src-tauri/src/sync/mod.rs` | SyncWake type | ✓ VERIFIED | Coalescing dirty flag, gate dispatch |
| `src-tauri/src/store/mod.rs` + `queries.rs` | M21 migration + IO helpers | ✓ VERIFIED | Forward-only, preserve-rows test |
| `src/components/RulesPanel.tsx` | Import/export UI + preview modal, caps, partial accept | ✓ VERIFIED | 1 MiB frontend pre-check, imported+updated>0 Accept; tsc clean |
| `src/components/Sidebar.tsx` + `src/hooks/useIdleStatus.ts` + `src/components/SyncStatus.tsx` | Push status + wakeup funnel | ✓ VERIFIED | Single idle_status fetch on mount; wired hook → command → watcher; wakeup → requestSync("poll") |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| rules_io validation | folder_rules table | import_rules command → single-tx queries path | ✓ WIRED | Preview (commit=false) + accept (commit=true) share one path; counts match |
| export_rules | versioned JSON | Normalized pretty envelope | ✓ WIRED | Fixpoint test proves byte-identity |
| IdleWatcher | SyncGate | SyncWake dirty flag, advisory-only; re-IDLE silent | ✓ WIRED | No direct sync call anywhere on wakeup path; re-IDLE emits nothing |
| capability-profile.json IDLE atom | IdleWatcher spawn | Multi-account spawn_if_allowed gate | ✓ WIRED | Every account consulted; any true → spawn; absent → abstain fail-closed |
| RulesPanel | rules_io | In-panel import/export → Tauri commands | ✓ WIRED | Backend result rendered verbatim; caps mirrored frontend |
| Sidebar | IdleWatcher state | useIdleStatus → idle_status command | ✓ WIRED | Honest 3-state mapping |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| RulesPanel preview | ImportPreview.errors/warnings | `import_rules(commit=false)` over live store | ✓ FLOWING | Backend-validated, verbatim render |
| Sidebar push dot | PushView label | `idle_status` watcher snapshot poll | ✓ FLOWING | Live state, never hardcoded |
| SyncStatus refresh | syncingRef + requestSync | SyncGate single-flight | ✓ FLOWING | Wakeup funnels into same entry |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| lib suite | `cargo test -p sge --lib` (with build-env.sh) | 425 passed, 0 failed, 1 ignored (+5 new vs prior 420) | ✓ PASS |
| bins suite | `cargo test -p sge --bins` | live_gate 19+6+15 headless green, 7 targets ok | ✓ PASS |
| rules_io slice | `cargo test -p sge --lib rules_io` | 15 passed (incl. oversize/overcount, rename-frees-name, preview≡commit, multi-account gate) | ✓ PASS |
| idle slice | `cargo test -p sge --lib idle` | 16 passed (incl. reidle-silence) | ✓ PASS |
| frontend types | `npx tsc --noEmit` | clean, exit 0 | ✓ PASS |
| live-gate shape | `live_import_export_gate_skips_offline`, `live_idle_leg_skips_offline` | SKIP on missing `SGE_LIVE_IMAP_USER`, zero network offline | ✓ PASS |

### Probe Execution

No phase-declared probes (no `scripts/*/tests/probe-*.sh` for Phase 29). Skipped with reason.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| IMP-01 | 29-01, 29-03 | Versioned rule JSON portability + validation + UI | ✓ SATISFIED | Truths 1-6, 15 rules_io tests, atomic tx, caps, tsc clean |
| IDLE-01 | 29-02, 29-03 | INBOX push over manager-owned watcher + status UI | ✓ SATISFIED | Truths 7-11, silent re-IDLE, multi-account gate, transcript tripwires |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | None blocking | — | No TBD/FIXME/XXX/console-only handlers. `placeholder=` hits are HTML input attributes (legitimate UI) and one code comment mentioning "placeholders" for overwrite-then-trust fields — not stubs |

### Constraints Audit

- `source scripts/build-env.sh` before every cargo invocation; no global `LD_LIBRARY_PATH` exported.
- BODY.PEEK-only holds: watcher issues SELECT/CAPABILITY/IDLE/DONE/LOGOUT only (asserted by `idle_module_emits_read_only_verbs`); no FETCH added.
- Single-session invariant: no manager import in `idle.rs`.
- M21 forward-only with preserve-rows test; `imap_outbox`/`send_queue` untouched.
- Redaction: envelope carries rule metadata only; wakeups carry no content.
- Zero new deps (Cargo.toml untouched).
- `src-tauri/.live-test.env` is gitignored + untracked (git ls-files: 0) — the phase created no creds file; live legs SKIP offline per sandbox contract.
- No SMTP/send changes.

### Human Verification Required

None blocking. Suggested (not required) manual UAT: with an IDLE-capable server (profile atom true), log in → sidebar dot turns green Push; send self-mail → wakeup syncs within seconds; Import in RulesPanel with hand-edited invalid JSON → red errors, Accept disabled; oversize file (>1 MiB) → plain-language rejection. Not performable headlessly against `mail.utfpr.edu.br` (IDLE=false, operator creds required).

### Gaps Summary

No gaps. All 11 must-have truths verified with test evidence over current file state (425 lib tests green, +5 new fix tests); the single IMPORT_EXPORT-UI-gate deviation remains a recorded accepted override, not a gap. Review-fix round closed all 7 findings with no regressions.

---

_Verified: 2026-10-14T05:36:40Z_
_Verifier: the agent (gsd-verifier)_
