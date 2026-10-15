# Phase 38: Reply / Reply-All UI/UX When Message Open

## Domain

This phase implements Reply and Reply-All functionality when the user opens a message. Reply populates To with the sender; Reply-All populates To with sender and Cc with all recipients (excluding self). Both build In-Reply-To/References headers for proper RFC 5322 threading and pre-fill the compose window with quoted message context (from message cache — BODY.PEEK-only, no new fetch).

## Decisions Captured

### Reply vs Reply-All
- **Reply**: To = original message's From address; Cc = empty.
- **Reply-All**: To = original message's From; Cc = all recipients (To + Cc from original, minus self) — RFC 2822 threading.

### Threading Headers
- **In-Reply-To**: original message's Message-ID.
- **References**: original message's References + Message-ID, preserving thread chain.

### Compose Window Integration
- **Reuses Phase 37 Compose Window** — same UI, pre-populated with threading headers and quoted context.
- **Uses Phase 14 contract** for header fields; `drafts.rs` handles draft save; `send_queue.rs` (Phase 34/35/36) handles enqueue + SMTP.

### Quote Attribution
- **From message cache** — message already in SQLite (BODY.PEEK read paths). Display sender name + date above quoted content. No new IMAP fetch needed.

### Constraints (from v1.7 Roadmap)
- BODY.PEEK-only invariant preserved — no new fetch paths.
- UID-only addressing; UIDVALIDITY-first gating preserved.
- Zero new core dependencies (uses existing lettre 0.11, rusqlite, drafts, send_queue).
- No new secret storage (keyring reuse).
- Sandbox contracts preserved (redaction + pre-flight).
- No SMTP server config changes (smtp.utfpr.edu.br:587 fixed).
- Phase 33 deferred permanently (not revisited).

## Canonical References

- `.planning/ROADMAP.md` — Phase 38 description (v1.7 roadmap)
- `.planning/REQUIREMENTS.md` — Reply/Reply-All integration requirements
- `.planning/phases/37-compose-ui-new-message-button/37-CONTEXT.md` — Compose Window UI (Phase 37 — dependency)
- `.planning/phases/36-compose-integration-outbox-ux/36-CONTEXT.md` — OutboxBadge + SendStatus
- `.planning/phases/35-lettre-transport-live-smtp-gate/35-CONTEXT.md` — SMTP transport + live gate
- `.planning/phases/34-send-queue-hardening-crash-recovery-m10-finalize/34-CONTEXT.md` — Send queue M10 contract
- `.planning/phases/14-compose-ui/14-CONTEXT.md` — Phase 14 Compose UI contract
- `.planning/phases/13-send-pipeline/13-CONTEXT.md` — M10 send_queue schema

## Code Context

- `src-tauri/src/drafts.rs` — Draft save/APPEND, header discipline (Phase 14)
- `src-tauri/src/store/queries.rs` — Message fetch from cache (BODY.PEEK paths)
- `src-tauri/src/store/mod.rs` — SQLite storage, message schema
- `src/components/DraftEditor.tsx` — Compose window UI (Phase 37 will define; Phase 38 consumes)
- `src-tauri/src/imap/session.rs` — IMAP session, Message-ID/References handling
- `src-tauri/src/send_queue.rs` — M10 contract, enqueue, state machine
- `src-tauri/src/commands/send.rs` — `queue_send` command
- `Cargo.toml` — no new dependencies expected

## Noted for Later (deferred ideas — not part of phase domain)

- Attachment streaming (large files — future optimization)
- Draft templates / scheduled send (future UX milestone)
- Forward message UI (future phase)

## Next Steps

`/gsd-plan-phase 38` — Plan Reply / Reply-All threading + quote attribution + compose window reuse.