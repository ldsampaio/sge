---
phase: 39-llm-settings-foundation
plan: 03
subsystem: api
tags: [rust, tauri-ipc, sqlite, rusqlite-migration, reqwest, native-tls, keyring]

# Dependency graph
requires:
  - phase: 39-llm-settings-foundation
    provides: Plan 39-01 LlmSettings + validate + M23 store; Plan 39-02 keyring custody + fenced renderer (commands wrap all of it)
provides:
  - Egress-gated Tauri settings commands (get/save/reset-template/clear-key) with presence-bool views
  - LlmSettingsView / LlmSettingsInput IPC types (key-redacted Debug)
  - SCHEMA_VERSION 23 + M23 fresh-and-upgrade gate tests
  - reqwest json/stream/native-tls widening with committed lockfile (rustls/ring absent, hyper-tls present)
affects: [phase-40-generation-flow, phase-41-compose-integration]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 9200
  tasks: 3
  commits: 3

# Tech tracking
tech-stack:
  added: []
  patterns: [testable-sync-core-behind-thin-async-commands, classify-without-touching input validators, manual-Debug-redaction for secret-carrying IPC inputs]

key-files:
  created:
    - src-tauri/src/llm/commands.rs
  modified:
    - src-tauri/src/llm/mod.rs
    - src-tauri/src/lib.rs
    - src-tauri/src/store/mod.rs
    - src-tauri/Cargo.toml
    - src-tauri/Cargo.lock

key-decisions:
  - "Commands live in llm/commands.rs (not inline in lib.rs) so the plan's own llm::commands test filter resolves; lib.rs gains only four generate_handler entries"
  - "Empty-string api_key is rejected BEFORE any write (invalid input touches nothing), even though the plan lists key handling after the SQLite step"
  - "Manual Debug redaction on LlmSettingsInput so a stray {:?} can never print key material"
  - "reqwest widening is a build-dependency change only; API-coverage detector verdict for this phase: detected false"

patterns-established:
  - "Sync core (apply_save/apply_reset_template/view_of/classify_api_key) tested against memory store + memory keyring; async #[tauri::command] fns stay thin (Mutex guard, spawn_blocking, read_view)"
  - "Presence re-probed after every mutation (re-read, never assumed) so views stay honest"

requirements-completed: [LLM-01, LLM-02, LLM-03, LLM-04]

coverage:
  - id: D1
    description: "get/save/reset/clear settings commands with presence-bool views, save key semantics (None unchanged, empty rejected, clear-only removal), template-only reset"
    requirement: "LLM-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/commands.rs#save_with_invalid_url_leaves_stored_row_unchanged"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/commands.rs#save_with_api_key_none_preserves_prior_presence_true"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/commands.rs#save_with_empty_api_key_errors_naming_key_and_writes_nothing"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/commands.rs#reset_restores_template_only_keeping_url_and_model"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/commands.rs#clear_removes_key_and_returns_keyless_view"
        status: pass
    human_judgment: false
  - id: D2
    description: "SCHEMA_VERSION 23 with fresh-install seed and M1..M22 upgrade paths proven plus single-row CHECK enforcement"
    requirement: "LLM-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#schema_version_is_23_with_m23_llm_settings"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#m23_upgrade_from_v22_preserves_rows_and_seeds_settings"
        status: pass
      - kind: unit
        ref: "src-tauri/src/store/mod.rs#m23_single_row_check_rejects_second_id"
        status: pass
    human_judgment: false
  - id: D3
    description: "reqwest widened to json/stream/native-tls with committed lockfile; cargo tree shows hyper-tls chain and no rustls/ring"
    requirement: "LLM-04"
    verification:
      - kind: other
        ref: "cargo tree -i rustls (no match) + cargo tree -i ring (no match) + cargo tree -i hyper-tls (v0.6.0 under reqwest)"
        status: pass
    human_judgment: false
  - id: D4
    description: "Zero network I/O in the phase (no Client construction, no send in command code; reqwest widening is build-only)"
    requirement: "LLM-04"
    verification:
      - kind: unit
        ref: "grep -rn reqwest::Client|\\.send( src-tauri/src/llm/ (doc-comment mentions only, no code)"
        status: pass
    human_judgment: false

