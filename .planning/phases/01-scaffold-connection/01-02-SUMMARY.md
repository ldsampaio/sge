---
phase: 01-scaffold-connection
plan: "02"
subsystem: infra
tags: [imap, async-imap, tls, native-tls, utfpr, dovecot, cli-harness]

# Dependency graph
requires:
  - phase: 01-01
    provides: [Tauri shell, frozen connect_account signature, pinned crates]
provides:
  - Real 3-mode IMAP session (implicit TLS / STARTTLS / plain-local) with 30 s timeout
  - Typed plain-language errors naming host vs credentials vs TLS
  - imap_probe CLI harness sharing the exact probe core with the Tauri command
  - Mixed-source probe transcript fixture with live Dovecot server profile
  - Documented imap-proto NAMESPACE parser gap + regression tripwire test
affects: [01-03-login-ux, sync-engine, local-store]

# Actuals (#2632) — pairs with the plan's `estimate` to calibrate future estimates.
actuals:
  tokens: 0
  tasks: 3
  commits: 2

# Tech tracking
tech-stack:
  added: [native-tls 0.2 (direct, for error mapping)]
  patterns: [spawn_blocking + async-std block_on sync thread, raw-command transcript drive, manual STARTTLS upgrade]

key-files:
  created: [src-tauri/src/imap/mod.rs, src-tauri/src/imap/errors.rs, src-tauri/src/imap/session.rs, src-tauri/src/imap/probe.rs, src-tauri/src/bin/imap_probe.rs, fixtures/probe-transcript.txt, fixtures/stub_server.py]
  modified: [src-tauri/src/lib.rs, src-tauri/Cargo.toml, src-tauri/Cargo.lock]

key-decisions:
  - "NAMESPACE command NOT issued: imap-proto 0.16 cannot parse NAMESPACE responses and async-imap poisons the session on any parse failure; profile derived from CAPABILITY + LIST"
  - "Cert-exception flag is plumbed but always refused loudly (never bypassed); grep gate stays empty"
  - "Transcript redacts password only (username left intact; single-char names annihilated text)"
  - "connect_account went async (IPC-invisible); signature params/return unchanged"

patterns-established:
  - "probe::run_probe is the single shared core: Tauri command and CLI are thin callers"
  - "Every ImapError names the failing part for verbatim UI surfacing; passwords never in strings"
  - "M1 read-only: SELECT + capability/mailbox probes only; grep gates enforce no STORE/BODY fetch"

requirements-completed: [CONN-01, CONN-02]

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Three security modes connect with correct handshakes; failures map to plain-language host/credentials/TLS errors"
    requirement: "CONN-01"
    verification:
      - kind: unit
        ref: "cargo test -p sge imap:: (18 tests incl. error mapping, timeout const, cert refusal, plain-local guard)"
        status: pass
      - kind: integration
        ref: "live: implicit-TLS handshake + greeting + CAPABILITY vs mail.utfpr.edu.br:993; live auth-rejection with dummy user; stub: full LOGIN..SELECT..SUMMARY round-trip"
        status: pass
    human_judgment: false
  - id: D2
    description: "imap_probe CLI harness runs the full probe through shared code and prints a mailbox summary"
    requirement: "CONN-02"
    verification:
      - kind: other
        ref: "cargo run --bin imap_probe -- --help exits 0; stub run prints SUMMARY selected_mailbox=INBOX exists=3 uid_validity=12345"
        status: pass
    human_judgment: false
  - id: D3
    description: "Probe transcript fixture with server profile header, no secret bytes, honest source marking"
    requirement: "CONN-02"
    verification:
      - kind: other
        ref: "grep counts CAPABILITY/LIST/SELECT/NAMESPACE all nonzero; negative grep for both dummy passwords empty; SOURCE: MIXED header"
        status: pass
    human_judgment: false

# Metrics
duration: 2h10min
completed: 2026-10-03
status: complete
---

# Phase 1 Plan 2: Connection Core Summary

**Real 3-mode IMAP session with typed plain-language errors, CLI probe harness, and a mixed live+stub transcript fixture — including a parser-gap find that saved the live probe**

## Performance

- **Duration:** ~2h10min (dominated by a real async-imap debugging saga, see below)
- **Started:** 2026-10-03T02:05Z
- **Completed:** 2026-10-03T04:15Z
- **Tasks:** 3
- **Files modified:** 9

## Accomplishments

- `imap::{mod,errors,session,probe}` module: ImplicitTls/StartTls/PlainLocal transports over async-imap 0.11 + async-native-tls (system CA store), 30 s whole-probe timeout, dedicated blocking thread
- Typed `ImapError` (Unreachable / Timeout / AuthRejected / TlsUntrusted / CertExceptionRefused / Protocol / SelectFailed), each naming the failing part; passwords never in strings (Debug redacts, transcript scrubs)
- `connect_account` keeps its frozen signature and now delegates to `probe::run_probe` via `spawn_blocking`
- `imap_probe` CLI: `--host/--port/--mode/--username/--password|SGE_IMAP_PASSWORD/--allow-untrusted/--allow-plain-local/--pre-auth-only/--help`, exit codes 0/1/2, invocation documented
- Fixture `probe-transcript.txt`: LIVE Dovecot profile (pre-auth caps from mail.utfpr.edu.br:993) + LOCAL-STUB authenticated exchange (real client bytes vs `stub_server.py`) with honest `SOURCE: MIXED` header and re-probe instructions
- Live findings: 993-only server (port 143 times out — STARTTLS path ships untested-live), dummy-user login correctly rejected with the credentials error

## Task Commits

Plan committed as feat + docs pair (stable hashes; no amend-chasing):

