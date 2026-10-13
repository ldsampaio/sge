# Feature Research: SGE v1.5 — Sync Upgrades, Classifier Follow-Throughs, Sieve Export, Compose Hardening

**Domain:** Desktop IMAP client — push sync (IDLE), fast resync (CONDSTORE/QRESYNC), rule import/export, learning-from-overrides, per-category auto-confirm, override history, batch dry-run + retry, Sieve export
**Researched:** 2026-10-13
**Confidence:** MEDIUM (protocol behavior HIGH from RFCs; Rust-crate support MEDIUM from web sources only, not verified against Cargo.lock; UX expectations MEDIUM from competitor docs)

> Scope: v1.5 NEW features only. Already shipped (do not re-research): sync, triage, folders, drafts, SMTP send, compose, search+filters, auto-archive, smart rules, auto-classify with confirm/batch. Prior feature research (v1.3 auto-classify) was superseded by this file; its P2-deferred items (override history view, batch dry-run + retry, JSON import/export, learning-from-overrides, Sieve) are now in scope and detailed below.

## Feature Landscape

### Table Stakes (Users Expect These)

Once a client promises "live mail" and "safe bulk operations", these are assumed. Missing them = stale inbox or scary batch runs.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| IDLE push with poll fallback | Thunderbird, Apple Mail, K-9/Thunderbird-mobile, Outlook all show new mail within seconds. Users read "auto-refresh" as push; SGE's 5-min poll (Phase 8) feels broken beside webmail. RFC 2177 is a 1997 standard — its absence is conspicuous. | MEDIUM | Behavior: after SELECT, client sends `IDLE`, server holds the connection and pushes untagged `EXISTS`/`EXPUNGE`/`RECENT`/`FETCH (FLAGS)` immediately. Client sends `DONE` to break out, runs its normal sync/fetch, re-issues IDLE. Re-issue at least every ~29 min (servers MAY log off idle clients at timeout). Capability-gated: only if `CAPABILITY` advertises `IDLE`, else poll. One IDLE connection per watched mailbox — v1.5 should IDLE INBOX only and keep the 5-min poll as the multi-folder/fallback path through the existing single-flight `SyncGate`. SGE: extend `imap/session.rs` + `sync/worker.rs`; async-imap 0.11 exposes an `idle()` handle (verify against Cargo.lock at plan time — if missing, raw-command it or evaluate `daaki-imap`/`io-imap`, both of which advertise IDLE). |
| CONDSTORE fast flag resync (QRESYNC if advertised) | Large UTFPR mailboxes make full UID range-diffs expensive; Dovecot/Gmail advertise CONDSTORE and clients (Thunderbird, Apple Mail) use MODSEQ to pull only flag changes. Users expect Seen-flag changes from webmail/phone to appear locally without a full re-sync. | MEDIUM–HIGH | Behavior: server tracks a `MODSEQ` per message and `HIGHESTMODSEQ` per mailbox. Client stores last-seen MODSEQ in SQLite, then `SELECT … (CONDSTORE)` returns only messages with `MODSEQ > stored` (flag deltas). QRESYNC (RFC 7162 §6) goes further: `SELECT/EXAMINE … (QRESYNC uidvalidity last-uidnext known-uids …)` returns vanished-UID (expunged) sets + changed flags in one round trip — no UID range walk. Capability-gated in layers: QRESYNC ⇒ CONDSTORE implied; CONDSTORE-only servers (e.g. Gmail) get the flag-delta path; neither ⇒ keep Phase 9 UID range-diff backfill as fallback. SGE: MODSEQ columns in SQLite (`store/`), extend `sync/` convergence; async-imap 0.11 very likely lacks CONDSTORE/QRESYNC primitives (crates that do: `daaki-imap`, `io-imap` — both advertise CONDSTORE/QRESYNC per their READMEs) → flag "crate gap" as plan-time research: raw untagged-response parsing vs dependency swap. |
| Batch dry-run / preview | Gmail's retroactive-apply checkbox shows the matching-conversation count before committing; Outlook rule "Run Rules Now" scopes to a folder first; every destructive-adjacent bulk op in modern UX has a preview. v1.3 batch moves whole accounts with only a post-hoc report — users will not run it twice without a "what would happen" mode. | LOW–MEDIUM | Behavior: run the exact batch planner (classifier or rule matcher) with the write path disabled; render the same report shape (N would move, per-folder breakdown, K to `A Classificar`, errors) with a [Run for real] action. SGE: reuse `classify/batch.rs` planner + report renderer with a `dry_run` flag; zero new IMAP surface. Dry-run is also the natural pre-flight for retry-failed (re-preview the failed subset). |
| Batch retry-failed | Every batch system surfaces partial failure (Thunderbird filter-run errors, Gmail partial applies, S3-style batch job reports). A 5,000-message run that fails 12 ways with no retry path forces a full re-run — slow and untrustworthy. | LOW | Behavior: persisted run record keeps per-item status (moved / skipped / failed + error); failure list view offers [Retry failed] (re-runs only failed UIDs through the same MOVE/outbox path) and [Dismiss]. SGE: extend the existing batch run record + journaled batch machinery (`classify/batch.rs`, Phase 10 MOVE outbox in `imap/`); retry reuses the offline queue so it survives disconnects. Standard retry-with-backoff applies to transient IMAP errors (Azure retry-pattern guidance: bounded attempts, then surface as exception). |
| Override history view | Gmail's filter list and Superhuman's label activity both let users audit what automation did; v1.3 already records overrides (UID, old→new category, timestamp) but shows no UI. A correction log with no viewer is write-only telemetry. | LOW | Behavior: chronological list (timestamp, message subject/sender, old → new category, origin: user-corrected vs auto), filterable by category, with per-entry [Move back] and per-category correction counts ("you corrected Finances → 7×"). SGE: read-only UI over the existing overrides table; counts feed learning-from-overrides and per-category auto-confirm eligibility. |
| Rule import/export (smart rules + taxonomy JSON) | Thunderbird stores filters in portable `msgFilterRules.dat`; Gmail exports filters as XML for backup/migration; users reinstalling or sharing setups expect their rules to survive. SGE smart rules (v1.4) and taxonomy (v1.3) are currently trapped in local SQLite. | LOW–MEDIUM | Behavior: export = serialize rules/taxonomy to versioned JSON file (schema version field for forward-compat); import = validate then upsert. Validation mirrors the v1.3 rules: cycle check on taxonomy parents, duplicate-name detection, reserved `Auto` root guard, per-rule predicate validation (same validators as the rules editor). Import conflicts (same name, different predicate) resolve via user choice: keep / replace / keep-both(renamed) — never silent overwrite. SGE: extend `classify/rules.rs` + `classify/taxonomy.rs` with serde schema + validation; small file-picker UI. This is the portable substrate Sieve export also consumes. |
| Compose end-to-end hardening (send, drafts, reply, reply-all) | Non-negotiable for any mail client: every reference client (Thunderbird, Apple Mail, Gmail) guarantees reply threading, single-copy drafts, and exactly-once Sent filing. v1.5 does NOT rebuild compose — it verifies the v1.2 pipelines (`drafts.rs`, `send_queue.rs`, `smtp.rs`, outbox replay) live and fixes gaps. | LOW–MEDIUM (verify + fix, not rebuild) | Expected behavior checklist: reply/reply-all set correct `In-Reply-To`/`References` and quote attribution; draft APPEND keeps one server copy per session (established v1.2 invariant); send files to Sent exactly once via Message-ID dedupe; offline queue retries with visible backoff; failures surface, never silent-drop. Depends on live validation with real creds under the sandbox contract (`SGE-Test-<run-id>` scoping). |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Learning-from-overrides (no fine-tuning) | Nobody in the desktop space does this well: Thunderbird/Gmail require manual filter editing after every mistake; Superhuman retunes opaque prompts. SGE can close the loop locally — each correction quietly improves future suggestions — while honoring the PROJECT.md scope guard (no Laya checkpoint fine-tuning; logging hooks only). | MEDIUM | Recommended mechanism (active-learning-lite, all local): (1) keyword-weight adaptation — an override X→Y up-weights the message's distinctive tokens for Y and down-weights for X in the keyword-assisted layer (`classify/suggest.rs`, `classify/rules.rs`); (2) per-category confidence-threshold adaptation — categories with high override rates get their auto-suggest threshold raised automatically. Both are interpretable, reversible (decay weights on further corrections), and need no training pipeline. Requires: override history (above) as the dataset + already-planned logging hooks. Explicitly NOT model fine-tuning — keep that as the stated v2+ path. |
| Per-category auto-confirm (trust escalation) | Graduated autonomy is rare: Gmail filters are all-or-nothing; Thunderbird filters always run unconfirmed. SGE can offer "you've accepted `Auto/Financeiro` 20× with no corrections — auto-file future ones?" — automation earned per category, revocable in one click. | MEDIUM | Behavior: eligibility = N consecutive accepted suggestions in a category with zero overrides + confidence above threshold (tune N, default ~15–20); UI offers opt-in per category (never global, never default-on); auto-filed mail gets a distinct badge + still appears in activity so misfiles are discoverable; one-click revoke returns the category to confirm-mode. Depends on: confirm dialog pipeline (v1.3), override stats (history view), confidence store. Anti-abuse: a single override in an auto-confirmed category pauses auto-confirm and re-prompts. |
| Sieve export (server-side rule script) | True webmail parity: Thunderbird/Gmail power users expect filters to run even when the desktop client is closed. No local-only competitor offers one-click "take my rules server-side". SGE smart rules map cleanly onto Sieve `header`/`address` tests + `fileinto` actions (RFC 5228 base language — no variables, no loops, GUI-generatable by design). | MEDIUM–HIGH | Behavior: generate a `require ["fileinto"];`-style script from smart rules (one `if header/address … { fileinto "Folder"; }` block per rule, deterministic ordering, `elsif` chains for same-priority rules); present in a read-only preview with [Copy] and [Download .sieve]; upload via ManageSieve (RFC 5804) ONLY if the server advertises it, else export-only with instructions. Safety: never overwrite an existing server script without downloading/backing it up first and showing a diff; append-or-replace is an explicit user choice. Caveats (validate at plan time): PROJECT.md previously scoped this out as "a second product" — keep it export-first, upload-if-trivial; `mail.utfpr.edu.br` Sieve/ManageSieve support is unknown → capability-probe during planning, graceful degrade to export-only. Pure-Rust Sieve generation needs no new protocol crate for export; upload needs a ManageSieve client (check `sieve-form`-class crates or raw implementation). |
| IDLE-triggered classify-on-arrival | Push + on-device classification composed: new mail arrives via IDLE → headers land → Laya classifies → confirm banner appears within seconds. Neither Thunderbird (push, but header-rules only) nor AI-mail clients (content classification, but cloud-poll) offer instant *offline* content triage. | LOW–MEDIUM (composition) | Almost free once IDLE + classify-on-sync both exist: IDLE wake feeds the same post-sync classify hook v1.3 built for poll arrivals. Must preserve the "classify async, never block first paint" invariant and the single-email always-confirmed gate (auto-confirm categories excepted). Thrash guard: classify once per UID; flag-only wakeups (no new UIDs) skip inference. |
| Live-validation closure + bundle/UAT proof | Process-as-feature: v1.1–v1.4 deferred every live gate (no creds in this environment); v1.5 runs `live-21/22/26.sh` against a real account under the sandbox contract and proves `npm run build` on Node 20+ plus human click-through UAT. Competitors don't ship this, but users feel it — it's what turns "252+ tests green" into "actually works against my server". | LOW (execution, not code) | Behavior: credentialed runs via git-ignored `.live-test.env`, `SGE-Test-<run-id>` folder scoping, own-address-only sends, post-run cleanup; draft-send gap (FAIL BY DESIGN) closed or explicitly re-deferred with reason; UAT covers search/filter restore, archive trigger→undo, rules tracer. Depends on nothing new — unblocks confidence in everything above. |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| IDLE-only sync (remove the poll timer) | "Push is modern; polling wastes battery/bandwidth" | Servers silently drop IDLE (NAT timeouts, 29-min cap, single-mailbox limitation); multi-folder coverage needs N held connections; a dropped IDLE with no poll = silently stale mail. The v1.3 decision log already documents convergence blind spots from skipped sweeps. | Keep the 5-min poll as fallback/supervisor: IDLE for INBOX immediacy, poll tick verifies IDLE health (reconnect if dead) and covers non-IDLEd folders. Same guarded `SyncGate` code path for both. |
| Assuming QRESYNC everywhere | "One fast path for all servers" | Many servers are CONDSTORE-only (Gmail is the canonical example); some offer neither. Blind QRESYNC SELECTs fail against them. | Three-tier capability ladder: QRESYNC → CONDSTORE deltas → Phase 9 UID range-diff. Probe `CAPABILITY` at connect; store per-account capability profile; test each tier in CI with a fake server. |
| Laya fine-tuning on user mail in v1.5 | "It should really learn from my corrections" | Same verdict as v1.3: training pipeline (dataset curation, eval harness, checkpoint versioning, rollback) dwarfs the milestone; override volume after one milestone is too small to generalize; PROJECT.md scope guard explicitly excludes runtime checkpoint changes. | Keyword-weight + threshold adaptation (above) + logging hooks that bank training data for a future milestone. |
| Global auto-file / auto-confirm by default | "Full automation" demo appeal | One wrong unattended MOVE on a boleto/matrícula email destroys trust; IMAP MOVEs are hard to audit after the fact (v1.3 already decided: single-email always-confirmed). | Per-category earned opt-in only (above), with pause-on-override and visible auto-file activity. |
| Blind Sieve upload (overwrite server script) | "One click and my webmail matches" | Users may have hand-written server scripts; a blind PUT destroys them; `fileinto` targets may not exist server-side (Sieve fails closed on missing mailboxes without `:create`-style handling). | Backup-then-diff flow: fetch existing script, show diff, explicit append/replace choice; validate `fileinto` targets against the IMAP folder tree first. |
| Reclassifying on every flag/sync event | "Everything always perfectly labeled" | Wastes sidecar inference cycles per poll tick; taxonomy edits or weight adaptation can cause move→re-move thrash loops (already a v1.3 anti-feature). | Classify once per UID (new arrivals + explicit triggers); taxonomy/weight changes apply prospectively + optional explicit re-run batch with dry-run first. |
| Multi-mailbox IDLE fan-out in v1.5 | "Push for every folder" | N held connections × reconnect/backoff supervision × server IDLE limits (many servers cap concurrent IDLEs per account); complexity explosion for marginal gain (non-INBOX folders change rarely and are poll-covered). | IDLE INBOX only in v1.5; revisit fan-out after single-mailbox IDLE proves stable. |

