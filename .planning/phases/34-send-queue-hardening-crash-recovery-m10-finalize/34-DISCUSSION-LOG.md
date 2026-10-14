# Phase 34: Send Queue Hardening + Crash Recovery (M10 finalize) - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-10-14
**Phase:** 34 - Send Queue Hardening + Crash Recovery (M10 finalize)
**Areas discussed:** State-machine hardening, Crash recovery at start_sync, Uncertain reconcile path, Migration M11 (preserve-rows)

---

## State-machine hardening

| Option | Description | Selected |
|--------|-------------|----------|
| Preserve v1.2 frozen contract | Keep frozen 5-state (queued|sending|sent|failed|uncertain) with attempts/backoff (30s base / 15min cap / ±20% jitter), 25 MB cap enforced at enqueue. Matches v1.2 Phase 13 contract and 32-VERIFICATION (all 12 send_queue tests green). | ✓ |
| Refine state machine | Add sub-state tracking (e.g., sending→awaiting-smtp) or tighten backoff bounds — would change send_queue.rs state machine and test assertions. | |
| Document only — no change | Only document the existing frozen contract explicitly in CONTEXT.md (no behavioral change) — minimal risk given 32 audit passed. | |

**User's choice:** Preserve v1.2 frozen contract
**Notes:** No behavioral changes. Cap enforcement note: 25 MB cap is enforced by idempotent inserts; no changes needed.

---

## Crash recovery at start_sync

| Option | Description | Selected |
|--------|-------------|----------|
| Keep existing reset | Reset `sending→queued` at `start_sync` entry (existing v1.2 behavior, 32-VERIFICATION `crash_recovery_resets_only_sending` green). Never lose queued items; no `sent`/`failed` mutation. | |
| Extend to startup reconcile | Extend reset to also reconcile `uncertain` via Sent SEARCH by Message-ID at startup (not just flush-time). Matches ROADMAP 'uncertain reconcile'. | ✓ |
| Conditional (deferred) | Make reset configurable / conditional — but no config mechanism exists in send_queue; would require new settings table (scope creep). | |

**User's choice:** Extend to startup reconcile (SEARCH only)
**Notes:** `start_sync` now resets `sending→queued` and also runs SEARCH to confirm/clear `uncertain` rows. Prevents residual `uncertain` state from persisting across restarts.

---

## Uncertain reconcile path

| Option | Description | Selected |
|--------|-------------|----------|
| SEARCH-only, no re-SMTP | Sent SEARCH by Message-ID (not re-SMTP) to confirm sent; if found → `sent`; else stay `uncertain`. Preserves immutable `.eml` bytes and avoids SMTP retry of already-sent items. Matches ROADMAP 34 description. | ✓ |
| Re-SMTP retry (rejected by prior audit) | Re-SMTP attempt with new envelope — risks duplication, breaks immutable-byte contract, violates 'reconcile-not-resend' from v1.2 audit. | |
| SEARCH-first, hold uncertain | Hybrid: SEARCH first; if uncertain after SEARCH, keep `uncertain` (no second path). Keeps decision simple. | |

**User's choice:** SEARCH-only path
**Notes:** Use Sent SEARCH by Message-ID to locate the message in Sent. If found → transition row to `sent`; if not found → leave `uncertain` (no second path). This preserves the immutable `.eml` contract and avoids SMTP duplication.

---

## Migration M11 (preserve-rows)

| Option | Description | Selected |
|--------|-------------|----------|
| Conditional M11 with test | Only if M10→M11 schema delta found (e.g., new column for attempts cap tracking). Preserve-rows test required per v1.2 audit (M9→M10→M11 forward-only preserved). | ✓ |
| No M11 needed | No migration needed — M10 schema sufficient for hardening (state-machine logic lives in `send_queue.rs`, not DB). Confirm via `store/mod.rs` diff. | |
| Bump version (deferred — pre-existing) | Add defensive schema version bump (SCHEMA_VERSION 11) regardless — but v1.2 audit already notes `SCHEMA_VERSION = 11` vs test expecting 10 (pre-existing); avoid repeating. | |

**User's choice:** Conditional M11 migration
**Notes:** M11 migration only needed if an M10→M11 schema delta appears (e.g., a new column to track the `search_time` for uncertain reconciliation). If a migration is required, use forward‑only with preserve‑rows tests. Otherwise, M10 schema is sufficient for hardening — no new DB columns are required.

---

## the agent's Discretion

- Keyring integration: reuse existing OS keyring path (no new secret storage). Locked in Phase 32 and carries forward.
- Deferred ideas: search/filter UI for send_queue rows (belongs in a UI-focused phase); live SMTP send gating with real credentials (Phase 35); compose UI integration with outbound mail (Phase 36).

## Deferred Ideas

- Search/filter UI for send_queue rows — belongs in a UI-focused phase
- Live SMTP send gating with real credentials — Phase 35
- Compose UI integration with outbound mail — Phase 36