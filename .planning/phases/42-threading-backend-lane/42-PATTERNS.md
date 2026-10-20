# Phase 42: Threading Backend Lane — Pattern Map

**Mapped:** 2026-10-20
**Files analyzed:** 12 (2 new modules + 1 capture script + 9 modified)
**Analogs found:** 12 / 12 (all analogs are git-TRACKED source — `git ls-files` verified for every path named below; no gitignored mirror/runtime paths are cited)

Every anchor cited was re-opened live in this session. Line numbers are current state on `main` (HEAD).

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `src-tauri/src/imap/references.rs` (NEW) | leaf parser module (pure fn) | transform (bytes → normalized id chain) | `src-tauri/src/imap/mutf7.rs:1` | exact (pure leaf codec + fixture suite + `mod.rs` registration) |
| `src-tauri/src/store/threads.rs` (NEW) | service wrapper (SQL ↔ crate ↔ UPDATE) | batch/epoch CRUD | `src-tauri/src/store/queries.rs:793` (`get_reply_context`) + `:1487` (`set_pending_delete`) | role-match (only SQL-wrapper module in `store/`) |
| `src-tauri/src/imap/mod.rs` (MODIFY, 1 line) | module registration | n/a | `src-tauri/src/imap/mod.rs:15-24` (`pub mod` block) | exact |
| `src-tauri/src/store/mod.rs` (MODIFY) | migration/config + fixture tests | CRUD (schema) | `store/mod.rs:193-207` (M25 const) / `:260` (ALTER precedent) / `:2047-2075` (upgrade fixture pair) | exact |
| `src-tauri/src/store/schema.sql` (MODIFY = **verify, do not touch**) | config (v1 baseline) | file-I/O | `src-tauri/src/store/schema.sql:29-43` | exact (forward-migration-only invariant) |
| `src-tauri/src/imap/headers.rs` (MODIFY) | parser + wire lane | request-response (IMAP FETCH) | `src-tauri/src/imap/headers.rs:293-313` (`fetch_envelope_batch`) | exact (self-analog: new sibling lane) |
| `src-tauri/src/store/queries.rs` (MODIFY) | SQL home (single-SQL-module) | CRUD | `queries.rs:491-511` (`UPSERT_MESSAGE_SQL`) + `:874-900` (`list_messages`) | exact |
| `src-tauri/src/sync/worker.rs` (MODIFY) | sync engine driver (backfill lane + epoch hooks + mock) | streaming/batch | `worker.rs:1556-1643` (chunk loop + gap-diff + strikes) | exact |
| `src-tauri/src/commands/sync.rs` (MODIFY) | Tauri command (IPC payload) | request-response | `commands/sync.rs:1103-1119` (`list_messages`) + `:915` (`get_draft`) | exact |
| `src-tauri/src/lib.rs` (MODIFY, 1 line) | command registration | request-response | `src-tauri/src/lib.rs:503` / entry `:529` | exact |
| `src/types.ts` (MODIFY) | TS payload mirror | request-response | `src/types.ts:4-15` (`MessageRow`) | exact |
| `scripts/live-42.sh` (NEW) | capture script (deferred live leg) | file-I/O | `scripts/live-26.sh:1` (tracked) | role-match |
| `src-tauri/Cargo.toml` (MODIFY, 1 pin) | config | n/a | `src-tauri/Cargo.toml:78` (`chrono = "0.4"`) | exact |

## Pattern Assignments

### `src-tauri/src/imap/references.rs` (NEW — parser module, transform)

**Analog:** `src-tauri/src/imap/mutf7.rs:1` (tracked; 350 lines) — pure leaf module: file-level `//!` doc stating the grammar + rules + degradation policy, no imports, public pure fns, `#[cfg(test)] mod tests` fixture block at `:234`.

```rust
//! IMAP modified UTF-7 codec (RFC 3501 §5.1.3) for mailbox names.
// ...
/// Decode an IMAP modified-UTF-7 mailbox name to display form.
///
/// Rules: printable ASCII (except `&`) passes through; `&-` is a literal
/// `&`; `&<modified-base64>-` decodes as UTF-16BE (`,` stands in for `/`).
/// Malformed shifts degrade to U+FFFD rather than failing — a folder name
/// must never break the tree render.
pub fn decode_modified_utf7(name: &str) -> String {
```

