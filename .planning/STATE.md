---
gsd_state_version: "1.0"
milestone: v1.6
milestone_name: Roadmap — SMTP Send Pipeline
status: complete
stopped_at: "Milestone v1.6 COMPLETE (Phases 34-36 done). Deferred: New Message button + compose window, Reply/Reply-All UX → v1.7 milestone"
last_updated: "2026-10-14T23:11:21.945Z"
state_head: 792294476af1680db7a2e5774d48d194dffd7aa8
progress:
  total_phases: 3
  completed_phases: 1
  total_plans: 3
  completed_plans: 3
  percent: 33
---

gsd_state_version: "1.0"
milestone: v1.6
milestone_name: SMTP Send Pipeline
current_phase: 35
current_phase_name: Lettre Transport + Live SMTP Gate
status: complete
stopped_at: Phase 35 executed — Lettre 0.11 transport + live gate + flush integration verified
last_updated: "2026-10-14T20:45:00.000Z"
last_activity: 2026-10-14
last_activity_desc: Phase 35 (Lettre Transport + Live SMTP Gate) completed: smtp.rs transport/scrubber/live-gate; worker flush_send_queue verified; all tests pass
state_head: 34b0e00c
progress:[███░░░░░░░] 33%
  total_phases: 3
  completed_phases: 1
  total_plans: 6
  completed_plans: 1
  percent: 33
prev_milestone: v1.5 (phases 27-32 complete; phase 33 deferred permanently; audit passed; archive: .planning/STATE.md.v1.5-archive)
next_plan: 36-01 (Compose Integration + Outbox UX — Phase 36)

## Session

**Last session:** 2026-10-14T23:11:21.913Z
**Stopped at:** Milestone v1.6 COMPLETE (Phases 34-36 done). Deferred: New Message button + compose window, Reply/Reply-All UX → v1.7 milestone
**Resume file:** /home/lucas/Documentos/Projetos/sge/.planning/phases/35-lettre-transport-live-smtp-gate/35-01-SUMMARY.md

## Phase 35 — Lettre Transport + Live SMTP Gate (completed)

- **Artifacts deployed:**
  - `src-tauri/src/smtp.rs` — Lettre 0.11 sync over `spawn_blocking`; per-account pool; `SendGate` independent of `SyncGate`; fail-closed STARTTLS (`Tls::Required`); MIME single-render to outbox; envelope-only BCC; APPEND to Sent on success (verbatim `.eml`, `\Seen`); APPEND failure → `sent-unfiled` retry; live gate (exit-2 SKIP with no creds, never creates creds); transcript scrubber (redacts envelope addresses from logs; body never logged).
  - `src-tauri/src/sync/worker.rs` — `flush_send_queue` with SMTP + Sent APPEND + `sent_unfiled` state transition; crash-recovery preserved (`sending`→`queued` before flush).
  - `src-tauri/src/commands/sync.rs` — existing `queue_send` / `retry_send` / `send_status` commands (Phase 13 frozen contract).
- **Verification results:**
  - `cargo test -p sge --lib smtp::tests` — 10 passed (live gate exit-2, STARTTLS enforcement, scrubber redaction, envelope parsing, verdict confidentiality, pool caching, reply classification, timeout→uncertain).
  - `cargo test -p sge --lib sync::worker` — 50 passed (flush paths, crash recovery, reconcile, gate serialization).
  - `cargo test -p sge --lib send_queue::tests` — 14 passed (enqueue, dedupe, crash-recovery, 25 MB cap, status).
- **No new core dependencies added** (`regex` already pinned at 1.13.1; `lettre` already pinned at 0.11.23).
- **Keyring reuse preserved** — `SmtpAccount` loaded by caller from existing `KeyringStore::load` / in-memory account; no new secret storage.

## Next

- **Phase 36:** Compose Integration + Outbox UX (compose UI → `queue_send`; OutboxBadge; SendStatus; failed-retry action; end-to-end verification compose → queue → send → Sent-folder APPEND; audit / milestone close v1.6).
