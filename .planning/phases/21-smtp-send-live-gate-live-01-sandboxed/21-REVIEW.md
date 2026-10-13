---
phase: 21-smtp-send-live-gate-live-01-sandboxed
reviewed: 2026-10-13T00:00:00Z
depth: standard
files_reviewed: 4
files_reviewed_list:
  - src-tauri/src/bin/live_gate_21.rs
  - src-tauri/tests/live_gate_21_safety.rs
  - scripts/live-21.sh
  - src-tauri/Cargo.toml
findings:
  critical: 0
  warning: 4
  info: 4
  total: 8
status: issues_found
---

# Phase 21: Code Review Report

**Reviewed:** 2026-10-13T00:00:00Z
**Depth:** standard
**Files Reviewed:** 4
**Status:** issues_found

## Summary

Reviewed the Phase 21 verification-only live-gate harness (3707-line bin,
1010-line offline safety suite, bash wrapper, `[[bin]]` registration). No
production code changed. The high-risk contracts hold up well under tracing:
recipient guard aborts before every `enqueue_send` (STEP 2 gate plus per-leg
re-checks at bin lines 1643, 1949, 2152, 2403), credentials are env-only with
`--password`/`-p` argv refusal (exit 2), the `TimeoutTransport` fault emits
`Uncertain` with provably zero inner calls, the scan uses fixed-substring
matching with empty-secret guards, and cleanup is retry-then-manual with
tracked-UID-only expunges. No hardcoded secrets, no dangerous functions, no
debug artifacts found. Four warnings (one misleading cleanup report, one
latent wrong-folder expunge fallback, one ps-visible secret in the wrapper,
blocking sleeps in async code) and four info items below. No finding blocks
the human live run, but WR-01 and WR-02 touch the destructive-cleanup path
and should be fixed before live execution.

## Warnings

### WR-01: Cleanup prints a success-shaped detail when expunge/re-SEARCH failed but LIST is clean

**File:** `src-tauri/src/bin/live_gate_21.rs:3323-3338`
**Issue:** The closing `log_step(TOTAL_STEPS, "cleanup", ok && clean, detail)`
computes the pass flag correctly, but the `detail` branch ignores `ok`: when
an `expunge_tracked` or post-cleanup re-SEARCH fails (`ok == false`) yet the
folder LIST is unchanged (`clean == true`), the detail claims `tracked uids
expunged, LIST matches pre-run snapshot (mids ...)` — asserting the exact
thing that failed. Worse, `manual_cleanup_steps(tracked)` (the Message-IDs,
UIDs, folders, and webmail path the operator needs) is rendered only in the
`else` (`!clean`) arm, so it is withheld precisely in the expunge-failure
case where the operator needs it most. The exit code still returns 1, but the
human-readable STEP line says cleanup succeeded.
**Fix:**
```rust
} else if clean && ok {
    format!(
        "tracked uids expunged, LIST matches pre-run snapshot (mids {:?})",
        tracked.message_ids
    )
} else {
    manual_cleanup_steps(tracked)
},
```

### WR-02: Missing Sent wire falls back to "INBOX" for Sent-UID expunge

**File:** `src-tauri/src/bin/live_gate_21.rs:3222`
**Issue:** `let sent_folder = tracked.sent_wire.clone().unwrap_or_else(|| "INBOX".to_string());`
When preflight never resolved the Sent wire, cleanup expunges `tracked.sent_uids`
and re-SEARCHes "Sent" through the INBOX wire. UIDs are per-folder (the
T-21-05 invariant this same file documents at line 1210-1212), so an INBOX
expunge of a Sent UID deletes the wrong mail. Currently latent: every early
abort path reaches `cleanup` with both `sent_uids` and `message_ids` empty,
so the fallback loops execute zero iterations. But a future leg that records
a Sent UID before `sent_wire` is set (or a refactor that reorders preflight)
turns this silent fallback into cross-folder deletion.
**Fix:** Fail closed — skip Sent cleanup and force the manual path when the
wire is unknown:
```rust
let Some(sent_folder) = tracked.sent_wire.clone() else {
    log_step(tracked, TOTAL_STEPS, "cleanup", false,
        &format!("Sent wire unknown — refusing INBOX fallback; {}", manual_cleanup_steps(tracked)));
    // ... still run REPORT + temp-file removal, return 1
};
```

### WR-03: Wrapper passes live passwords as `grep` argv (visible via `ps`)

