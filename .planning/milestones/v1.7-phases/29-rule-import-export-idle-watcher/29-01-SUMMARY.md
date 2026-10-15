---
phase: 29-rule-import-export-idle-watcher
plan: 01
subsystem: rules-portability
tags: [IMP-01, import-export, rules, json, migration-M21]
requires: [phase-25-folder-rules]
provides: [rules_io-envelope, import-export-commands, rule_version-column]
affects: [29-02, 29-03, phase-31-sieve]
tech-stack:
  added: []
  patterns: [versioned-json-envelope, dry-run-preview-then-commit, warn-but-allow-import]
key-files:
  created: []
  modified: [src-tauri/src/classify/rules_io.rs, src-tauri/src/store/queries.rs, src-tauri/src/store/mod.rs, src-tauri/src/commands/classify.rs]
decisions:
  - "Envelope carries the real Phase-25 row shape (slug ids, RuleConditions/RuleScope JSON) instead of the plan's UUID/timestamp sketch"
  - "rule_version bumps stay server-side so export-import-export is byte-identical"
  - "M21 adds rule_version/rule_etag forward-only with preserve-rows test"
metrics:
  duration: ~2h
  completed: 2026-10-14
status: complete
actuals:
  tokens: 62000
  tasks: 3
  commits: 2
commits:
  - 7976150: initial rules_io draft (never compiled — superseded)
  - 0594d24: complete rules_io against Phase-25 schema + M21 + commands
plan_head_before: 93f7b3f
---

# Phase 29 Plan 01: Rules Import/Export Core Summary

Versioned JSON rule portability (IMP-01) over the Phase-25 `folder_rules` table: export, validated import with dry-run preview, conflict resolution with version bumps, and a byte-identical round-trip fixpoint proven by test.

## What Was Built

- **`src-tauri/src/classify/rules_io.rs`** (rewritten): `RuleExport` envelope `{version: 1, rules[]}`, `export_rules()` (deterministic key/row order via BTreeMap + evaluation ordering), `import_rules(json, commit)` (dry-run preview vs commit upsert), structural validation with plain-language errors, warn-but-allow folder/category existence checks, `idle_allowed_from_profile` / `default_profile_path` capability helpers shared with 29-02.
- **`src-tauri/src/store/queries.rs`**: `RuleRow` gains `rule_version: i64` + `rule_etag: Option<String>`; `export_all_rules()`, `get_all_rule_names()`, `upsert_rule_import()` (insert → version 1, update → old+1), all through bound parameters.
- **`src-tauri/src/store/mod.rs`**: M21 forward migration (`SCHEMA_VERSION` 12→13, `rule_version INTEGER NOT NULL DEFAULT 1`, `rule_etag TEXT`) + `m21_adds_rule_version_preserving_rows` test.
- **`src-tauri/src/commands/classify.rs`**: `export_rules` / `import_rules(json, commit)` Tauri commands (frontend never validates — T-29-09).

## Tests

11 `classify::rules_io` tests (round-trip fixpoint byte-identical, dry-run writes nothing, empty pattern, reserved names ×4, duplicate names file+store, priority range, dest_kind, unknown-folder warn-but-allow, id-update version bump, live-gate SKIP, profile-atom gate) + M21 preserve-rows + schema-version assert. Full lib suite: 404 passed at commit time.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Prior commit 7976150 never compiled — rewrote rules_io.rs**
- **Found during:** Task 1 verification (cargo check: 21 errors)
- **Issue:** Referenced `uuid` crate (not a dependency), `chrono::DateTime` serde (feature off), `Rule{rule_id, priority: u16, conditions_json…}` fields that don't exist on the real `Rule` struct, and query helpers that were never added.
- **Fix:** Rewrote against the real schema (`RuleRow` with slug `id: String`, `priority: i64`, JSON TEXT columns); envelope carries row fields; no new deps.
- **Commit:** 0594d24

**2. [Rule 2 - Correctness] Plan's UUID/timestamp schema adapted to Phase-25 slug ids**
- **Issue:** Plan specified `rule_id: UUID`, `created_at/updated_at` in the envelope; real rows use slug ids and `RuleRow` doesn't project timestamps.
- **Fix:** Envelope uses `id: String` (validated non-empty), timestamps stay server-side. Round-trip fixpoint holds exactly because version bumps stay out of the envelope.
- **Impact:** Phase 31 Sieve export consumes row shape, not UUIDs — noted in `affects`.

**3. [Rule 3 - Blocking] Tauri build script needed the gitignored sidecar stub**
- **Issue:** `cargo check/test` failed in `tauri-codegen` resource validation: `binaries/sge-laya-x86_64-unknown-linux-gnu doesn't exist`.
- **Fix:** Created an empty stub at the gitignored path (local-only, never committed). No repo pollution (`git status` clean of it).

## Auth Gates

None.

## Known Stubs

None — all commands, queries, and migration are wired and tested.

## Threat Flags

None — no new trust surface beyond the plan's threat model (file→JSON→parameterized-SQL; T-29-01 mitigated by validation-before-write, T-29-03 by metadata-only export, T-29-SC by zero new deps).

## Self-Check: PASSED

- `src-tauri/src/classify/rules_io.rs`, `store/queries.rs`, `store/mod.rs`, `commands/classify.rs` all present and compiling.
- Commits 7976150 + 0594d24 exist in `git log`.
- `cargo test -p sge --lib classify::rules_io` → 11 passed; `m21_adds` → passed.