## Feature Dependencies

```
IDLE push (INBOX)
    └──requires──> Phase 8 poll path + SyncGate (fallback supervisor, same code path)
    └──requires──> CAPABILITY probing (imap/probe.rs) extended with IDLE flag
    └──requires──> Phase 9 backfill/convergence (IDLE wake reuses UID range-diff for the actual fetch)
    └──enhances──> Classify-on-arrival (IDLE wake → post-sync classify hook)

CONDSTORE / QRESYNC fast path
    └──requires──> CAPABILITY probing (CONDSTORE/QRESYNC flags per account)
    └──requires──> SQLite MODSEQ columns (store/) + HIGHESTMODSEQ per folder
    └──requires──> Phase 9 convergence logic (third tier: QRESYNC → CONDSTORE → UID range-diff)
    └──conflicts──> async-imap 0.11 capability gap (plan-time: raw commands vs daaki-imap/io-imap)

Batch dry-run
    └──requires──> classify/batch.rs planner + report renderer (write-path flag)
    └──enhances──> Batch retry-failed (preview the failed subset before re-run)

Batch retry-failed
    └──requires──> Batch run record with per-item status (classify/batch.rs)
    └──requires──> Phase 10 MOVE verb + offline outbox (retry transport)

Override history view
    └──requires──> v1.3 overrides table (already recorded)
    └──enhances──> Learning-from-overrides (dataset) + per-category auto-confirm (eligibility stats)

Learning-from-overrides
    └──requires──> Override history (dataset + stats)
    └──requires──> classify/suggest.rs + classify/rules.rs keyword/threshold layer
    └──requires──> Logging hooks for future training data (scope-guard ceiling: no checkpoint changes)

Per-category auto-confirm
    └──requires──> Confirm-dialog pipeline (v1.3) + confidence store
    └──requires──> Override stats per category (history view)
    └──conflicts──> Global auto-file (explicitly rejected; per-category opt-in only)

Rule import/export (JSON)
    └──requires──> classify/rules.rs + classify/taxonomy.rs stores + editor validators
    └──enhances──> Sieve export (portable rule model is the generation substrate)

Sieve export
    └──requires──> Rule import/export model (canonical rule representation)
    └──requires──> IMAP folder tree (fileinto target validation, Phase 11 roles/create)
    └──requires──> Server capability probe (ManageSieve?) — upload path conditional
    └──conflicts──> Blind server-script overwrite (backup-then-diff mandated)

Compose hardening
    └──requires──> v1.2 pipelines (drafts.rs, send_queue.rs, smtp.rs, outbox replay) — verify + fix only
    └──requires──> Live validation with real creds (sandbox contract) — proof vehicle

Live validation closure + bundle/UAT
    ──cross-cuts──> ALL v1.5 features (proof vehicle, no code dependency)
```

