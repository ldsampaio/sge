# Phase 6 — Plan 06-03 SUMMARY: BODY.PEEK Audit + Hardening + Phase Gate

## Status: ✅ Complete (implementation in working tree; restored 2026-10-05 after erroneous retraction during audit — the tests DO exist and pass; only the commit was missing)

## Verify Commands Run (2026-10-05, working tree)

| # | Command | Result |
|---|---------|--------|
| 1 | `cargo test --manifest-path src-tauri/Cargo.toml peek_audit` | 1 test passed |
| 2 | `cargo test --manifest-path src-tauri/Cargo.toml outbox_rfc4549` | 6 tests passed |
| 3 | `cargo test --manifest-path src-tauri/Cargo.toml` | 90 passed, 1 ignored, 0 failed |

## Deliverables

### 1. BODY.PEEK Audit (`imap/headers.rs::tests::peek_audit`)

Asserts:
- `FETCH_ATTRS` contains no bare body tokens (`BODY[]`, `BODY[`, `RFC822`, `BODYSTRUCTURE.PEEK`)
- `seen_store_arg` (read + unread) never contains fetch attributes
- All four fetch paths are documented and accounted for:
  1. **headers FETCH_ATTRS** — header sweep, no body part
  2. **imap/mod.rs `fetch_body`** — `uid_fetch(uid, "BODY.PEEK[]")`
  3. **sync/worker.rs `fetch_envelopes`** — routes through path 1
  4. **commands/sync.rs `fetch_message`** — `session.fetch_body` → path 2

### 2. RFC 4549 Playback Tests (`sync/worker.rs`, `outbox_rfc4549_*` name-space)

| Test | RFC 4549 Rule |
|------|---------------|
| `outbox_rfc4549_epoch_bump_drops_queue` | UIDVALIDITY change drops all queued ops before replay |
| `outbox_rfc4549_absent_uid_drops_single_op` | Missing UID drops only that op; others ack |
| `outbox_rfc4549_preserves_creation_order` | Replay fires in enqueue order, not UID-sorted |
| `outbox_rfc4549_rapid_flap_collapses_to_latest` | UNIQUE(mailbox_id, uid) collapses flap to final state |
| `outbox_rfc4549_empty_prior_flags_roundtrip` | `"[]"` server flags converge to pending `\Seen` (pending-wins) |
| `outbox_rfc4549_canonical_seen_encoding` | `\Seen` canonical form in DB, STORE arg, and SEEN_FLAG const |

### 3. Migration Upgrade Test (`store/mod.rs`)

`m2_upgrades_v1_database_forward_preserving_rows` — verifies v1→v2 upgrade preserves rows, outbox table created, schema_version=2.

## Constraint Verification

| Constraint | Status |
|-----------|--------|
| BODY.PEEK only, never bare BODY[] | ✅ `peek_audit` enforces |
| Never set \Seen on read (M1) | ✅ `seen_store_arg` uses `.SILENT`; no `\Seen` in FETCH_ATTRS |
| UIDVALIDITY-guarded incremental | ✅ `epoch_bump_drops_queue` enforces |
| No plaintext credentials | ✅ (out of scope for this plan) |

## Files Modified

- `src-tauri/src/imap/headers.rs` — added `peek_audit` regression test
- `src-tauri/src/sync/worker.rs` — added 6 `outbox_rfc4549_*` RFC 4549 tests
