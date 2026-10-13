#!/usr/bin/env bash
# live-26.sh — Phase 26 sandboxed live-gate wrapper (D-36/D-37/D-44/D-50).
#
# Sources the git-ignored env file as env vars (never CLI args), optionally
# seeds the OS keyring once, runs the live_gate_26 bin capturing a /tmp log,
# scans the log + SQLite dump for credential substrings, verifies the
# replay-before-sweep order evidence plus the per-gate REPORT rows, retries
# bin cleanup on the same backoff schedule, and propagates the bin exit code.
#
# EXIT: 0 all-pass · 1 assertion FAIL or secret-leak · 2 pre-flight skip.
set -uo pipefail

ENV_FILE="src-tauri/.live-test.env"
if [ ! -r "$ENV_FILE" ]; then
    echo "SKIP: $ENV_FILE missing or unreadable — copy src-tauri/.live-test.env.example and fill it (never committed)"
    exit 2
fi

# D-44: values travel as env vars, never argv. Tracing stays OFF while
# secrets are in scope (a `set -x` would print them into the log).
set -a
# shellcheck source=/dev/null
. "$ENV_FILE"
set +a

# --seed-only short-circuit: seed the keyring, no IMAP traffic, no scan.
if [ "${1:-}" = "--seed-only" ]; then
    cargo run --manifest-path src-tauri/Cargo.toml --bin live_gate_26 --seed-only 2>&1
    CODE="$?"
    unset SGE_LIVE_IMAP_PASS SGE_LIVE_SMTP_PASS || true
    exit "$CODE"
fi

LOG="/tmp/sge-live26-$$.log"
cargo run --manifest-path src-tauri/Cargo.toml --bin live_gate_26 "$@" 2>&1 | tee "$LOG"
CODE="${PIPESTATUS[0]}"

# The bin prints `DUMP <path>` for the temp-Store dump it wrote pre-cleanup.
DUMP="$(grep -E '^DUMP ' "$LOG" | tail -1 | awk '{print $2}' || true)"
if [ -z "${DUMP:-}" ] || [ ! -r "${DUMP:-}" ]; then
    # Fall back to the newest dump from this PID family (bin always names
    # dumps sge-live26-<pid>-<ts>.dump).
    DUMP="$(ls -t /tmp/sge-live26-*.dump 2>/dev/null | head -1 || true)"
fi

# D-50: fail the run if the log or dump carries a credential substring.
# Fixed-substring grep (-F) — passwords contain regex metachars. Secrets are
# read ONCE into shell vars here, then unset before any further echo.
# SECRET-LEAK names WHICH artifact matched, never the secret itself.
LEAKED=""
for secret in "${SGE_LIVE_IMAP_PASS:-}" "${SGE_LIVE_SMTP_PASS:-}" "${SGE_LIVE_IMAP_USER:-}"; do
    if [ -n "$secret" ]; then
        if grep -F -q "$secret" "$LOG" 2>/dev/null; then
            LEAKED="log"
        fi
        if [ -n "${DUMP:-}" ] && [ -r "$DUMP" ] && grep -F -q "$secret" "$DUMP" 2>/dev/null; then
            LEAKED="dump"
        fi
    fi
done
if [ -n "$LEAKED" ]; then
    echo "SECRET-LEAK: credential substring found in $LEAKED artifact (see which artifact matched, never the secret)"
    CODE=1
fi
unset SGE_LIVE_IMAP_PASS SGE_LIVE_SMTP_PASS || true

# D-50 order evidence: when a replay-bearing STEP passed (4 seen-roundtrip
# via replay_outbox, 7 reconnect-recovery via the full pass), the captured
# log must contain the worker's own `imap replay:` line (emitted at Step 4b,
# structurally before the sweep code runs) — file order is the evidence.
for N in 4 7; do
    if grep -q "STEP $N/9 .*: PASS" "$LOG" 2>/dev/null; then
        if ! grep -q "imap replay:" "$LOG" 2>/dev/null; then
            echo "ORDER-FAIL: STEP $N passed but no 'imap replay:' line in $LOG (pre-sweep evidence missing)"
            CODE=1
        fi
    fi
done