### Dependency Notes

- **Sync upgrades compose with, not replace, existing machinery:** IDLE is a wake-up source feeding the Phase 8/9 sync path; CONDSTORE/QRESYNC are faster ways to compute the same delta the range-diff already computes. All three tiers must funnel through the single-flight `SyncGate` — do not build a parallel sync engine.
- **The batch trust pair (dry-run + retry) is one work item, not two:** dry-run is the planner with writes off; retry is the run record with writes re-on for failures. Build the per-item-status run record first, both features fall out.
- **Override history is the foundation of the learning pair:** history view banks the dataset and the per-category stats; learning (weight/threshold adaptation) and auto-confirm (eligibility) are both consumers. Build history first.
- **Rule JSON portability precedes Sieve:** the canonical serializable rule model serves import/export AND Sieve generation. Do not generate Sieve from SQLite rows directly — generate from the portable model so round-trips stay lossless.
- **Crate gap is the critical plan-time unknown:** async-imap 0.11's IDLE handle needs verifying and its CONDSTORE/QRESYNC support is doubtful (web evidence only). The plan phase must spike: raw `IDLE`/`SELECT (CONDSTORE)`/`QRESYNC` against a dovecot test container vs adopting `daaki-imap`/`io-imap`. This single spike de-risks both sync features.
- **Sieve upload is conditional, export is not:** export (pure string generation + preview UI) has no server dependency and ships regardless; upload waits on the ManageSieve capability probe. Plan them as separate shippable slices so an unsupported server doesn't block export.