**Collision rules for the new module:**
- MUST NOT import `headers::decode_header_words` (`headers.rs:120-188`). Proof already in the repo: that decoder scans `=?` prefixes and rewrites Q-words (`<X=?utf-8?Q?=42?=@host>` → `<XB@host>`). Keep a contrast unit test beside the parser tests (assert its output length differs), per PITFALLS #2.
- MUST reuse `headers.rs:103-109` `decode_bytes_opt` semantics (raw lossy decode + `trim_end_matches('\r')`, no 2047 pass) for the msg-id bytes that feed it.
- Module owns no I/O and no SQL; parse point is exactly once at fetch time (store keeps the normalized JSON verbatim).
- Fixture names are the contract (`folded_two_line`, `framed_multi`, `bare_and_case`, `malformed_or_empty`, `duplicate_ids`, `chain_cap_overflow`, `encoded_shaped_ids`) and are the `cargo test -p sge references_` filter in the VALIDATION map.

**Registration line that must also change** — `src-tauri/src/imap/mod.rs:15-24`, alphabetical, insert `pub mod references;` between `pub mod probe;` (`:22`) and `pub mod roles;` (`:23`):

```rust
pub mod bodies;      // :15
pub mod errors;
pub mod headers;
pub mod idle;
pub mod manager;
pub mod mutf7;
pub mod probe;       // :22  ← insert `pub mod references;` here
pub mod roles;       // :23
pub mod session;
pub mod trash;       // :24
```

Secondary analog for the doc-comment/paranoia style of a small leaf module: `src-tauri/src/imap/roles.rs:1` ("never a second detector to drift" — one implementation, thin wrappers).

---

### `src-tauri/src/store/threads.rs` (NEW — wrapper, per-folder epoch)

**Analog A (scoped folder SQL + typed row):** `src-tauri/src/store/queries.rs:793-810` `get_reply_context`, plus its row struct at `:775-786`.

```rust
pub fn get_reply_context(
    conn: &Connection,
    mailbox: &str,
    uid: u32,
) -> StoreResult<Option<ReplyContextRow>> {
    match conn.query_row(
        "SELECT m.from_addr, m.subject, m.cc_addrs, m.message_id, m.preview, \
         (SELECT b.body_text FROM message_bodies b WHERE b.message_id = m.id) \
         FROM messages m JOIN mailboxes mb ON m.mailbox_id = mb.id \
         WHERE mb.name = ?1 AND m.uid = ?2",
        rusqlite::params![mailbox, uid],
```