# Metrics
duration: 40min
completed: 2026-10-15
status: complete
---

# Phase 39 Plan 03: Settings IPC Surface + Migration Gate + TLS Widening Summary

**Four egress-gated Tauri settings commands with presence-bool views, SCHEMA_VERSION corrected to 23 with fresh+upgrade gate tests, and reqwest widened to native-tls-only — full lib suite green (513 passed), zero network I/O**

## Performance

- **Duration:** 40 min
- **Started:** 2026-10-15T14:25:00Z
- **Completed:** 2026-10-15T15:04:25Z
- **Tasks:** 3
- **Files modified:** 6

## Accomplishments

- `llm/commands.rs`: `get_llm_settings` / `save_llm_settings` / `reset_llm_template` / `clear_llm_api_key` — validate-first, keyring only in `spawn_blocking`, presence-bool views, all four registered in `generate_handler`
- `LlmSettingsView` (no key field) + `LlmSettingsInput` (`None` = unchanged, `Some` empty rejected, clear-only removal) with key-redacting manual `Debug`
- `SCHEMA_VERSION` 14 → 23 with renamed assert plus a 3-test M23 gate module (fresh seed, v22-upgrade preserve+seed, id-2 CHECK rejection)
- `reqwest` widened to `json`/`stream`/`native-tls` (`default-features = false` kept); lockfile committed; tree gate: `hyper-tls v0.6.0` present, `rustls`/`ring` absent
- T-39-01/02/SC mitigated in-task: key never in views/errors/logs, template-only reset proven, native-tls-only closure audited

## Task Commits

Each task was committed atomically:

1. **Task 1: Tauri settings commands, egress-gated** - `bf08e63` (feat)
2. **Task 2: SCHEMA_VERSION fix plus fresh-and-upgrade gate** - `943bf51` (fix)
3. **Task 3: reqwest widening plus TLS tree gate** - `8478c6e` (chore)

## Files Created/Modified

- `src-tauri/src/llm/commands.rs` - Four Tauri commands + sync testable core (`apply_save`, `apply_reset_template`, `view_of`, `classify_api_key`) + 7 tests
- `src-tauri/src/llm/mod.rs` - `pub mod commands;` + `LlmSettingsView` / `LlmSettingsInput` + redacted `Debug` + `From<&LlmSettingsInput>`
- `src-tauri/src/lib.rs` - Four `generate_handler!` entries (no other changes)
- `src-tauri/src/store/mod.rs` - `SCHEMA_VERSION = 23`, renamed assert with fresh-seed checks, new `m23_llm_settings_gate_tests` module (2 tests)
- `src-tauri/Cargo.toml` - reqwest feature widening with Phase 39 comment
- `src-tauri/Cargo.lock` - Committed with hyper-tls/tokio-native-tls closure

## Decisions Made

