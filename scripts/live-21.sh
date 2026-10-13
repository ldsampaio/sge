#!/usr/bin/env bash
# live-21.sh — Phase 21 sandboxed live-gate wrapper (D-04/D-09/D-13/D-14/D-15).
#
# Sources the git-ignored env file as env vars (never CLI args), optionally
# seeds the OS keyring once, runs the live_gate_21 bin capturing a /tmp log,
# then re-verifies the send audit (D-14, five rules + D-09 flush visibility)
# and the secret scan (D-13: log + in-bin full-DB dump) from the log lines.
# Propagates the bin exit code (audit/scan failures force exit 1).
#
# EXIT: 0 all-pass · 1 assertion FAIL / audit FAIL / secret-leak · 2 pre-flight skip.
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

LOG="$(mktemp /tmp/sge-live21-XXXXXX.log)"
cargo run --manifest-path src-tauri/Cargo.toml --bin live_gate_21 "$@" 2>&1 | tee "$LOG"
CODE="${PIPESTATUS[0]}"

# Audit + scan only run when the gate itself ran (STEP lines present):
# --seed-only exits 0 with no STEPs, and exit-2 SKIP has nothing to audit.
if [ "$CODE" -ne 2 ] && grep -q "^STEP " "$LOG"; then
    # ── AUDIT stage (D-14 five rules + D-09 flush visibility) ──
    # The bin aborts on any violation first; this re-verifies from the
    # AUDIT-* lines (defense in depth). Counts only, never secret values.
    AUDIT_FAIL=""
    # D-09: the production flush-done schedule lines must be visible in logs.
    if ! grep -F -q "[SGE send] flush done" "$LOG"; then
        AUDIT_FAIL="${AUDIT_FAIL} flush-done-log-missing;"
    fi
    # Bin-side verdict present and passing.
    if grep -q "^AUDIT summary: FAIL" "$LOG"; then
        AUDIT_FAIL="${AUDIT_FAIL} bin-summary-fail;"
    elif ! grep -q "^AUDIT summary: PASS" "$LOG"; then
        AUDIT_FAIL="${AUDIT_FAIL} bin-summary-missing;"
    fi
    # Rule 1 (independent): every AUDIT-ROW is to==[self], cc/bcc empty.
    SELF_JSON="[\"${SGE_LIVE_SELF:-}\"]"
    ROW_TOTAL=$(grep -c "^AUDIT-ROW " "$LOG" || true)
    ROW_OK=$(grep -F -c "to=${SELF_JSON} cc=[] bcc=[]" "$LOG" || true)
    if [ "$ROW_TOTAL" -eq 0 ] || [ "$ROW_OK" != "$ROW_TOTAL" ]; then
        AUDIT_FAIL="${AUDIT_FAIL} recipients(${ROW_OK}/${ROW_TOTAL});"
    fi
    # Rules 2+3: every .eml prefixed and Bcc:-free.
    if grep -q "^AUDIT-EML .* subject_ok=false" "$LOG"; then
        AUDIT_FAIL="${AUDIT_FAIL} unprefixed-subject;"
    fi
    if grep -q "^AUDIT-EML .* bcc_absent=false" "$LOG"; then
        AUDIT_FAIL="${AUDIT_FAIL} bcc-header-present;"
    fi
    # Rule 2 sweep: no AUDIT-SUBJECT line may lack the sandbox prefix.
    UNPREFIXED=$(grep "^AUDIT-SUBJECT " "$LOG" | grep -v -F "SGE-TEST-" || true)
    if [ -n "$UNPREFIXED" ]; then
        AUDIT_FAIL="${AUDIT_FAIL} unprefixed-subject-line;"
    fi
    # Count AUDIT-SUBJECT lines; fail if zero (cover vacuous passes).
    SUBJECT_TOTAL=$(grep -c "^AUDIT-SUBJECT " "$LOG" || true)
    if [ "$SUBJECT_TOTAL" -eq 0 ]; then
        AUDIT_FAIL="${AUDIT_FAIL} missing-subject-line-count;"
    fi
    # Rule 4: Message-IDs unique per run.
    DUP_MID=$(grep "^AUDIT-MID " "$LOG" | sort | uniq -d || true)
    if [ -n "$DUP_MID" ]; then
        AUDIT_FAIL="${AUDIT_FAIL} reused-message-id;"
    fi
    # Rule 5: per-leg transport call counts reconcile (reconcile legs add 0).
    if grep -q "^AUDIT-CALLS .* ok=false" "$LOG"; then
        AUDIT_FAIL="${AUDIT_FAIL} call-count-mismatch;"
    fi
    if [ -n "$AUDIT_FAIL" ]; then
        echo "AUDIT run: FAIL ${AUDIT_FAIL} (see which check named, never secrets)"
        CODE=1
    else
        echo "AUDIT run: PASS rows=${ROW_TOTAL} (self-only recipients, prefixed subjects, no Bcc: header, unique MIDs, call counts reconcile)"
    fi

    # ── SCAN stage (D-13) ──
    # Fixed-substring grep over the log for the full password values
    # (passwords contain regex metachars — never regex), plus the in-bin
    # full-DB-dump verdict (the temp DB is removed by cleanup, so the bin
    # scans its dump in-process and prints SCAN-DB). Any hit names WHICH
    # artifact matched, never the secret.
    SCAN_FAIL=""
    for secret in "${SGE_LIVE_IMAP_PASS:-}" "${SGE_LIVE_SMTP_PASS:-}"; do
        if [ -n "$secret" ] && printf '%s' "$secret" | grep -F -q -f /dev/stdin "$LOG"; then
            SCAN_FAIL="${SCAN_FAIL} log-leak;"
        fi
    done
    if grep -q "SECRET-LEAK" "$LOG"; then
        SCAN_FAIL="${SCAN_FAIL} secret-leak-marker;"
    fi
    if grep -q "^SCAN-DB .* FAIL" "$LOG"; then
        SCAN_FAIL="${SCAN_FAIL} db-scan-fail;"
    elif ! grep -q "^SCAN-DB .* PASS" "$LOG"; then
        SCAN_FAIL="${SCAN_FAIL} db-scan-missing;"
    fi
    if [ -n "$SCAN_FAIL" ]; then
        echo "SCAN run: FAIL ${SCAN_FAIL} (see which artifact matched, never the secret)"
        CODE=1
    else
        echo "SCAN run: PASS (log + full temp-DB dump carry no credential substring)"
    fi