1. **IMAP session + errors + probe + CLI + fixture** - `ccf9b26` (feat)
2. **Plan 01-02 summary** - `(this docs commit)` (docs)

## Files Created/Modified

- `src-tauri/src/imap/mod.rs` - SecurityMode, AccountConfig (Debug-redacted), MailboxSummary, Transcript, timeout const + unit tests
- `src-tauri/src/imap/errors.rs` - ImapError + io/TLS/login mapping + sniff classifier + unit tests
- `src-tauri/src/imap/session.rs` - 3 transports, manual STARTTLS upgrade, shared drive (CAPABILITY/LIST/STATUS/SELECT/LOGOUT), policy guards + regression test
- `src-tauri/src/imap/probe.rs` - `run_probe` / `run_pre_auth_probe` shared core with 30 s timeout
- `src-tauri/src/bin/imap_probe.rs` - CLI harness (documented above)
- `src-tauri/src/lib.rs` - `connect_account` delegates (now async; IPC signature unchanged)
- `src-tauri/Cargo.toml` - added `native-tls 0.2` direct dep for error mapping
- `.planning/.../fixtures/probe-transcript.txt` - mixed-source fixture
- `.planning/.../fixtures/stub_server.py` - loopback stub (never logs LOGIN)

## Decisions Made

- Cert-exception override (`--allow-untrusted`) is plumbed end to end but always refused with a loud warning: verification is never bypassed, so the `danger_accept_invalid` grep gate stays empty by construction
- Transcript redacts the password only; username redaction was removed after it annihilated readable text for short names (username appears in error strings by design anyway)
- `connect_account` became `async fn` to await `spawn_blocking` (IPC-invisible; params/return identical)
- Plain-local in `connect_account` auto-confirms only for loopback hosts; remote plaintext is always refused

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] NAMESPACE command not issued (parser gap poisons sessions)**
- **Found during:** Task 3 (fixture capture — full probe died after NAMESPACE against the stub)
- **Issue:** imap-proto 0.16 has NO RFC 2342 NAMESPACE response parser, and async-imap permanently closes a session's read side after ANY response parse failure (`ImapStream::read_closed`: later reads return `None` instantly while writes keep working). Issuing NAMESPACE against any server that supports it kills the session in a silent cascade (writes pipeline, reads EOF, misleading downstream errors). Diagnosed via a timestamped TCP relay after a long bisect; the failure would have hit the live Dovecot server too.
- **Fix:** No NAMESPACE command issued. Namespace profile derived from post-login CAPABILITY (advertised or not) + LIST delimiter, recorded as transcript notes. Added regression tripwire `namespace_response_is_unparseable` (fails the day imap-proto learns NAMESPACE, signalling re-enablement).
- **Files modified:** `src-tauri/src/imap/session.rs`
- **Verification:** 18/18 `cargo test -p sge imap::` pass incl. the new test; stub full-probe round-trips LOGIN..SELECT..SUMMARY cleanly
- **Committed in:** ccf9b26 (part of plan commit)

**2. [Rule 3 - Blocking] Added direct `native-tls 0.2` dependency**
- **Found during:** Task 1 (session + errors)
- **Issue:** `errors.rs` maps `native_tls::Error` (handshake failures are always TLS-layer by construction), but `native-tls` was only transitive via async-native-tls — direct use needs a direct dep.
- **Fix:** `cargo add native-tls@0.2` (matches locked 0.2.18).
- **Files modified:** `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`
- **Verification:** `cargo check` clean
- **Committed in:** ccf9b26 (part of plan commit)

---

**Total deviations:** 2 auto-fixed (1 bug, 1 blocking)
**Impact on plan:** Deviation 1 is the highest-value find of the phase — it prevents a guaranteed live-server failure and constrains Phase 2's sync design (unparseable responses are fatal-session; reconnect, don't retry in place). No scope creep.

## Issues Encountered

- Debugging the NAMESPACE poison took ~1h (symptoms mimicked a dozen other causes: buffered writes, tag mismatches, stale binaries). A timestamped TCP relay (`relay2.py`, throwaway in /tmp) cracked it: client pipelined all commands while reads instantly EOF'd — the read_closed signature.
- `pkill -f <pattern>` twice killed my own shell (pattern matched the command line). Used bracket-free alternatives afterward.
- `create-tauri-app` lesson carried over: none (scaffold done in 01-01).

## User Setup Required

None - no external service configuration required. (Real-user credentials intentionally NOT used; dummy-only probes against the live server.)

## Next Phase Readiness

- Plan 01-03 builds the full login UX + keyring save on top of this core; no blockers
- Phase 2 must know: (a) server is Dovecot, 993-only, AUTH=PLAIN post-login (re-probe live for the post-login set); (b) imap-proto parse failures are fatal-session — sync engine must reconnect on Protocol errors; (c) STARTTLS path needs a live 143 server to verify (UTFPR has none)
- CLI demo for the human: `SGE_IMAP_PASSWORD=... ./src-tauri/target/debug/imap_probe --host mail.utfpr.edu.br --username <user>` prints transcript + SUMMARY (or the plain-language failure)

## Self-Check: PASSED

- All 7 created files + lib.rs FOUND on disk; `cargo check`/`clippy`/`fmt` clean, 18 unit tests pass
- `imap_probe --help` exits 0; live handshake + live auth-rejection + stub full-probe all demonstrated this session
- Fixture greps: CAPABILITY=9, LIST=7, SELECT=4, NAMESPACE=9; both dummy passwords absent

---
*Phase: 01-scaffold-connection, Plan: 02*
*Completed: 2026-10-03*
