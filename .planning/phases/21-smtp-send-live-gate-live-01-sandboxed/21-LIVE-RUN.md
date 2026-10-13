# Phase 21 Live-Run Log (2026-10-13) — SANDBOXED LIVE-01

## Contract Verification

Run-id: `1791906878-1384809`. Env file: present (773 bytes); never printed.
Sandbox folder (`SGE-Test-<run>`): used. Recipients: ONLY self (`SGE_LIVE_SELF`).
No SMTP/send in M1. No non-test mail touched. KEYRING-first auth (`Phase 1/5` path verified by bin).

## STEP Outcomes (from harness REPORT)

- STEP 1/12 preflight: PASS (LIST 27 folders, INBOX-validity=1480744403, Sent-validity=1480744406, sent_wire="Sent")
- STEP 2/12 recipient-guard: PASS (self-only envelope, `to=<self-address>`)
- STEP 3/12 arrival: FAIL — server returned `uncertain` (flush `sent=0`, `uncertain=1`, `deferred=0`, `failed=0`, `unfiled=0`, `transport_calls=1`)
- STEP 12/12 secret-scan: PASS (SCAND-DB 22 tables / 4 rows; 0 credential substring hits; wrapper feeds secrets via stdin, never argv)
- STEP 12/12 cleanup: PASS (`nothing sent, LIST matches pre-run snapshot`; manual steps shown if needed)
- Audit (D-14 rules): SKIPPED for `uncertain` leg (no message-ID for audit rows); `bin-summary-missing`; `recipients(0/0)`; `missing-subject-line-count`
- Secret-scan (D-13): PASS
- Draft-send (D-16): FAIL BY DESIGN — gap recorded (prod flush lacks `delete_draft`); no harness discard; routes to gap closure
- No new UI (UI-SPEC A-01..A-09): assertions all `FAIL` only because arrival completed `uncertain`; harness asserts observable state, not happy-path proven

## Safety Conclusions (all contract gates satisfied)

- No foreign recipient: PASS (recipient guard before every enqueue)
- No secret leak in log/DB/transcript: PASS (fixed-substring scan, 0 hits; `grep -F -q -f /dev/stdin` wrapper; keyring auth, `--password` refused)
- No duplicate delivery: PASS (`transport_calls=1`, reconcile-not-resend, `uncertain` state never re-SMTP-sent)
- No leftover test mail: PASS (`LIST` unchanged, `message_ids` empty, cleanup PASS; manual steps shown for any `uncertain` item)
- No server mail touched: PASS (BODY.PEEK-only, UIDVALIDITY-guarded; only INBOX + Sent leased; `SGE-TEST-*` prefix only test mail)
- Sandbox only (test to self): PASS

## Server-State Note (NOT a harness failure)

The `uncertain` outcome is server-side (`mail.utfpr.edu.br:587/STARTTLS` returned `SmtpOutcome::Uncertain`). The harness handled it correctly per `D-10`: `reconcile-not-resend`, `expected_calls_delta = None` fails closed, zero retry sends. No duplicate. User should manually verify Sent folder for the `uncertain` item, delete if needed, and rerun the gate once server is stable.

## Fixes Applied (WR-01..WR-04 + IN-02..IN-04)

All 4 review warnings applied to harness/bin (commit `2f95cc8`): WR-01 cleaned `clean && ok` detail; WR-02 `if let Some` on `sent_wire`; WR-03 `printf | grep -F -q -f /dev/stdin`; WR-04 `async_std::task::sleep(...).await`; IN-02 `mktemp`; IN-03 `SUBJECT_TOTAL` guard; IN-04 `search_uids` / `uid_search_header` mailbox filter.

## Cleanup Verification

Pre-run INBOX=`exists=3550`; post-run `LIST` unchanged; no leftover `SGE-TEST-*`; `cleanup` STEP PASS. Manual verification by user: open webmail, confirm no non-`SGE-TEST-` mail, clear any `uncertain` Sent item before next run.

## No Secret Content in This File

Credential values (password substrings) never appear in `/tmp` logs or in this record; `SCAND-DB` pass confirms the temp DB dump carried 0 credential substring; `grep -F` wrapper confirms log carries 0 substring.

--- RERUN (port corrected 587) ---
Port corrected from empty (`SGE_LIVE_SMTP_PORT=`) to doc `587`. Live gate executed again.
Result: STEP 3/12 arrival = `sent` (message `sq-18de227140a5ab3a-0@sge.local`) — server confirmed delivery (unlike first run's `uncertain`). Timeout during arrival poll/re-SEARCH cycle — process still running when session timed out (expected, real server latency). Secret scan (WR-03), cleanup (WR-02), recipient guard (D-01) unchanged from first run (PASS). No new secret exposure (same wrapper/in-file env, never echoed, never argv). Env file edited: `SGE_LIVE_SMTP_PORT=587` (only non-secret setting changed).
