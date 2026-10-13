#!/usr/bin/env bash
# live-22.sh — Phase 22 sandboxed live-gate wrapper (D-13/D-14/D-15/D-03).
#
# Sources the git-ignored env file as env vars (never CLI args), optionally
# seeds the OS keyring once, runs the live_gate_22 bin capturing a /tmp log,
# scans the log for credential substrings, and propagates the bin exit code.
#
# EXIT: 0 all-pass · 1 assertion FAIL or secret-leak · 2 pre-flight skip.
set -uo pipefail

ENV_FILE="src-tauri/.live-test.env"
if [ ! -r "$ENV_FILE" ]; then
    echo "SKIP: $ENV_FILE missing or unreadable — copy src-tauri/.live-test.env.example and fill it (never committed)"
    exit 2
fi

# D-13: values travel as env vars, never argv. Tracing stays OFF while
# secrets are in scope (a `set -x` would print them into the log).
set -a
# shellcheck source=/dev/null
. "$ENV_FILE"
set +a

LOG="/tmp/sge-live22-$$.log"
cargo run --manifest-path src-tauri/Cargo.toml --bin live_gate_22 "$@" 2>&1 | tee "$LOG"
CODE="${PIPESTATUS[0]}"

# D-15 (tracer slice): fail the run if the log carries a credential
# substring. Fixed-substring grep — passwords contain regex metachars.
LEAKED=""
for secret in "${SGE_LIVE_IMAP_PASS:-}" "${SGE_LIVE_SMTP_PASS:-}"; do
    if [ -n "$secret" ] && grep -F -q "$secret" "$LOG"; then
        LEAKED="log"
    fi
done
if [ -n "$LEAKED" ]; then
    echo "SECRET-LEAK: credential substring found in $LOG (see which artifact matched, never the secret)"
    CODE=1
fi

# Scrub secrets from this shell before any further output.
unset SGE_LIVE_IMAP_PASS SGE_LIVE_SMTP_PASS || true
exit "$CODE"
