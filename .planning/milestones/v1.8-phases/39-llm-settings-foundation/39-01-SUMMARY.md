---
phase: 39-llm-settings-foundation
plan: 01
subsystem: database
tags: [rust, sqlite, rusqlite-migration, validation, thiserror, reqwest-url]

# Dependency graph
requires:
  - phase: 31-learning-loop
    provides: M22 migration chain head that M23 extends
provides:
  - M23 single-row `llm_settings` table seeded with DEFAULT_* values
  - LlmSettings type + DEFAULT_* consts + DEFAULT_TEMPLATE (no key field)
  - validate() save-time policy (https-only/http-loopback, temp 0-2, tokens 1-4096, timeout >= 1)
  - get_llm_settings / upsert_llm_settings round-trip (single-writer, idempotent)
affects: [39-02-prompt-renderer, 39-03-settings-commands, phase-40-generation-flow]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 7467
  tasks: 3
  commits: 3

# Tech tracking
tech-stack:
  added: []
  patterns: [single-row CHECK(id=1) table with in-migration seed, validate-first persistence, unit-variant error enum]

key-files:
  created:
    - src-tauri/src/llm/mod.rs
    - src-tauri/src/llm/validate.rs
  modified:
    - src-tauri/src/store/mod.rs
    - src-tauri/src/store/queries.rs
    - src-tauri/src/lib.rs

key-decisions:
  - "Unit-only LlmSettingsError variants (no payload) so secrets cannot be interpolated into errors by construction"
  - "Single-line DEFAULT_TEMPLATE with no quotes so the M23 seed literal is byte-identical; drift guarded by seed_defaults_equal_consts test"
  - "SCHEMA_VERSION const left at 14 per plan instruction; the 14-vs-23 fix belongs to Plan 39-03"

patterns-established:
  - "Validate-first persistence: upsert_llm_settings accepts only validate() output, documented in queries.rs header"
  - "Single-writer comment above upsert_llm_settings (Mutex + last-writer-wins on id = 1)"

requirements-completed: [LLM-01, LLM-04]

coverage:
  - id: D1
    description: "M23 llm_settings single row seeds DEFAULT_* values and round-trips validated edits; double-save stays one identical row"
    requirement: "LLM-01"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/mod.rs#seed_defaults_equal_consts"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/mod.rs#save_then_read_round_trips_edited_values"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/mod.rs#double_save_leaves_one_identical_row"
        status: pass
    human_judgment: false
  - id: D2
    description: "Invalid URL / empty model / out-of-range temperature / tokens / non-positive timeout rejected with field-named plain-language reasons; timeout unclamped above 1"
    requirement: "LLM-04"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/validate.rs#temperature_edges_accept_min_max_reject_adjacent"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/validate.rs#max_tokens_edges_accept_min_max_reject_adjacent"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/validate.rs#timeout_rejects_non_positive_accepts_any_positive_unclamped"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/validate.rs#rejects_non_http_schemes"
        status: pass
      - kind: unit
        ref: "src-tauri/src/llm/validate.rs#stored_url_is_trimmed_and_slash_stripped"
        status: pass
    human_judgment: false
  - id: D3
    description: "API key never appears in SQLite file, error strings, or logs (key has no field, no column, no error payload)"
    requirement: "LLM-04"
    verification:
      - kind: unit
        ref: "src-tauri/src/llm/mod.rs#sqlite_dump_contains_no_key_material"
        status: pass
    human_judgment: false
  - id: D4
    description: "Zero network I/O in this phase (no reqwest Client construction, no send call in llm/ or store/)"
    requirement: "LLM-01"
    verification:
      - kind: unit
        ref: "grep -rn reqwest::Client src-tauri/src/llm/ src-tauri/src/store/ (no code matches)"
        status: pass
    human_judgment: false

# Metrics
duration: 25min
completed: 2026-10-15
status: complete
---

# Phase 39 Plan 01: LLM Settings Foundation Summary

**M23 single-row llm_settings table with seeded defaults, reqwest::Url-validated save policy, and idempotent read/write round-trip — 17 unit tests green, zero network I/O**

## Performance

- **Duration:** 25 min
- **Started:** 2026-10-15T15:00:00Z
- **Completed:** 2026-10-15T15:25:00Z
- **Tasks:** 3
- **Files modified:** 5

## Accomplishments

