# Architecture Research: Laya Auto-Classify Integration (v1.3)

**Domain:** On-device email classification inside an existing IMAP desktop client
**Researched:** 2026-10-10
**Confidence:** HIGH (existing SGE architecture — direct repo read); MEDIUM (Laya sidecar contract — official skill docs + docs.rs, verified against installed `laya 0.3.20`; Tauri sidecar mechanics — official Tauri v2 docs, cross-verified)

## Standard Architecture

### System Overview

v1.3 adds one new lane to the existing system. Nothing in the current
lanes changes shape — the classifier **consumes** the Phase 10/11
machinery (MOVE, CREATE) and the sync/store substrate, it does not
rebuild any of it.

```
┌──────────────────────────────────────────────────────────────────┐
│                          React UI (new surfaces)                  │
│  ┌──────────────┐ ┌──────────────┐ ┌────────────┐ ┌────────────┐ │
│  │ Suggestion   │ │ Taxonomy     │ │ Override   │ │ Batch      │ │
│  │ chip +       │ │ editor +     │ │ (re-move + │ │ progress + │ │
│  │ Confirm btn  │ │ JSON import  │ │ record)    │ │ report     │ │
│  └──────┬───────┘ └──────┬───────┘ └─────┬──────┘ └─────┬──────┘ │
└─────────┼────────────────┼───────────────┼──────────────┼────────┘
          │ Tauri IPC (new commands, existing Channel event pattern)
┌─────────┼────────────────┼───────────────┼──────────────┼────────┐
│         ▼                ▼               ▼              ▼       │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              NEW: classify/ module (Rust)                 │  │
│  │  taxonomy.rs (load/validate) │ bridge.rs (sidecar HTTP)  │  │
│  │  worker.rs (queue + ClassifyGate) │ suggest.rs (shape)   │  │
│  └──────┬───────────────┬───────────────────────┬───────────┘  │
│         │               │                       │              │
│  ┌──────▼──────┐ ┌──────▼────────┐ ┌────────────▼──────────┐   │
│  │  MODIFIED:  │ │ NEW: sidecar  │ │ MODIFIED: existing    │   │
│  │  store/     │ │ laya serve    │ │ imap/manager +        │   │
│  │  (M12:      │ │ 127.0.0.1     │ │ sync/worker (post-    │   │
│  │  taxonomy + │ │ loopback HTTP │ │ sync hook, MOVE +     │   │
│  │  labels)    │ │ multilingual  │ │ CREATE reuse)         │   │
│  └─────────────┘ └───────────────┘ └───────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| Laya sidecar | Typed `choice`/`noul` answers over email state; zero text generation | `laya[serve]` (or `laya-rs2`) HTTP server on `127.0.0.1`, spawned via Tauri `externalBin` + `shell().sidecar().spawn()` |
| `classify/bridge.rs` (NEW) | Loopback HTTP client: `POST /predict`, health probe, timeout/retry | `reqwest` (check fit) or `std::net` minimal client; single-flight requests (Laya serves one forward pass at a time) |
| `classify/taxonomy.rs` (NEW) | Load/validate taxonomy JSON (5 top + children + fallback + rules), version it | `serde_json` + JSON-schema-ish validation in Rust; default UTFPR taxonomy as bundled `resources/` file |
| `classify/worker.rs` (NEW) | Background classification queue drained under `ClassifyGate`; never blocks sync | `tauri::async_runtime::spawn` loop, same `try_begin`/skip shape as `SyncGate` |
| `classify/suggest.rs` (NEW) | Shape sidecar answers into suggestions (primary + runner-up secondary + confidence + sensitive flag) | Pure function: Laya JSON → `Suggestion` struct; confidence gate → `A Classificar` |
| Store M12 (MODIFIED) | `taxonomy` (versioned), `labels` (primary+secondary per message), `label_overrides`, `classify_queue`/`batch_runs` | Forward-only migration, all SQL in `queries.rs` (single-SQL-module invariant) |
| SessionManager (MODIFIED, additive) | `ensure_auto_tree()` (CREATE missing `Auto/*` paths) + reuse of `move_message_in` | Same lease → attempt → reconnect-retry-once template; no new verbs needed |
| sync/worker (MODIFIED, hook only) | After a pass commits new UIDs, enqueue classify jobs (fire-and-forget) | 3–5 line hook at pass end; sync never awaits classification |
| Commands (MODIFIED `commands/sync.rs` or NEW `commands/classify.rs`) | `classify_message`, `confirm_suggestion` (= MOVE), `override_label`, `batch_classify`, `import_taxonomy` | Copy the `set_seen` optimistic+durable template |

## Recommended Project Structure

```
src-tauri/
├── binaries/                       # NEW: laya-sidecar-<target-triple> (externalBin)
│   └── laya-sidecar-x86_64-unknown-linux-gnu
├── resources/                      # NEW: default taxonomy (bundle resources)
│   └── taxonomy-utfpr-ptbr.json
├── src/
│   ├── classify/                   # NEW module
│   │   ├── mod.rs                  #   public surface: ClassifyError, Suggestion, ClassifyHandle
│   │   ├── taxonomy.rs             #   load/validate/version taxonomy JSON
│   │   ├── bridge.rs               #   sidecar lifecycle + POST /predict client
│   │   ├── worker.rs               #   queue drain + ClassifyGate + batch runner
│   │   └── suggest.rs              #   answer shaping, confidence gate, sensitive rule
│   ├── commands/
│   │   └── classify.rs             # NEW (or fold into sync.rs): 5 commands below
│   ├── imap/manager.rs             # MODIFIED: +ensure_auto_tree (additive)
│   ├── sync/worker.rs              # MODIFIED: +post-pass enqueue hook (additive)
│   ├── store/                      # MODIFIED: M12 migration + queries
│   ├── lib.rs                      # MODIFIED: AppState +classification state, setup() spawns sidecar
│   └── capabilities/default.json   # MODIFIED: shell:allow-spawn for the sidecar
src/
├── components/
│   ├── ClassifyChip.tsx            # NEW: suggestion + Confirm / Correct
│   ├── TaxonomyEditor.tsx          # NEW: categories/keywords/rules + JSON import
│   └── BatchProgress.tsx           # NEW: batch progress + report
```

### Structure Rationale

- **`classify/` mirrors `smtp/` and `imap/`:** one module per transport-ish
  boundary. The sidecar is a third transport (loopback HTTP) next to IMAP
  and SMTP — giving it its own module keeps the failure domains obvious
  (sidecar down ≠ mail down).
- **Commands in their own file:** `commands/sync.rs` already holds ~2000+
  lines (sync + organize + drafts + send). A new `commands/classify.rs`
  avoids further growth; registration in `lib.rs invoke_handler` is one line.
- **Taxonomy as data, not code:** categories/keywords/rules ship as a
  versioned JSON resource so the UI editor and the JSON import share one
  format, and future taxonomy updates don't need a recompile.

## Architectural Patterns

### Pattern 1: Laya sidecar over loopback HTTP (bundled, offline)

**What:** Laya runs as a child process of the Tauri app (Tauri `externalBin`
sidecar, `src-tauri/binaries/laya-sidecar-<triple>`), exposing `POST
/predict` on `127.0.0.1`. The Rust backend is an HTTP client; the React
frontend never talks to Laya directly. Spawned in `setup()`, killed on
app exit (Tauri tracks sidecars; no orphan processes).

**When to use:** This is the mandated shape — PROJECT.md pins "bundled
offline sidecar", and the Laya integration skill prescribes exactly this
for non-Python hosts ("Anything else: run Laya as a small local HTTP
sidecar, bind 127.0.0.1"). Native-Rust inference crates (`laya-candle`,
`laya-rs`) exist but are the wrong call here: CPU-only inference is
~1 s+/prediction vs ~20–60 ms via the tuned sidecar, and they would add
candle + weights (~400 MB) into the main binary's build graph.

**Trade-offs:** + offline (no mail leaves the machine — satisfies the
security constraint); + typed answers, never generated text (the
sensitive-data rule is structurally easier: there are no justifications
to leak passwords into); + checkpoint preload once at startup (~25–35 s
cold — must happen in background, see Pitfall 1); − new runtime dep to
bundle (`tauri-plugin-shell` not yet in `Cargo.toml`; `externalBin`
triple-suffixed binary; new capability entries); − first cold
prediction after idle may stall on checkpoint reload (mitigate: preload
`multilingual` at sidecar start, keep warm with the sync hook's steady
traffic).

**Example:**
```rust
// bridge.rs — single-flight client (Laya serves one forward pass at a time)
pub struct LayaBridge {
    client: reqwest::Client,   // or minimal http client
    base_url: String,          // http://127.0.0.1:<port>
    lock: tokio::sync::Mutex<()>,
}
impl LayaBridge {
    pub async fn predict(&self, state: serde_json::Value, questions: serde_json::Value)
        -> Result<LayaAnswers, ClassifyError>
    {
        let _guard = self.lock.lock().await;      // serialize forward passes
        // POST {base_url}/predict {state, questions}, timeout ~10 s
    }
    pub async fn health(&self) -> bool { /* GET /health */ }
}
```

### Pattern 2: Single-pass multi-question predict (hierarchy without N+1)

**What:** One `predict` call per email carrying **three questions**:
`category` (`choice` over the 5 top-level + `A Classificar`, each option
described with its children keywords folded into the description),
`sensitive` (`noul`: "does this email contain passwords/codes/personal
secrets?"), and optionally `priority` (`score`) if the UI wants it. All
questions share one forward pass (~7–16 ms marginal cost each). The
**secondary label is free**: it's the runner-up of the same `category`
distribution (Laya returns per-option probabilities). The **child
category is deterministic**: keyword match from the taxonomy within the
winning top-level — no second model call.

**When to use:** Always for v1.3. Keeps per-mail cost to one forward pass
(~50 ms GPU / ~150 ms CPU), which is what makes classify-on-sync viable
for a 200-mail backfill (~10–30 s background, acceptable with progress).

**Trade-offs:** + no latency cascade; + secondary label can't contradict
primary (same distribution); − child resolution is keyword-based, dumber
than a model call (acceptable: child only refines the folder name under
an already-chosen top-level; override path corrects mistakes); − option
descriptions must stay information-dense (fold children keywords in, cap
length). If the taxonomy ever exceeds ~20 leaf options flattened, use
Laya's shortlist pattern — with 5 top-level options we are far below the
20-option guardrail, no shortlist needed.

**Example:**
```json
{
  "state": {
    "from": "secretaria@utfpr.edu.br",
    "subject": "Reunião do colegiado adiada",
    "snippet": "Prezados, a reunião do colegiado...",
    "lang_hint": "pt-BR"
  },
  "questions": {
    "category": {
      "type": "choice",
      "instructions": "Qual categoria desta mensagem? Responda apenas com a etiqueta.",
      "criteria": {
        "Academico": "aulas, provas, colegiado, secretaria, disciplinas...",
        "Administrativo": "rh, ponto, memorandos, portarias...",
        "Financeiro": "pagamentos, bolsas, reembolsos...",
        "Pessoal": "familiares, amigos, assuntos particulares...",
        "Avisos": "informes gerais, eventos, comunicados...",
        "A Classificar": "nada acima se aplica com clareza"
      }
    },
    "sensitive": { "type": "noul", "instructions": "Contém senhas, códigos ou dados sigilosos?" }
  }
}
```

### Pattern 3: Classify-behind-sync (queue, never inline)

**What:** The sync pass does **not** call Laya. At pass end it enqueues
`(mailbox_id, uid)` jobs for newly-seen UIDs into a `classify_queue`
table (or in-memory channel + durable table — see §Data Flow) and
returns. A separate `classify` worker, gated by a `ClassifyGate`
(single-flight clone of `SyncGate`), drains the queue: build state from
cached headers + `preview` (never force a body FETCH), call sidecar,
write `labels` row, emit `ClassificationReady` event. Manual classify
and batch classify push into the **same** queue with different priority
flags — one drain path, three entry points.

**When to use:** This ordering is load-bearing: sync holds the IMAP lease
and the store lock in brief sections; awaiting a 50–150 ms model call
per message inside the pass would serialize the mailbox behind the GPU
and break the poll cadence. Queue-behind also gives crash durability
(survives restart) and natural batching for the whole-account flow.

**Trade-offs:** + sync latency unchanged; + offline classification works
(queue drains when the sidecar is up — sidecar is local, so "offline"
only means IMAP down, classification still runs); + batch mode is just
"enqueue everything with bulk flag"; − suggestions arrive seconds after
sync (UI must handle `unclassified → classifying → suggested` states);
− queue table needs its own epoch hygiene (UIDVALIDITY bump drops queued
jobs for that mailbox, same RFC 4549 rule as the other outboxes).

### Pattern 4: Confirm-then-MOVE through the proven outbox (no new IMAP verbs)

**What:** Confirming a suggestion is **exactly** a Phase 10 move with a
pre-step: `ensure_auto_tree()` CREATEs any missing `Auto/<Top>/<Child>`
path (reusing `create_mailbox_in`), then the existing `move_message_in`
+ `imap_outbox` optimistic+durable flow runs unchanged. The classifier
never touches IMAP directly. Override = confirm to a different folder +
insert `label_overrides` row (records the correction; no fine-tuning in
v1.3 per out-of-scope). Batch = loop over suggestions calling the same
confirm path with `bulk_run_id`, no per-mail dialog, progress events +
final report.

**When to use:** Every physical move in v1.3. This is the single most
important constraint in this document: **zero new SyncSession verbs**.
MOVE/COPY-fallback, UIDPLUS-gated expunge, reconnect-retry, epoch
gating, pre-sweep replay — all already proven. v1.3 inherits them for
free and cannot regress them.

**Trade-offs:** + smallest possible IMAP blast radius (additive
`ensure_auto_tree` only); + offline confirm works via `imap_outbox`
replay; + undo window comes free if confirm reuses the Trash-style
reverse-MOVE pattern; − `Auto/` tree creation needs the Phase 11 guards
(INBOX protection is irrelevant here, but `\Noselect` + delimiter +
modified-UTF-7 leaf encoding rules all apply to `Auto/...` names).

## Data Flow

### Request Flow

```
Classify-on-sync (primary flow):
  Sync pass commits new UIDs
      ↓ (hook: enqueue, no await)
  classify_queue ← (mailbox_id, uid, source='sync'|'manual'|'batch')
      ↓ (ClassifyGate::try_begin; worker loop)
  Worker: read cached subject/from/preview (store lock, brief)
      ↓
  LayaBridge::predict (loopback HTTP, serialized, ~50 ms)
      ↓
  suggest::shape (confidence gate → fallback; runner-up → secondary)
      ↓
  labels row write (primary, secondary, confidence, model_version)
      ↓
  Channel event ClassificationReady → UI chip appears

