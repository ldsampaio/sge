# Phase 37: Compose UI / New Message Button + Compose Window

## Domain

This phase creates a global "New Message" toolbar button that opens a Compose Window (Thunderbird-style). The window provides To/Cc/Bcc/Subject/Body/attachments UI wired to Phase 14 contract → `queue_send` enqueue. The Compose Window reuses the existing `drafts.rs` structure; no new storage schema needed (send_queue M10 finalized from Phase 34). The button and window are new UI surfaces, not new core dependencies.

## Decisions Captured

### New Message Button
- **Global toolbar button** (visible in main reading pane header). Triggers Compose Window.
- **No SMTP connection at create time** — message is queued to `send_queue` (Phase 35 transport); live SMTP only on `queue_send`.

### Compose Window
- **Modal / standalone window** (Thunderbird-style). Fields: To (required), Cc, Bcc, Subject, Body (rich text / plain text fallback), Attachments (file picker → `.eml` blob attachment).
- **Uses Phase 14 contract** for header fields; `drafts.rs` handles draft save/APPEND; `send_queue.rs` handles enqueue.
- **BCC envelope-only** — stripped from rendered `.eml` headers (Phase 35 contract preserved).

### Integration
- **Compose UI → `queue_send`** (Phase 14 contract → Phase 35 `smtp.rs` → Phase 34 `send_queue.rs`). End-to-end verification: new message → enqueue → SMTP send (live gate exit-2 if no creds) → Sent APPEND.
- **Zero new core dependencies** — uses existing lettre 0.11, async-imap, rusqlite, drafts modules.

### Milestone Constraints (from v1.7 Roadmap)
- BODY.PEEK-only preserved; UID-only addressing; UIDVALIDITY-first gating preserved.
- Zero new core dependencies; keyring reuse preserved; sandbox contracts preserved.
- No SMTP server config changes (smtp.utfpr.edu.br:587 fixed).
- Phase 33 deferred permanently (not revisited).
- End-to-end verification: compose → queue → send → Sent APPEND confirmed locally.

## Canonical References

- `.planning/ROADMAP.md` — Phase 37 description (v1.7 roadmap)
- `.planning/REQUIREMENTS.md` — Compose integration requirements
- `.planning/phases/36-compose-integration-outbox-ux/36-CONTEXT.md` — Milestone close preparation; deferred items promoted
- `.planning/phases/35-lettre-transport-live-smtp-gate/35-CONTEXT.md` — SMTP transport, live gate, APPEND contract
- `.planning/phases/34-send-queue-hardening-crash-recovery-m10-finalize/34-CONTEXT.md` — Send queue M10 contract hardened
- `.planning/phases/14-compose-ui/14-CONTEXT.md` — Phase 14 Compose UI contract (To/Cc/Bcc/Subject/Body/attachments)
- `.planning/phases/13-send-pipeline/13-CONTEXT.md` — M10 send_queue schema
- `Cargo.toml` — lettre 0.11 pinned, zero new dependencies

## Code Context

- `src-tauri/src/drafts.rs` — Phase 14 Compose UI (To/Cc/Bcc/Subject/Body/attachments)
- `src-tauri/src/commands/send.rs` — `queue_send` / `retry_send` (Phase 35)
- `src-tauri/src/commands/sync.rs` — `replay_outbox` (Phase 35, outbox replay)
- `src-tauri/src/send_queue.rs` — M10 contract, enqueue, state machine
- `src-tauri/src/sync/worker.rs` — flush + APPEND (Phase 35)
- `src-tauri/src/store/queries.rs` — send_queue queries
- `.planning/phases/14-compose-ui/14-01-PLAN.md` — Phase 14 plan (reference for compose contract)
- `.planning/phases/36-compose-integration-outbox-ux/36-01-PLAN.md` — Milestone v1.6 close plan

## Noted for Later (deferred ideas — not part of phase domain)

- Phase 38: Reply / Reply-All UI/UX when message open (next v1.7 phase)
- Attachment streaming (large files — future optimization milestone)
- Draft templates / scheduled send (future UX milestone)
- Phase 33 deferred permanently (ManageSieve — no future milestone planned)

## Next Steps

`/gsd-plan-phase 37` — Plan the Compose Window and New Message button for v1.7 milestone.