## MVP Definition (v1.5 scope)

### Launch With (v1.5)

Minimum to "close every open thread" per PROJECT.md:

- [ ] IDLE push for INBOX with poll fallback/supervisor — the headline sync upgrade; same `SyncGate` path
- [ ] CONDSTORE delta sync with UID range-diff fallback (QRESYNC if the spike shows it cheap, else CONDSTORE-only + documented follow-up) — fast path without a second sync engine
- [ ] Batch dry-run + retry-failed over the per-item run record — makes whole-account batch trustworthy
- [ ] Override history view with per-category counts — banks the dataset, visible control
- [ ] Rule import/export (JSON, validated) — portability for smart rules + taxonomy
- [ ] Learning-from-overrides v1 (keyword-weight + threshold adaptation, logging hooks) — within the no-fine-tuning scope guard
- [ ] Per-category auto-confirm (earned opt-in, pause-on-override) — graduated autonomy
- [ ] Sieve export (preview + copy/download; upload only if ManageSieve probes positive) — webmail parity slice
- [ ] Compose hardening verified live (send/drafts/reply/reply-all over v1.2 pipelines) — no rebuild
- [ ] Live-validation closure (`live-21/22/26.sh` with real creds, sandbox contract) + Node 20+ bundle + human UAT — proof vehicle for all of the above

