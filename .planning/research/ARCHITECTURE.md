# Architecture Research: v1.5 Additions Integration

**Domain:** Live-sync protocol upgrades (IDLE/CONDSTORE), Sieve export, override-learning loop inside an existing single-session IMAP desktop client
**Researched:** 2026-10-13
**Confidence:** HIGH (existing SGE architecture — direct repo read of `imap/`, `sync/`, `store/`, `classify/`, `commands/`, `lib.rs`, `Cargo.toml`; vendored `async-imap 0.11.3` + `imap-proto 0.16.7` source reads); MEDIUM (RFC 7162 wire semantics — RFC text + client-implementation write-ups, cross-verified; Sieve RFC 5228 script shape — established standard, no new-source verification needed)

## Standard Architecture

### System Overview

v1.5 adds three lanes and hardens one. Nothing in the existing lanes changes shape — every addition **consumes** the proven machinery (single-session leases, SyncGate single-flight, forward-only migrations, durable outboxes, ClassifyGate worker) and does not rebuild any of it.

```
┌──────────────────────────────────────────────────────────────────────┐
│                           React UI (new surfaces)                     │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌──────────────────┐   │
│  │ Sync status│ │ History    │ │ Rule import│ │ Dry-run plan +   │   │
│  │ (IDLE dot +│ │ views      │ │ /export +  │ │ retry-failed +   │   │
│  │ modseq age)│ │ (overrides │ │ Sieve copy │ │ auto-confirm     │   │
│  │            │ │ /batches)  │ │ /download) │ │ toggles          │   │
│  └──────┬─────┘ └──────┬─────┘ └──────┬─────┘ └────────┬─────────┘   │
└─────────┼──────────────┼──────────────┼────────────────┼─────────────┘
          │ Tauri IPC (new commands, existing Channel event pattern)   │
┌─────────┼──────────────┼──────────────┼────────────────┼─────────────┐
│         ▼              ▼              ▼                ▼             │
│  ┌──────────────┐ ┌──────────────────────────────────────────────┐  │
│  │ NEW:         │ │ NEW: classify/ additions (pure Rust, no IMAP) │  │
│  │ imap/idle.rs │ │ learn.rs (aggregate) │ sieve.rs (generate)   │  │
│  │ IdleWatcher  │ │ planner.rs (dry-run) │ rules_io.rs (imp/exp) │  │
│  │ (2nd conn,   │ └──────────────────┬───────────────────────────┘  │
│  │ read-only)   │                    │                              │
│  │              │ ┌──────────────────▼───────────────────────────┐  │
│  │              │ │ MODIFIED: store/ (M20 override_events,        │  │
│  │              │ │ M21 auto-confirm policy; highest_modseq       │  │
│  │              │ │ column ALREADY EXISTS — just start writing)   │  │
│  └──────┬───────┘ └──────────────────────────────────────────────┘  │
│         │ triggers (never bypasses)                                  │
│  ┌──────▼────────────────────────────────────────────────────────┐  │
│  │ UNTOUCHED SHAPE, EXTENDED PATH: sync/worker pass               │  │
│  │  step 2 SELECT → select_condstore when advertised (additive)   │  │
│  │  step 4/5 sweep → CHANGEDSINCE flag-only fast path (additive)  │  │
│  │  entry: poll timer (existing) + IDLE wakeup (new, same gate)   │  │
│  └──────┬────────────────────────────────────────────────────────┘  │
│         │ leases (unchanged: lease → attempt → reconnect-retry-once) │
│  ┌──────▼────────────────────────────────────────────────────────┐  │
│  │ SessionManager (ONE owned session — IDLE NEVER runs on it)     │  │
│  │ + 2–4 new SyncSession trait methods (condstore verbs, additive)│  │
│  └───────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| `imap/idle.rs` IdleWatcher (NEW) | Owns a SECOND, dedicated read-only connection; SELECTs one mailbox, IDLE-loops, wakes the sync pass on `NewData` | Own `connect_sync` + `Option<BoxedSession>` take/put loop on a background task; DONE → keep session across cycles; poll timer stays fallback |
| CONDSTORE fast path (MODIFIED pass) | `select_condstore` → compare `highest_modseq` → `CHANGEDSINCE` flag-only refresh; new-UID envelopes + expunge-diff unchanged | 2–4 new `SyncSession` trait methods wrapping already-typed `async-imap` APIs (`select_condstore`, raw-string `uid_fetch`/`uid_search`); store `highest_modseq` per mailbox |
| `classify/sieve.rs` (NEW) | Pure function: (M19 rules + taxonomy) → RFC 5228 script text | String builder, zero deps; `require ["fileinto"]`; non-expressible predicates → comments + UI warning |
| `classify/rules_io.rs` (NEW) | Versioned JSON export/import of M19 rules (+ auto-confirm policy) | `serde_json` envelope `{version, rules[]}`; same validate-then-version++ shape as `import_taxonomy` |
| `override_events` log (NEW table, M20) | Append-only learning substrate: every confirm/override/dismiss/auto-confirm outcome with context | Forward-only migration; writer = existing `confirm/override/batch` paths (3-line hook each, never a new write path) |
| `classify/learn.rs` (NEW) | Aggregates `override_events` → keyword-weight hints / taxonomy keyword suggestions (NO model fine-tuning — scope guard) | Pure read-model over SQL aggregates; surfaced in history view + rule editor suggestions |
| `classify/planner.rs` (NEW) | Dry-run planner: same matching logic as batch, `execute=false` returns plan without writing anything | Shared matcher with `batch.rs`; plan = per-message (dest, rule-id, confidence, conflicts) |
| History views (NEW UI + 2–3 read commands) | Read models over `override_events` + `batch_runs`/`batch_items` + `labels` | SELECT-only commands; no new tables |
| Compose hardening (MODIFIED only) | Verify + fix gaps in `drafts.rs` / `send_queue` / reply path | No new components; test-driven fixes |

## Recommended Project Structure

```
src-tauri/src/
├── imap/
│   ├── idle.rs               # NEW: IdleWatcher (2nd conn, read-only loop)
│   ├── session.rs            # MODIFIED: none (connect_sync reused as-is)
│   ├── manager.rs            # MODIFIED: additive only (condstore lease helper,
│   │                         #   capability-cache already exists — reuse it)
│   └── mod.rs                # MODIFIED: +SyncSession trait methods (additive)
├── sync/
│   ├── worker.rs             # MODIFIED: step-2 condstore branch + CHANGEDSINCE
│   │                         #   flag refresh + highest_modseq persist (additive)
│   └── mod.rs                # MODIFIED: +IdleWake event / SyncEvent variant (additive)
├── classify/
│   ├── sieve.rs              # NEW: pure RFC 5228 generator
│   ├── rules_io.rs           # NEW: rule import/export envelope
│   ├── learn.rs              # NEW: override-event aggregator (read-only)
│   ├── planner.rs            # NEW: dry-run planner (shared matcher w/ batch.rs)
│   └── batch.rs              # MODIFIED: +retry-failed entry (re-enqueue by status)
├── store/
│   ├── mod.rs                # MODIFIED: M20 (+ M21 if policy needs columns)
│   └── queries.rs            # MODIFIED: override_events + policy + history helpers
│                             #   (single-SQL-module invariant holds)
├── commands/
│   ├── sync.rs               # MODIFIED: +idle_status/start_idle/stop_idle (thin)
│   └── classify.rs           # MODIFIED: +export/import_rules, export_sieve,
│                             #   dry_run_batch, retry_failed_batch, override_history
└── lib.rs                    # MODIFIED: AppState +idle handle slot, setup() spawn
src/
├── components/
│   ├── SyncStatus.tsx        # MODIFIED: IDLE dot + last-sync/modseq age
│   ├── HistoryView.tsx       # NEW: overrides + batches + auto-confirms
│   ├── RuleIo.tsx            # NEW: import/export + Sieve pane (copy/download)
│   └── DryRunReport.tsx      # NEW: plan table + confirm → real run
```

### Structure Rationale

- **`imap/idle.rs` is its own module, not a manager method:** the watcher owns a *separate connection lifecycle* (connect → SELECT → IDLE → DONE → re-IDLE, reconnect on drop). Putting it in `manager.rs` invites the fatal mistake of running IDLE on the owned session. A separate module with no access to the manager's session makes the invariant structural, not conventional.
- **`sieve.rs` / `rules_io.rs` / `learn.rs` / `planner.rs` live in `classify/`:** they are pure functions over the classify/rule domain with zero IMAP contact — the same failure-domain logic that gave `classify/` its own module in v1.3 (sidecar down ≠ mail down; Sieve export works fully offline).
- **No new outbox table for overrides:** `override_events` is append-only telemetry, not a durable work queue — it needs no epoch hygiene, no replay, no drain. One table per durable *intent* stays the rule; telemetry is not intent.

## Architectural Patterns

### Pattern 1: IDLE as a wakeup trigger, never as a sync transport (dedicated second connection)

**What:** A new `IdleWatcher` opens its **own** connection via the existing `connect_sync` (same `AccountConfig` clone, same TLS discipline), SELECTs **one** mailbox (INBOX first), and loops `idle().init() → wait() → done()`. On `IdleResponse::NewData` it does NOT fetch anything itself — it calls the **same** `start_sync` entry through the **same** `SyncGate::try_begin` (skip-if-busy). Poll timer stays: as fallback when IDLE is unadvertised/broken, and as the multi-folder sweeper (IDLE watches one mailbox; SGE syncs many since v1.1).

**When to use:** Always for v1.5 IDLE. This is the load-bearing constraint of the whole milestone.

**Trade-offs:** + zero change to sync correctness (every wakeup runs the proven pass, outboxes replay first as today); + poll cadence can relax when IDLE is healthy (battery/bandwidth win) without losing the safety net; + `wait_with_timeout(29 min)` matches the RFC 2177 re-IDLE advice and async-imap already defaults `wait()` to 29 min; − one extra TCP+TLS connection per account (negligible for single-account Linux client); − IDLE watches one mailbox per connection: INBOX gets push, other folders stay on poll (document as designed behavior, not a bug); − `Session::idle(self)` **consumes** the session, so the watcher must take/put it in an `Option` (Delta Chat pattern) — never fight the borrow checker by restructuring the manager.

**Example:**
```rust
// imap/idle.rs — sketch (ownership discipline is the whole point)
pub struct IdleWatcher {
    config: AccountConfig,          // cloned, never the manager's
    stop: Arc<AtomicBool>,
}
impl IdleWatcher {
    async fn run(&self, wake: SyncWake) {
        let mut session: Option<BoxedSession> = None;
        loop {
            if self.stop.load(Ordering::SeqCst) { break; }
            let mut s = match session.take() {
                Some(s) => s, None => connect_sync(&self.config).await?,
            };
            s.select_mailbox("INBOX").await?;
            let mut idle = s.idle();            // consumes; typed API exists
            idle.init().await?;
            let (wait, _stop_src) = idle.wait(); // 29-min default
            match wait.await? {
                IdleResponse::NewData(resp) => {
                    s = idle.done().await?;      // session back, keep it
                    session = Some(s);
                    wake.request_sync("idle");    // → SyncGate::try_begin, skip-if-busy
                }
                IdleResponse::Timeout => { s = idle.done().await?; session = Some(s); continue; }
                IdleResponse::ManualInterrupt => { s = idle.done().await?; session = Some(s); break; }
            }
        }
    }
}
```

### Pattern 2: CONDSTORE fast path inside the existing pass (capability-gated, additive)

**What:** The vendored stack already implements the CONDSTORE *read* path end-to-end — verified in source: `Session::select_condstore()` → `Mailbox.highest_modseq` (from `OK [HIGHESTMODSEQ]`), `STATUS HIGHESTMODSEQ` folds into the same field via `parse_mailbox`, `Fetch.modseq: Option<u64>` is surfaced, and `uid_fetch`/`uid_search` take raw strings so `UID FETCH x (UID FLAGS MODSEQ)` / `(CHANGEDSINCE n)` / `UID SEARCH MODSEQ n` pass through with zero parser work (`imap-proto 0.16.7` already parses `MODSEQ`/`HIGHESTMODSEQ`/`VANISHED`). The schema **already has** `mailboxes.highest_modseq` (NULL = unknown — the exact RFC 7162 posture). v1.5 work is therefore *wiring*, not protocol implementation:
1. Step 2 SELECT: if cached caps contain `CONDSTORE`, call `select_condstore`, compare returned `highest_modseq` vs stored; on epoch (UIDVALIDITY) change, clear stored modseq (RFC rule) and run the full sweep.
2. Modseq unchanged → skip flag refresh entirely (stronger than the Phase 9 convergence heuristic — server-attested, not inferred).
3. Modseq advanced → `UID FETCH 1:* (UID FLAGS MODSEQ) (CHANGEDSINCE <stored>)` → apply flag diffs locally, persist new `highest_modseq`. New arrivals (UIDNEXT gap) still take the envelope sweep; expunges still take the SEARCH-ALL diff (CONDSTORE does not report expunges — that is QRESYNC's job).

**When to use:** Whenever the server advertises CONDSTORE. No CONDSTORE → byte-identical existing path (capability gate at lease time via the already-cached `cached_capabilities`).

**Trade-offs:** + kills the "remote flag-only changes surface within ~5 intervals" blind spot (PROJECT.md decision log) — flag changes from other clients/phone converge next pass with a tiny FETCH; + zero new dependencies, zero new tables (column exists); − `* VANISHED` mid-FETCH lands in async-imap's `pub(crate)` unsolicited channel, invisible to SGE — so **no VANISHED reliance**: expunge detection stays SEARCH-diff (correct with CONDSTORE alone); full QRESYNC (`ENABLE` + `SELECT (QRESYNC …)` + `VANISHED (EARLIER)`) is deliberately deferred — it needs raw `run_command`/`read_response` parsing SGE would own, and Gmail-class servers advertise CONDSTORE without QRESYNC anyway.

**Example:**
```rust
// SyncSession trait additions (object-safe PinBox shape, like all siblings)
fn select_condstore(&mut self, name: &str) -> PinBox<'_, Result<MailboxSummaryWithModseq, SyncError>>;
fn fetch_changed_since<'a>(&'a mut self, modseq: u64) -> PinBox<'a, Result<Vec<FlagDelta>, SyncError>>;
// FlagDelta { uid, flags, modseq } — built from Fetch::uid + Fetch::flags + Fetch::modseq
```

### Pattern 3: Override-event log first, learning second (append-only telemetry → pure aggregator)

**What:** Every classify outcome path (`confirm_suggestion`, `override_label`, dismiss, batch auto-confirm hit/miss) appends one row to a new `override_events` table: `(mailbox_id, uid, suggested, final, confidence, taxonomy_version, rule_id?, source, created_at)` — content-free pointers only (same sensitive-data discipline as `labels`: never email text). `classify/learn.rs` is a **read-only** aggregator over that table: per-(suggested→corrected) confusion counts, low-confidence patterns, keyword candidates for the rule editor. History views read the same table. Nothing trains Laya (scope guard: "no fine-tuning runtime changes beyond logging hooks").

**When to use:** All v1.5 learning work. The table is the milestone's learning substrate; any future model work consumes it.

**Trade-offs:** + logging hooks are 3-line appends in proven paths — shippable independently of any aggregation UI; + aggregates are pure functions, unit-testable with fixture rows, no sidecar/IMAP; − aggregates are only as good as event volume (single-user client: suggestions surface after dozens of overrides, never promise "smart after 3 clicks" copy).

### Pattern 4: Dry-run as execute=false on the shared matcher (one matching implementation, two callers)

**What:** `classify/planner.rs` owns the rule/label→(dest, conflicts) matching both dry-run and real batch call. `dry_run_batch` runs it with `execute=false`: no queue writes, no label writes, no MOVE — returns the plan (per-message dest + firing rule + confidence + conflict flags). The confirmed run calls the **same** matcher with `execute=true` (existing batch path). `retry_failed_batch` re-enqueues `batch_items` with `status=failed` only (M16 already tracks per-item status — no new table).

**When to use:** Batch dry-run + retry-failed. Mirrors the established "one implementation, two callers" shape (`run_move_on_session`, `save_draft_on_session`, `file_sent_on_session`).

**Trade-offs:** + dry-run can never drift from real behavior (same code); + retry is a filtered re-enqueue, inheriting progress/report UI; − planner must be side-effect-free by construction — review gate: `planner.rs` takes `&` references only, returns owned plan (compiler-enforced if no `&mut Store` param exists).

### Pattern 5: Sieve export as offline pure generation (no ManageSieve, no upload)

**What:** `classify/sieve.rs`: `(M19 rules in priority order, taxonomy fallbacks)` → RFC 5228 text with `require ["fileinto"]`, `if anyof/header/address` tests, explicit `keep;` default. Rule predicates with no Sieve equivalent (Laya-label triggers, confidence thresholds) emit as `# NOTE: <rule> needs manual review — <reason>` comments plus a UI warning count. Delivery = clipboard copy + `.sieve` file download (existing `fs`/`dialog` plugins). Server upload (ManageSieve, RFC 5804) is **out of scope** — "export generator," not "Sieve sync."

