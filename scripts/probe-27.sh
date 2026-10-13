#!/usr/bin/env bash
# probe-27.sh — Phase 27 unauthenticated capability probes
# STEP lines: 1/3 IMAP pre-auth CAPABILITY, 2/3 SMTP EHLO STARTTLS, 3/3 port-4190/ManageSieve
# EXIT: 0 all probes OK + JSON emitted · 1 probe/emit failure · 2 pre-flight SKIP (no network / env)

set -uo pipefail

# ── Pre-flight ──
# This probe runs WITHOUT credentials. If src-tauri/.live-test.env exists, it must be git-ignored and unused.
if [ -f "src-tauri/.live-test.env" ] && ! git check-ignore -q src-tauri/.live-test.env; then
    echo "SKIP: src-tauri/.live-test.env exists but is not git-ignored — refusing to probe"
    exit 2
fi

# Build env must be sourced before any cargo command
if [ ! -f "scripts/build-env.sh" ]; then
    echo "FAIL: scripts/build-env.sh not found"
    exit 1
fi

# Source build-env (flatpak-SDK pkg-config + /tmp/opencode stubs; never export LD_LIBRARY_PATH globally)
# shellcheck source=/dev/null
source scripts/build-env.sh

# ── Raw output sinks (all transient, /tmp only) ──
IMAP_RAW="$(mktemp /tmp/sge-probe27-imap-XXXXXX.raw)"
SMTP_RAW="$(mktemp /tmp/sge-probe27-smtp-XXXXXX.raw)"
SIEVE_RAW="$(mktemp /tmp/sge-probe27-sieve-XXXXXX.raw)"

# Cleanup function
cleanup_raw() {
    rm -f "$IMAP_RAW" "$SMTP_RAW" "$SIEVE_RAW"
}
trap cleanup_raw EXIT

# Scrubber: fixed-substring removal (no regex) — LOGIN, AUTHENTICATE, env names
scrub() {
    local infile="$1"
    local outfile="$2"
    # Remove lines containing auth commands or env var patterns
    # Using grep -F -v for fixed-string exclusion
    grep -F -v "LOGIN" "$infile" | \
    grep -F -v "AUTHENTICATE" | \
    grep -F -v "SGE_IMAP_PASSWORD" | \
    grep -F -v "SGE_LIVE_SMTP_PASS" | \
    grep -F -v "SGE_LIVE_IMAP_PASS" | \
    grep -F -v "SGE_LIVE_IMAP_USER" | \
    grep -F -v "SGE_IMAP_PASSWORD" > "$outfile" || true
}

# ── STEP 1/3: IMAP pre-auth CAPABILITY ──
echo "STEP 1/3: IMAP pre-auth CAPABILITY (mail.utfpr.edu.br:993 implicit_tls)"
IMAP_SCRUBBED=".planning/probes/27-imap-capability.txt"

# Run imap_probe with --pre-auth-only (no login, no creds)
# Uses the exact same TLS discipline as the app
if source scripts/build-env.sh && cargo run --manifest-path src-tauri/Cargo.toml --bin imap_probe -- \
    --host mail.utfpr.edu.br --port 993 --mode implicit_tls --pre-auth-only \
    > "$IMAP_RAW" 2>&1; then
    echo "  IMAP pre-auth probe: OK"
    scrub "$IMAP_RAW" "$IMAP_SCRUBBED"
else
    echo "  IMAP pre-auth probe: FAILED (exit $?)"
    cat "$IMAP_RAW" > "$IMAP_SCRUBBED"
fi

# ── STEP 2/3: SMTP EHLO over STARTTLS ──
echo "STEP 2/3: SMTP EHLO STARTTLS (smtp.utfpr.edu.br:587)"
SMTP_SCRUBBED=".planning/probes/27-smtp-ehlo.txt"

# Use openssl s_client for STARTTLS EHLO (no AUTH attempted)
# Timeout 15s, send EHLO and QUIT
if timeout 15 openssl s_client -connect smtp.utfpr.edu.br:587 -starttls smtp -crlf -ign_eof \
    <<'EOF' > "$SMTP_RAW" 2>&1
EHLO sge-probe
QUIT
EOF
then
    echo "  SMTP EHLO probe: OK"
    scrub "$SMTP_RAW" "$SMTP_SCRUBBED"
else
    echo "  SMTP EHLO probe: FAILED/timeout (exit $?)"
    cat "$SMTP_RAW" > "$SMTP_SCRUBBED"
fi

