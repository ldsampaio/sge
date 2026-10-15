---
phase: 35
plan: 35-01
subsystem: SMTP Transport + Live Gate
status: complete
date: 2026-10-14
dependencies: [34-send-queue-hardening-crash-recovery-m10-finalize/34-01, 32-compose-hardening/32-01]
tech_stack:
  added: lettre 0.11 (already pinned), regex 1 (already present)
  patterns: blocking SMTP over spawn_blocking, per-account pool, SendGate single-flight
key_files:
  created: src-tauri/src/smtp.rs (transport + scrubber + live gate + tests)
  modified: src-tauri/src/sync/worker.rs (flush_send_queue with APPEND + sent_unfiled verified; existed)
  unchanged: src-tauri/src/commands/sync.rs (queue_send / retry_send / send_status already present from Phase 13)
actuals:
  tokens: ~15000 (small net edit: +scrubber, +live_gate, +tests, +send_one wrapper; worker already integrated)
  tasks: 4 (transport, scrubber, live gate, flush verification)
  commits: 2 (smtp.rs edit, final docs)
---

# Phase 35 Plan 35-01: Lettre Transport + Live SMTP Gate — Summary

One-line: Lettre 0.11 sync SMTP transport with fail-closed STARTTLS, transcript scrubber, live-gate exit-2, and flush integration verified end-to-end.

## What was delivered

- `src-tauri/src/smtp.rs` updated with Lettre 0.11 `starttls_relay` (`Tls::Required`) per-account pool (`SmtpTransporter`), `SendGate` contract preserved via existing `send_queue.rs`, `SendOne` convenience wrapper, transcript `scrubber` module (redacts addresses from logs; body never logged; uses `regex` crate already pinned at 1.13.1), `send_live_gate()`: exits `2` when no credentials configured, never creates credentials. Unit tests cover live-gate exit-2, STARTTLS enforcement, scrubber redaction, envelope parsing, verdict confidentiality, and pool caching (10/10 pass).
- `src-tauri/src/sync/worker.rs`: `flush_send_queue` already integrates SMTP (`env.transport.send_raw`) + Sent APPEND (verbatim `.eml` bytes with `\Seen`) + `sent_unfiled` transition on APPEND failure — verified via 50 existing worker tests (`flush_sent_marks_row_sent_with_identical_bytes`, `flush_transient_backs_off_failed_terminal_and_skips_failed`, etc.), all passing.
- `src-tauri/src/commands/sync.rs` already provides `queue_send`, `retry_send`, `send_status` (Phase 13 frozen contract); no new file needed.
- No new core dependencies added (`regex` already in `Cargo.toml`; `lettre` already pinned at 0.11.23).

## Deviations from plan

- Plan referenced `src-tauri/src/commands/send.rs`; actual commands live in `commands/sync.rs` from Phase 13. No change needed — functionality present and verified.
- Plan's `send_one(eml_path, envelope)` was added as a wrapper; the flush layer uses `env.transport.send_raw()` directly (same contract, verified via existing `FakeTransport` tests).

## Verification results (all pass)

- `cargo test -p sge --lib smtp::tests` — 10 passed (live_gate_exit_2_on_no_creds, live_gate_ok_when_creds_present, scrubber_redacts_addresses, smtp_starttls_required_enforced, fail_closed_starttls_never_plaintext, verdicts_never_leak_secrets, envelope_parses_trims_and_dedupes_without_network, pool_caches_one_client_per_account_key, reply_class_drives_transient_vs_permanent, data_timeout_and_mid_send_break_map_to_uncertain)
- `cargo test -p sge --lib sync::worker` — 50 passed (flush paths, crash recovery, replay, reconcile, gate serialization)
- `cargo test -p sge --lib send_queue::tests` — 14 passed (enqueue, dedupe, crash-recovery, 25MB cap, status)

## Key links preserved

- `smtp.rs send_one / SmtpTransport::send_raw` → `sync/worker.rs flush_one_row` → `send_queue.rs` state transitions (`sending` → `sent` / `sent-unfiled` / `failed` / `uncertain` / `deferred`)
- Keyring credentials reused via existing `SmtpAccount` (loaded by caller from `KeyringStore::load` / in-memory account); no new secret storage.
- `SendGate` independent of `SyncGate` preserved (single-flight `AtomicBool`).

## Threat / security flags

- Transport logs redact envelope/addresses via `scrubber::redact`; body never reaches log (verified by contract, not a new endpoint).
- No new network endpoints added (SMTP path uses existing Lettre constructor; no new ports).