**When to use:** The v1.5 Sieve target. Revisit upload only as a separate milestone (new crate, new credentials surface, new failure domain).

**Trade-offs:** + zero new deps, fully offline, trivially testable (golden-file tests on fixture rules); + honest scope (v1.4 already deferred Sieve once as "a second product"); − semantic gaps documented, not hidden: Sieve is first-match-stop while SGE rules have their own conflict policy — generator emits rules in SGE priority order with `stop;` per branch and documents the mapping in a header comment.

## Data Flow

### Request Flow

```
IDLE wakeup flow (new trigger, old path):
  IdleWatcher NewData(EXISTS/RECENT/FETCH)
      ↓ (no fetch, no parse beyond signal)
  wake.request_sync → SyncGate::try_begin (skip-if-busy, same as poll tick)
      ↓
  start_sync pass (UNCHANGED shape; step 2 takes condstore branch when advertised)
      ↓ post-pass enqueue hook (existing v1.3 hook)
  classify_queue → ClassifyGate → labels → ClassificationReady events

CONDSTORE fast-path flow (inside pass, INBOX or any folder):
  lease → caps contain CONDSTORE?
      ↓ yes → select_condstore → (uid_validity, highest_modseq)
  epoch changed? → clear stored modseq → FULL sweep (existing steps 4–7)
  modseq == stored? → SKIP flag refresh (server-attested converged)
  modseq advanced? → UID FETCH CHANGEDSINCE stored (FLAGS+MODSEQ only)
      → apply flag diffs → persist new highest_modseq
      → new-UID envelopes (UIDNEXT gap) + SEARCH-ALL expunge diff (unchanged)

Override-learning flow (telemetry → aggregate → surface):
  confirm/override/dismiss/auto-confirm → append override_events row (no await, no gate)
      ↓ (independent, on demand)
  learn::aggregate(conn) → confusion matrix + keyword hints (pure read)
      ↓
  history view commands + rule-editor suggestions (UI only)

Dry-run / retry flow:
  dry_run_batch → planner::plan(&rules, &headers) → DryRunReport (zero writes)
      ↓ user confirms
  batch_classify (existing) with execute=true → same matcher → moves via imap_outbox
  retry_failed_batch → re-enqueue batch_items WHERE status=failed → same drain
```

