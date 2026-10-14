# Phase 36: Compose Integration + Outbox UX

## Domain

This phase wires the Compose UI (from Phase 14 contract) to the send pipeline: To/Cc/Bcc/Subject/Body/attachments → `queue_send`. Implements OutboxBadge (pending send count), SendStatus (sending / failed / retry), and failed-retry action. Preserves `imap_outbox` replay contract (separate from `send_queue`). Enables end-to-end verification: compose → queue → send → Sent-folder APPEND confirmed locally. Prepares for milestone audit/v1.6 close with 6-check on artifacts + state + git.

## Decisions Captured

### Compose UI Integration
- **Compose UI → `queue_send`** wires To/Cc/Bcc/Subject/Body/attachments to existing send_queue enqueue path (Phase 13/34 contract). Uses Phase 14 Compose UI drafts as input.

### OutboxBadge & SendStatus
- **OutboxBadge**: shows pending send count (`queued + sending` from send_queue state machine).
- **SendStatus**: displays `sending / failed / retry` states with failed-retry action button.
- **Failed-retry**: action transitions `failed` rows back to `queued` for retry (respects backoff).

### IMAP Outbox Replay
- **`imap_outbox` replay contract preserved** — separate from `send_queue`; no overlap. Sync worker replays outbox via existing Phase 10 discipline.

### End-to-End Verification
- **Local Sent-folder APPEND confirmation**: after SMTP send, verify Sent APPEND via IMAP SEARCH (Message-ID) matches queued item.
- **Verification scope**: compose → queue → send → Sent APPEND confirmed locally (not live gate — Phase 35 handles live SMTP).

### Milestone Close Preparation
- **6-check audit readiness**: artifacts (code, plans, context), state (DB schemas, migrations), git (clean history, tagged releases).
- Preserves all prior constraints: BODY.PEEK-only, UID-only, UIDVALIDITY-first gating, SessionManager single-session.

### Key Dependencies
- **Zero new core dependencies**: lettre 0.11 pinned, native-tls only if needed (SMTP already uses STARTTLS).
- **No SMTP server config changes**: host/port fixed (`smtp.utfpr.edu.br:587`).
- **Phase 33 deferred permanently**: ManageSieve not revisited.

## Canonical References

- `.planning/ROADMAP.md` — Phase 36 description and v1.6 constraints
- `.planning/REQUIREMENTS.md` — Compose UI → send_queue wiring requirements
- `.planning/phases/35-lettre-transport-live-smtp-gate/35-CONTEXT.md` — Lettre transport, STARTTLS, Sent APPEND
- `.planning/phases/34-send-queue-hardening-crash-recovery-m10-finalize/34-CONTEXT.md` — StartSync, conditional M11, keyring reuse
- `.planning/phases/14-compose-ui/14-CONTEXT.md` — Compose UI contract (To/Cc/Bcc/Subject/Body/attachments)
- `.planning/phases/10-delete-move/10-CONTEXT.md` — imap_outbox replay discipline
- `.planning/phases/13-send-pipeline/13-CONTEXT.md` — M10 send_queue contract
- `Cargo.toml` — lettre 0.11 pinned, zero new dependencies

## Code Context

- `src-tauri/src/drafts.rs` — Phase 14 Compose UI (To/Cc/Bcc/Subject/Body/attachments)
- `src-tauri/src/send_queue.rs` — enqueue path, state machine (queued|sending|sent|failed|uncertain)
- `src-tauri/src/sync/worker.rs` — flush pass, outbox replay, Sent APPEND (from Phase 35)
- `src-tauri/src/store/queries.rs` — queue queries, state transitions
- `src-tauri/src/commands/sync.rs` — replay_outbox command (Phase 10)
- `src-tauri/src/commands/send.rs` — queue_send / retry_send / send_status (Phase 35)
- `Cargo.toml` — lettre 0.11 pinned, no new core deps

## Noted for Later (deferred ideas — not part of phase domain)

- ManageSieve integration (Phase 33 deferred permanently)
- Attachment streaming (large files — future optimization)
- Draft templates / scheduled send (future UX enhancements)
- **New Message button + compose window (Thunderbird-style)** — v1.7 milestone
- **Reply / Reply-All UI/UX when message open** — v1.7 milestone