- Commands placed in `llm/commands.rs` rather than inline in `lib.rs`: the plan's own verify filter (`cargo test -p sge --lib llm::commands`) requires that module path, and it keeps the Tauri command layer beside the lane it serves. `lib.rs` gains only handler registrations.
- Empty-string `api_key` rejected before the SQLite write (not after, as the plan's step order could be read): the plan's key-link invariant "invalid input touches nothing" outranks the prose ordering, and a rejected save must not leave a half-written row.
- Manual `Debug` for `LlmSettingsInput` showing `api_key: Some(<redacted>)`: derived `Debug` would print key material on any incidental `{:?}` logging — redaction by construction, same posture as the payload-free error enums.
- `apply_reset_template` returns the validated row for the caller to persist (rather than writing itself), so the async command holds the single store-`Mutex` guard across read+write with no lock juggling in shared code.
- Recorded in code and here: widening is a build-dependency change enabling `Url` parsing now and the Phase 40 client later — no external-API surface integrated, API-coverage detector verdict `detected: false`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Commands placed in new `llm/commands.rs` though plan `files` lists only `lib.rs`**

- **Found during:** Task 1 (Tauri settings commands)
- **Issue:** The plan's `<verify>` runs `cargo test -p sge --lib llm::commands`, which matches nothing if the commands and their tests live inline in `lib.rs` (crate-root tests filter as `tests::…`). The `files` list and the verify filter contradicted each other.
- **Fix:** Created `src-tauri/src/llm/commands.rs` holding the four `#[tauri::command]` fns plus the sync core and 7 tests; declared `pub mod commands;` in `llm/mod.rs` (2-line precedent from 39-02); `lib.rs` gains only the four `generate_handler!` entries. The Phase-40/abort handoff comment sits above `save_llm_settings` where the command lives.
- **Files modified:** src-tauri/src/llm/commands.rs (new), src-tauri/src/llm/mod.rs, src-tauri/src/lib.rs
- **Verification:** `cargo test -p sge --lib llm::commands` → 7 passed; lib.rs diff greps clean for `Client` / `.send(`
- **Committed in:** bf08e63 (Task 1 commit)

**2. [Rule 2 - Missing Critical] `LlmSettingsView` / `LlmSettingsInput` types did not exist anywhere**

- **Found during:** Task 1 (commands need view/input shapes; `read_first` referenced a "view shape from Plan 01" that was never created)
- **Issue:** No view/input types existed — commands could not return presence-bool views without inventing an ad-hoc shape in the command layer.
- **Fix:** Added both types to `llm/mod.rs` per the plan's own Artifacts table ("LlmSettingsView / LlmSettingsInput in src-tauri/src/llm"), with the key-redacted `Debug` hardening above.
- **Files modified:** src-tauri/src/llm/mod.rs
- **Verification:** `input_debug_redacts_key_material` passes; full lib suite green
- **Committed in:** bf08e63 (Task 1 commit)

---

**Total deviations:** 2 auto-fixed (1 blocking, 1 missing-critical)
**Impact on plan:** Both required for the plan's own acceptance criteria to be satisfiable; no scope creep — no new dependencies, no network code, no UI, no Phase 40 client.

## Issues Encountered

None beyond the deviations above. Pre-existing repo state untouched: the full-suite `tests/live_gate_21_safety.rs` compile failure from uncommitted v1.7-era `drafts.rs` work remains out of scope; verification scope `cargo test -p sge --lib` is fully green (513 passed, 2 ignored — the ignores are the pre-existing live-keyring tests). `cargo fetch` network access worked; no sandbox blockage.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Phase 39 complete (3/3 plans): Phase 40 can build the generation client on `save_llm_settings`-validated rows, `render_prompt`, and the widened reqwest (native-tls `Client`); the timeout abort/cancel handoff is documented above `save_llm_settings`.
- Phase 41 settings UI consumes the four commands directly (`has_api_key: bool` drives the key prompt; `reset_llm_template` restores text only; key removal only via `clear_llm_api_key`).
- `SCHEMA_VERSION 23` is now truthful — the next migration is M24.

## Self-Check: PASSED

- All created files exist: `src-tauri/src/llm/commands.rs` — FOUND
- All commits exist: `bf08e63`, `943bf51`, `8478c6e` — FOUND (`git log --oneline` verified)
- No file deletions in any task commit — verified via `git diff --diff-filter=D` (empty)
- No stub patterns (`TODO`/`FIXME`/`placeholder`/`unimplemented`) in new files — verified via grep
- `cargo test -p sge --lib llm::commands` → 7 passed; `cargo test -p sge --lib store::` → 72 passed; full `--lib` → 513 passed, 0 failed
- `cargo tree -i rustls` / `-i ring` → no match; `cargo tree -i hyper-tls` → v0.6.0 under reqwest
- Only `reqwest::Client`/`.send(` matches in `llm/` are doc-comment prose, no code

---
*Phase: 39-llm-settings-foundation*
*Completed: 2026-10-15*