# D-50 in-bin scan evidence: the bin must have printed its SCAN-DB verdict
# over the dump (password-substring scan, usernames excluded to avoid
# self-address false positives — see bin comment).
if grep -q "^STEP " "$LOG" 2>/dev/null; then
    if grep -q "^SCAN-DB .* FAIL" "$LOG" 2>/dev/null; then
        echo "SCAN-DB run: FAIL (see bin SCAN-DB line above)"
        CODE=1
    elif ! grep -q "^SCAN-DB .* PASS" "$LOG" 2>/dev/null; then
        echo "SCAN-DB run: FAIL (bin SCAN-DB verdict missing)"
        CODE=1
    fi
fi

# D-46 REPORT rows: every live leg (gates 6/7/8 = STEPs 4-7) must emit its
# one-line REPORT row. A gate without a row is an incomplete run.
for GATE in seen-roundtrip tree-browse-badges arrival-tick reconnect-recovery; do
    if grep -q "STEP .*/9 $GATE: PASS" "$LOG" 2>/dev/null; then
        if ! grep -q "^REPORT gate=$GATE verdict=PASS" "$LOG" 2>/dev/null; then
            echo "REPORT-FAIL: STEP $GATE passed but its REPORT row is missing"
            CODE=1
        fi
    fi
done

# T-26-06 PEEK gate: no non-PEEK BODY section may appear in the transcript.
# The harness only ever FETCHes via BODY.PEEK[]; any other BODY[ section
# would set \Seen as a side effect and corrupt the gate-6 asserts.
if grep -E "BODY\[" "$LOG" 2>/dev/null | grep -v "BODY.PEEK" | grep -q .; then
    echo "PEEK-FAIL: non-PEEK BODY section in transcript (Seen side-effect risk)"
    CODE=1
fi

# D-37 wrapper retry: on MANUAL-CLEANUP, retry the bin's --cleanup-only for
# the exact leftover folder on the same 5s/15s/45s schedule before
# surfacing the manual steps. The folder name is parsed from the bin's
# `MANUAL-CLEANUP folder=<name>:` line (never re-derived).
if grep -q "MANUAL-CLEANUP" "$LOG" 2>/dev/null; then
    LEFTOVER="$(grep -o 'folder=SGE-Test-[^ :]*' "$LOG" | head -1 | cut -d= -f2 || true)"
    if [ -n "${LEFTOVER:-}" ]; then
        for WAIT in 5 15 45; do
            echo "wrapper cleanup-retry: waiting ${WAIT}s then --cleanup-only $LEFTOVER"
            sleep "$WAIT"
            # Re-source env for the retry (secrets were unset above).
            set -a
            # shellcheck source=/dev/null
            . "$ENV_FILE"
            set +a
            cargo run --manifest-path src-tauri/Cargo.toml --bin live_gate_26 --cleanup-only "$LEFTOVER" 2>&1 | tee -a "$LOG"
            RETRY_CODE="${PIPESTATUS[0]}"
            unset SGE_LIVE_IMAP_PASS SGE_LIVE_SMTP_PASS || true
            if [ "$RETRY_CODE" = 0 ]; then
                echo "wrapper cleanup-retry: $LEFTOVER removed"
                # The run itself passed — only cleanup needed a retry.
                if ! grep -q ": FAIL" "$LOG" 2>/dev/null; then
                    CODE=0
                fi
                break
            fi
        done
    fi
    if grep -q "MANUAL-CLEANUP" "$LOG" 2>/dev/null && ! grep -q "cleanup-retry: .* removed" "$LOG" 2>/dev/null; then
        echo "MANUAL STEPS REQUIRED (cleanup retries exhausted):"
        grep "MANUAL-CLEANUP" "$LOG" | tail -2
        CODE=1
    fi
fi

# Run report tail (the bin prints per-STEP lines + REPORT; the wrapper adds
# the scan/audit verdicts — all in the /tmp log, never the repo).
echo "SCAN-VERDICT: $([ "$CODE" = 0 ] && echo PASS || echo CHECK-LOG) log=$LOG dump=${DUMP:-none}"

unset SGE_LIVE_IMAP_USER SGE_LIVE_SELF || true
exit "$CODE"