### Add After Validation (v1.5.x)

- [ ] QRESYNC full path — if v1.5 ships CONDSTORE-only, add vanished-set resync after observing real MODSEQ behavior against `mail.utfpr.edu.br`
- [ ] Multi-mailbox IDLE fan-out — after single-mailbox IDLE proves stable (server IDLE limits measured, not assumed)
- [ ] Confidence-threshold setting UI — expose tuning after observing real per-category override rates (v1.3 already deferred this; learning-v1 generates the data to tune it with)
- [ ] Scheduled batch runs — dry-run + retry prove the manual path first; scheduling is a timer over a proven op

### Future Consideration (v2+)

- [ ] Laya fine-tuning on banked override data — needs the accumulated dataset + eval harness; explicitly out of scope until then
- [ ] Cross-device taxonomy/rule sync — no sync story exists in SGE at all yet
- [ ] Full ManageSieve two-way sync (download server script → convert to smart rules) — reverse direction of export; a second-product surface of its own

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| IDLE push + poll fallback | HIGH (mail feels instant) | MEDIUM | P1 |
| CONDSTORE deltas (+QRESYNC if cheap) | HIGH (large-mailbox speed) | MEDIUM–HIGH | P1 |
| Batch dry-run + retry-failed | HIGH (batch trust) | LOW–MEDIUM | P1 |
| Override history view | MEDIUM (control + dataset) | LOW | P1 |
| Rule import/export JSON | MEDIUM (portability) | LOW–MEDIUM | P1 |
| Learning-from-overrides v1 | MEDIUM–HIGH (compounding value) | MEDIUM | P1 |
| Per-category auto-confirm | MEDIUM (earned convenience) | MEDIUM | P1 |
| Sieve export (export-first) | MEDIUM (webmail parity) | MEDIUM | P1 (export slice); upload P2-conditional |
| Compose hardening (verify+fix) | HIGH (core client promise) | LOW–MEDIUM | P1 |
| Live validation + bundle/UAT | HIGH (proof, unblocks trust) | LOW (execution) | P1 |
| QRESYNC full path (if deferred) | MEDIUM | MEDIUM | P2 |
| Multi-mailbox IDLE | LOW–MEDIUM | HIGH | P3 |
| Scheduled batch | LOW–MEDIUM | LOW | P3 |

