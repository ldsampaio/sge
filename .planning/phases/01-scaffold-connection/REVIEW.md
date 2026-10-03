---
phase: 01-scaffold-connection
reviewed: 2026-10-03T05:30:00Z
depth: standard
files_reviewed: 16
files_reviewed_list:
  - src-tauri/src/lib.rs
  - src-tauri/src/main.rs
  - src-tauri/src/creds.rs
  - src-tauri/src/imap/mod.rs
  - src-tauri/src/imap/errors.rs
  - src-tauri/src/imap/session.rs
  - src-tauri/src/imap/probe.rs
  - src-tauri/src/bin/imap_probe.rs
  - src/components/LoginForm.tsx
  - src/components/SecuritySelector.tsx
  - src/App.tsx
  - src-tauri/tauri.conf.json
  - src-tauri/capabilities/default.json
  - src-tauri/Cargo.toml
  - .github/workflows/ci.yml
  - .planning/phases/01-scaffold-connection/fixtures/stub_server.py
findings:
  critical: 0
  warning: 11
  info: 7
  total: 18
status: issues_found
---

# Phase 01: Code Review Report

**Reviewed:** 2026-10-03T05:30:00Z
**Depth:** standard
**Files Reviewed:** 16
**Status:** issues_found

## Summary

Reviewed the full Phase 1 scope (commits f664075..1608a10): Tauri scaffold + IPC
contract, 3-mode IMAP session with typed errors, shared probe core + CLI harness,
keyring save slice, login UI, CI, and fixture stub. The architecture is sound —
frozen `connect_account` signature, shared `probe::run_probe` core, fail-closed TLS
(no bypass exists; `danger_accept` grep is clean), read-only M1 invariant holds
(no STORE/BODY fetch anywhere), and the NAMESPACE parser-gap handling is correct.

No Critical (ship-blocking) defects: nothing crashes, leaks secrets remotely, or
loses data. But 11 Warnings merit fixes: a local credential disclosure via CLI
argv, a credential-lifecycle gap (saved secrets can never be revoked from the UI),
input-normalization holes at the host/port trust boundary the threat model claims
is validated, an unverified-live STARTTLS path, misleading error mapping, an
uncapped transcript read, missing WebView CSP, CI that never runs the 21+ tests it
claims as gates, un-zeroized password material, and a bundle target contradicting
the Linux-only constraint.

## Warnings

### WR-01: CLI `--password` flag exposes the password in the process table

**File:** `src-tauri/src/bin/imap_probe.rs:78,148-153`
**Issue:** The full probe accepts `--password PASS` on the command line. On Linux,
`/proc/<pid>/cmdline` is world-readable, so any local user running `ps` during the
probe window sees the real mailbox password. The USAGE text nudges toward
`SGE_IMAP_PASSWORD`, but the flag remains a footgun for a credential-bearing tool.
**Fix:**
```rust
// Reject --password outright, or at minimum warn; env-only:
// "--password" => {
//     eprintln!("refusing: pass the password via SGE_IMAP_PASSWORD (argv is visible via ps)");
//     return ExitCode::from(2);
// }
```

### WR-02: Saved credentials can never be revoked from the UI; opt-out leaves stale secrets on disk

**File:** `src/components/LoginForm.tsx:87-99`
**Issue:** Two compounding gaps: (a) `clear_credentials` exists in `lib.rs` but no
UI element ever calls it — once saved, the user has no in-app way to forget the
account. (b) With remember-me unchecked the keyring is deliberately untouched, so
a user who previously saved and now unchecks still has the old entry reloaded (and
`rememberMe` re-checked) on next launch. CONTEXT promises "without it, credentials
are memory-only" — false for any returning user with a stale entry.
**Fix:**
```tsx
// On successful connect with rememberMe unchecked, clear any prior entry:
await invoke("clear_credentials");
// And add a "Forget saved login" button calling clear_credentials.
```

### WR-03: Backend never normalizes `host` — untrimmed value reaches TCP/TLS/SNI; `host:port` paste becomes a bogus hostname

