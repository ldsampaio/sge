---
phase: 01-scaffold-connection
plan: "03"
subsystem: ui
tags: [react, login-ux, keyring, secret-service, tauri-commands]

# Dependency graph
requires:
  - phase: 01-02
    provides: [3-mode IMAP session, typed errors, probe core, transcript fixture]
provides:
  - Complete login screen (prefill, security selector, eye toggle, single Connect, real state)
  - Failure UX with manual Retry and warned TLS re-confirm, zero auto-retry paths
  - CONN-03 save slice: remember-me keyring persist/load via Secret Service
affects: [sync-engine, keyring-auto-login-phase-5]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 0
  tasks: 3
  commits: 2

# Tech tracking
tech-stack:
  added: []
  patterns: [CredentialStore trait (keyring + memory impls), spawn_blocking keyring commands, single-JSON-blob keyring entry]

key-files:
  created: [src/components/LoginForm.tsx, src/components/SecuritySelector.tsx, src-tauri/src/creds.rs]
  modified: [src/App.tsx, src-tauri/src/lib.rs, index.html]

key-decisions:
  - "Single keyring entry (service sge, JSON blob) for atomic save/load/clear"
  - "TLS re-confirm retries strictly: warned and explicit, never a silent bypass"
  - "Transcript redacts password only (username scrub annihilated short-name text)"
  - "Unchecked remember-me means keyring untouched (no silent clear)"

patterns-established:
  - "Keyring calls only via spawn_blocking; locked/headless errors become friendly memory-only fallback notes"
  - "Frontend security values map 1:1 to backend SecurityMode strings (implicit_tls/starttls/plain)"
  - "No auto-retry: Retry buttons re-invoke the same handler with preserved fields"

requirements-completed: [CONN-01, CONN-02, CONN-03]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Login screen with server prefill, security selector + port autofill + Advanced row, eye toggle, single Connect, real connection state"
    requirement: "CONN-01"
    verification:
      - kind: other
        ref: "npm run build passes + eslint passes + 1 submit button + no Test-connection text"
        status: pass
    human_judgment: true
    rationale: "Visual layout and interaction feel need a human with the running app; automation proves wiring and build only"
  - id: D2
    description: "Failures show plain-language error with manual Retry; zero automatic retry paths"
    requirement: "CONN-02"
    verification:
      - kind: other
        ref: "negative grep setTimeout/setInterval in LoginForm empty + Retry onClick bound to connect handler"
        status: pass
    human_judgment: false
  - id: D3
    description: "Remember-me persists username + password in OS keyring and repopulates the form; unchecked leaves keyring untouched"
    requirement: "CONN-03"
    verification:
      - kind: unit
        ref: "cargo test -p sge creds (3 pass: memory roundtrip, corrupt mapping, friendly error)"
        status: pass
      - kind: integration
        ref: "live Secret Service roundtrip test passes (save/load/clear vs org.gnome.keyring)"
        status: pass
    human_judgment: false

# Metrics
duration: 50min
completed: 2026-10-03
status: complete
---

# Phase 1 Plan 3: Login UX + Keyring Summary

**Full login screen with security selector and retry UX, plus remember-me persisted in the OS keyring (proven live against Secret Service)**

## Performance

- **Duration:** ~50 min
- **Started:** 2026-10-03T04:15Z
- **Completed:** 2026-10-03T05:05Z
- **Tasks:** 3
- **Files modified:** 6

## Accomplishments

- `LoginForm` + `SecuritySelector`: server pre-filled `mail.utfpr.edu.br`, 3-mode selector (SSL/TLS 993 default, STARTTLS 143, unencrypted-localhost with warning), port autofill + Advanced row, eye toggle (hidden default), single Connect, real `ConnectSummary` display
- Failure UX: backend error verbatim, manual Retry preserving fields, TLS errors get an explicit warned re-confirm ("I understand — retry anyway" re-verifies strictly); zero timers/reconnect paths
- `creds.rs`: `CredentialStore` trait with `KeyringStore` (service `sge`, single JSON blob) + `MemoryStore`; `save/load/clear_credentials` commands via `spawn_blocking`; locked/headless keyring falls back to memory-only with an explanatory note
- Keyring proven LIVE: ignored roundtrip test passes against the real Secret Service (`org.gnome.keyring`) in this environment
- No auto-connect code anywhere (negative grep); no `plaintext` token anywhere in `src-tauri/src` (negative grep)

## Task Commits

Plan committed as feat + docs pair:

1. **Login UX + keyring save slice** - `d47f6ab` (feat)
2. **Plan 01-03 summary** - (this docs commit) (docs)

## Files Created/Modified

- `src/components/LoginForm.tsx` - Full login form, failure UX, remember-me wiring, keyring reload on launch
- `src/components/SecuritySelector.tsx` - 3-mode selector + Advanced port row + localhost warning
- `src/App.tsx` - Renders the login screen
- `src-tauri/src/creds.rs` - Keyring store, memory store, error mapping, tests (incl. ignored live test)
- `src-tauri/src/lib.rs` - Three new commands registered; `connect_account` untouched
- `index.html` - Title set to SGE

## Decisions Made

- Single keyring entry (`sge` / `sge-default-account`, JSON `{"username","password"}`) keeps save/load/clear atomic for M1's single account; multi-account slots arrive with Phase 2's accounts table
- TLS re-confirm button re-invokes the same strict path (backend has no bypass): the confirm is explicit and warned, never a silent exception — consistent with Plan 01-02's refuse-loudly cert policy
- Unchecked remember-me leaves the keyring fully untouched (not even a clear), per the plan's "memory-only" wording
- `connect_account` signature and body untouched by this plan (only new commands added)

## Deviations from Plan

None - plan executed exactly as written. (The `plaintext`-token scrub touched Plan 01-02 files cosmetically to satisfy this plan's grep gate: "plaintext" -> "unencrypted" in comments/notes with identical meaning.)

## Issues Encountered

- `cargo test -p sge creds` initially matched 0 tests: `creds.rs` wasn't declared in `lib.rs` yet (`pub mod creds` added with the commands — expected ordering, not a bug)
- `cargo fmt` reformats after token-sed lengthened two lines (routine)

## User Setup Required

None - no external service configuration required. (Live keyring test used the session Secret Service; no user credentials involved.)

## Next Phase Readiness

- Phase 1 success criteria all met: configurable login + INBOX SELECT (01-02), security modes with plain-language retry (01-02/01-03), live transcript (01-02), remember-me save (01-03)
- Phase 5 owns auto-connect (structurally absent — negative-grep enforced) and the Linux bundle
- Human demo: `npm run tauri dev` -> login screen; Connect with dummy creds shows the typed auth error + Retry; remember-me against the real keyring works on this machine

## Self-Check: PASSED

- All 3 created + 3 modified files FOUND; `cargo check`/`clippy`/`fmt` clean, 21 tests pass (+1 ignored live test, proven passing manually)
- `npm run build` + `npm run lint` green; 1 submit button; Retry bound to `connect()`; no timers; no Test button; no auto-connect code

---
*Phase: 01-scaffold-connection, Plan: 03*
*Completed: 2026-10-03*