Confirm flow (user accepts suggestion):
  UI Confirm → confirm_suggestion(uid, dest=Auto/<Top>/<Child>)
      ↓
  ensure_auto_tree (CREATE missing levels, Phase 11 guards) → move_message_in
      ↓ (existing Phase 10 path: optimistic local delete + imap_outbox enqueue
          → lease attempt → ack/dequeue → next sync reconciles dest folder)
  labels row: status=suggested → confirmed; UI moves mail to Auto/ folder

Batch flow (whole account, unconfirmed):
  batch_classify → enqueue ALL unclassified UIDs (bulk_run_id, progress total)
      ↓ worker drains (same path) → auto-confirm each suggestion ≥ threshold,
          below threshold stays `A Classificar`
      ↓ BatchProgress events → final report (moved counts per category, review list)
```

### State Management

```
labels.status: pending → suggested → confirmed | overridden | dismissed
  - pending:     enqueued, no answer yet (UI: subtle "classificando…" state)
  - suggested:   sidecar answered, awaiting user (UI chip + Confirm/Correct)
  - confirmed:   user accepted; MOVE issued through imap_outbox
  - overridden:  user picked a different folder; correction recorded
  - dismissed:   user rejected suggestion without moving (stays put)
```

### Key Data Flows

1. **Sync → classify (fire-and-forget):** worker hook enqueues jobs; sync
   never waits. Dedupe: `UNIQUE(mailbox_id, uid)` on the queue; re-sync
   of an already-labeled message is a no-op (labels keyed
   `(mailbox_id, uid)` upsert-once, never overwritten except by
   re-classify command).
2. **Classify → suggest → UI:** `labels` write + `ClassificationReady`
   event (existing `Channel<SyncEvent>` pattern extended with
   `ClassificationReady / ClassificationProgress / BatchReport`).
3. **Confirm → MOVE:** identical to Phase 10 `move_message` command path,
   dest constrained to `Auto/...` (single-email) or user-picked folder
   (override). `\Seen` preservation, no body re-FETCH, pending indicator —
   all inherited.
4. **Taxonomy → questions:** UI editor / JSON import writes `taxonomy`
   (version++); worker builds Laya `criteria` from the active version and
   stamps `labels.taxonomy_version` so stale suggestions are identifiable
   after a taxonomy edit (old labels kept, flagged `stale=1` — never
   silently rewritten).

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| 1 mailbox, <1k msgs (target user) | As designed. Single-pass per mail, serial loopback calls. Backfill ~1–3 min CPU, seconds GPU. No change needed. |
| 5k–20k msgs backfill | Drain in UID order with cooperative cancel (reuse `sync_cancel` pattern); batch progress UI mandatory; consider 4-mail mini-batches only if measured idle — do NOT parallelize forward passes (Laya serializes internally; concurrent calls interleave badly per skill guidance). |
| 100k+ msgs | Out of scope for personal UTFPR use. If ever: cap snippet length harder, skip already-labeled, resumable `batch_runs` cursor (already in schema). |

### Scaling Priorities

1. **First bottleneck: cold checkpoint load (~25–35 s).** Sidecar preloads
   `multilingual` at app start in background; `classify_status` reports
   `warming` until `/health` is ready; queue simply waits. Never block
   first sync on it.
2. **Second bottleneck: CPU-only inference (~140 ms/mail).** Acceptable for
   incremental sync (a dozen new mails ≈ 2 s background). Whole-account
   batch on CPU needs the progress UI to be honest — it is, by design.

## Anti-Patterns

### Anti-Pattern 1: Calling Laya inline in the sync pass

**What people do:** `fetch_envelopes` → `predict` → write label, all inside
`sync_with_session`.
**Why it's wrong:** Holds the IMAP lease across model latency; stalls
poll cadence; a sidecar crash/hang fails the sync (violates
"replay-failure-never-fails-sync" and its classify analogue).
**Do this instead:** Post-pass enqueue + independent worker (Pattern 3).
Classify degrades to `pending`; sync never notices.

### Anti-Pattern 2: New IMAP verbs or a second IMAP session for Auto/ moves

**What people do:** Add `MOVE_CLASSIFIED` verb / open a dedicated session
for the classifier.
**Why it's wrong:** Breaks the single-session invariant (§1.1 of the v1.2
research: two sessions → EXPUNGE on wrong selection = data loss); forks
the replay discipline that Phase 10 proved.
**Do this instead:** `ensure_auto_tree` + existing `move_message_in` +
`imap_outbox`. Zero new verbs (Pattern 4).

### Anti-Pattern 3: Storing or logging email text in classification tables/logs

**What people do:** Persist the snippet/state sent to Laya for
"debuggability", or log suggestion justifications.
**Why it's wrong:** Violates the sensitive-data rule (passwords/codes in
SQLite/logs) and the no-secrets discipline. Laya returns no free text —
log labels + confidence + timings only.
**Do this instead:** Labels store `(mailbox_id, uid, primary, secondary,
confidence, taxonomy_version, status)` — pointers to the message, never
its content. Truncate state client-side (subject ≤ 200 chars, snippet ≤
1000 chars) before POST.

### Anti-Pattern 4: Frontend calling the sidecar directly

**What people do:** React `fetch('http://127.0.0.1:PORT/predict')` to skip
backend plumbing.
**Why it's wrong:** Bypasses taxonomy versioning, confidence gating, the
sensitive rule, and label persistence; two writers (UI + worker) race on
`labels`; CSP/ports become UI concerns.
**Do this instead:** All classification through Tauri commands; sidecar
port never leaves Rust (bind 127.0.0.1, ephemeral port chosen by the
backend, passed as sidecar argv).

## Integration Points

### New Tauri commands (all in NEW `commands/classify.rs`)

| Command | Input | What it does | Reuses |
|---------|-------|--------------|--------|
| `classify_message` | `mailbox, uid` | Enqueue single job (priority manual), fast-path drain if gate free | classify worker |
| `confirm_suggestion` | `mailbox, uid, dest` | `ensure_auto_tree` + Phase 10 move path; mark label `confirmed` | `move_message_in`, `imap_outbox`, Phase 11 CREATE |
| `override_label` | `mailbox, uid, dest, corrected_primary` | Same as confirm to user folder + `label_overrides` insert | same + overrides table |
| `batch_classify` | `mailbox? (none = all)` | Enqueue all unlabeled + `batch_runs` row; drain auto-confirms ≥ threshold | worker bulk mode |
| `import_taxonomy` | `json` | Validate → `taxonomy` version++ → mark prior labels `stale` | taxonomy.rs |
| `classify_status` | — | `{sidecar: warming\|ready\|down, queued, pending_count}` | bridge health + queue depth |

New events on the existing `Channel` pattern: `ClassificationReady(uid,
primary, confidence)`, `ClassificationProgress(done, total)`,
`BatchReport(run_id, per_category_counts, review_list)`.

### SQLite M12 (MODIFIED `store/`, forward-only, preserve-rows test)

```sql
-- M12-A: versioned taxonomy (one active row; history kept)
CREATE TABLE taxonomy (
  version     INTEGER PRIMARY KEY,            -- monotonic, worker stamps labels
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  active      INTEGER NOT NULL DEFAULT 0,     -- exactly one row active=1
  definition  TEXT NOT NULL                   -- canonical taxonomy JSON
);
-- M12-B: labels per message (pointers only — NEVER email content)
CREATE TABLE labels (
  mailbox_id       INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid              INTEGER NOT NULL,
  primary_label    TEXT NOT NULL DEFAULT 'A Classificar',
  secondary_label  TEXT,
  child_label      TEXT,                       -- keyword-resolved child (nullable)
  confidence       REAL NOT NULL DEFAULT 0,
  taxonomy_version INTEGER NOT NULL DEFAULT 1,
  status           TEXT NOT NULL DEFAULT 'pending',  -- pending|suggested|confirmed|overridden|dismissed
  stale            INTEGER NOT NULL DEFAULT 0,       -- taxonomy moved on
  updated_at       TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (mailbox_id, uid)
);
CREATE INDEX idx_labels_status ON labels(status);
-- M12-C: override corrections (learning signal for future, display now)
CREATE TABLE label_overrides (
  id           INTEGER PRIMARY KEY,
  mailbox_id   INTEGER NOT NULL, uid INTEGER NOT NULL,
  suggested    TEXT NOT NULL, corrected TEXT NOT NULL,
  created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
-- M12-D: classification work queue (RFC 4549 epoch hygiene like its siblings)
CREATE TABLE classify_queue (
  id           INTEGER PRIMARY KEY,
  mailbox_id   INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  uid          INTEGER NOT NULL,
  source       TEXT NOT NULL DEFAULT 'sync',  -- sync|manual|batch
  bulk_run_id  INTEGER,                        -- NULL unless batch
  uid_validity INTEGER NOT NULL,              -- epoch at enqueue; stale → drop
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  UNIQUE (mailbox_id, uid)
);
-- M12-E: batch runs (progress + report)
CREATE TABLE batch_runs (
  id           INTEGER PRIMARY KEY,
  started_at   TEXT NOT NULL DEFAULT (datetime('now')),
  finished_at  TEXT,
  total        INTEGER NOT NULL DEFAULT 0,
  done         INTEGER NOT NULL DEFAULT 0,
  report       TEXT                           -- JSON per-category counts + review list
);
```

Why separate tables (not columns on `messages`): labels have their own
lifecycle (stale flags, overrides, re-classify) and the queue needs
epoch-gated replay semantics identical to `flag_outbox`/`imap_outbox` —
the codebase's established pattern is one table per durable intent
(§3.3 of the v1.2 research). `messages` stays the sync-owned source of
truth; `labels` is keyed `(mailbox_id, uid)` alongside it and cascades
on mailbox delete (same `drop_*_for_mailbox` helper shape).

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| sync worker → classify worker | `classify_queue` table (durable handoff) | Sync never awaits; worker polls/drains under `ClassifyGate` |
| Rust backend → sidecar | Loopback HTTP `POST /predict`, `GET /health` | Serialized by bridge mutex; 10 s timeout → `attempts++`, stays queued |
| classify worker → IMAP | **None directly** — only via `imap_outbox` + manager | Confirmed moves are Phase 10 moves; classifier holds no lease |
| taxonomy → worker | Active `taxonomy` row read per drain batch | Questions rebuilt when `version` changes; labels stamped |
| backend → UI | Tauri commands + `Classification*` channel events | Same Channel pattern as `SyncEvent`/`Send*` |

### Sidecar packaging (Tauri v2, verified against official docs)

- `tauri.conf.json`: `bundle.externalBin: ["binaries/laya-sidecar"]` +
  triple-suffixed binary at
  `src-tauri/binaries/laya-sidecar-x86_64-unknown-linux-gnu` (Linux-only
  per project constraint — one triple needed).
- `capabilities/default.json`: `shell:allow-spawn` (+ `allow-execute` if
  one-shot health probes are used) with matching `name` + argv validators
  (port, `--model multilingual`, `--device auto`).
- Checkpoint weights (~322–421 M params): ship as `bundle.resources`
  (resolved via `$RESOURCE`) OR sidecar self-contained with weights
  adjacent (working dir = binary dir). Spike must decide — weights next
  to the binary is simpler for offline-first; resources path needs
  `fs:allow-resource-read-recursive`.
- New deps: `tauri-plugin-shell` (spawn), HTTP client for bridge
  (`reqwest` — verify async-std compat; else `surf`/`ureq` in
  `spawn_blocking`, matching the lettre precedent of keeping the runtime
  unmixed).
- Dev fallback: if the sidecar binary is absent (`tauri dev` without
  build), bridge connects to a locally-run `laya serve` on a documented
  port — same code path, zero special-casing in the worker.

## New vs Modified — explicit file list

### NEW files

| File | Purpose |
|------|---------|
| `src-tauri/src/classify/{mod,taxonomy,bridge,worker,suggest}.rs` | Classification lane (see §Structure) |
| `src-tauri/src/commands/classify.rs` | 6 commands above |
| `src-tauri/binaries/laya-sidecar-<triple>` | Bundled sidecar binary |
| `src-tauri/resources/taxonomy-utfpr-ptbr.json` | Default taxonomy (5 top + children + fallback + rules) |
| `ClassifyChip.tsx`, `TaxonomyEditor.tsx`, `BatchProgress.tsx` | UI surfaces |

New deps: `tauri-plugin-shell`, HTTP client. No new SQLite features.

### MODIFIED files

| File | Change | Risk |
|------|--------|------|
| `imap/manager.rs` | +`ensure_auto_tree` (CREATE missing `Auto/` levels, Phase 11 guards) | Low — additive, existing verb |
| `sync/worker.rs` | +post-pass enqueue hook (new UIDs → `classify_queue`) | Low — 3–5 lines, no await, failure never fails sync |
| `sync/mod.rs` | +`ClassifyGate` (clone of `SyncGate`) | Low — proven shape |
| `store/mod.rs` | M12 migration, `SCHEMA_VERSION` 11→12 | Low — follow M2–M11 pattern |
| `store/queries.rs` | taxonomy/labels/queue/batch helpers (single-SQL invariant) | Medium — largest diff, mechanical |
| `commands/sync.rs` or `mod.rs` | `sync_status` += queue depth (badge) | Low |
| `lib.rs` | AppState +bridge/gate, `setup()` spawn, `invoke_handler` | Low-Medium (sidecar lifecycle) |
| `tauri.conf.json`, `capabilities/default.json` | externalBin + resources + shell perms | Low — doc-driven |
| `Cargo.toml` | `tauri-plugin-shell` + HTTP client pins | Low |

### UNTOUCHED (must stay invariant)

- `SyncSession` trait — **zero new verbs**. `BODY.PEEK[]` reads,
  ammonia sanitize, `flag_outbox`/`imap_outbox`/`send_queue` schemas and
  replay semantics, convergence/tombstone logic, `SyncGate` semantics,
  keyring/secret discipline.

## Suggested build order (dependency-ordered; Phase 10/11 machinery is the floor)

1. **Sidecar packaging spike.** Bundle a hello-world sidecar via
   `externalBin` + `shell:allow-spawn`, spawn in `setup()`, `/health`
   green in dev **and** in a `bun run tauri build` Linux bundle. Resolves
   the two unknowns up front: weights placement (resources vs adjacent)
   and cold-start time on the user's machine. No UI, no schema.
2. **Taxonomy + M12 store.** Default UTFPR JSON, `taxonomy.rs`
   validation, M12 migration + queries + preserve-rows test. Headless —
   testable with `cargo test` only.
3. **Bridge + worker + labels (no moves).** `predict` client, single-pass
   3-question shape, confidence gate, queue drain under `ClassifyGate`,
   `labels` writes, `ClassificationReady` events, `classify_message` +
   `classify_status` commands. Verify: sync a folder → labels appear with
   sane pt-BR categories; sidecar down → `pending`, sync unaffected.
4. **Confirm/override → MOVE (the Phase 10/11 payoff).**
   `ensure_auto_tree` + `confirm_suggestion`/`override_label` through
   `imap_outbox`; suggestion chip UI. Verify: confirm moves to
   `Auto/<Top>/`; offline confirm replays; override records correction;
   `A Classificar` never moves.
5. **Taxonomy editor + JSON import.** UI over `import_taxonomy`; stale-flag
   display. Verify: import bumps version, old labels flagged, worker uses
   new criteria.
6. **Batch classify.** `batch_runs`, bulk enqueue, auto-confirm ≥
   threshold, progress + report UI. Verify on a test folder: counts add
   up, below-threshold mail stays `A Classificar`, report lists reviews.
7. **Polish + audit:** queue-depth badges, `classifying` list states, FTS
   over labels (optional — `messages_fts` join, cheap if wanted),
   full `cargo test` + live round-trip against `mail.utfpr.edu.br`.

Why this order: packaging risk first (only true unknown — everything
else is proven-pattern reuse); store before worker (can't write what has
no table); suggestions before moves (trust gate — user sees labels work
before anything moves); single confirmed moves before batch (batch is a
loop over the confirm path; an unproven confirm at batch scale is how
mail ends up in the wrong tree); UI editor after the engine it edits.

## Risks & open questions

1. **Cold start (~25–35 s checkpoint load)** blocks nothing by design
   (background preload, `warming` status), but first-run UX must say so —
   else "classificação não funciona" bug reports. Mitigate with honest
   status copy.
2. **CPU-only fallback latency** (~140 ms/mail): fine incremental, slow
   whole-account. Batch progress UI is the mitigation; no architecture
   change needed.
3. **`Auto` root vs user folders:** reserve `Auto` (leaf-encoded,
   delimiter-joined, Phase 11 rules); if the user already owns `Auto`,
   confirm-then-nest (`Auto` reuse with confirmation) — decide in planning,
   one dialog.
4. **Snippet fidelity:** labels derive from cached headers + `preview`
   (~200 chars) — no body FETCH forced (headers-first invariant holds).
   If accuracy disappoints on evaluation, the escalation is
   fetch-`body_text`-for-`pending`-only — a worker change, not an
   architecture change.
5. **Confidence threshold:** no universal value (Laya docs: "validate
   thresholds on representative data"). Ship a default (e.g. 0.6) behind a
   taxonomy-rule field, tunable in the editor; evaluation on real pt-BR
   mail during Phase verification sets it.
6. **Multilingual routing:** force `model: multilingual` (pt-BR traffic is
   the known case; the English checkpoint is confidently wrong on
   non-Latin scripts). Router auto-detect stays as fallback for mixed mail.

## Sources

- SGE repo at research time (`imap/manager.rs`, `sync/{mod,worker}.rs`,
  `store/{mod,schema.sql,queries.rs}`, `commands/sync.rs`, `lib.rs`,
  `Cargo.toml`) + `.planning/research/ARCHITECTURE.md` (v1.2 baseline §1)
  + `PROJECT.md` v1.3 scoping — HIGH confidence (direct source read).
- Laya `laya-integration` skill (`github.com/wdobry/laya-playground`,
  skills/laya-integration/SKILL.md): sidecar pattern, single-pass
  multi-question, one-forward-pass-at-a-time lock, 20-option shortlist
  guardrail, latency figures — MEDIUM-HIGH (project's own integration
  guidance + live `laya_status` confirming package `laya 0.3.20` present;
  checkpoint names/flags should be re-verified at build time against the
  pinned sidecar version).
- `docs.rs/laya`, `docs.rs/laya-candle`, `laya-rs` family READMEs
  (Rust-native options assessed and rejected) — MEDIUM.
- Tauri v2 official docs (`v2.tauri.app/develop/sidecar`,
  `v2.tauri.app/develop/resources`): `externalBin` + triple suffix +
  `shell:allow-spawn` + resources mapping — MEDIUM (official docs,
  cross-verified across docs repo + community write-ups; not yet
  exercised in this repo).

---
*Architecture research for: v1.3 Auto-Classify (Laya) integration*
*Researched: 2026-10-10*
