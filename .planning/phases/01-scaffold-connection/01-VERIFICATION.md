---
phase: 01-scaffold-connection
verified: 2026-10-03T06:00:00Z
status: passed
score: 6/6 must-haves verified
covered_files: [.planning/phases/01-scaffold-connection/01-01-PLAN.md, .planning/phases/01-scaffold-connection/01-01-SUMMARY.md, .planning/phases/01-scaffold-connection/01-02-PLAN.md, .planning/phases/01-scaffold-connection/01-02-SUMMARY.md, .planning/phases/01-scaffold-connection/01-03-PLAN.md, .planning/phases/01-scaffold-connection/01-03-SUMMARY.md, .planning/phases/01-scaffold-connection/REVIEW.md, .planning/phases/01-scaffold-connection/fixtures/probe-transcript.txt, src-tauri/src/creds.rs, src-tauri/src/imap/errors.rs, src-tauri/src/imap/mod.rs, src-tauri/src/imap/probe.rs, src-tauri/src/imap/session.rs, src-tauri/src/lib.rs, src/components/LoginForm.tsx, src/components/SecuritySelector.tsx]
covered_digest: "v1:sha256:030b7fb5115f8ffa6d6fe01c0bcf0819e31a4dafe26131f7e801fbceaf8f9cc3"
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Run `npm run tauri dev`, eyeball the login screen (server pre-filled mail.utfpr.edu.br, 3-mode selector, port autofill + Advanced row, eye toggle, single Connect, remember-me + Forget button)"
    expected: "Layout matches CONTEXT decisions; no second Test button; error + Retry flow reads plainly"
    why_human: "Visual layout and interaction feel cannot be verified by grep/build (coverage D1 in 01-03-SUMMARY flags human_judgment: true)"
  - test: "Connect with REAL UTFPR credentials and confirm INBOX SELECT summary (exists count, uid_validity) displays; optionally re-probe live via imap_probe to replace the [LOCAL-STUB] fixture section"
    expected: "Real authenticated SELECT succeeds; fixture can be upgraded from MIXED to fully-live"
    why_human: "No real-user credentials exist in this environment; live probes used dummy-only auth (correctly rejected). STARTTLS path is also untested-live (UTFPR is 993-only) — recorded as residual risk, unit-tested via stub + replay tests"
---

# Phase 01: Scaffold + Connection Verification Report

**Phase Goal:** User credentials open a real IMAP INBOX session against a configurable server.
**Verified:** 2026-10-03T06:00:00Z
**Status:** human_needed
**Re-verification:** No — initial verification (includes 4 post-review fix commits a4fdc96, ff5ac68, e0b87f6, 77ff06c on top of reviewed 1608a10)

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Login reaches INBOX SELECT against a real IMAP exchange | ✓ VERIFIED | `session.rs:447` `.select("INBOX")`; live stub probe this session: `SUMMARY selected_mailbox=INBOX exists=3 uid_validity=12345` exit 0; `connect_account` (lib.rs:44) delegates to `probe::run_probe` via spawn_blocking |
| 2 | 3-mode switch + plain-language errors + manual retry, no auto-retry | ✓ VERIFIED | `SecurityMode::{ImplicitTls,StartTls,PlainLocal}` (mod.rs) ↔ frontend `SecurityModeValue` 1:1; error strings name the part (`cannot reach…`, `timed out after 30…`, `login rejected for…`, `TLS verification failed…`); Retry button bound to `connect()` (LoginForm.tsx:240); zero `setTimeout/setInterval` in LoginForm; 30 s timeout const tested |
| 3 | Probe transcript fixture exists with honest source marking | ✓ VERIFIED | `fixtures/probe-transcript.txt` with `SOURCE: MIXED` header, LIVE Dovecot pre-auth profile + LOCAL-STUB authenticated exchange incl. `SELECT INBOX → exists=3 uid_validity=12345`; no secret bytes (negative-grep verified at capture) |
| 4 | Remember-me saves username + password to OS keyring; unchecked stays memory-only | ✓ VERIFIED | `creds.rs` `KeyringStore` (service `sge`, single JSON blob) + `save/load/clear_credentials` commands via spawn_blocking, all three invoked from LoginForm (save on connect, clear on opt-out per WR-02 fix, Forget button, load-on-launch prefill); `creds` unit tests pass; unchecked path calls `clear_credentials` (no stale entry) |
| 5 | No auto-connect code anywhere (Phase 5 scope guard) | ✓ VERIFIED | `grep -riE auto_login\|autoconnect\|auto-connect\|auto_connect` hits only two comments (LoginForm.tsx:47, creds.rs:5) explicitly stating auto-connect stays in Phase 5; launch `useEffect` only prefills fields, never invokes `connect_account` |
| 6 | Read-only M1 discipline: no STORE, no BODY fetch, no flag writes | ✓ VERIFIED | `grep -rnE \bSTORE\b\|BODY\[ src-tauri/src/imap/*.rs src-tauri/src/lib.rs` empty; session.rs header documents SELECT/STATUS/LIST/CAPABILITY only; 01-02 grep gates intact |