**File:** `src-tauri/src/lib.rs:51-62`, `src-tauri/src/imap/session.rs:53-59`
**Issue:** Validation checks `host.trim().is_empty()` but the raw `host` (with
leading/trailing whitespace) flows into `socket_addr`, DNS, TLS SNI/hostname
verification, and error strings. Worse, a user pasting `mail.utfpr.edu.br:993`
into the server field hits `socket_addr`'s colon branch and dials
`[mail.utfpr.edu.br:993]:993` — DNS fails and the user gets a misleading "cannot
reach" error. The threat model (T-plan) claims "validate host/port ranges in Rust,
never trust frontend" — the validation is thinner than claimed. Same untrimmed
pass-through exists in the CLI (`imap_probe.rs:96-102`).
**Fix:**
```rust
let host = host.trim().to_string();
let host = host.rsplit_once(':') /* only if trailing numeric port */ ...;
// At minimum: trim, and split off a trailing :port to either use it or reject plainly.
```

### WR-04: Frontend port can become `NaN`/out-of-range; backend `u16` deserialization then fails opaquely

**File:** `src/components/SecuritySelector.tsx:79`, `src/components/LoginForm.tsx:79-85`, `src-tauri/src/lib.rs:54`
**Issue:** Clearing the number input yields `Number("")` → `0` (handled as
`InvalidPort`), but a non-numeric state yields `NaN` → serialized as `null` →
Tauri/serde rejects `u16` with a technical deserialization error, bypassing the
locked "plain-language error naming the failing part" UX. Negative or `>65535`
values fail the same way. The `InvalidPort` guard only catches `0`.
**Fix:**
```tsx
// Clamp/validate in the component before invoke:
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  setStatus({ kind: "error", message: "Port must be a number between 1 and 65535." });
  return;
}
```

### WR-05: STARTTLS handshake path ships without live verification against any server

**File:** `src-tauri/src/imap/session.rs:248-291`
**Issue:** The fixture honestly records that `mail.utfpr.edu.br:143` times out
(993-only server), so `starttls_upgrade` — the most handshake-complex code in the
phase, including the "stray greeting" assumption in `establish_starttls` — has never
run against a real server. A wrong assumption here (e.g., async-imap choking on a
post-STARTTLS greeting) fails only in users' hands. Security-critical and
untested-live is exactly what this phase was supposed to eliminate.
**Fix:** Verify against a local STARTTLS-capable stub (extend `stub_server.py`
with a STARTTLS upgrade branch) and record the transcript; or mark the mode
experimental in the UI until a live 143 server is available.

### WR-06: All post-login `NO` responses map to `SelectFailed`, misnaming LIST/STATUS failures

**File:** `src-tauri/src/imap/errors.rs:167-180`
**Issue:** `session_err` converts every `Up::No(detail)` to
`ImapError::SelectFailed`, so a `LIST` or `STATUS` rejection surfaces as "INBOX
could not be selected" — the error names the wrong failing part, contradicting
the D-errors intent the UI relies on verbatim.
**Fix:**
```rust
// Thread the operation name through, e.g. session_err(err, host, port, op: "LIST")
// and format "{op} failed ({detail})" instead of forcing SelectFailed.
```

### WR-07: `read_until_tag` accumulates unbounded lines within the 30 s window

**File:** `src-tauri/src/imap/session.rs:200-219`
**Issue:** The pre-auth/STARTTLS raw-line loop pushes every server line into a
`Vec<String>` with no line-count, line-length, or byte cap. A rogue or compromised
server can stuff megabytes of untagged slop into the transcript (kept in memory,
rendered, printed) before the outer timeout fires. The threat model names the
server "untrusted bytes" — this read path trusts it without bound.
**Fix:**
```rust
const MAX_PROBE_LINES: usize = 200;
const MAX_LINE_LEN: usize = 16 * 1024;
// Enforce inside read_until_tag; bail with ImapError::Protocol on excess.
```

### WR-08: WebView ships with `"csp": null` — no Content Security Policy

**File:** `src-tauri/tauri.conf.json:20-22`
**Issue:** The app explicitly disables CSP. Harmless today (local React bundle
only, no remote content, React escapes interpolation), but this is a mail client
one phase away from rendering attacker-controlled email HTML. Establishing a
restrictive CSP now is nearly free and much harder to retrofit later.
**Fix:**
```json
"security": { "csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src ipc: http://ipc.localhost" }
```

### WR-09: CI never runs `cargo test` (or audit) despite the threat model claiming both as gates