else
    echo "AUDIT run: SKIPPED (gate did not run — seed-only or pre-flight skip)"
    echo "SCAN run: SKIPPED (gate did not run — seed-only or pre-flight skip)"
fi

# ── Capability capture (Phase 27) ──
# Emit pre-auth atoms from 27-01 probe transcripts (agent-verifiable, no re-parsing IMAP).
# Post-login leg is operator-only; record SKIP with non-empty reason + resume pointer.
if [ "$CODE" -ne 2 ] && grep -q "^STEP " "$LOG"; then
    PREAUTH_ATOMS=$(grep -E "(IDLE|CONDSTORE|QRESYNC)" .planning/probes/27-imap-capability.txt 2>/dev/null | tr '\n' ',' | sed 's/,$//' || echo "none")
    echo "CAPABILITY-IMAP-PREAUTH atoms=$PREAUTH_ATOMS"
    echo "CAPABILITY-IMAP-POSTLOGIN atoms=SKIP reason=operator-only-credentialed-leg resume=/gsd-verify-work 21"
fi

# ── UAT checklist pointer (Phase 27) ──
# Operator executes credentialed legs; trace to UAT-27.md for the human click-through.
echo "UAT: Human click-through checklist -> .planning/phases/27-live-validation-gate-capability-probes/UAT-27.md"

# Scrub secrets from this shell before any further output.
unset SGE_LIVE_IMAP_PASS SGE_LIVE_SMTP_PASS || true
exit "$CODE"