**Priority key:**
- P1: Must have for v1.5 (milestone goal commits to closing these threads)
- P2: Should have, add when possible (conditional on probes: ManageSieve, QRESYNC cost)
- P3: Nice to have, future consideration (needs stability data from P1s first)

## Competitor Feature Analysis

| Feature | Thunderbird | Gmail / Apple Mail | AI-mail (Superhuman/Shortwave) | SGE v1.5 Approach |
|---------|-------------|--------------------|---------------------------------|-------------------|
| Push / live arrival | IDLE + poll fallback, per-folder | Push (proprietary/GMAP extensions) | Push (cloud) | IDLE for INBOX + 5-min poll supervisor, one SyncGate path |
| Fast resync | CONDSTORE + QRESYNC where advertised | CONDSTORE (Gmail); proprietary deltas | Server-side (no client sync) | Tiered: QRESYNC → CONDSTORE → UID range-diff, capability-probed |
| Bulk preview | Run-filters scope, no real dry-run | Match-count preview on retroactive apply | N/A (labels, not moves) | Full dry-run rendering the real report shape, writes off |
| Bulk failure handling | Error dialogs, manual re-run | Silent partials, manual re-apply | N/A | Per-item run record + retry-failed subset |
| Correction learning | Manual filter editing | Manual filter editing | Opaque prompt retune (cloud) | Local keyword-weight + threshold adaptation, interpretable + reversible |
| Graduated auto-file | Filters always run (no gating) | Filters always run (no gating) | Auto-labels always on | Per-category earned opt-in, pause-on-override, revocable |
| Rule portability | msgFilterRules.dat (portable file) | Filter export XML | N/A (prompts, not portable) | Versioned JSON import/export, validated, conflict-resolving |
| Server-side rules | Sieve via plugin (Sieve extension) | Native (same engine everywhere) | Native (same engine) | Sieve export-first (preview/copy/download); ManageSieve upload conditional |
| Correction audit | None (edits are the audit) | Filter list (rules, not corrections) | Minimal | Override history view: what changed, per-category counts, move-back |