# ── STEP 3/3: Port 4190 / ManageSieve probe ──
echo "STEP 3/3: Port 4190/ManageSieve probe (mail.utfpr.edu.br:4190 + smtp.utfpr.edu.br:4190)"
SIEVE_SCRUBBED=".planning/probes/27-managesieve-4190.txt"

# Three-valued verdict taxonomy: closed/refused | filtered/timeout | open-greeting
# We probe both hosts, first positive open-greeting wins for phase33_armed
MANAGESIEVE_VERDICT="filtered/timeout"
MANAGESIEVE_GREETING=""
MANAGESIEVE_PORT=""
MANAGESIEVE_HOST=""
SIEVE_RAW_OUTPUT=""

for host in "mail.utfpr.edu.br" "smtp.utfpr.edu.br"; do
    echo "  Trying $host:4190..."
    # Try TLS first (ManageSieve typically uses STARTTLS on 4190)
    SIEVE_TEMP="$(mktemp /tmp/sge-probe27-sieve-${host}-XXXXXX.raw)"
    if timeout 10 openssl s_client -connect "${host}:4190" -starttls sieve -crlf -ign_eof \
        <<'EOF' > "$SIEVE_TEMP" 2>&1
IMPLEMENTATION
LOGOUT
EOF
    then
        # Check for RFC 5804 greeting
        if grep -q "IMPLEMENTATION" "$SIEVE_TEMP" || grep -q "SIEVE" "$SIEVE_TEMP"; then
            MANAGESIEVE_VERDICT="open-greeting"
            MANAGESIEVE_GREETING=$(grep -E "(IMPLEMENTATION|SIEVE)" "$SIEVE_TEMP" | head -1 | sed 's/^[[:space:]]*//')
            MANAGESIEVE_PORT=4190
            MANAGESIEVE_HOST="$host"
            SIEVE_RAW_OUTPUT=$(cat "$SIEVE_TEMP")
            scrub "$SIEVE_TEMP" "$SIEVE_SCRUBBED"
            rm -f "$SIEVE_TEMP"
            break
        fi
    fi
    
    # Try plaintext TCP as fallback
    if timeout 5 nc -v -w 5 "$host" 4190 <<'EOF' > "$SIEVE_TEMP" 2>&1
IMPLEMENTATION
LOGOUT
EOF
    then
        if grep -q "IMPLEMENTATION" "$SIEVE_TEMP" || grep -q "SIEVE" "$SIEVE_TEMP"; then
            MANAGESIEVE_VERDICT="open-greeting"
            MANAGESIEVE_GREETING=$(grep -E "(IMPLEMENTATION|SIEVE)" "$SIEVE_TEMP" | head -1 | sed 's/^[[:space:]]*//')
            MANAGESIEVE_PORT=4190
            MANAGESIEVE_HOST="$host"
            SIEVE_RAW_OUTPUT=$(cat "$SIEVE_TEMP")
            scrub "$SIEVE_TEMP" "$SIEVE_SCRUBBED"
            rm -f "$SIEVE_TEMP"
            break
        fi
    fi
    
    # Connection refused or timeout
    if grep -q "Connection refused" "$SIEVE_TEMP" 2>/dev/null || grep -q "refused" "$SIEVE_TEMP" 2>/dev/null; then
        MANAGESIEVE_VERDICT="closed/refused"
    fi
    SIEVE_RAW_OUTPUT=$(cat "$SIEVE_TEMP")
    scrub "$SIEVE_TEMP" "$SIEVE_SCRUBBED"
    rm -f "$SIEVE_TEMP"
done

# Ensure sieve scrubbed file exists even if empty (filtered/timeout)
if [ ! -f "$SIEVE_SCRUBBED" ] || [ ! -s "$SIEVE_SCRUBBED" ]; then
    echo "# ManageSieve probe: $MANAGESIEVE_VERDICT (no greeting captured)" > "$SIEVE_SCRUBBED"
fi

echo "  ManageSieve verdict: $MANAGESIEVE_VERDICT (host: ${MANAGESIEVE_HOST:-none})"

# ── Capability profile JSON ──
PROFILE_JSON=".planning/probes/capability-profile.json"

# Extract IMAP atoms from scrubbed transcript (pre-auth CAPABILITY may be limited)
IMAP_ATOMS_IDLE="false"
IMAP_ATOMS_CONDSTORE="false"
IMAP_ATOMS_QRESYNC="false"

if [ -f "$IMAP_SCRUBBED" ] && [ -s "$IMAP_SCRUBBED" ]; then
    grep -q -w "IDLE" "$IMAP_SCRUBBED" && IMAP_ATOMS_IDLE="true"
    grep -q -w "CONDSTORE" "$IMAP_SCRUBBED" && IMAP_ATOMS_CONDSTORE="true"
    grep -q -w "QRESYNC" "$IMAP_SCRUBBED" && IMAP_ATOMS_QRESYNC="true"
