# Project Research Summary

**Project:** SGE — Linux IMAP Desktop Client (v1.5: sync upgrades, classifier follow-throughs, Sieve export, compose hardening)
**Domain:** Desktop IMAP email client (Rust + Tauri v2 + React + SQLite)
**Researched:** 2026-10-13
**Confidence:** HIGH (architecture/stack vendored-source reads); MEDIUM (protocol wire behavior, competitor UX, crate youth)

## Executive Summary

SGE v1.5 closes every open thread from v1.1–v1.4 on top of a proven single-session IMAP core (SessionManager lease discipline, SyncGate single-flight, forward-only migrations, durable outboxes, ClassifyGate worker). Experts build this class of upgrade — IDLE push, CONDSTORE/QRESYNC fast resync, server-side rule export, correction-driven learning — as **additive lanes that consume existing machinery, never as parallel engines**. The research converges: IDLE is an advisory wakeup feeding the same sync pass; CONDSTORE is a branch inside the existing pass; Sieve export is an offline pure function; learning is append-only telemetry plus a read-only aggregator. No new sync engine, no second writer, no model fine-tuning.

The recommended approach is **probes first, then headless sync core, then user-visible trust features, with Sieve upload strictly conditional**. Live CAPABILITY (IDLE/CONDSTORE/QRESYNC) + port-4190/ManageSieve probes gate every protocol decision before code; CONDSTORE wiring lands before the IDLE watcher so push wakeups run a cheap pass; the override-event log lands before any learning/auto-confirm consumer; export-text-first Sieve always ships while upload waits on a positive probe. Compose hardening (verify + fix the v1.2 lettre pipelines, never rebuild) runs parallelizable anytime once the sandbox pre-flight is proven.

The key risks are structural, not speculative: (1) IDLE holding the single manager-owned session hostage — mitigated by a manager-owned IdleWatcher on a **dedicated second connection**, wakeup-only through SyncGate, poll retained as fallback/supervisor; (2) MODSEQ trusted across a UIDVALIDITY epoch change — mitigated by UIDVALIDITY-first gating with stored-modseq discard plus transcript tripwire tests; (3) per-category auto-confirm + learner forming a self-reinforcing misfile loop — mitigated by shipping history → auto-confirm → learner as one unit with opt-in-per-category, pause-on-override, explicit-signals-only training, and digest + one-click undo; (4) real-creds live runs leaking secrets — mitigated by a transcript scrubber + git-ignored pre-flight + post-run secret grep as entry criteria before the first credentialed run.

## Key Findings

### Recommended Stack

No new IMAP stack. Everything v1.5 needs except Sieve upload is already pinned: async-imap 0.11.3 IDLE handle + `select_condstore` + `run_command*` escape hatches (verified in vendored source, not web docs), imap-proto 0.16 MODSEQ/HIGHESTMODSEQ parsing, tokio sync/time supervision, rusqlite 0.37 + rusqlite_migration 2.x schema, serde/chrono, lettre 0.11. The single addition is io-managesieve 0.2.0 (`native-tls` feature only) for the Sieve **upload leg only** — and it is added only if the port-4190 probe succeeds. See STACK.md for install/pin records and the 0.12-bump rejection.

**Core technologies:**
- async-imap 0.11.3 IDLE (`Session::idle` → `Handle::init/wait`, 29-min re-IDLE) — push wakeups, zero new deps.
- async-imap 0.11.3 `select_condstore` + `run_command*` hatch + imap-proto 0.16 MODSEQ parsing — CONDSTORE fast path as wiring, not protocol work.
- io-managesieve 0.2.0 (`default-features=false, features=["native-tls"]`, `spawn_blocking`-hosted) — Sieve upload leg only, probe-gated.
- rusqlite 0.37 + rusqlite_migration 2.x — `override_events` log, per-folder `highestmodseq` (column already exists), rule version/etag.
- tokio 1 (`rt/sync/time`) + futures 0.3 — IDLE supervision, `watch`/`Notify` fan-in to SyncGate, debounce.

### Expected Features

SGE v1.5 ships the full close-the-threads set: IDLE push (INBOX-only) + poll supervisor, CONDSTORE deltas (QRESYNC if the spike shows it cheap, else CONDSTORE-only with documented follow-up), batch dry-run + retry-failed over a per-item run record, override history view, versioned JSON rule import/export, learning-from-overrides v1 (keyword-weight + threshold adaptation, logging hooks, no fine-tuning), per-category auto-confirm (earned opt-in, pause-on-override), Sieve export (preview/copy/download; upload conditional), compose hardening verified live, and live-validation closure + bundle/UAT as the proof vehicle. See FEATURES.md for the table-stakes/differentiator/anti-feature analysis and dependency graph.

