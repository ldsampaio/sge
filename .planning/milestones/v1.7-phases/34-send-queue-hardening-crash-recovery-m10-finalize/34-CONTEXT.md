# Phase 34: Send Queue Hardening + Crash Recovery (M10 finalize)

## Domain

This phase finalizes the `send_queue` schema (M10 already defined in v1.2) and hardens the state machine (queued→sending→sent/failed/uncertain) with attempts/backoff, 25 MB cap enforcement, crash‑recovery at `start_sync` (reset `sending→queued`), and `uncertain` reconciliation via Sent SEARCH by Message-ID (not re‑SMTP). No SMTP/send in this phase; BODY.PEEK‑only reads remain. UID‑only addressing with UIDVALIDITY-first gating. Zero new core dependencies.

## Decisions Captured

### State‑machine hardening
- **Preserve the v1.2 frozen 5‑state contract** (queued | sending | sent | failed | uncertain). No behavioral changes — the contract was already verified green at Phase 32 (12 send_queue tests pass). 25 MB cap enforcement and attempts/backoff (30 s base / 15 min cap / ±20 % jitter) remain unchanged.

### Crash‑recovery at `start_sync`
- **Extend reset to reconcile `uncertain` items on startup via Sent SEARCH by Message-ID** (no re‑SMTP). Per user choice, `start_sync` now resets `sending→queued` and also runs SEARCH to confirm/clear `uncertain` rows. This prevents residual `uncertain` state from persisting across restarts, matching the ROADMAP requirement that `uncertain` be reconciled via SEARCH, not re‑SMTP.

### Uncertain reconcile path
- **SEARCH‑only path**: use Sent SEARCH by Message-ID to locate the message in Sent. If found → transition row to `sent`; if not found → leave `uncertain` (no second path). This preserves the immutable `.eml` contract and avoids SMTP duplication.

### Migration M11 (preserve‑rows)
- **Conditional**: M11 migration only needed if an M10→M11 schema delta appears (e.g., a new column to track the `search_time` for uncertain reconciliation). If a migration is required, use forward‑only with preserve‑rows tests (per v1.2 audit M9→M10→M11). Otherwise, M10 schema is sufficient for hardening — no new DB columns are required.

### Keyring integration
- **Reuse existing OS keyring path** (no new secret storage). This decision was locked in Phase 32 and carries forward.

### Startup behavior overview
- On process start, `start_sync` resets any `sending` rows back to `queued` and performs SEARCH reconciliation of `uncertain` rows via Message-ID. No live SMTP connections are attempted. All reads use BODY.PEEK only.

## Canonical References (downstream agents MUST read before planning)

- `.planning/ROADMAP.md` — Phase 34 description and v1.6 constraints
- `.planning/REQUIREMENTS.md` — M10 schema definition, forward‑only migrations, preserve‑rows tests
- `.planning/STATE.md` — current_phase=34; stopped_at notes
- `.planning/phases/32-compose-hardening/32-CONTEXT.md` — keyring reuse, STARTTLS Tls::Required, sandbox contracts preserved
- `.planning/phases/32-compose-hardening/32-VERIFICATION.md` — send_queue test results (12/12 green); crash_recovery_resets_only_sending
- `.planning/phases/32-compose-hardening/32-01-PLAN.md` — M10 contract confirmation (draft_open/save APPEND; send_queue insert; keyring path; SMTP STARTTLS config)
- `.planning/milestones/archived-autonomous-20261014/32-01-PLAN.md` — send_queue integration in sync/worker.rs, Sent filing leg
- `.planning/milestones/v1.2-phases/13-send-pipeline/13-CONTEXT.md` — M10 send_queue table definition (id, message_id, state, attempts, next_retry_at, last_error, draft_id, created_at, eml_path, from, to, cc, bcc JSON)
- `.planning/milestones/v1.2-phases/13-send-pipeline/13-01-SUMMARY.md` — M10 send_queue table + queries; send_queue.rs state machine
- `.planning/milestones/v1.2-phases/13-send-pipeline/13-03-PLAN.md` — frozen contract docs from send_queue.rs

## Code Context (reusable assets)

- `src-tauri/src/send_queue.rs` — M10 state machine; SendGate; enqueue path; backoff; crash recovery; contract docs; 12 unit tests (status_snapshot, double_enqueue, bcc_envelope_only, crash_recovery_resets_only_sending)
- `src-tauri/src/store/mod.rs` — M10_SEND_QUEUE_SQL migration wiring; preserve-rows test; queries
- `src-tauri/src/sync/worker.rs` — flush pass on reconnect after replay_dirty_drafts; crash‑recovery reset at launch
- `src-tauri/src/commands/sync.rs` — save_draft/get_draft/discard_draft command bodies; local-first ordering
- `src-tauri/src/lib.rs` — `pub mod send_queue` registration
- `Cargo.toml` / `Cargo.lock` — lettre 0.11 pinned; zero new core dependencies

## Noted for Later (deferred ideas — new capabilities, not part of phase domain)

- Search/filter UI for send_queue rows (belongs in a UI‑focused phase)
- Live SMTP send gating with real credentials (Phase 35)
- Compose UI integration with outbound mail (Phase 36)

## Next Steps

`/gsd-plan-phase 34` — Research and plan the hardened send_queue with the decisions captured above. The canonical CONTEXT.md is now complete and locked for downstream agents (researcher, planner, executor).