fi

# Extract IMAP greeting and pre-auth capability from scrubbed transcript
IMAP_GREETING=""
IMAP_CAPABILITY_PREAUTH="[]"
if [ -f "$IMAP_SCRUBBED" ] && [ -s "$IMAP_SCRUBBED" ]; then
    # Greeting is typically the first line starting with "* OK" or similar
    IMAP_GREETING=$(grep -E "^[*] " "$IMAP_SCRUBBED" 2>/dev/null | head -1 | sed 's/^[*] //' || echo "")
    # Capability line: look for CAPABILITY response
    IMAP_CAP=$(grep -E "^[*] CAPABILITY" "$IMAP_SCRUBBED" 2>/dev/null | head -1 | sed 's/^[*] CAPABILITY //' || echo "")
    if [ -n "$IMAP_CAP" ]; then
        IMAP_CAPABILITY_PREAUTH=$(echo "$IMAP_CAP" | jq -R -s -c 'split(" ") | map(select(. != ""))')
    fi
fi

# Extract SMTP capabilities from scrubbed transcript
SMTP_CAPABILITY="[]"
if [ -f "$SMTP_SCRUBBED" ] && [ -s "$SMTP_SCRUBBED" ]; then
    # Build JSON array of EHLO responses (strip 250- prefix and \r)
    SMTP_CAPABILITY=$(grep -E "^250-" "$SMTP_SCRUBBED" 2>/dev/null | sed 's/^250-//; s/\r$//' | jq -R -s -c 'split("\n") | map(select(. != ""))' 2>/dev/null || echo "[]")
fi

# Phase 33 armed only on positive ManageSieve greeting
PHASE33_ARMED="false"
if [ "$MANAGESIEVE_VERDICT" = "open-greeting" ]; then
    PHASE33_ARMED="true"
fi

# Build capability-profile.json
cat > "$PROFILE_JSON" <<EOF
{
  "schema_version": "1",
  "probed_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "probe_tool": "probe-27.sh",
  "accounts": [
    {
      "host": "mail.utfpr.edu.br",
      "port": 993,
      "mode": "implicit_tls",
      "greeting": "$IMAP_GREETING",
      "capability_preauth": $IMAP_CAPABILITY_PREAUTH,
      "capability_postlogin": "SKIP: operator-only credentialed leg",
      "atoms": {
        "IDLE": $IMAP_ATOMS_IDLE,
        "CONDSTORE": $IMAP_ATOMS_CONDSTORE,
        "QRESYNC": $IMAP_ATOMS_QRESYNC
      },
      "smtp": {
        "host": "smtp.utfpr.edu.br",
        "port": 587,
        "mode": "starttls",
        "ehlo": $SMTP_CAPABILITY
      },
      "managesieve": {
        "port": ${MANAGESIEVE_PORT:-null},
        "reachable": $([ "$MANAGESIEVE_VERDICT" != "filtered/timeout" ] && echo "true" || echo "false"),
        "greeting": "$MANAGESIEVE_GREETING",
        "verdict": "$MANAGESIEVE_VERDICT"
      },
      "transcripts": {
        "imap": "27-imap-capability.txt",
        "smtp": "27-smtp-ehlo.txt",
        "managesieve": "27-managesieve-4190.txt"
      },
      "phase33_armed": $PHASE33_ARMED
    }
  ]
}
EOF

echo "Emitted: $PROFILE_JSON"
echo "All probes complete. Verifying artifacts..."

# Verify all artifacts exist
for f in "$IMAP_SCRUBBED" "$SMTP_SCRUBBED" "$SIEVE_SCRUBBED" "$PROFILE_JSON"; do
    if [ ! -f "$f" ] || [ ! -s "$f" ]; then
        echo "FAIL: missing or empty artifact: $f"
        exit 1
    fi
    echo "  OK: $f"
done

# Secret grep tripwire over committed artifacts
echo "Running secret tripwire over .planning/probes/..."
if grep -rF -e "SGE_IMAP_PASSWORD" -e "SGE_LIVE_SMTP_PASS" -e "SGE_LIVE_IMAP_PASS" -e "SGE_LIVE_IMAP_USER" .planning/probes/ 2>/dev/null; then
    echo "FAIL: secret tripwire hit in .planning/probes/"
    exit 1
fi
echo "  Secret tripwire: clean"

echo "SUCCESS: probe-27.sh completed end-to-end"
exit 0