### State Management

```
mailboxes.highest_modseq: NULL → <n> → <n'> (monotonic per epoch; cleared on UIDVALIDITY bump)
  - NULL: server lacks CONDSTORE OR never selected with CONDSTORE (legacy rows) → full sweep
  - set: CONDSTORE fast path eligible

override_events: append-only (no status machine — rows are immutable facts)
  - consumers: learn::aggregate (counts), history views (lists), future model work

batch_items.status: pending → done | failed (existing M16) — retry reads failed only
labels.status: unchanged machine (pending → suggested → confirmed | overridden | dismissed)
```

### Key Data Flows

1. **IDLE → sync (wakeup only):** watcher never touches store/IMAP-writes; the gate serializes IDLE wakeups with poll ticks and manual refreshes — three triggers, one pass.
2. **Sync → modseq store:** pass persists `highest_modseq` alongside the existing `uid_next` write (same transaction that writes sync state — no new lock discipline).
3. **Classify paths → override_events:** fire-and-forget appends; aggregation never blocks confirm latency (read on view-open, not on write).
4. **Rules → Sieve text:** one-direction generation at export time; no reverse import of Sieve (explicit non-goal — Sieve→SGE parsing is a second product).

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| 1 mailbox, <1k msgs (target user) | As designed. IDLE on INBOX + 5-min poll elsewhere; CONDSTORE makes steady-state passes near-zero-FETCH. No change needed. |
| 5k–20k msgs, multi-folder | IDLE stays INBOX-only (one connection per watched mailbox; do NOT open N IDLE connections — connection-per-folder is the anti-pattern). CONDSTORE pays off most here: flag-only CHANGEDSINCE vs full envelope re-sweep. |
| 100k+ msgs | Out of scope (personal UTFPR use). If ever: per-folder modseq already partitions the problem; QRESYNC revisit only with measured reconnect cost. |