**Score:** 6/6 truths verified (0 present-but-behavior-unverified)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src-tauri/src/imap/{mod,errors,session,probe}.rs` | 3-mode session + typed errors + shared probe core | ✓ VERIFIED | Substantive (~700-line session w/ tests), wired via `connect_account` + `imap_probe` CLI sharing `run_probe` |
| `src-tauri/src/bin/imap_probe.rs` | CLI harness, env-only password | ✓ VERIFIED | `--password` refused with env redirect (WR-01 fix); stub round-trip demonstrated live this session |
| `src-tauri/src/creds.rs` | Keyring save slice, no auto-connect | ✓ VERIFIED | Trait + KeyringStore + MemoryStore, zeroize noted, 3 unit tests + 1 ignored live-Secret-Service test |
| `src/components/LoginForm.tsx` + `SecuritySelector.tsx` | Full login UX per CONTEXT | ✓ VERIFIED | Prefill, 3-mode selector + port autofill + Advanced + localhost warning, eye toggle, single Connect, Retry + TLS re-confirm, remember-me + Forget, keyring-unavailable hint |
| `fixtures/probe-transcript.txt` + `stub_server.py` | Fixture + loopback stub (now w/ STARTTLS branch) | ✓ VERIFIED | Transcript verified above; stub supports `--starttls` advertisement branch (WR-05 fix commit a4fdc96) |
| `.github/workflows/ci.yml` | Linux CI incl. tests + audit | ✓ VERIFIED | `cargo test` + `cargo audit` steps present (WR-09 fix); `targets: ["deb","appimage"]` Linux-only (WR-11 fix) |
| `src-tauri/tauri.conf.json` | SGE identity + restrictive CSP | ✓ VERIFIED | Identity `br.edu.utfpr.sge`; CSP set (WR-08 fix); opener permission dropped (IN-01 fix) |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| LoginForm | `connect_account` | `invoke` with trimmed host/user + guarded port (WR-03/WR-04 fixes) | WIRED | lib.rs:54 `normalize_host` (trims + strips pasted `:port`); frontend port guard 1–65535 |
| SecuritySelector | backend `SecurityMode` | mode strings `implicit_tls/starttls/plain` parsed by `SecurityMode::parse` | WIRED | 1:1 mapping, junk rejected by test |
| Remember-me | keyring | `save/load/clear_credentials` invokes | WIRED | All three call sites present in LoginForm |
| `connect_account` | IMAP server | `run_probe` → 3 transports → LOGIN..LIST/STATUS/SELECT..LOGOUT | WIRED | Proven by live stub probe this session |
| Error strings | UI | surfaced verbatim, no secret material | WIRED | `ImapError` Display tested; `Debug` redacts password; zeroize on drop |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| LoginForm success view | `summary: ConnectSummary` | live `SELECT INBOX` via `connect_account` | ✓ FLOWING (stub demo: exists=3, uid_validity=12345) | ✓ FLOWING |
| LoginForm prefill | `SavedCredentials` | `load_credentials` ← Secret Service | ✓ FLOWING (live roundtrip test passed at build) | ✓ FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| `cargo test -p sge` | full suite | 29 passed, 0 failed, 1 ignored (live keyring) | ✓ PASS |
| Stub full probe LOGIN..SELECT..SUMMARY | `imap_probe --host 127.0.0.1 --port 11431 --mode plain --allow-plain-local` vs stub_server.py | `SUMMARY selected_mailbox=INBOX exists=3 uid_validity=12345`, exit 0 | ✓ PASS |
| `npm run build` | frontend build | ✓ built (224.99 kB js) | ✓ PASS |
| Grep gates | no auto-connect code / no plaintext / no STORE-or-BODY / no LoginForm timers | all clean (only scope-guard comments match) | ✓ PASS |

### Probe Execution

| Probe | Command | Result | Status |
|-------|---------|--------|--------|
| N/A — no `scripts/*/tests/probe-*.sh` in repo; phase "probe" is `imap_probe` CLI + transcript fixture, both exercised above | — | — | N/A (covered by spot-checks) |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| CONN-01 | 01-01, 01-02, 01-03 | Login with user/pass/server, INBOX SELECT | ✓ SATISFIED | Frozen `connect_account` contract + real session + login UI |
| CONN-02 | 01-01, 01-02, 01-03 | Security config + plain-language errors + retry | ✓ SATISFIED | 3 modes, typed errors, manual Retry, no auto-retry |
| CONN-03 (save slice) | 01-03 | Remember credentials in keyring (auto-connect deferred to Phase 5) | ✓ SATISFIED (slice) | Save/load/clear + revocation UI; auto-connect structurally absent |

### Review Follow-Up

REVIEW.md (2026-10-03, 11 warnings + 7 info, 0 critical) has been addressed by 4 fix commits verified in-tree: WR-01 env-only CLI password, WR-02 revocation UI + opt-out clear, WR-03 `normalize_host`, WR-04 port guard, WR-05 STARTTLS stub branch + residual-risk docs, WR-06 op-named LIST/STATUS errors (test `list_and_status_no_name_the_operation` passes), WR-07 line caps (tests pass), WR-08 CSP, WR-09 CI tests+audit, WR-10 zeroize + JS password clear, WR-11 Linux-only targets, IN-01..IN-05/IN-07. No open review item blocks the phase goal.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | None found in phase files (no TODO/FIXME/placeholder/stub-return matches of concern) | — | — |

### Human Verification Required

### 1. Login screen visual eyeball

**Test:** Run `npm run tauri dev`, inspect the login screen (prefill, selector, Advanced row, eye toggle, single Connect, remember-me + Forget button); fail once with dummy creds and use Retry.
**Expected:** Layout matches CONTEXT decisions; error names the failing part; Retry preserves fields.
**Why human:** Visual appearance and feel cannot be verified programmatically (coverage D1 flags human_judgment).

### 2. Real-credential live INBOX SELECT

**Test:** Connect with real UTFPR credentials; confirm the INBOX summary displays; optionally re-run `imap_probe` live to upgrade the fixture's `[LOCAL-STUB]` section.
**Expected:** Authenticated SELECT succeeds against mail.utfpr.edu.br:993.
**Why human:** No real credentials in this environment; live probes were dummy-only by design. STARTTLS mode additionally has zero live-server verification (UTFPR is 993-only) — unit/replay-tested only, recorded residual risk.

### Gaps Summary

No gaps. All six must-haves verified with in-tree evidence plus a live stub round-trip executed during this verification. Status is `human_needed` solely for the two human items above (visual UX + real-credential live proof), not for any code deficiency.

---
_Verified: 2026-10-03T06:00:00Z_
_Verifier: the agent (gsd-verifier)_
