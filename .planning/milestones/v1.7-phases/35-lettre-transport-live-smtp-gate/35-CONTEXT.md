# Phase 35: Lettre Transport + Live SMTP Gate

## Domain

This phase implements live SMTP transport via Lettre 0.11 over `spawn_blocking` with a per-account connection pool. The `SendGate` remains independent of `SyncGate`. STARTTLS is enforced (`Tls::Required`); endpoint is `smtp.utfpr.edu.br:587`. MIME is rendered once to outbox; BCC is envelope-only. On success, APPEND an exact copy to Sent (verbatim bytes, `\Seen`). APPEND failure triggers `sent-unfiled` retry. Live gate: end-to-end send verified; exit-2 with no creds; never create creds. Transcript scrubber redacts addresses/content from logs.

## Decisions Captured

### Lettre Transport Integration
- **Use Lettre 0.11 sync over `spawn_blocking`** with per-account connection pool (pinned version). `SendGate` stays separate from `SyncGate`. STARTTLS `Tls::Required` (no opportunistic).

### SMTP Configuration
- **Host:Port = `smtp.utfpr.edu.br:587`** (STARTTLS). No new TLS crate needed; SMTP already uses STARTTLS, zero new core dependencies.

### MIME Rendering
- **Single-render to outbox** — MIME rendered once to `.eml` file; reused for SMTP and Sent APPEND (same bytes, immutable contract). BCC stripped from headers, envelope-only.

### Sent APPEND
- **APPEND verbatim bytes** to Sent folder on success, mark as `\Seen`. On APPEND failure → `sent-unfiled` retry state (no data loss).

### Live Gate Behavior
- **Exit-2 SKIP when no credentials configured** (empty config). Never auto-create creds; require explicit user setup. End-to-end send verified in sandbox before full gate.

### Transcript Scrubber
- **Redact only envelope/addresses** from transport logs; body never logged (preserves privacy). Use Phase 32 scrubber patterns.

### Keyring Integration
- **Reuse existing OS keyring path** (no new secret storage). Credentials flow from Phase 34's keyring reuse decision.

## Canonical References

- `.planning/ROADMAP.md` — Phase 35 description and v1.6 constraints
- `.planning/REQUIREMENTS.md` — Lettre 0.11 integration requirements
- `.planning/phases/34-send-queue-hardening-crash-recovery-m10-finalize/34-CONTEXT.md` — StartSync, conditional M11, keyring reuse
- `.planning/milestones/v1.2-phases/13-send-pipeline/13-CONTEXT.md` — M10 send_queue contract, lettre 0.11
- `.planning/phases/32-compose-hardening/32-CONTEXT.md` — keyring reuse, STARTTLS Tls::Required, sandbox contracts
- `Cargo.toml` — lettre 0.11 pinned, zero new dependencies

## Code Context

- `src-tauri/src/send_queue.rs` — M10 contract, enqueue path, state machine
- `src-tauri/src/sync/worker.rs` — flush_send_queue, reconcile_uncertain_sends
- `src-tauri/Cargo.toml` — lettre 0.11 pinned, spawn_blocking patterns
- `.planning/phases/34-send-queue-hardening-crash-recovery-m10-finalize/34-01-PLAN.md` — send_queue structure to integrate with