### Scaling Priorities

1. **First bottleneck: IDLE watcher reconnect storms** (flaky network → DONE/reconnect loop spinning). Mitigate with backoff on consecutive failures (same shape as send-queue backoff thinking: attempts + sleep schedule), and poll-timer floor (never rely solely on IDLE — it is a latency optimization, not a correctness dependency).
2. **Second bottleneck: override_events growth** (unbounded append-only). Cap with a retention prune (keep latest N per mailbox or T days — telemetry, not source of truth; document the window in the history view).

## Anti-Patterns

### Anti-Pattern 1: Running IDLE on the SessionManager-owned session

**What people do:** `manager.lease_for("INBOX")` → `session.idle()` on the leased session to "reuse the connection."
**Why it's wrong:** IDLE holds the connection indefinitely; the manager's async-mutex lease would serialize every sync/flag/move behind a 29-minute block. Worse, `idle(self)` *consumes* the session — the manager's `Option<Box<dyn SyncSession>>` cannot lend out of a held mutex guard and get it back. Breaks the single-session invariant and deadlocks the app.
**Do this instead:** IdleWatcher with its own `connect_sync` connection, read-only verbs only (SELECT + IDLE + DONE + LOGOUT), wakeup-via-gate (Pattern 1).

### Anti-Pattern 2: Trusting VANISHED / building QRESYNC on the typed API