**Must have (table stakes):**
- IDLE push for INBOX with 5-min poll fallback/supervisor — mail feels instant.
- CONDSTORE flag-delta resync with UID range-diff fallback — large-mailbox speed without a second engine.
- Batch dry-run + retry-failed — makes whole-account batch trustworthy (one work item, not two).
- Override history view with per-category counts — visible control + learning dataset.
- Rule import/export (versioned JSON, validated, conflict-resolving) — portability substrate Sieve also consumes.
- Compose hardening verified live (send/drafts/reply/reply-all) — verify + fix, not rebuild.
- Live-validation closure + bundle/UAT — proof vehicle for everything.

**Should have (competitive):**
- Learning-from-overrides v1 (local keyword-weight + threshold adaptation, interpretable + reversible) — compounding local value.
- Per-category auto-confirm (earned opt-in, precision display, digest + undo) — graduated autonomy.
- Sieve export-first (preview/copy/download) — webmail parity slice; upload P2-conditional on probe.
- IDLE-triggered classify-on-arrival — near-free composition once IDLE + classify-on-sync coexist.

**Defer (v2+):**
- Laya fine-tuning on banked override data — needs dataset volume + eval harness; scope-guarded out.
- Multi-mailbox IDLE fan-out — after single-mailbox IDLE proves stable (server IDLE limits measured).
- Full ManageSieve two-way sync (server-script → smart-rule import) — a second product surface.
- Scheduled batch runs, confidence-threshold setting UI — need stability/rate data from P1s first.

### Architecture Approach

Three new lanes plus compose hardening, all consuming proven machinery: `imap/idle.rs` IdleWatcher (own module, own connection, read-only loop), CONDSTORE branch inside `sync/worker.rs` (capability-gated, additive), `classify/{sieve,rules_io,learn,planner}.rs` pure functions (zero IMAP contact), `override_events` append-only telemetry (M20) with fire-and-forget hooks, history/batch UI as read models. Single-session ownership, SyncGate semantics, outbox replay order, UIDVALIDITY epoch gating, BODY.PEEK, keyring-only secrets, and forward-only migrations all stay invariant. See ARCHITECTURE.md for the system diagram, five patterns, data flows, and the explicit new/modified/untouched file list (new deps: none).

**Major components:**
1. `imap/idle.rs` IdleWatcher — dedicated second read-only connection, IDLE loop, gate wakeup only.
2. CONDSTORE fast path (`SyncSession` +2–4 methods, worker step-2 branch, `highestmodseq` persist) — flag-only CHANGEDSINCE refresh.
3. `classify/sieve.rs` + `rules_io.rs` — offline RFC 5228 generator + versioned JSON envelope.
4. `override_events` (M20) + `classify/learn.rs` — append-only log, pure read-only aggregator.
5. `classify/planner.rs` + batch retry — shared matcher with `execute=false` dry-run, failed-only re-enqueue.

### Critical Pitfalls

Top risks from PITFALLS.md (8 critical documented; 5 load-bearing below):

1. **IDLE holds the only connection hostage** — never park IDLE on the manager session, never a raw second `Client` beside it. Manager-owned IdleWatcher, DONE-on-all-exits + 29-min re-IDLE, advisory-wakeup-only through SyncGate, poll intact.
2. **MODSEQ trusted across a UIDVALIDITY epoch change** — UIDVALIDITY-first always; stored modseq discarded with mailbox state on epoch bump; per-connection capability gating; transcript tripwire tests for MODSEQ/VANISHED shapes.
3. **Auto-confirm trust erosion + silence-as-approval learner poisoning** — per-category opt-in default-off with precision display, N-streak eligibility, auto-disengage on override, journaled MOVEs + digest + one-click undo, explicit-signals-only training.
4. **Secret leakage in live runs** — transcript scrubber (LOGIN/AUTHENTICATE/env) + `git check-ignore` pre-flight + `/tmp`-only trace sinks + post-run secret grep, all as entry criteria before first real-creds run.
5. **Sieve second-product trap + replay races + import drift + compose sandbox** — 4190 probe before any uploader code (probe fails → export-text-only); all triggers through SyncGate with row-level outbox claims and dry-run `&`-only purity; versioned schema + round-trip fixpoint test + folder/pattern validation; reply-all recipient allowlist enforced on the *resolved* list + exactly-once via Message-ID dedupe.