- New `llm/` backend lane: `LlmSettings` (six non-secret fields, no key field) + `DEFAULT_*` consts + `DEFAULT_TEMPLATE` with exactly `{sender}`/`{subject}`/`{body}`
- `validate()` enforcing https-any-host / http-loopback-only URLs, temperature 0.0–2.0, max_tokens 1–4096, timeout ≥ 1 unclamped — all rejections field-named plain-language via payload-free `LlmSettingsError`
- M23 forward migration (single-row `CHECK (id = 1)` + in-migration seed) registered after M22; `get_llm_settings` / `upsert_llm_settings` in queries.rs
- T-39-01 mitigated: no key column, no key field, no error payload; dump-scan test proves no key material in SQLite

## Task Commits

Each task was committed atomically:

1. **Task 1: End-to-end validated-settings round-trip — one path only** - `5486be7` (feat)
2. **Task 2: URL boundary and normalization cases** - `306d4ef` (feat)
3. **Task 3: Numeric clamp edges and concurrency/idempotency backstops** - `e208140` (feat)

## Files Created/Modified

- `src-tauri/src/llm/mod.rs` - Lane root: LlmSettings, DEFAULT_* consts, DEFAULT_TEMPLATE + store round-trip tests
- `src-tauri/src/llm/validate.rs` - LlmSettingsError + validate/normalize + 12 boundary/clamp tests
- `src-tauri/src/store/mod.rs` - M23_LLM_SETTINGS_SQL const + M::up registration after M22
- `src-tauri/src/store/queries.rs` - get_llm_settings / upsert_llm_settings + single-writer invariant comment
- `src-tauri/src/lib.rs` - One line: `pub mod llm;` declaration (see deviation)

## Decisions Made

- Payload-free error enum: all six `LlmSettingsError` variants are unit variants with static messages, so key/mail-body interpolation is impossible by construction rather than by review discipline.
- Single-line template literal: `DEFAULT_TEMPLATE` avoids newlines and single quotes so the M23 seed `INSERT` embeds it byte-identically; `seed_defaults_equal_consts` fails the build on any future drift.
- Direct `f64` range comparison with no epsilon: adjacency values like 2.0000001 reject; NaN/Infinity reject via `contains`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Declared `pub mod llm;` in lib.rs despite "Do not touch lib.rs"**

- **Found during:** Task 1 (End-to-end validated-settings round-trip)
- **Issue:** The plan prohibits touching lib.rs, but Rust requires the `llm` lane root to be declared from the crate root — without it nothing compiles and `cargo test -p sge --lib llm` discovers zero tests.
- **Fix:** Added the single alphabetical declaration line `pub mod llm;` (between `imap` and `send_queue`). No commands, handlers, or other lib.rs surface touched.
- **Files modified:** src-tauri/src/lib.rs (1 line)
- **Verification:** `cargo test -p sge --lib llm` → 17 passed; full `store::` suite → 70 passed
- **Committed in:** 5486be7 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Minimal one-line declaration required for compilation; no scope creep. The lib.rs commands/keyring/Cargo.toml prohibitions were otherwise fully honored (no keyring, prompt, command, or dependency changes in this plan).

## Issues Encountered

None — plan executed as written apart from the one-line module declaration above. Pre-existing repo warnings (e.g. unused `n` in `category_precision_and_n`) and pre-existing uncommitted v1.7-era changes were left untouched.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Plan 39-02 can build the keyring entry + prompt renderer on `LlmSettings` / `DEFAULT_TEMPLATE`; Plan 39-03 owns the Tauri commands and the stale `SCHEMA_VERSION` (14-vs-23) const fix, which this plan deliberately left untouched per its `read_first` instruction.
- Timeout handoff documented in code: `timeout_secs >= 1` validate-only, no upper bound — Phase 40 owns effective abort/cancel.

## Self-Check: PASSED

- All created files exist: `src-tauri/src/llm/mod.rs`, `src-tauri/src/llm/validate.rs` — FOUND
- All commits exist: `5486be7`, `306d4ef`, `e208140` — FOUND (`git log --oneline` verified)
- No file deletions in any task commit — verified via `git diff --diff-filter=D`
- No stub patterns (`TODO`/`FIXME`/`placeholder`/`unimplemented`) in new files — verified via grep
- `cargo test -p sge --lib llm` → 17 passed; `cargo test -p sge --lib store::` → 70 passed

---
*Phase: 39-llm-settings-foundation*
*Completed: 2026-10-15*