**Analog B (scoped single-column UPDATE — shape of the epoch's UPDATE):** `src-tauri/src/store/queries.rs:1487-1495`:

```rust
pub fn set_pending_delete(
    conn: &Connection,
    mailbox_id: u64,
    uid: u32,
    pending: bool,
) -> StoreResult<()> {
    conn.execute(
        "UPDATE messages SET pending_delete = ?1 WHERE mailbox_id = ?2 AND uid = ?3",
        rusqlite::params![pending, mailbox_id, uid],
    )?;
    Ok(())
}
```

**Analog C (one-transaction batch, only one in the repo):** `src-tauri/src/classify/rules_io.rs:438` — `let tx = guard.conn_mut().transaction().map_err(|e| e.to_string())?;` then all writes go through `&tx`. Reuse this shape for the batched `thread_id` UPDATE (WAL churn under the single-writer Store mutex).

**Analog D (batch-loop + explicit "no tx wrapper needed" doc):** `src-tauri/src/store/queries.rs:543-551` (`upsert_messages_batch` doc explains when a transaction is NOT needed — mirror that reasoning for the epoch).

**Collision rules:**
- All SQL in the new file stays inside `store/` (`queries.rs:1-5` invariant comment: "No raw SQL escapes this module"). No SQL in `sync/`, `commands/`, or `imap/`.
- Compute scope is strictly `WHERE mailbox_id = ?` and MUST carry the same predicate fragment as `list_messages` (`m.pending_delete = 0`, `queries.rs:885`) — PITFALLS #5/#7: never group on bare `thread_id`; the contract is `thread_key = (mailbox_id, thread_id)`; NULL `thread_id` = own-thread sentinel.
- Never trust `mail-threading`'s `Thread.messages` order: persist only after a `(date_utc, uid)` re-sort (`queries.rs:886` is the same tiebreak pair — reuse the identical tuple, do not invent a third key).
- Early-exit on zero dirty rows; never call the epoch from inside `upsert_message`.
- Registration line that must also change: `src-tauri/src/store/mod.rs:14` — `pub mod queries;` → append `pub mod threads;` (the only `pub mod` in that file today).

---

### `src-tauri/src/imap/headers.rs` (MODIFY — wire lane + parse paths + guard tests)

**Analog:** the file's own `fetch_envelope_batch` at `:293-313` (the template for `fetch_refs_batch`).

```rust
pub async fn fetch_envelope_batch(
    session: &mut super::BoxedSession,
    range: &str,
) -> Result<Vec<MessageHeader>, SyncError> {
    let mut stream = session
        .uid_fetch(range, FETCH_ATTRS)
        .await
        .map_err(|e| SyncError::Protocol(format!("UID FETCH: {e}")))?;
    let mut out = Vec::new();
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|e| SyncError::Protocol(format!("FETCH stream: {e}")))?
    {
        let uid = fetch.uid.ok_or_else(|| SyncError::Protocol("missing UID in FETCH response".into()))?;
        out.push(parse_fetch_item(&fetch, uid));
    }
    Ok(out)
}
```

**Collision rules:**
- New constant is a **sibling**, never an edit of `FETCH_ATTRS` (`headers.rs:21`): `pub const FETCH_REFS_ATTRS: &str = "(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])";` placed directly below it. The tripwire at `headers.rs:440` (`!FETCH_ATTRS.contains("BODY.PEEK[HEADER")`) actively fights any inline addition — it must stay green.
- `MessageHeader` (`headers.rs:25-37`, 11 fields) gains additive fields only; `PartialEq`/`Eq` derive at `:24` stays — every existing test that constructs `MessageHeader { … }` literally (mock builders in `worker.rs:2115`) must still compile, so do NOT rename existing fields.
- `in_reply_to` copies from `Envelope.in_reply_to` on **both** parse paths: `parse_fetch_item` (`:233-285`, tuple destructure at `:244-262` + struct literal at `:272-284`) and `parse_fetch_attributes` (`:509-563`, struct pre-init at `:510-522` + `AttributeValue::Envelope` arm at `:527-543`). Both must produce identical `MessageHeader` shape.
- References bytes go through `Fetch::section` (`async-imap-0.11.3/src/types/fetch.rs:179-180`, `Option<&[u8]>`), never `fetch.body()` (`imap/mod.rs:fetch_body` path) and never `BODY[HEADER.FIELDS …]` without `.PEEK`.
- `peek_audit` (`headers.rs:463-504`) gains path 5: add a 5th tuple to `audited_paths` (`:487-498`) and change `assert_eq!(audited_paths.len(), 4)` (`:499-503`) to 5. `fetch_attrs_are_parenthesized` (`:428-443`) gains `FETCH_REFS_ATTRS` paren + attribute assertions.

**Guard excerpts to extend (verbatim):** `headers.rs:439-442` tripwire + `headers.rs:487-503` audit tail:

```rust
        assert!(
            !FETCH_ATTRS.contains("BODY[]") && !FETCH_ATTRS.contains("BODY.PEEK[HEADER"),
            "header sweep must not fetch bodies (read-only invariant): {FETCH_ATTRS}"
        );
        // … audited_paths tuples (4 today) …
        assert_eq!(
            audited_paths.len(),
            4,
            "peek_audit: all four fetch paths must be accounted for"
        );
```

**SAME-COMMIT INVARIANT #1 (collision rule):** the `FETCH_REFS_ATTRS` constant, the lane call site, the `peek_audit` 4→5 count assert, and the `fetch_attrs_are_parenthesized` extension land in **one commit**. A lane without the audit = red build; a `BODY[HEADER.FIELDS (REFERENCES)]` (missing `.PEEK`) perma-corrupts unread state server-side in one sweep with no client undo. No dev run against the live mailbox before this gate is green.

---

### `src-tauri/src/store/mod.rs` (MODIFY — M26 + SCHEMA_VERSION + fixture pair)

**Analog (ALTER-only migration const):** `store/mod.rs:260` inside `M4_BACKFILL_SQL`:

```rust
    "CREATE INDEX idx_tombstone_mailbox ON fetch_tombstones(mailbox_id);",
    "ALTER TABLE mailboxes ADD COLUMN sweeps_since_full INTEGER NOT NULL DEFAULT 0;",
);
```
One-line ALTER precedent: `store/mod.rs:271` `const M5_STATUS_TS_SQL: &str = "ALTER TABLE mailboxes ADD COLUMN status_synced_at TEXT;";`
Const naming/`concat!` layout precedent: `store/mod.rs:193-207` (`M25_LLM_MODELS_TABLE_SQL`).

**Analog (fixture pair):** `store/mod.rs:2042-2075` — `mod m23_llm_settings_gate_tests` with the truncated-baseline helper and the preserve-rows test:

```rust
#[cfg(test)]
mod m23_llm_settings_gate_tests {
    use super::*;
    use rusqlite_migration::{Migrations, M};

    /// Migration prefix through M22 (as shipped before Plan 39-03).
    fn migrations_through_m22() -> Migrations<'static> {
        Migrations::new(vec![
            M::up(include_str!("schema.sql")),
            M::up(M2_FLAG_OUTBOX_SQL),
            // … through M22 only …
        ])
    }

    #[test]
    fn m23_upgrade_from_v22_preserves_rows_and_seeds_settings() {
```
Second precedent: `store/mod.rs:1560` `fn m2_upgrades_v1_database_forward_preserving_rows()` (rebuilds a v1 DB, seeds a `messages` row, asserts it survives).

**Analog (the stale-version ambush):** `store/mod.rs:888` `fn schema_version_is_25_with_m23_llm_settings()` with `assert_eq!(SCHEMA_VERSION, 25);` at `:891`. Repo-wide grep confirms `SCHEMA_VERSION` appears exactly twice: definition `:19` and that assert `:891`.

**Collision rules:**
- Append `M::up(M26_THREADING_SQL)` after `M::up(M25_LLM_MODELS_TABLE_SQL)` (`store/mod.rs:107`, last entry of the vec at `:83-107`).
- M26 SQL = `ALTER … ADD COLUMN` only + one `CREATE INDEX`. **Zero `CREATE TABLE` copies of existing tables** (the M11/M17 rebuild-and-rename pattern at `:421-:423` / `:568` is NOT needed for ADD COLUMN and must not be copied).
- Fixture pair names are the `cargo test -p sge m26` filter; upgrade test must rebuild the baseline **through M25** (clone `migrations_through_m22` → `migrations_through_m25`), pre-seed ≥2 `messages` rows + mailbox state, apply production `apply_migrations`, assert every prior row byte-identical + new columns NULL + done-bit 0.

**SAME-COMMIT INVARIANT #2 (collision rule):** `M26_THREADING_SQL` + the vec append + `pub const SCHEMA_VERSION: u32 = 25` (`:19`) → `26` + the rename/extension of `schema_version_is_25_with_m23_llm_settings` (`:888`, assert `:891`) all land in **one commit**. A surviving `assert_eq!(SCHEMA_VERSION, 25)` is itself a red tripwire; M26 SQL without the bump = migration never applies in production while fresh in-memory tests stay green.

---

### `src-tauri/src/store/schema.sql` (MODIFY-listed but **UNTOUCHED** — confirm only)

Current `messages` table, verified live at `schema.sql:29-43` (whole file is 76 lines):

```sql
CREATE TABLE messages (
  id            INTEGER PRIMARY KEY,
  mailbox_id    INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,              -- IMAP UID, unique per mailbox
  message_id    TEXT,                          -- RFC822 Message-ID (nullable, not trusted as key)
  subject       TEXT NOT NULL DEFAULT '',
  from_addr     TEXT NOT NULL DEFAULT '',
  to_addrs      TEXT NOT NULL DEFAULT '',      -- JSON array (M1-simple; normalize later)
  cc_addrs      TEXT NOT NULL DEFAULT '[]',
  date_utc      TEXT NOT NULL,                 -- INTERNALDATE preferred over Date: header
  flags         TEXT NOT NULL DEFAULT '[]',    -- JSON: Seen, Flagged, Answered...
  has_attachments INTEGER NOT NULL DEFAULT 0,
  preview       TEXT NOT NULL DEFAULT '',      -- first ~200 chars of text/plain, for list + FTS
  UNIQUE (mailbox_id, uid)
);
```

Confirmed shape: `message_id` nullable **already exists** at `:33`; there is **no** `in_reply_to`, `references_json`, `thread_id`, nor any thread index anywhere in the file. `mailboxes` (`:18-25`) has no backfill done-bit.
**Collision rule:** forward-migration-only — these three columns + the `mailboxes` done-bit appear **only** in M26 ALTERs; adding them here would fork two schema baselines and break every upgrade fixture.

---

### `src-tauri/src/store/queries.rs` (MODIFY — additive SQL + payload row)

**Analog (SQL const + fn params growth):** `queries.rs:491-511` `UPSERT_MESSAGE_SQL` (11 params) + `:512` `upsert_message` signature (verified param order: `mailbox_id, uid, message_id, subject, from_addr, to_addrs, cc_addrs, date_utc, flags, has_attachments, preview`):

```rust
const UPSERT_MESSAGE_SQL: &str = concat!(
    "INSERT INTO messages ",
    "(mailbox_id, uid, message_id, subject, from_addr, to_addrs, ",
    " cc_addrs, date_utc, flags, has_attachments, preview) ",
    "VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ",
    "ON CONFLICT(mailbox_id, uid) DO UPDATE SET ",
    "  message_id      = excluded.message_id, ",
    // … one `field = excluded.field` line per column …
);
```

**Analog (additive SELECT behind a frozen signature):** `queries.rs:874-900`:

```rust
pub fn list_messages(
    conn: &Connection,
    mailbox: &str,
    limit: usize,
    offset: usize,
) -> StoreResult<Vec<MessageRow>> {
    let sql = concat!(
        "SELECT m.uid, m.subject, m.from_addr, m.to_addrs, ",
        "m.date_utc, m.flags, m.has_attachments, m.preview, mb.name ",
        "FROM messages m ",
        "JOIN mailboxes mb ON m.mailbox_id = mb.id ",
        "WHERE mb.name = ?1 AND m.pending_delete = 0 ",
        "ORDER BY m.date_utc DESC, m.uid DESC ",
        "LIMIT ?2 OFFSET ?3",
    );
```

**Analog (header-only member SELECT shape for `get_thread`):** `queries.rs:746-763` `get_message_by_uid` (returns `Option<MessageRow>`, `QueryReturnedNoRows → Ok(None)` — keep that three-arm match).

**Analog (contract row that feeds threading today):** `ReplyContextRow` `queries.rs:775-786` (`message_id` → `In-Reply-To`) — untouched contract; do not fork a second threading-header contract.

**Collision rules:**
- `MessageRow` (`:15-28`, 9 fields, `#[derive(Debug, Clone, Serialize)]` at `:15`) grows additively; existing field order in the SELECT at `:885-:887` (`WHERE … pending_delete = 0` / `ORDER BY m.date_utc DESC, m.uid DESC` / `LIMIT ?2 OFFSET ?3`) is frozen — append new columns after `mb.name` only, and keep `row_to_message` index mapping in lockstep (byte-compat sentinel test asserts the 9 original fields per row are identical).
- Any new member/thread SQL reuses the **exact** predicate fragment `m.pending_delete = 0` (as at `:885`, `:992`, `:2982`) — a divergent predicate makes a thread member appear in the list but not in `get_thread`.
- One serde round-trip test per new command key set, both directions, on the Rust side (no TS runner exists).

---

### `src-tauri/src/sync/worker.rs` (MODIFY — backfill lane + epoch hooks + mock mirror)

**Analog (chunked range loop + requested-vs-received accounting):** `worker.rs:1556-1643`, in particular the loud empty-stream guard at `:1592-1601` and the gap range-diff + strike writes at `:1606-1643`:

```rust
// async-imap's FETCH stream ends silently (no error) when the
// server rejects the command with NO/BAD — that once produced a
// "successful" 0-message sync. A non-empty request must yield
// headers; otherwise fail loudly instead of syncing nothing.
if headers.is_empty() {
    return Err(SyncError::Protocol(format!(
        "UID FETCH {range_str} returned no headers for {} requested UID(s) — \
         server may have rejected the command (see FETCH log line above)",
        chunk.len()
    )));
}
```
and the gap/strike half:
```rust
let returned: HashSet<u32> = headers.iter().map(|h| h.uid).collect();
let gap: Vec<u32> = chunk.iter().copied().filter(|u| !returned.contains(u)).collect();
// … re-request the gap once, then record_fetch_strike per still-missing uid …
```

**Analog (batching knob):** `worker.rs:37` `const BATCH_SIZE: u32 = 200;` — the backfill lane reuses chunking/resizable batches (D-02), plus `src-tauri/src/sync/mod.rs:91` `pub struct SyncGate` single-flight (`try_begin`, `sync/mod.rs:88-103`).

**Analog (hook site):** Step-5 marker `worker.rs:1492`; upsert loop ends ~`:1736`; Step 6 at `:1739`; Step 7 state write at `:1751` → recompute epoch goes **after the Step-5 batch loop and before `:1751`**.

**Analog (send-file hook):** the three APPEND-success branches — `:766` (`Ok(())` → mark-sent `:781`), `:813`, `:863` inside the ranges `:764-:800`, `:825`, `:873`; all bounded by `Self::filing_op` (`worker.rs:657`).
**Correction carried from RESEARCH:** CONTEXT.md:74 cites `send_queue.rs` — the Sent-filing hook belongs to these `worker.rs` APPEND-success sites, not to `src-tauri/src/send_queue.rs` (crate root, `enqueue_send:291`); keep the queue module byte-stable.

**Analog (mock mirror for the new lane):** `MockSession` struct `worker.rs:1810-1828` and its `fetch_envelopes` impl at `:1894` with the "only requested UIDs come back" filter (`:1903`). Mirror it as `fetch_refs_batch`-equivalent so `cargo test -p sge backfill_` / `send_file_join_sent_thread` exercise the lane offline.

**Collision rules:** tombstoned UIDs stay excluded from the backfill sweep set (`:1523`/`:1534` precedent); the per-folder done-bit lives in the DB (`mailboxes` ALTER inside M26), not a state.json sidecar; yield between ranges so `sync_status` stays live; the lane must diff requested-vs-received UIDs (a `section()` miss surfaces as **fewer items, no error**).

---

### `src-tauri/src/commands/sync.rs` (MODIFY — frozen command + new `get_thread`)

**Analog (frozen command body):** `commands/sync.rs:1103-1119` `list_messages` — `#[tauri::command]` at `:1102`, args `mailbox, limit, offset`, returns `Result<Vec<crate::store::queries::MessageRow>, String>` via `spawn_blocking` + `store.lock()` + `map_err(format!("store: {e}"))`. Body untouched; only `MessageRow` grows.

**Analog (new command shape):** `commands/sync.rs:915` `get_draft` (small store-read returning a typed struct) and `MessageView` `:2224-2239` for the "typed payload struct" convention.
**Untouchable:** `MessageView` at `:2224-2239` — `get_thread` is a separate command with its own header-only row type (no body fields).

**Collision rules:** camelCase→snake_case invoke convention for args follows `list_messages` (`mailbox`, `uid`); the TS mirror lands in the same commit; command name is exactly `get_thread`.

---

### `src-tauri/src/lib.rs` (MODIFY — 1 line)

`lib.rs:503` `.invoke_handler(tauri::generate_handler![` … existing entry `commands::sync::list_messages` at `:529`. Add `commands::sync::get_thread,` in the same list.
**Collision rule:** an unregistered command is dead code at runtime (Tauri v2 silently drops unknown args) while every Rust test stays green — registration is the same commit as the command.

---

### `src/types.ts` (MODIFY — mirror)

`src/types.ts:4-15` `export interface MessageRow { uid; mailbox; subject; from_addr; to_addrs; date_utc; flags; has_attachments; preview }` — grows additively, **required fields non-optional** (`tsc` fails when Rust drops one).
`src/types.ts:39-42` stale comment "Original References chain. **NOT parsed by the backend today**…" and `:43` `references?: string | null;` — comment must be updated in the **same commit** that lands the lane (no stale doc survives into Phase 43+).

**Collision rule (PITFALLS #1):** TS and Rust payload fields land in one commit; "tests green, feature absent" is the default suspicion. Round-trip proofs live on the Rust side — do **not** introduce vitest (zero-new-deps rule, REQUIREMENTS.md:16).

---

### `scripts/live-42.sh` (NEW — deferred live leg)

**Analog:** `scripts/live-26.sh:1` (tracked) — `#!/usr/bin/env bash`, header comment stating what it drives + `EXIT: 0 all-pass · 1 assertion FAIL or secret-leak · 2 pre-flight skip`, `set -uo pipefail`, `ENV_FILE="src-tauri/.live-test.env"` (git-ignored env file, never CLI args).
**Collision rule:** live legs are deferred-with-reason (no creds; keyring-only). Section A = capture `HEADER.FIELDS (REFERENCES)` bytes verbatim into the fixture corpus; section B = Zimbra refusal probe confirming the pre-declared `References = In-Reply-To` shallow-chain fallback. Never print credentials (CODESTYLE.md).

---

### `src-tauri/Cargo.toml` (MODIFY — 1 pin)

Insert `mail-threading = "0.1"` in `[dependencies]` (block at `:47`, `chrono = "0.4"` at `:78`, `[dev-dependencies]` at `:85`; verified absent today: `mail-threading` is not in Cargo.lock). Do **not** enable its serde feature — map `Thread` output manually in `store/threads.rs`.
**Gate:** `mail-threading` 0.1.3 is rated **[SUS]** (low downloads) by the Package Legitimacy Audit — the planner must keep a `checkpoint:human-verify` task ahead of this Cargo edit and keep the D-03 in-house ~250-LOC JWZ fallback live in the same phase. No other dependency changes (async-imap 0.11 / rusqlite 0.37 / rusqlite_migration 2 stay pinned).

## Shared Patterns

### Same-commit atomicity (the two hard invariants)
**Source:** CONTEXT D-01..D-05 + PITFALLS #3/#4; both are collision rules the planner must encode as one-task-per-invariant.
1. **`\Seen` guard commit:** `FETCH_REFS_ATTRS` constant + `fetch_refs_batch` lane + `peek_audit` path 5 (`audited_paths` `headers.rs:487-498`, count assert `:499-503` 4→5) + `fetch_attrs_are_parenthesized` extension (`:428-443`) in ONE commit. Proofs: `cargo test -p sge peek_audit`, `git grep -n 'BODY\[HEADER.FIELDS' src-tauri/src` ⇒ every hit `.PEEK`-prefixed.
2. **Schema commit:** `M26_THREADING_SQL` + `M::up` append (`store/mod.rs:107`) + `SCHEMA_VERSION` 25→26 (`store/mod.rs:19`) + rename of `schema_version_is_25_with_m23_llm_settings` (`:888`, assert `:891`) + fresh/upgrade fixture pair in ONE commit. Proofs: `cargo test -p sge m26`, `cargo test -p sge schema_version`.

### Guard-test extension (never bypass)
**Source:** `src-tauri/src/imap/headers.rs:428-443` and `:463-504` — extend existing assertions; do not add a parallel guard that leaves the old one passing vacuously.

### Migration fixture pair
**Source:** `store/mod.rs:1560` + `:2047-2075`. Every forward migration ships fresh (`Store::open_in_memory()` through the full vec) + upgrade (truncated baseline, pre-seed rows, apply production `apply_migrations`, assert byte-identical rows).

### UID-range batching + requested-vs-received accounting
**Source:** `src-tauri/src/imap/headers.rs:293-313` (lane issuer) + `src-tauri/src/sync/worker.rs:1556-1643` (chunk loop, empty-stream loud-fail, gap diff, strikes) + `worker.rs:1894` mock discipline. Apply to the References lane and the per-folder backfill (D-02).

### Scoped-folder SQL discipline
**Source:** `store/queries.rs:885` / `:992` / `:2982` (`m.pending_delete = 0`) and `:1487` (`WHERE mailbox_id = ?2 AND uid = ?3`). Apply to the thread SELECT, epoch UPDATE, and `get_thread` member SELECT: always `WHERE mailbox_id = ?` + `pending_delete = 0`, and expose grouping only as `thread_key = (mailbox_id, thread_id)`.

### Error taxonomy + logging
**Source:** `imap/mod.rs` `SyncError` (`Protocol/Io/Parse/State/Refused`) + CODESTYLE.md: `map_err(|e| SyncError::Protocol(format!("UID FETCH: {e}")))` at every await; store errors mapped `format!("store: {e}")` at the command boundary (`commands/sync.rs:1116`); `eprintln!("[SGE …] {e}")` never prints credentials.

### Payload mirror + serde round-trip
**Source:** `store/queries.rs:15` (`#[derive(Debug, Clone, Serialize)]`), `commands/sync.rs` return types, `src/types.ts:4-15`. One round-trip test per new command key set in each direction; TS required fields non-optional; gated by `npm run lint && npm run format:check && tsc` (no TS test runner).

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `src-tauri/src/store/threads.rs` | service wrapper | batch epoch over a 3rd-party crate | No JWZ/threading code exists anywhere in the repo (`grep thread_id` → 0 hits in `store/`); closest analogs are role-match (`queries.rs`) not exact. Crate API comes from RESEARCH.md (docs.rs) |
| `src-tauri/src/imap/references.rs` | pure byte parser | transform | No `HEADER.FIELDS` fetch lane exists today (`grep FETCH_REFS_ATTRS` → 0 hits); module *structure* analog exists (`mutf7.rs`), the parsing algorithm is new (~60-80 LOC) per D-05 fixture list |

## Out-of-Scope Reminders (do not propose)

- No subject-only grouping — D-03 locks `subject_merge: false`; subject-based grouping is rejected project-wide.
- No `BODY[…]` without `.PEEK`, and no extension of `FETCH_ATTRS` (`headers.rs:21`) with the References section.
- No UI / `src/components/**` change; `MessageView` (`commands/sync.rs:2224-2239`) and the flat list render stay byte-compatible. No SMTP/send work this phase.

## Metadata

**Analog search scope:** `src-tauri/src/imap/**`, `src-tauri/src/store/**`, `src-tauri/src/commands/**`, `src-tauri/src/sync/**`, `src-tauri/src/lib.rs`, `src-tauri/src/classify/rules_io.rs`, `src/types.ts`, `src-tauri/Cargo.toml`, `scripts/live-*.sh`
**Files scanned:** ~30 (glob + grep over 17 Rust modules + schema.sql + types.ts + 3 live scripts)
**Tracked-source gate:** all cited analog paths returned non-empty from `git ls-files` (verified `src-tauri/src/classify/rules_io.rs`, `scripts/live-26.sh`, and every `src-tauri/src/**` path) — no gitignored install/runtime mirrors cited
**Pattern extraction date:** 2026-10-20