**File:** `.github/workflows/ci.yml:48-58`
**Issue:** CI runs fmt/clippy/check/lint/build, but the 21+ unit tests — including
the `namespace_response_is_unparseable` regression tripwire and the error-mapping
suite that underpins the whole failure-UX contract — can rot green-CI. T-01-01
claims "CI runs cargo-audit footprint via pinned tree"; no audit step exists.
**Fix:**
```yaml
- name: Rust tests
  working-directory: src-tauri
  run: cargo test
```

### WR-10: Password material is cloned across threads and never zeroized; lingers in JS state after success

**File:** `src-tauri/src/imap/mod.rs:72-84`, `src-tauri/src/imap/session.rs:312-314`, `src/components/LoginForm.tsx:35-37`
**Issue:** `AccountConfig.password` is `String`-cloned into `login()`, moved into
`spawn_blocking` closures, and kept in React state indefinitely after a successful
connect (no clear, no disconnect path). `Debug` redaction prevents log leaks but
the bytes persist in allocator memory with no `zeroize`. For a credential-handling
app this is accepted as memory-only by CONTEXT, but zeroing on drop is cheap
insurance against core-dump/swap exposure.
**Fix:** Adopt `zeroize::Zeroizing<String>` for `AccountConfig.password` (and clear
the React password state after a remembered save); document the residual risk.

### WR-11: `bundle.targets: "all"` contradicts the Linux-only constraint

**File:** `src-tauri/tauri.conf.json:24-28`
**Issue:** AGENTS.md/CONTEXT lock "Linux-only bundle", but the scaffold default
`targets: "all"` will emit macOS/Windows bundles when bundling runs elsewhere.
Latent today (CI only builds Linux), wrong the first time someone runs
`tauri build` on another OS.
**Fix:**
```json
"targets": ["deb", "appimage"]
```

## Info

### IN-01: Unused `opener` plugin permission widens the IPC surface

**File:** `src-tauri/capabilities/default.json:8`, `src-tauri/src/lib.rs:124`
**Issue:** `opener:default` is granted though no frontend code uses it. Any future
content-execution context inherits URL/app-open capability for free. Scope down to
only the commands used, or drop the plugin until needed.

### IN-02: `is_loopback` allowlist is narrower than the loopback range, and undocumented

**File:** `src-tauri/src/imap/mod.rs:61-64`
**Issue:** Only `localhost`/`127.0.0.1`/`::1` (bracketed `::1` handled) pass; the
rest of `127.0.0.0/8`, `::ffff:127.0.0.1`, and `localhost.` are refused. Fail-closed
is the safe direction, but the boundary is undocumented and will confuse stub
testing on `127.0.0.2`. Document the exact allowlist in the plain-mode warning.

### IN-03: `load_credentials` failure is swallowed silently on launch

**File:** `src/components/LoginForm.tsx:56-58`
**Issue:** The `.catch(() => {})` keeps memory-only defaults with no user-visible
note, so a user expecting remembered credentials just sees empty fields. Surface
a subtle hint ("saved login unavailable — keyring locked?").

### IN-04: `save_credentials` accepts whitespace-only passwords

**File:** `src-tauri/src/lib.rs:92`
**Issue:** Guard is `password.is_empty()` while username is trimmed-checked; `"   "`
passes server-side (frontend blocks only truly-empty). Trim-check both or
document that password whitespace is significant and intended.

### IN-05: `"timed out"` sniff maps to `Unreachable` instead of the `Timeout` variant

**File:** `src-tauri/src/imap/errors.rs:78`
**Issue:** A transport error containing "timed out" with a generic `ErrorKind`
reports "cannot reach … (…timed out…)" rather than the dedicated 30 s `Timeout`
message, so timeout UX is inconsistent depending on which layer timed out.

### IN-06: Scaffold metadata left as template defaults

**File:** `src-tauri/Cargo.toml:6`
**Issue:** `authors = ["you"]`, `description = "A Tauri App"` are create-tauri-app
placeholders. Cosmetic; set real values before any distribution.

### IN-07: Transcript scrub skips passwords shorter than 3 chars

**File:** `src-tauri/src/imap/mod.rs:149-156`
**Issue:** Acknowledged belt-and-suspenders gap: a 1–2 char password is never
redacted from `render`. Risk is theoretical (LOGIN is never echoed), but the
threshold should be documented at the call site or removed in favor of always
scrubbing.

---

_Reviewed: 2026-10-03T05:30:00Z_
_Reviewer: the agent (gsd-code-reviewer)_
_Depth: standard_
