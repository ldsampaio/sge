---
phase: 01-scaffold-connection
plan: "01"
subsystem: infra
tags: [tauri, react, typescript, vite, eslint, ci, async-imap]

# Dependency graph
requires: []
provides:
  - Buildable Tauri v2 shell (app id br.edu.utfpr.sge) with login-to-backend IPC tracer
  - Permanent `connect_account` command signature for Plans 01-02/01-03 to fill
  - Pinned Rust crate set (tauri 2.12, async-imap 0.11.3, mail-parser 0.11, rusqlite 0.37, keyring 3)
  - ESLint + Prettier + MIT + Linux CI (ubuntu-22.04) from day one
affects: [01-02-connection-core, 01-03-login-ux, sync-engine, local-store]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 0
  tasks: 3
  commits: 1

# Tech tracking
tech-stack:
  added: [tauri 2.12, react 19, vite 8, typescript 6, async-imap 0.11.3, async-native-tls 0.5, async-std 1.13, mail-parser 0.11, rusqlite 0.37, rusqlite_migration 2.3, keyring 3.6, thiserror 2, chrono 0.4, futures 0.3, eslint 10, prettier 3]
  patterns: [tauri command IPC bridge, ConnectSummary DTO, thiserror-to-String command errors]

key-files:
  created: [src-tauri/src/lib.rs, src/App.tsx, src-tauri/tauri.conf.json, .github/workflows/ci.yml, eslint.config.js, .prettierrc, LICENSE]
  modified: [src-tauri/Cargo.toml, package.json, index.html, vite.config.ts]

key-decisions:
  - "Scaffolded with create-tauri-app 4.7.4 (-y non-interactive) instead of hand-wiring, per plan"
  - "Bundle sketch lives as comments in ci.yml, not tauri.conf.json (tauri-build hard-errors on unknown fields)"
  - "Tracer keeps minimal host/port validation in Rust per trust-boundary rule; body returns imap-not-wired"

patterns-established:
  - "connect_account(host, port, security, username, password) -> Result<ConnectSummary, String> is the frozen IPC contract"
  - "Password held only for the invoke call: never logged, never persisted (until Plan 01-03 keyring)"
  - "CI runs lint + build + fmt + clippy + check + identity assertion on ubuntu-22.04"

requirements-completed: [CONN-01, CONN-02]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Login form Connect click reaches the Rust command and displays its result string"
    requirement: "CONN-01"
    verification:
      - kind: other
        ref: "npm run build passes + cargo check passes + tauri.conf.json identifier assertion passes"
        status: pass
    human_judgment: true
    rationale: "Visual confirmation of the login form in tauri dev requires a human with a display; automation proves IPC wiring and build only"
  - id: D2
    description: "cargo check, frontend lint, and Linux CI definition pass from day one"
    requirement: "CONN-02"
    verification:
      - kind: other
        ref: "cargo check + cargo clippy + cargo fmt --check + npm run lint + python yaml parse of ci.yml"
        status: pass
    human_judgment: false
  - id: D3
    description: "Crate pins from STACK.md resolve and compile"
    requirement: "CONN-01"
    verification:
      - kind: other
        ref: "cargo tree shows single async-imap v0.11.3 line; Cargo.lock committed"
        status: pass
    human_judgment: false

# Metrics
duration: 45min
completed: 2026-10-03
status: complete
---

# Phase 1 Plan 1: Scaffold + Tracer Summary

**Buildable Tauri v2 shell (br.edu.utfpr.sge) with login-to-backend IPC tracer, pinned IMAP/SQLite/keyring crates, and Linux CI**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-10-03T01:20Z
- **Completed:** 2026-10-03T02:05Z
- **Tasks:** 3
- **Files modified:** ~44 (mostly scaffold + icons)

## Accomplishments

- Tauri v2 shell scaffolded via official create-tauri-app 4.7.4 (react-ts, npm) with SGE identity throughout
- Tracer IPC slice proven: login form (server pre-filled `mail.utfpr.edu.br`) invokes `connect_account`, displays `imap-not-wired` stub error
- All STACK.md pins resolve and compile: tauri 2.12.1, async-imap 0.11.3, mail-parser 0.11.9, rusqlite 0.37.0, rusqlite_migration 2.3.0, keyring 3.6.3
- ESLint + Prettier green; MIT LICENSE; gitignore covers target/node_modules/dist; Cargo.lock + package-lock.json committed
- GitHub Actions Linux CI (ubuntu-22.04 + webkit2gtk-4.1 stack) covering lint/build/fmt/clippy/check/identity assertion

## Task Commits

Plan committed atomically per orchestrator instruction (single commit for the plan):

1. **Tracer + pins + hygiene + CI** - `78b339c` (feat)

**Plan metadata:** SUMMARY amended into `78b339c` (docs: plan 01-01 complete)

## Files Created/Modified

- `src-tauri/src/lib.rs` - `connect_account` IPC contract + `ConnectSummary` + typed `ConnectError`
- `src/App.tsx` - Minimal login form (server/username/password, single Connect button, result display)
- `src-tauri/tauri.conf.json` - Identity `br.edu.utfpr.sge` / `SGE` / `SGE`
- `src-tauri/Cargo.toml` - All pins + async-imap 0.11.3 pin record comment
- `src-tauri/Cargo.lock` - Committed per supply-chain rule (T-01-01)
- `.github/workflows/ci.yml` - Linux CI + Phase 5 bundle sketch comments
- `eslint.config.js`, `.prettierrc`, `LICENSE`, `.gitignore` - Toolchain + hygiene
- Scaffold remainder (`package.json`, `vite.config.ts`, `src-tauri/icons/*`, etc.)

## Decisions Made

- `create-tauri-app -y --identifier br.edu.utfpr.sge` used non-interactively (no TTY in agent env); productName/window title normalized to `SGE` afterward
- Template `greet` command removed — single-path tracer only, no other call sites
- Tracer validates host non-empty / port non-zero in Rust (trust boundary: never trust frontend), keeping the frozen signature

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Bundle sketch cannot live in tauri.conf.json**
- **Found during:** Task 3 (Linux CI from day one)
- **Issue:** Plan asked for the Phase 5 bundle sketch "as comments in tauri.conf.json", but JSON has no comments and tauri-build hard-errors on unknown fields (`unknown field '_phase5BundleSketch'` failed the build script)
- **Fix:** Reverted the extra key; placed the sketch as a comment block at the top of `.github/workflows/ci.yml` instead, with a note explaining why
- **Files modified:** `.github/workflows/ci.yml`, `src-tauri/tauri.conf.json` (reverted)
- **Verification:** `cargo check` passes after revert; sketch text grep-able in ci.yml
- **Committed in:** 78b339c (part of plan commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Cosmetic relocation only; no scope or behavior change. Phase 5 still has its sketch.

## Issues Encountered

- `create-tauri-app` requires a TTY (`IO error: not a terminal`); solved with the `-y/--yes` non-interactive flag (4.7.4 supports it)
- First `script -qec` scaffold attempt hung on prompts and had to be abandoned (no stray processes left)

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Plan 01-02 fills the `connect_account` body with the real `imap::session` module behind the unchanged signature
- `imap_probe` CLI harness + live transcript fixture are next; no blockers

## Self-Check: PASSED

- `src-tauri/src/lib.rs`, `src/App.tsx`, `.github/workflows/ci.yml`, `LICENSE` all FOUND on disk
- Commit `78b339c` FOUND in git log
- `cargo check` + `npm run build` + `npm run lint` all green at commit time

---
*Phase: 01-scaffold-connection, Plan: 01*
*Completed: 2026-10-03*
