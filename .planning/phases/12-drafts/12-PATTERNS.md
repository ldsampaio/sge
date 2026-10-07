# Phase 12: Drafts — Pattern Map

**Mapped:** 2026-10-07
**Method:** inline orchestrator mapping (no `gsd-pattern-mapper` agent available in this runtime). Every file reuses an exact analog from `../10-delete-move/10-PATTERNS.md` (§ refs) or `../11-folder-crud/11-PATTERNS.md`.
**Files analyzed:** 8 (4 backend groups + 3 UI + 1 shared)
**Analogs found:** 8 / 8

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `src-tauri/src/store/mod.rs` + `queries.rs` (M9 `drafts` + queries) | model/store | CRUD | 10-PATTERNS §4 (M2/M7 DDL + preserve-rows test + outbox query templates) | exact |
| `src-tauri/src/drafts.rs` (new; Message-ID + raw RFC 5322 renderer, pure) | service (codec) | pure function | 11-PATTERNS §1 (`mutf7.rs` codec purity + roundtrip test style) | exact |
| `src-tauri/src/imap/mod.rs` (`append_message` + `uid_search_header` verbs) | service (transport) | request-response | 10-PATTERNS §1 (`set_seen` impl 435-450 + drain discipline) | exact |
| `src-tauri/src/imap/manager.rs` (`save_draft_copy_in`, `discard_server_copy_in`) | service (session owner) | request-response | 10-PATTERNS §2 (`move_message_in` lease-hold multi-leg + reconnect-retry) | exact |
| `src-tauri/src/sync/worker.rs` (`replay_dirty_drafts` + MockSession arms) | service (sync engine) | batch | 10-PATTERNS §3 (`replay_imap_outbox` pre/post-sweep + MockSession arm pattern) | exact |
| `src-tauri/src/commands/sync.rs` (`save_draft`/`get_draft`/`discard_draft`) | controller (Tauri command) | request-response | 10-PATTERNS §5 (`set_seen` skeleton 179-276 + `{ acked, pending_count, detail }` tone) | exact |
| `src/components/DraftEditor.tsx` (new) + Drafts view wiring | component | request-response | 10-PATTERNS §7 (`toggleFlag` optimistic + pending wash) + `SyncStatus.tsx:218` poll-timer | role-match |
| `src/components/SyncStatus.tsx` (dirty-draft depth in pending) | component | event-driven | 10-PATTERNS §8 (`sync_status.pending_count` both-outboxes sum) | exact |

## Pattern Assignments

### 1. Store — M9 + draft queries (model)
**Analog:** 10-PATTERNS §4. `SCHEMA_VERSION` 8 → 9; `M9_DRAFTS_SQL` DDL per RESEARCH §4; forward-only + preserve-rows test copying `m7_adds_imap_outbox_and_pending_delete_preserving_rows` (`store/mod.rs:528-573`). Queries mirror outbox templates: `upsert_draft` (`ON CONFLICT(id) DO UPDATE`, no attempts column), `get_draft`, `delete_draft`, `list_dirty_drafts`, `mark_draft_clean(id, server_uid)`. Lock never across `.await`.

### 2. `drafts.rs` — renderer (codec, pure)
**Analog:** 11-PATTERNS §1. Pure functions, no I/O, roundtrip tests against `mail-parser` (like mutf7 roundtrips against decode). `message_id = <{compose_id}@sge.local>` — compose id is a UI uuid, no new crate. Minimal `=?UTF-8?B?` encoded-word helper for Subject/display names.

### 3. `imap/mod.rs` — `append_message` + `uid_search_header` (transport)
**Analog:** 10-PATTERNS §1. Trait entries with UID-only docs; impl drains the stream (`try_collect`) and maps errors `SyncError::Protocol("APPEND …: {e}")` / `("UID SEARCH HEADER …: {e}")`. Flags literal `"(\\Draft \\Seen)"` as a named const (mirrors `seen_store_arg`/`deleted_store_arg`). RESEARCH §1: no APPENDUID parsing — `Session::append` returns `()`.

### 4. `manager.rs` — `save_draft_copy_in` (service)
**Analog:** 10-PATTERNS §2 `move_message_in` lease-hold shape: ONE `lease_for(drafts_wire)`, drive `lease.session().append_message/uid_search_header/store_deleted/uid_expunge` directly, single reconnect-retry around the whole sequence with the same `eprintln!` log shape. Single-UID, no chunking. `SyncError::Refused` never retried.

### 5. `worker.rs` — `replay_dirty_drafts` (batch)
**Analog:** 10-PATTERNS §3. Row-iteration shape of `replay_imap_outbox`; per-row failure keeps `dirty=1`, never fails the sync. MockSession arms (`appended_calls`, `search_header_results`, `fail_append`) next to existing arms. Call site next to the imap_outbox replay.

### 6. `commands/sync.rs` — draft commands (controller)
**Analog:** 10-PATTERNS §5. `load_account_config` → `manager_for` → `spawn_blocking`+`block_on`; Drafts wire resolved via `roles::resolve_roles` at save time; Missing → `Refused("drafts-missing")` for the UI create-confirm flow. Local upsert `dirty=1` before any network (local-first). Result `DraftSaveResult { id, dirty, server_uid, acked, pending_count }`.

### 7-8. UI — `DraftEditor`, Drafts view, SyncStatus depth
**Analog:** 10-PATTERNS §7/§8. Dirty flag mirrors `pendingUids` wash; 30 s `setInterval` copies `SyncStatus.tsx:218`; indicator copy in `SyncStatus` tone ("Salvo"/"Salvando…"); `pending_count` sums dirty drafts (extends the Phase 10 both-outboxes sum).

## No Analog Found
| File | Role | Reason |
|---|---|---|
| `UID SEARCH HEADER Message-ID` reconcile | transport logic | No header-search precedent (`search_uids` is bare `ALL`); planner defines the single/multi/zero-result rule fresh per RESEARCH §2 |
| Raw RFC 5322 renderer | codec output | No MIME-emission precedent (lettre arrives Phase 14); hand-rolled minimal renderer with mail-parser round-trip proof |

## Metadata
**Analog search scope:** `src-tauri/src/imap/`, `src-tauri/src/sync/`, `src-tauri/src/store/`, `src-tauri/src/commands/`, `src/components/`, `src/`
**Pattern extraction date:** 2026-10-07