## Reconciled Tensions (binding decisions)

Four cross-file tensions surfaced during synthesis. These rulings are binding on the roadmapper and planners — do not re-litigate without new vendored-source or live-probe evidence.

### 1. CONDSTORE support — ARCHITECTURE wins (HIGH vendored-source evidence)

ARCHITECTURE verified in vendored source that `select_condstore`, `HIGHESTMODSEQ`/`MODSEQ` parsing, raw-string `uid_fetch`/`uid_search` passthrough, and the `mailboxes.highest_modseq` column **all already exist** — v1.5 work is wiring, not protocol. FEATURES/STACK doubt (MEDIUM/LOW, web-only sources, written before the vendored read) is superseded. **Rule:** CONDSTORE ships as the tier-2 fast path (QRESYNC → CONDSTORE → UID range-diff ladder retained); the plan-time spike **confirms** wiring against a fake-server transcript, it does not re-decide feasibility. **QRESYNC stays deferred** — `* VANISHED` lands in async-imap's `pub(crate)` unsolicited channel invisible to SGE, so full QRESYNC needs owned `run_command`/`read_response` parsing; expunge detection stays SEARCH-diff (correct with CONDSTORE alone). Revisit QRESYNC only as a measured follow-up (v1.5.x), never inside the CONDSTORE phase.

### 2. Second connection for IDLE — structural constraint (ARCHITECTURE + PITFALLS agree)

Both files converge and the synthesis records it as non-negotiable: the IdleWatcher owns a **dedicated second connection** minted via the existing `connect_sync` (same `AccountConfig` clone, same TLS discipline, manager-consistent reconnect/backoff), runs **SELECT + IDLE + DONE + LOGOUT read-only verbs only**, and signals exclusively via `SyncWake::request_sync` → `SyncGate::try_begin` (skip-if-busy, coalesced dirty-flag, never a second sync engine). **It never parks on the manager session** (`Session::idle(self)` consumes the session — parking it would serialize every verb behind a 29-minute block and break the lease state machine). The watcher lives in its own `imap/idle.rs` module with no access to the manager's session so the invariant is structural, not conventional. Poll stays as fallback (unadvertised/broken IDLE) and multi-folder sweeper.

### 3. Sieve upload — export-text-first always ships; upload is a probe-gated slice

STACK proposes io-managesieve 0.2.0 upload-conditional-on-probe; ARCHITECTURE says offline string-generation only (upload = scope creep). **Rule:** `classify/sieve.rs` export (preview + copy + `.sieve` download, golden-file tested, `fileinto`-only subset, non-expressible predicates as comments + warning count) **always ships** with zero new deps and zero server dependency. The ManageSieve upload leg (port-4190 reachability + greeting + `require`-capability check → `cargo add io-managesieve --no-default-features --features native-tls` → PUTSCRIPT/CHECKSCRIPT/SETACTIVE behind explicit confirm with backup-then-diff, never auto-sync) is a **separate shippable slice added only if the probe succeeds**; probe fails → phase converts to export-text-only with documented deferral, and no uploader code is written. Coexistence badges ("managed by Sieve" mutual exclusion with local auto-file) ride with whichever slice ships.

### 4. Build order — ONE recommended order (probes first, compose parallelizable)