**What people do:** `UID FETCH (CHANGEDSINCE n)` and expect expunged UIDs to surface through `uid_fetch`'s stream.
**Why it's wrong:** `* VANISHED` responses route to async-imap's `pub(crate)` unsolicited channel — SGE code cannot see them; the fetch stream silently omits expunges. Treating "no VANISHED seen" as "nothing expunged" corrupts the expunge-diff.
**Do this instead:** CONDSTORE-only posture (Pattern 2): expunges via the existing SEARCH-ALL diff. Defer QRESYNC (raw `run_command` + manual `read_response` parse) to a measured follow-up.

### Anti-Pattern 3: Second writer on labels / rules from the planner or exporter

**What people do:** Dry-run "caches" its plan by writing tentative `labels` rows; Sieve export "syncs" by rewriting rule rows.
**Why it's wrong:** Two writers race with the classify worker and the rule CRUD commands; tentative rows leak into history views and the learning substrate (override_events would record phantom outcomes).
**Do this instead:** Planner takes `&` refs and returns owned plans (compiler-enforced purity); exporter is a pure function of its inputs (Pattern 4/5).

### Anti-Pattern 4: Sieve upload scope creep (ManageSieve in v1.5)

**What people do:** "While generating the script, just PUSH it to the server too — one more crate."
**Why it's wrong:** ManageSieve (RFC 5804) is a second authenticated protocol surface (new port, new TLS policy, new credential handling, per-server script-name semantics, active-script activation = server-side behavior change outside the sandbox contract). v1.4 already scoped Sieve out as "a second product" for exactly this reason.
**Do this instead:** Export-only (copy/download). Upload is a separate milestone with its own live-gate sandbox.