**File:** `scripts/live-21.sh:89-93`
**Issue:** The SCAN stage runs `grep -F -q "$secret" "$LOG"` with each full
password value as a command-line argument. Command lines are world-readable
via `/proc/<pid>/cmdline` while the process runs, so any local co-tenant
snapshotting the process table during the scan learns the credential. This
directly contradicts the harness's own stated rule — the bin refuses
`--password`/`-p` with the message `argv is visible via ps` (bin line 3389)
— yet the wrapper reintroduces the identical exposure for the scan. Short
lived and single-user-desktop scoped, hence Warning rather than Critical.
**Fix:** Keep secrets out of argv, e.g. feed the needle via stdin or an
env-fed process substitution instead of a pattern argument:
```bash
for var in SGE_LIVE_IMAP_PASS SGE_LIVE_SMTP_PASS; do
    secret="${!var:-}"
    if [ -n "$secret" ] && printf '%s' "$secret" | grep -F -q -f /dev/stdin "$LOG"; then
        SCAN_FAIL="${SCAN_FAIL} log-leak;"
    fi
done
```
(Note: `grep -f file` reads patterns from the file, keeping the secret out
of `/proc` cmdline. Verify `-f /dev/stdin` portability for the target
platform, or write the needle to a `mktemp` file with `0600` and unlink it.)

### WR-04: Blocking `std::thread::sleep` inside `async` cleanup paths

**File:** `src-tauri/src/bin/live_gate_21.rs:1044,3258`
**Issue:** `expunge_tracked` (line 1044) and the sandbox-folder DELETE retry
loop (line 3258) call `std::thread::sleep` for 5s/15s/45s backoffs while
executing on the `async-std` executor thread. Each backoff parks an executor
thread (worst case 65s per UID/folder across the 3 attempts), stalling all
other tasks scheduled on that thread. The rest of the harness already uses
`async_std::task::sleep(...).await` (e.g. the arrival poll at line 749).
**Fix:**
```rust
async_std::task::sleep(std::time::Duration::from_secs(*secs)).await;
```

## Info

### IN-01: `flags_contain_seen` substring-matches "unseen"

**File:** `src-tauri/src/bin/live_gate_21.rs:161-162`
**Issue:** `flags.to_ascii_lowercase().contains("seen")` returns true for any
flag string containing the substring, e.g. a hypothetical `Unseen` keyword or
`$NotSeen`. Real IMAP system flags never contain that substring, so this is a
latent precision wart, not a live-path bug.
**Fix:** Parse on flag-token boundaries instead of substring search, e.g.
split on non-alphanumeric/ `\` characters and compare whole tokens to
`\seen`/`seen`.

### IN-02: Predictable `/tmp` log path without exclusive creation

**File:** `scripts/live-21.sh:26-28`
**Issue:** `LOG="/tmp/sge-live21-$$.log"` is predictable and `tee` follows
symlinks, so a local attacker (or a stale run) pre-creating that path as a
symlink can redirect the harness log (log-content injection) or clobber the
symlink target. Single-user-desktop scoped.
**Fix:** `LOG=$(mktemp /tmp/sge-live21-XXXXXX.log)` and quote as today.

### IN-03: Wrapper subject-prefix sweep passes vacuously with zero subject lines

**File:** `scripts/live-21.sh:62-65`
**Issue:** `UNPREFIXED=$(grep "^AUDIT-SUBJECT " "$LOG" | grep -v -F "SGE-TEST-" || true)`
is empty — and the audit PASSES — when the log contains no `AUDIT-SUBJECT`
lines at all (e.g. a run that sent nothing, or a future refactor that drops
the line). The `ROW_TOTAL -eq 0` guard covers `AUDIT-ROW`, but there is no
symmetric `SUBJECT_TOTAL -eq 0` fail for Rule 2.
**Fix:** Count subject lines and fail when zero alongside a non-SKIP run,
mirroring the `ROW_TOTAL` check.

### IN-04: `FakeSession` double ignores the selected mailbox (fidelity caveat)

**File:** `src-tauri/tests/live_gate_21_safety.rs:76-82,248-264`
**Issue:** `search_uids` and `uid_search_header` scan all stored messages
regardless of the selected mailbox, so an offline test could pass a
wrong-folder SEARCH that would fail (or mis-resolve) against the real
server. The live legs select per-wire leases, so this gap is test-only, but
it weakens the doubles as a guard against folder-scoping regressions of
exactly the T-21-05 kind this harness cares about.
**Fix:** Filter `messages` by `self.selected` in `search_uids` /
`uid_search_header` (and record the mailbox at `append_message`, as today).

---

_Verified clean (spot-checked, no findings filed): argv `--password=x` form
falls into the unknown-flag exit-2 arm; `keyring_or_env` falls back to env
only on username mismatch; `expected_calls_delta` unknown-leg `None` fails
closed in both REPORT and wrapper; temp DB is scanned before removal and
removed after REPORT; `Cargo.toml` change is the `[[bin]] live_gate_21`
registration only; wrapper propagates `PIPESTATUS[0]` with audits skipped
exactly on the exit-2/`--seed-only` paths._

_Reviewed: 2026-10-13T00:00:00Z_
_Reviewer: the agent (gsd-code-reviewer)_
_Depth: standard_