## Sources

- RFC 2177 (IMAP4 IDLE command — B. Leiba, 1997): IDLE/DONE flow, untagged EXISTS/EXPUNGE push, 29-min re-issue advisory, capability gating (HIGH)
- RFC 7162 (CONDSTORE + QRESYNC — obsoletes RFC 4551/5162): MODSEQ/HIGHESTMODSEQ flag deltas, QRESYNC SELECT parameter with known-UIDs + vanished sets, CONDSTORE-implied-by-QRESYNC (HIGH)
- RFC 5228 (Sieve base language — Guenther/Showalter, 2008): `fileinto`/`keep`/`discard`, `header`/`address` tests, `require ["fileinto"]`, no-variables/loops design for safe server-side GUI-generated filters (HIGH)
- RFC 5804 (ManageSieve, referenced for upload-conditional planning) (MEDIUM — not fetched, standard-track protocol for script upload)
- async-imap / async_imap docs.rs + chatmail/async-imap repo: Rust IMAP client surface (MEDIUM — IDLE handle assumed present, CONDSTORE/QRESYNC doubtful; verify vs Cargo.lock at plan time)
- daaki-imap (crates.io: tokio/rustls, advertises IDLE + CONDSTORE/QRESYNC + MOVE) and io-imap (lib.rs: incremental resync via CONDSTORE/QRESYNC + mailbox watcher) as fallback-crate candidates (MEDIUM — README claims, not integration-verified)
- Thunderbird message-filters + msgFilterRules.dat portability, Gmail filter export/retroactive-apply UX, Superhuman auto-labels/splits, Shortwave AI filters (MEDIUM — competitor docs, carried over from v1.3 research)
- Active-learning literature (CACM, Wikipedia active learning): query-the-user correction loop as the principled basis for learning-from-overrides v1 (MEDIUM — conceptual grounding, not a specific algorithm recommendation)
- Azure Retry pattern guidance: bounded retries with backoff, then surface as exception — basis for retry-failed semantics (MEDIUM)
- SGE PROJECT.md v1.5 scoping (sandbox contract, no-fine-tuning guard, Sieve tentatively in-scope, compose verify-not-rebuild) + codebase map (`sync/worker.rs`, `imap/session.rs` + `probe.rs`, `classify/batch.rs` + `rules.rs` + `suggest.rs` + `taxonomy.rs`, `drafts.rs`, `send_queue.rs`, `smtp.rs`) (HIGH — local ground truth)

---
*Feature research for: SGE v1.5 (sync upgrades, classifier follow-throughs, Sieve export, compose hardening)*
*Researched: 2026-10-13*