### Anti-Pattern 5: Blocking confirm latency on learning aggregation

**What people do:** `override_label` → recompute aggregates → update suggestions inline.
**Why it's wrong:** Holds the command path behind unbounded SQL aggregation; a large `override_events` table makes every confirm slower. Violates classify-behind-sync's spirit (never inline what can queue).
**Do this instead:** Append-only write on confirm (O(1) INSERT); aggregate on view-open / on explicit refresh (Pattern 3).

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| IMAP server IDLE (RFC 2177) | 2nd connection, capability-gated (`IDLE` atom), `wait()` 29-min | Server MAY drop idle conns — re-IDLE loop + poll fallback; never the sole trigger |
| IMAP server CONDSTORE (RFC 7162 §3) | `select_condstore` + CHANGEDSINCE/MODSEQ via raw-string fetch/search args | Gate on `CONDSTORE` atom from cached caps; NOMODSEQ/HIGHESTMODSEQ-absent → full sweep |
| Sieve host (human-mediated) | Copy/download `.sieve` text; no protocol | UTF-8 folder names in `fileinto` (NOT modified-UTF-7 wire form); header comment documents order semantics |
| OS keyring | Unchanged (watcher clones in-memory `AccountConfig`, never re-reads keyring per loop) | No new secrets surface |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| IdleWatcher → sync pass | `SyncWake::request_sync` → `SyncGate::try_begin` (skip-if-busy) | Same gate as poll/manual — three triggers, one pass; watcher holds no store lock, no lease |
| Sync pass → store | `highest_modseq` persisted with existing sync-state write | Same brief-lock discipline (never across await) |
| Classify confirm/override/batch → override_events | Fire-and-forget INSERT in existing functions | Failure never fails the confirm (log-and-continue, worker replay discipline) |
| planner ↔ batch.rs | Shared matcher fn; `execute: bool` selects dry vs real | One implementation, two callers (established codebase shape) |
| sieve/rules_io ↔ store | Read rules/taxonomy rows; write nothing (export) / version++ row (import) | Import mirrors `import_taxonomy` validation shape |
| backend → UI | New `SyncEvent::IdleWake` / `ModseqAdvanced` variants on existing Channel | Same Channel pattern as `ClassificationReady` |