PITFALLS orders by risk-gating (probes → IDLE → CONDSTORE → classifier → batch → compose → Sieve last); ARCHITECTURE orders by engineering dependency (event log → CONDSTORE → rule io/policy → IDLE → dry-run/retry → Sieve → learn/history → compose anytime); FEATURES orders by data dependency (history → learning/auto-confirm; run-record → dry-run/retry). The conflict that matters is CONDSTORE-vs-IDLE: PITFALLS puts CONDSTORE after IDLE (MODSEQ depends on stable SELECT behavior) while ARCHITECTURE puts CONDSTORE before IDLE (every wakeup runs the pass — a cheap pass makes push cheap). **Ruling: ARCHITECTURE wins**, because the separate-connection decision (#2 above) removes PITFALLS's premise — the watcher never touches the manager's SELECT lifecycle, so IDLE cannot destabilize CONDSTORE semantics. PITFALLS's genuine concern is preserved as in-phase acceptance criteria instead (UIDVALIDITY-first gating + epoch-clears-modseq test + transcript tripwire inside the CONDSTORE phase). The unified order below keeps capability probes/capability-gates first (PITFALLS), telemetry-before-consumers and model-before-generator (ARCHITECTURE/FEATURES), Sieve last and conditional (all three), and compose hardening parallelizable anytime (ARCHITECTURE).

## Implications for Roadmap

Based on research, suggested phase structure (ONE recommended order):

### Phase 1: Live-validation gate + capability probes
**Rationale:** Every protocol decision (IDLE-vs-poll, CONDSTORE-vs-baseline, Sieve-upload-vs-export-only) depends on the real server's advertisements, unknown today. Probes-first is the unanimous cross-file demand.
**Delivers:** Scrubber + sandbox pre-flight as code (entry criteria), recorded `CAPABILITY` (IDLE/CONDSTORE/QRESYNC atoms) + port-4190/ManageSieve probe transcripts, per-account capability profile, `live-21/22/26.sh` green path, secret-grep-clean artifacts.
**Addresses:** Live-validation closure, compose-hardening unblock, IDLE/CONDSTORE/Sieve entry gates.
**Avoids:** Secret leakage (Pitfall 4), building uploaders against untestable endpoints (Pitfall 5), assuming capabilities (all tiers).

### Phase 2: Override-event log + CONDSTORE fast path (headless sync core)
**Rationale:** Telemetry before consumers (can't aggregate what isn't logged); cheap-pass before push-wakeup (IDLE multiplies pass cost, CONDSTORE divides it); both headless (`cargo test` only), no UI, no trigger changes.
**Delivers:** M20 `override_events` + 3-line hooks in confirm/override/dismiss/batch paths; `SyncSession` +2–4 condstore methods, worker step-2 branch, `highestmodseq` persist with UIDVALIDITY-first gating.
**Uses:** rusqlite_migration forward-only pattern; vendored `select_condstore`/MODSEQ parsing (no new deps).
**Avoids:** MODSEQ/epoch interplay (Pitfall 2 — epoch-clears-modseq test + transcript tripwire are exit criteria), learning-before-dataset inversion.

### Phase 3: Rule import/export + IDLE watcher
**Rationale:** The versioned rule envelope settles predicate expressiveness the Sieve generator will consume; IDLE lands on top of the now-cheap pass with the structural second-connection design. Both extend proven paths (import mirrors `import_taxonomy`; watcher wakes the same gate).
**Delivers:** `rules_io.rs` versioned JSON + validate-stage-commit + round-trip fixpoint test; `idle.rs` IdleWatcher + DONE/re-IDLE heartbeat + `idle_status` commands + honest Push/Polling/Offline status UI.
**Implements:** Shared-schema import/export; manager-owned dedicated-connection IDLE.
**Avoids:** Connection hijack (Pitfall 1 — mid-IDLE kill test: reconnect, no duplicate sync), import drift (Pitfall 7), IDLE-only-sync anti-feature (poll stays supervisor).

### Phase 4: Batch trust pair (dry-run + retry-failed)
**Rationale:** The per-item-status run record makes both features fall out; dry-run is the planner with writes off, retry is the record with writes re-on for failures. Needs the Phase 3 trigger funnel (all callers through SyncGate) plus row-level claims.
**Delivers:** `planner.rs` shared matcher (`&`-only purity), dry-run report UI with [Run for real], retry-failed filtered re-enqueue reusing the offline outbox, UID re-resolution at confirm.
**Addresses:** Batch dry-run, batch retry-failed, stale-preview and double-replay handling.
**Avoids:** Replay races (Pitfall 6 — burst-EXISTS + double-click-retry stress: each row executes once).

### Phase 5: Sieve export-text + history views + learning v1 + auto-confirm
**Rationale:** Generator reads the Phase 3 rule model; history banks the Phase 2 dataset into visible stats; learning and auto-confirm consume both as one coherent unit with governors — never split the feedback loop from its governor across phases.
**Delivers:** `sieve.rs` golden-file-tested export (copy/download, warning counts); override/batch history views; `learn.rs` keyword-weight + threshold adaptation (explicit signals only); per-category opt-in auto-confirm with precision display, pause-on-override, digest + one-click undo.
**Addresses:** Sieve export slice, override history, learning-from-overrides, per-category auto-confirm.
**Avoids:** Trust erosion + silence-as-approval (Pitfall 3 — override-flips-to-manual test, training-neutrality test), Sieve/local flapping (coexistence badges).

### Phase 6: Compose hardening (parallelizable anytime after Phase 1)
**Rationale:** Disjoint files (`drafts.rs`, `send_queue.rs`, reply path), verify + fix only over v1.2 pipelines — no ordering dependency on sync/classifier work beyond the Phase 1 sandbox pre-flight. Run alongside Phases 2–5 to keep the critical path short.
**Delivers:** Reply/reply-all threading + attribution, one-copy draft invariant, exactly-once Sent filing via Message-ID dedupe, offline-queue backoff, expanded-recipient allowlist (resolved To+Cc+Bcc) with foreign-Cc abort test, synthetic fixtures only.
**Avoids:** Sandbox violations + duplicate sends (Pitfall 8), secret-bearing fixtures in logs.

### Phase 7 (conditional): ManageSieve upload slice
**Rationale:** Only if the Phase 1 port-4190 probe succeeded. Separate slice, separate confirmation, never auto-sync.
**Delivers:** io-managesieve (`native-tls` only, `spawn_blocking`-hosted) PUTSCRIPT/CHECKSCRIPT/SETACTIVE behind backup-then-diff + explicit append/replace choice + `fileinto`-target validation against the folder tree. Probe failed → this phase does not exist; export-text-only stands as the shipped scope.
**Avoids:** Second-product trap (Pitfall 5 — no uploader code before recorded probe, no round-trip import, `fileinto`-only subset).

### Phase Ordering Rationale

- **Probes gate protocol work** (PITFALLS/STACK/FEATURES unanimous): no IDLE/CONDSTORE/upload code before recorded capability evidence; each tier degrades silently to its baseline, never hard-fails.
- **Telemetry → model → consumers** (ARCHITECTURE/FEATURES): event log (Ph.2) → rule envelope (Ph.3) → planner/history/learn (Ph.4–5); Sieve generator reads the portable model, never SQLite rows directly.
- **Cheap pass before push** (ARCHITECTURE, premise secured by #2): CONDSTORE (Ph.2) before IDLE wakeups (Ph.3) so push traffic runs the fast path; all triggers funnel through the one SyncGate engine.
- **Feedback loop ships with its governor** (PITFALLS/FEATURES): history + auto-confirm + learner are one phase (Ph.5); trust features never land without precision display, circuit breaker, and digest/undo.
- **Sieve last and splittable** (all three): export-text (Ph.5) unconditional, upload (Ph.7) probe-conditional; an unsupported server never blocks export.
- **Compose off the critical path** (ARCHITECTURE): disjoint files, verify-not-rebuild — parallelize after the sandbox gate.

### Research Flags

Phases likely needing deeper research during planning (`--research-phase`):
- **Phase 2 CONDSTORE slice:** confirm vendored wiring shapes (`select_condstore` return, `Fetch.modseq` plumbing, raw-string CHANGEDSINCE args) against pinned source at plan time; fake-server transcript tripwire design.
- **Phase 3 IDLE slice:** thread/runtime fit (dedicated `std::thread` vs blocking-pool slot for the long-lived wait loop); DONE/reconnect/backoff state machine detail.
- **Phase 5 auto-confirm slice:** eligibility threshold tuning (N-streak default ~10–20), digest cadence, retention/prune window for `override_events`.
- **Phase 7 upload slice (if probed positive):** io-managesieve 0.2.0 integration spike (blocking client vs I/O-free coroutine over existing transport); server script-name/activation semantics.

Phases with standard patterns (skip research-phase):
- **Phase 4 batch pair:** established planner-with-flag + run-record-filter shapes; Azure retry-pattern standard.
- **Phase 5 Sieve export-text:** RFC 5228 `fileinto` subset is textbook string templating with golden-file tests; no protocol surface.
- **Phase 6 compose:** v1.2 pipelines already proven; verification checklist is enumerated in FEATURES.md.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | IDLE/CONDSTORE APIs + MODSEQ parsing verified in vendored async-imap 0.11.3 / imap-proto 0.16.7 source; pins recorded. Only youth-risk item is io-managesieve 0.2.0 (production-extracted but 0.x) — quarantined behind the probe gate. |
| Features | MEDIUM | RFC-grounded behavior HIGH; Rust-crate support claims were web-only at research time (superseded by vendored reads where they conflicted); competitor UX from docs, not teardowns. |
| Architecture | HIGH | Direct repo read of `imap/`, `sync/`, `store/`, `classify/`, `commands/`, `lib.rs`, `Cargo.toml` plus vendored crate source; new/modified/untouched file list explicit; four prior-milestone invariants named. |
| Pitfalls | MEDIUM | Codebase-grounded claims HIGH (lease pattern, epoch hygiene, NAMESPACE scar, sandbox contract); RFC-grounded MEDIUM; crate-ecosystem claims LOW (websearch tier, none-surfaced-ManageSieve-crate claim already corrected by STACK's io-managesieve find). |

**Overall confidence:** MEDIUM-HIGH (buildable; remaining unknowns are probe-answered, not research-answered)

### Gaps to Address

- **Real-server capabilities unknown:** IDLE/CONDSTORE/QRESYNC atoms + port-4190/ManageSieve greeting for `mail.utfpr.edu.br` unprobed in this environment. Handle: Phase 1 probe transcripts are entry gates; every tier has a no-capability baseline that ships instead.
- **io-managesieve 0.2.0 integration fit:** young crate, blocking API vs single-session async runtime. Handle: plan-time spike (blocking-via-`spawn_blocking` vs I/O-free coroutine vs ~200-line hand-roll fallback); crate added only on probe success.
- **Threshold tuning without data:** auto-confirm N-streak, per-category thresholds, `override_events` retention window, IDLE debounce interval. Handle: ship conservative defaults (N≈10–20, telemetry retained months not forever), tune from observed override rates in v1.5.x.
- **QRESYNC cost unmeasured:** whether the `run_command` + manual VANISHED-parse follow-up is worth it given CONDSTORE-only covers Gmail-class servers. Handle: explicitly deferred; decide from real MODSEQ/reconnect behavior post-CONDSTORE.
- **Zimbra-filter-API possibility:** if UTFPR runs Zimbra without ManageSieve, upload has no endpoint. Handle: probe decides; export-text-only + documented deferral is the honest pre-agreed outcome.

## Sources

### Primary (HIGH confidence)
- Vendored `async-imap-0.11.3` source (`client.rs` select_condstore/idle/run_command, `extensions/idle.rs` Handle/IdleResponse/29-min guidance, `parse.rs` HIGHESTMODSEQ folding) + `imap-proto-0.16.7` (`types.rs` HighestModSeq/ModSeq/Vanished, `parser/rfc7162.rs`) — CONDSTORE read path + IDLE handle ground truth.
- SGE repo at research time (`imap/{mod,manager,session}.rs`, `sync/{mod,worker}.rs`, `store/{mod,schema.sql}`, `classify/*`, `commands/*`, `lib.rs`, `Cargo.toml`) + `.planning/PROJECT.md` v1.5 scoping + `.planning/STATE.md` — existing architecture, pins, sandbox contract, scope guards.
- RFC 2177 (IDLE/DONE, 29-min advisory), RFC 7162 (CONDSTORE/QRESYNC, MODSEQ/HIGHESTMODSEQ, epoch rules), RFC 5228 (Sieve `fileinto`/`header`/`address` base language).

### Secondary (MEDIUM confidence)
- docs.rs `io-managesieve 0.2.0` (pimalaya, MIT/Apache, native-tls feature, RFC 5804 coverage) — upload-leg candidate, needs integration spike.
- Thunderbird/Gmail/Apple Mail/Superhuman filter/push/bulk-UX docs; Azure retry-pattern guidance; active-learning literature (conceptual grounding for learning-v1).
- RFC 5804 (ManageSieve, referenced not fetched); go-imap/mailledger CONDSTORE notes; Thunderbird/neomutt CONDSTORE experience (Gmail CONDSTORE-yes/QRESYNC-no).

### Tertiary (LOW confidence)
- `daaki-imap` / `io-imap` README CONDSTORE/QRESYNC claims — fallback-crate candidates only; not needed given vendored verification.
- `async-imap` 0.12 (0.12/0.17 parser change) — explicitly rejected for v1.5; separate-migration material.
- Original PITFALLS "no ManageSieve crate surfaced" claim — corrected by STACK's io-managesieve find; retained here so planners don't re-search blindly.

---
*Research completed: 2026-10-13*
*Ready for roadmap: yes*