## New vs Modified — explicit file list

### NEW files

| File | Purpose |
|------|---------|
| `src-tauri/src/imap/idle.rs` | IdleWatcher: 2nd conn lifecycle + IDLE loop + gate wakeup |
| `src-tauri/src/classify/sieve.rs` | Pure RFC 5228 generator (golden-file tested) |
| `src-tauri/src/classify/rules_io.rs` | Rule import/export versioned envelope |
| `src-tauri/src/classify/learn.rs` | Override-event aggregator (read-only) |
| `src-tauri/src/classify/planner.rs` | Dry-run planner (shared matcher) |
| `HistoryView.tsx`, `RuleIo.tsx`, `DryRunReport.tsx` | UI surfaces (read models + export pane + plan table) |

New deps: **none**. No new SQLite features. No new IMAP crates.

### MODIFIED files

| File | Change | Risk |
|------|--------|------|
| `imap/mod.rs` | +2–4 `SyncSession` methods (select_condstore, changed-since fetch, modseq search) + `FlagDelta` type | Low — additive, typed async-imap APIs already exist; Fake/Mock doubles gain canned methods |
| `imap/manager.rs` | +condstore lease helper (-analogous to `*-in` family); capability cache reused for CONDSTORE/IDLE gating | Low — additive, existing lease → retry template |
| `sync/worker.rs` | Step-2 condstore branch + CHANGEDSINCE refresh + modseq persist | Medium — touches the pass, but branch-gated (no-caps = old path byte-identical); convergence tests stay green |
| `sync/mod.rs` | +`SyncEvent` variants (IdleWake, ModseqAdvanced) | Low |
| `classify/batch.rs` | +retry-failed entry (filter `status=failed`) | Low — reuses drain |
| `store/mod.rs` | M20 `override_events` (+M21 policy iff M14 review_settings can't hold it — check first) | Low — forward-only pattern M2–M19 |
| `store/queries.rs` | events/policy/history helpers (single-SQL invariant) | Medium — largest diff, mechanical |
| `commands/sync.rs` | +`idle_status`/`start_idle`/`stop_idle` (thin, spawn watcher) | Low |
| `commands/classify.rs` | +7 commands (rules io, sieve, dry-run, retry, history, policy) | Low-Medium (surface area, each thin) |
| `lib.rs` | AppState +idle slot, `setup()` spawn/stop | Low |
| Compose files (`drafts.rs`, `send_queue.rs`, reply path) | Verify + fix gaps only | Low — hardening, no architecture |

### UNTOUCHED (must stay invariant)

- Single-session ownership (`SessionManager` state machine, lease → reconnect-retry-once, capability-cache invalidation).
- `SyncGate` semantics; outbox replay order (pre-sweep `imap_outbox`, post-sync `flag_outbox`); convergence/tombstone logic; UIDVALIDITY epoch gating everywhere.
- `BODY.PEEK` discipline; ammonia sanitize; keyring-only secrets; forward-only migrations with preserve-rows tests.

## Suggested build order (dependency-ordered)

1. **Override-event log (M20 + 3-line hooks).** Telemetry before consumers: confirm/override/dismiss/batch paths append rows. Headless, `cargo test` only. Unlocks everything learning-shaped.
2. **CONDSTORE fast path (trait methods + step-2 branch + modseq persist).** Hooks the existing pass; FakeSession doubles extend naturally (canned modseq). Verify: CONDSTORE-capped Fake shows flag-only refresh; non-CONDSTORE Fake takes old path byte-identical; epoch bump clears modseq. No UI.
3. **Rule import/export + auto-confirm policy.** Versioned envelope over M19 (+M21 iff M14 insufficient — spike the M14 read first, one query). Gives the Sieve generator and the policy UI their data model.
4. **IDLE watcher (new connection + gate wakeup + status UI).** After CONDSTORE because every IDLE wakeup runs the pass — a cheap pass makes push cheap. Verify: NewData → pass runs; busy gate → skip; server without IDLE → poll-only with honest status; kill conn mid-IDLE → backoff reconnect, poll covers the gap.
5. **Dry-run planner + retry-failed.** Planner shares the batch matcher (pure, `&`-only); retry filters `batch_items`. Verify: dry-run writes nothing (row-count assertions); confirm executes the identical plan; retry touches failed-only.
6. **Sieve export generator.** Reads the rule model from step 3; golden-file tests. Verify: fixture rules → expected script; Laya-only rules → commented + warning count; folder names UTF-8.
7. **Learn aggregator + history views.** Reads the step-1 log; pure aggregates + SELECT-only commands + UI. Verify: fixture events → expected confusion counts; empty log → honest empty state (never "smart after 3 clicks" copy).
8. **Compose hardening.** Verify reply/reply-all/draft/send end-to-end over v1.2 pipelines; fix gaps test-first. No new architecture by design.

Why this order: telemetry before aggregation (can't surface what isn't logged); cheap-pass before push-wakeup (IDLE multiplies pass cost — CONDSTORE divides it); rule model before Sieve (generator reads the model, and import/export validation settles predicate expressiveness first); dry-run before its UI; learning last among features (needs event volume + the policy model to be meaningful); compose hardening parallelizable anytime (disjoint files).

## Risks & open questions

1. **IDLE + `spawn_blocking` runtime fit.** Commands run sync inside `spawn_blocking + block_on`; the watcher is a *long-lived* loop, not a pass — parking an IDLE wait on a blocking-thread pool thread is acceptable (one thread, mostly parked) but must not starve the pool: give it a dedicated `std::thread` (like the probe pattern) rather than the shared pool. Decide in planning; both work.
2. **Multi-folder IDLE fan-out explicitly rejected** (one conn per folder = connection leak shape). INBOX-only push + poll sweep is the documented posture.
3. **CONDSTORE `NOMODSEQ` mailboxes** (server supports CONDSTORE but not persistent modseq for a folder): `highest_modseq=None` after `select_condstore` → permanent full-sweep for that folder. Correct per RFC; log once per folder, not per pass.
4. **`highest_modseq` type width:** `u64` end-to-end (imap-proto `HighestModSeq(u64)`, `Fetch.modseq: Option<u64>`); SQLite INTEGER is i64 — modseq is 63-bit by RFC (fits i64), store as INTEGER with a `>= 0` check.
5. **Sieve `fileinto` folder existence:** export cannot CREATE server folders (no upload) — emit a header checklist of folders the user must create server-side. Honest UX for the offline-generation posture.
6. **History-view retention:** `override_events` unbounded growth → retention prune policy (decide N/days in planning; telemetry, safe to prune).

## Sources

- SGE repo at research time (`imap/{mod,manager,session}.rs`, `sync/{mod,worker}.rs`, `store/{mod,schema.sql}`, `classify/*`, `commands/{sync,classify}.rs`, `lib.rs`, `Cargo.toml`) + `.planning/research/ARCHITECTURE.md` (v1.3 baseline) + `PROJECT.md` v1.5 scoping — HIGH confidence (direct source read).
- Vendored `async-imap 0.11.3` source (`client.rs` select_condstore/idle/run_command, `parse.rs` HIGHESTMODSEQ/MODSEQ folding, `types/{fetch,mailbox,mod}.rs`, `extensions/idle.rs` Handle/IdleResponse) + `imap-proto 0.16.7` (`types.rs` Vanished/HighestModSeq/ModSeq, `parser/rfc7162.rs`) — HIGH confidence (direct source read).
- RFC 7162 (CONDSTORE/QRESYNC) text + go-imap/mailledger client implementation notes + Thunderbird/neomutt CONDSTORE experience (Gmail: CONDSTORE yes, QRESYNC no) — MEDIUM-HIGH (standard text, cross-verified across implementations).
- RFC 5228 (Sieve fileinto/anyof/header) — MEDIUM (established standard; golden-file tests at build time are the real verification).

---
*Architecture research for: v1.5 IDLE/CONDSTORE/Sieve/override-learning integration*
*Researched: 2026-10-13*
