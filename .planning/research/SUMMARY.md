# Project Research Summary

**Project:** SGE v1.3 Auto-Classify (Laya on-device email classification)
**Domain:** Linux IMAP desktop mail client (Rust + Tauri v2 + React + SQLite) + on-device AI classification lane
**Researched:** 2026-10-10
**Confidence:** HIGH-MEDIUM (stack HIGH, architecture HIGH/MEDIUM, pitfalls HIGH, features MEDIUM)

## Executive Summary

SGE v1.3 adds one new lane — automatic email classification and confirm-gated auto-filing — onto a proven IMAP desktop client. Experts build this shape as a **bundled offline sidecar over loopback HTTP**: Laya runs as a Tauri `externalBin` child process, the Rust backend is its only client, the React frontend never touches it, and every physical mail move reuses the already-proven Phase 10/11 MOVE + CREATE + offline-outbox machinery with zero new IMAP verbs. Classification runs **behind sync** (post-pass queue, never inline), suggestions are **confirm-before-move** for single mail and **report-plus-journal** for batch, and low-confidence mail lands in an explicit `A Classificar` review bucket instead of being misfiled.

The dimension researchers disagreed on sidecar shape (PyInstaller+torch vs lean ONNX/IPC+download) and HTTP contract (`/v1/systemone` vs `/predict`). This summary resolves both with a decision matrix below: **v1.3 ships the PyInstaller-frozen `laya[serve]` 0.4.2 sidecar speaking `POST /v1/systemone` on `127.0.0.1`, CPU-only torch, multilingual checkpoint only, weights as Tauri `resources`** — because it is the only path that simultaneously honors the user's bundled-offline decision, uses Laya's official verified serving surface, and satisfies Tauri's `externalBin` single-binary contract. The lean ONNX/IPC shape is deferred as a measured fallback, not a parallel option.

The key risks are bundle size (~1 GB installed — the honest price of offline ML), cold-start latency (seconds, must be backgrounded), flat-taxonomy accuracy cliffs, silent truncation of long threads, unconfirmed MOVE harm, batch mid-run corruption, and secret leakage into justifications/logs. Every one of these has a structural mitigation (not UI polish) assigned to a specific build phase — the roadmap below orders packaging risk first and moves last.

## Key Findings

### Recommended Stack

The Cargo delta is two lines; the config delta is `externalBin` + `resources` + one capability entry. Everything else is build-time tooling, never shipped. See `STACK.md` for the full closure and install commands.

**Core technologies:**
- `laya[serve] == 0.4.2` (Python, Apache-2.0): typed-decision inference + `laya-serve` HTTP server — the milestone's chosen classifier, zero-shot, no fine-tuning toolchain
- `laya-multilingual` checkpoint only (322M params, 644 MB weights, pinned Hub revision): pt-BR mail routes here by default; English checkpoint is confidently wrong on Portuguese — ship exactly one checkpoint with `LAYA_MAX_LOADED=1`
- `tauri-plugin-shell = "2"` (stable 2.4.0): the Tauri-blessed sidecar spawn/supervise mechanism from Rust backend
- `reqwest = "0.12"` (async, plain localhost HTTP): backend → sidecar client; fits the existing tokio runtime, no TLS features needed
- PyInstaller (≈6.x) + CPU-only torch wheel: freeze `laya[serve]` into one `sge-laya-<target-triple>` binary; CPU-only torch (~200 MB vs ~800 MB+ CUDA) is the single biggest size lever
- Tauri config: `bundle.externalBin`, `bundle.resources` (weights as resources, NOT inside the binary), `shell:allow-execute` with pinned argv, loopback-only env (`LAYA_HOST=127.0.0.1`, preload, per-boot API key)

### Expected Features

See `FEATURES.md`. Competitor pattern (Thunderbird filters, Gmail retroactive-apply, Superhuman auto-labels) converges on: classify on arrival, confirm gate, correction path, fallback bucket, retroactive batch — SGE's differentiator is doing all of it **fully offline with a pt-BR academic taxonomy**.

**Must have (table stakes):**
- Taxonomy store in SQLite + shipped UTFPR default hierarchy — nothing works without it
- Laya sidecar serving classifications offline — the milestone's identity
- Classify-on-sync + manual per-email classify, both with confirm-before-move dialog — the core loop
- `Auto/` tree auto-created on demand (Phase 11 CREATE reuse)
- Override UI (one-click correct + log) — trust repair
- `A Classificar` fallback bucket + review list — safety net
- Primary + secondary SQLite labels (secondary never moves, clearly badged local-only)
- Sensitive-data redaction across justifications/logs — non-negotiable privacy constraint
- Batch whole-account run with progress + persisted report (no per-email confirm) — the reorganization payoff
- Taxonomy editor UI (add/rename/delete with guards)

**Should have (competitive, v1.3.x if pressure):**
- Taxonomy JSON import/export with validation (cycles, duplicates, reserved `Auto` root) — differentiator, portable across machines
- Confidence threshold setting — ship a sensible default first, expose tuning after observing real distributions
- Override history view — log from day one, build UI once corrections accumulate
- Batch retry-failed + dry-run — after the happy path proves itself
- Justification line per classification (redacted at generation, transient UI string, never persisted)

**Defer (v2+ / explicit anti-features):**
- Learning/fine-tuning from overrides — record now, learn later
- Server-side (Sieve) rule export — second product
- Cross-device taxonomy sync — no sync story exists yet
- Cloud AI fallback ("use GPT when unsure") — violates offline constraint, never for mail content
- Auto-delete/auto-archive via classifier — worst case is `A Classificar`, never delete
- Unconfirmed single-email moves — always-confirmed is structural, not polish

### Architecture Approach

One new lane (`classify/` module + sidecar + M12 store tables) consuming existing machinery; no existing lane changes shape. The sidecar is a third transport (loopback HTTP) next to IMAP and SMTP. Sync never awaits classification; the classifier never touches IMAP directly — all moves flow through `imap_outbox`. See `ARCHITECTURE.md` for the diagram, M12 schema, and command list.

**Major components:**
1. Laya sidecar (`laya[serve]`, `127.0.0.1` loopback HTTP) — typed `choice`/`noul` answers, zero text generation, preloaded multilingual checkpoint
2. `classify/` Rust module (`taxonomy.rs` + `bridge.rs` + `worker.rs` + `suggest.rs`) — taxonomy load/validate, single-flight sidecar client, queue drain under `ClassifyGate`, answer shaping with confidence gate
3. Store M12 (`taxonomy` + `labels` + `label_overrides` + `classify_queue` + `batch_runs`) — versioned taxonomy, pointer-only labels (never email content), durable queue with UIDVALIDITY epoch hygiene
4. Post-sync hook + confirm path (`sync/worker.rs` 3–5 line enqueue, `ensure_auto_tree` + Phase 10 `move_message_in`) — zero new SyncSession verbs
5. React surfaces (`ClassifyChip` + `TaxonomyEditor` + `BatchProgress`) — all through Tauri commands + `Classification*` channel events; sidecar port never leaves Rust

### Critical Pitfalls

Top risks from `PITFALLS.md` (7 critical; condensed to the 5 that shape the roadmap):

1. **Sidecar packaging explodes bundle size / breaks first-run** — wrong triple suffix, PyInstaller missing data files, orphan processes, warmup races, port conflicts. Avoid: packaging spike FIRST proving size, cold-start, lifecycle, and `tauri build` artifact before any UX.
2. **Flat taxonomy blows the choice-option budget** — accuracy cliff past ~20 options + temperature-0.10 overconfidence bug for `choice:11+`. Avoid: hierarchical routing (top-level ≤8, child keyword-resolved), every `choice` <11 options, confidence threshold + abstain.
3. **Auto-MOVE files mail wrong with no way back** — zero-shot pt-BR accuracy is ~0.6–0.7, not silent-filing grade. Avoid: backend-enforced confirm gate (IPC-level, not just UI), secondary label never moves, override as first-class path, `A Classificar` for unsure mail.
4. **Batch corrupts state mid-run** — UIDVALIDITY shifts, throttling, partial moves. Avoid: resumable job (snapshot + chunk 25–50 + per-chunk UIDVALIDITY re-check + per-message move journal + undo-batch), full `Auto/` tree created up front.
5. **Sensitive data leaks into justifications/logs/labels** — justifications echo secrets; default logging persists bodies. Avoid: defense in depth — redact-before-inference, output regex filter, log IDs/scores only, store label IDs never justification text, backend-enforced.

Two more load-bearing: silent 1024-token truncation (evidence-selection pipeline: subject + sender + quote-stripped head + signals, never full bodies) and taxonomy edits invalidating labels (stable UUID category IDs + versioned taxonomy + rename/merge/delete migration; overrides pinned).

## Sidecar Decision Matrix (resolves researcher disagreement)

Researchers diverged on two axes. Decision below is FINAL for v1.3; the alternative is a deferred fallback with an explicit trigger, not a build-time option.

### Axis 1: HTTP contract — `/v1/systemone` vs `/predict`

| Criterion | `POST /v1/systemone` (+ `/batch`) — `laya-serve` | `POST /predict` — `examples/server.py` |
|---|---|---|
| Official serving surface? | **Yes** — the `serve` extra's Jev-compatible endpoint, entry point `laya-serve` | No — dev playground (serves a GUI + `/predict`), not the integration surface |
| Batch route? | **Yes** (`/v1/systemone/batch`, capped 64 states, internal token-split) | No |
| Router parity (routing, `lang_guess`, `max_len`, `min_confidence`)? | **Yes** | Partial |
| Source confidence | **HIGH** (live `pyproject.toml` 0.4.2 + README read 2026-10-10) | LOW-MEDIUM (older `laya 0.3.20` install + integration-skill doc) |
| Tauri fit | Same localhost POST either way | Same localhost POST either way |

**Decision: `POST /v1/systemone` (+ `/batch` for whole-account).** The `/predict` reference in ARCHITECTURE.md describes the dev example server, which STACK.md §4 explicitly rejects ("Do NOT add `examples/server.py` as the sidecar"). Planners: update the `bridge.rs` sketch to `/v1/systemone`; keep the single-flight mutex, 10 s timeout, and health-probe shape unchanged.

### Axis 2: Sidecar shape — PyInstaller+torch (bundled) vs lean ONNX/IPC+download

| Criterion | **A: PyInstaller + CPU torch, models as `resources` (v1.3 pick)** | B: Lean ONNX / stdin-IPC + first-run model download |
|---|---|---|
| Honors user's bundled-offline decision (PROJECT.md)? | **Yes** — installs and classifies with no network, day one | Partial — first run needs a ~650 MB download with progress/resume/checksum; fails on metered/offline installs |
| Tauri `externalBin` reality | **Native fit** — single self-contained executable + triple suffix, the documented contract | Same binary contract, but adds a download manager + checksum + resume + offline-degraded UX to build |
| Official / verified path | **Yes** — `laya[serve]` torch path is the numerics-verified reference (`verify/numerics_check.py`) | No — requires reimplementing Router/tokenizer/head-wiring in Rust (`ort`) or living inside the less-mature `laya[onnx]` surface; parity surface is one test file |
| Bundle size | ~1 GB installed (sidecar ~250–400 MB + weights ~650 MB) — heavy but honest | Lighter binary (onnxruntime ~50–100 MB) + deferred weights — lighter installer, heavier first-run |
| Latency | ~200 ms CPU single-mail in the short-input band (subject + snippet) | Comparable-or-better per-pass, but cold paths (download, export parity bugs) dominate v1.3 risk |
| Failure modes | Known Tauri sidecar modes (triple suffix, warmup, orphans, port) — all mitigated by the packaging spike | All of A's process modes PLUS download corruption, checksum drift, Hub re-upload changing weights, ONNX export parity gaps |
| Work to build | One freeze script + `resources` wiring + supervision | Freeze script + download UX + checksum pinning + IPC framing + Router reimplementation/verification |

**Decision: A for v1.3.** Rationale in one line: B trades megabytes of installer for the milestone's two hard constraints (bundled-offline day one; verified classification behavior) — that trade is backwards for this milestone.

**Fallback trigger (B becomes relevant):** if the packaging spike measures installed size or cold-start beyond what the Linux-packaging phase can ship (e.g. `.deb` hosting limits, target-machine RAM < 322M-fp32 residency ~1.3 GB), THEN authorize a v1.3-stretch spike migrating the sidecar interior from torch to `laya[onnx]` **inside the same `externalBin` contract** (same binary name, same `/v1/systemone` surface, weights still as `resources`). Never "solve" size with hosted inference — that trades megabytes for the privacy constraint. Never run A and B in parallel during v1.3.

**Naming note:** STACK uses `sge-laya-<triple>`; ARCH/PITFALLS use `laya-sidecar-<triple>`. Either satisfies Tauri; roadmapper picks one and pins it in CI (assert the triple-suffixed binary exists in the built bundle).

## Implications for Roadmap

Single ordered build list resolving the four research roadmaps (ARCH §Suggested build order is the backbone; FEATURES dependencies set the floor; PITFALLS gates set the phase exits; STACK constrains the packaging choice). Packaging risk first, store before worker, suggestions before moves, single confirmed moves before batch, editor after the engine it edits.

### Phase 1: Sidecar packaging spike

**Rationale:** The only true unknown — everything else is proven-pattern reuse. PITFALLS P3 mandates proving size, cold-start, and lifecycle before any classification UX exists.
**Delivers:** Hello-world `laya[serve]` frozen via PyInstaller, `externalBin` + triple suffix, spawn in `setup()` + kill-on-exit, `/health` green in dev AND in a `tauri build` Linux bundle, cold-start seconds measured, weights-placement decided (`resources` default), `LAYA_MAX_LOADED=1` + preload verified.
**Addresses:** Offline sidecar feature (P1); STACK `tauri-plugin-shell` + `reqwest` pins.
**Avoids:** P3 (bloated/broken bundle); kills the ONNX-vs-torch debate with numbers.
**Uses:** `laya[serve]==0.4.2`, PyInstaller, CPU torch, multilingual-only checkpoint (pinned revision + sha256).
**Exit gate:** "Looks Done" packaging row — sidecar present in `.deb`/AppImage, cold-start timed, no orphans after quit.

### Phase 2: Taxonomy + M12 store

**Rationale:** FEATURES dependency — the SQLite taxonomy table is the foundation classify, editor, import, and batch all stand on. Headless, `cargo test`-only.
**Delivers:** Shipped UTFPR pt-BR default taxonomy JSON (5 top + children + fallback + rules, stable UUID IDs, versioned), `taxonomy.rs` validation, M12 migration (`taxonomy`, `labels`, `label_overrides`, `classify_queue`, `batch_runs`) + queries + preserve-rows test.
**Addresses:** Taxonomy store + UTFPR default (P1); primary/secondary label columns.
**Avoids:** P7 (taxonomy-edit invalidation — IDs + versioning from day one, never names-as-keys); anti-pattern of logging content (pointer-only labels schema).
**Research flag:** Standard patterns (migration M2–M11 template, single-SQL-module invariant) — skip research-phase.

### Phase 3: Bridge + worker + suggestions (no moves)

**Rationale:** Trust gate — user sees labels work before anything is allowed to move. Single-pass 3-question shape keeps per-mail cost at one forward pass so classify-on-sync is viable.
**Delivers:** `bridge.rs` (`/v1/systemone` single-flight client + health probe + ephemeral-port + per-boot API key), `worker.rs` (queue drain under `ClassifyGate`, fire-and-forget post-sync hook), `suggest.rs` (confidence gate → `A Classificar`, runner-up → secondary, child keyword-resolved), `labels` writes, `ClassificationReady` events, `classify_message` + `classify_status` commands, evidence-selection pipeline (quote-strip → signal-extract → budget-fit; subject ≤200, snippet ≤1000), redaction-before-inference + output filter + log hygiene.
**Delivers (verified):** Sync a folder → sane pt-BR labels appear; sidecar down → `pending`, sync unaffected; password-reset mail → no secret in DB/logs/UI.
**Addresses:** Classify-on-sync + manual classify (minus confirm), confidence display, secondary label, justification (transient, redacted), sensitive-data rule.
**Avoids:** P1 (hierarchical questions, every `choice` <11 options, threshold + abstain tuned on pt-BR sample), P2 (truncation unit tests + length-stratified accuracy), P6 (privacy pipeline test), anti-patterns 1/3/4 (no inline calls, no content storage, no frontend-to-sidecar).
**Research flag:** Needs `/gsd-plan-phase --research-phase` — Laya question-shaping (hierarchical `choice` + `noul` sensitive + optional coarse `score`) and evidence-selection budgets must be validated against pinned 0.4.2 docs at plan time, since ARCH sketched the older `/predict` shape.

### Phase 4: Confirm / override → MOVE (the Phase 10/11 payoff)

**Rationale:** Smallest IMAP blast radius — `ensure_auto_tree` + existing `move_message_in` + `imap_outbox`; zero new verbs. Single confirmed moves must prove themselves before batch multiplies them.
**Delivers:** `confirm_suggestion` / `override_label` commands, suggestion chip UI (destination + confidence + redacted justification + Move/Change/Dismiss), `A Classificar` review list, override log writes, queue-depth badges, `classifying` list states.
**Delivers (verified):** Confirm moves to `Auto/<Top>/<Child>`; offline confirm replays; override MOVE-backs + pins + survives next sync; `A Classificar` never moves; direct-IPC confirm-bypass refused (backend enforcement); secondary label produces zero IMAP traffic.
**Addresses:** Confirm dialog, override + log, fallback bucket, primary/secondary model.
**Avoids:** P4 (confirm-gate + override + move-report as success criteria), UX pitfalls (silent filing, fatigue-by-design-avoidance, "what did it do" surface).
**Research flag:** Standard patterns (Phase 10 optimistic+durable template, Phase 11 CREATE guards) — skip research-phase.

### Phase 5: Taxonomy editor + JSON import

**Rationale:** UI after the engine it edits; taxonomy edits are folder-rename + label-remapping problems that need the Phase 4 move path to exist first.
**Delivers:** `TaxonomyEditor` UI + `import_taxonomy` (validate → version++ → stale-flag old labels), rename/merge/delete migration (RENAME folder + relabel / MOVE + remap / orphan + prompt), `Auto`-root reservation + import sanitization (charset, depth, delimiter/encoding rules).
**Addresses:** Taxonomy editor (P1); JSON import/export (P2 — import now, export when users ask).
**Avoids:** P7 verification (rename/merge/delete migration tests + override survival); security (malicious taxonomy sanitization).
**Research flag:** Standard patterns — skip research-phase.

### Phase 6: Batch whole-account classify

**Rationale:** Batch is a loop over the proven confirm path (dialog off, progress on) — building it before Phase 4 is how mail ends up in the wrong tree. Explicit + unconfirmed-by-design with report + undo, never 3,000 dialogs.
**Delivers:** `batch_classify` + `batch_runs` (bulk enqueue, auto-confirm ≥ threshold, chunked 25–50 with per-chunk UIDVALIDITY re-check), full `Auto/` tree pre-created up front, per-message move journal (enables resume + undo-batch), `BatchProgress` UI + persisted per-category report + review list + retry-failed.
**Delivers (verified):** Kill-mid-batch → journal resume without dupes/skips; UIDVALIDITY-change → abort-to-resync; undo-batch restores originals; live large-folder gate against the real throttling server.
**Addresses:** Batch run + report (P1); retry/dry-run (P2 stretch).
**Avoids:** P5 (journal + resume + undo as success criteria, not stretch); confirmation-fatigue UX.
**Research flag:** Needs `/gsd-plan-phase --research-phase` — chunk sizing, journal schema vs `imap_outbox` interplay, and live-server throttling behavior need verification against the real UTFPR server at plan time.

### Phase Ordering Rationale

- **Dependencies:** Taxonomy store (Phase 2) precedes everything that reads it; suggestions (Phase 3) precede moves (Phase 4); confirmed moves precede batch (Phase 6); engine precedes editor (Phase 5). Batch and editor are order-independent of each other — sequence editor first only because its blast radius is smaller.
- **Risk-front-loading:** Packaging (the sole unknown) goes first so a size/latency surprise redirects the milestone before UX code exists; privacy (P6) lands inside Phase 3 before any justification is displayed or logged.
- **Pitfall gating:** Each phase exits through its PITFALLS "Looks Done" row (packaging → accuracy/truncation → confirm-gate → taxonomy migration → batch resume/undo), so verification compounds instead of arriving at the end.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 3 (bridge + worker):** Laya 0.4.2 question-shaping + evidence budgets must be re-verified (ARCH sketched the outdated `/predict` shape); threshold default needs a pt-BR accuracy gate design.
- **Phase 6 (batch):** Chunk/journal/UIDVALIDITY interplay + live UTFPR throttling behavior need real-server verification.

Phases with standard patterns (skip research-phase):
- **Phase 1:** Tauri `externalBin` + PyInstaller freeze is doc-driven (STACK §2 install block); measure, don't research.
- **Phase 2:** Migration + queries follow the M2–M11 template mechanically.
- **Phase 4:** Phase 10/11 verb reuse is the project's own proven pattern.
- **Phase 5:** Editor-over-versioned-store is standard CRUD + migration.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Live `pyproject.toml` 0.4.2 + PyPI + HF file tree + Tauri v2 docs + crates.io, all read 2026-10-10; only delta is two Cargo lines |
| Features | MEDIUM | Competitor UX from current web sources; Laya-specific UX inferred from SGE constraints; milestone scoping already pinned in PROJECT.md |
| Architecture | HIGH-MEDIUM | HIGH on SGE side (direct repo read of manager/sync/store/commands); MEDIUM on Laya contract (integration-skill docs + installed 0.3.20, checkpoint flags to re-verify at build) |
| Pitfalls | HIGH | Laya limits from official docs + community benchmarks; IMAP/Tauri modes from official docs + project history; every pitfall has a phase-mapped verification |

**Overall confidence:** HIGH-MEDIUM — the integration shape is settled and the build order is dependency-forced; remaining uncertainty is measured numbers (cold-start seconds, CPU ms/mail, pt-BR accuracy, installed GB), all owned by explicit phase exit gates.

### Gaps to Address

- **Measured numbers unknown until Phase 1/3 spikes:** cold-start latency, per-mail CPU latency, installed bundle size, RAM residency (~1.3 GB est.). Handle: packaging spike measures first; planning sets UX timeouts from numbers, not guesses.
- **Confidence threshold default (e.g. 0.6):** no universal value; must be tuned on representative pt-BR UTFPR-like mail. Handle: Phase 3 accuracy gate stratified by hierarchy level + email-length bucket.
- **`Auto`-root collision (user already owns `Auto`):** confirm-then-nest dialog decided at Phase 4 planning, not now.
- **Snippet fidelity ceiling:** labels derive from cached headers + preview (~200 chars), no forced body FETCH. Handle: if Phase 3 evaluation disappoints, escalate to fetch-`body_text`-for-`pending`-only (worker change, not architecture change).
- **Checkpoint revision pin:** pin the Hub snapshot hash + sha256 in the sidecar build script at Phase 1 build time so a Hub re-upload can't silently change weights.
- **FTS over labels:** optional Phase 6+ polish (`messages_fts` join) — decide after the engine works.

## Sources

### Primary (HIGH confidence)
- Laya `pyproject.toml` @ main v0.4.2 (live fetch) — deps, extras, `laya-serve` entry point — STACK.md §1
- Laya README @ main (live fetch) — Router, checkpoint table, serve env vars, `laya-ts` packaging notes — STACK.md §1
- HuggingFace `convaiinnovations/laya-multilingual` file tree — 678 MB repo / 644 MB weights — STACK.md §1
- Tauri v2 docs (`develop/sidecar`, `develop/resources`, `reference/config`) — `externalBin` + triple suffix + capabilities + resources — STACK.md + ARCHITECTURE.md
- crates.io `tauri-plugin-shell` 2.4.0 (requires `tauri ^2.12`) — STACK.md §1
- SGE repo source read (`imap/manager.rs`, `sync/{mod,worker}.rs`, `store/{mod,schema.sql,queries.rs}`, `commands/sync.rs`, `lib.rs`, `Cargo.toml`) + PROJECT.md v1.3 scoping — ARCHITECTURE.md
- Laya limits (option budget ~20, 1024-token multilingual context, `choice:11+` temperature clamp, `noul`/`score` weak spots) via official-docs synthesis — PITFALLS.md
- Tauri `externalBin` + IMAP RFC 4549 UIDVALIDITY + OWASP LLM02:2025 (output filtering required) — PITFALLS.md

### Secondary (MEDIUM confidence)
- Competitor UX (Thunderbird filters, Gmail retroactive-apply, Superhuman auto-labels, Shortwave splits, Dynamics 365 classification, ThinkAutomation review gates) — FEATURES.md
- Laya integration skill (`laya-integration` SKILL.md + `laya 0.3.20` install) — single-pass multi-question, serial forward-pass lock, latency figures — ARCHITECTURE.md (noted: re-verify flags at 0.4.2 build time)
- `docs.rs/laya`, `laya-candle`, `laya-rs` READMEs (Rust-native options assessed and rejected) — ARCHITECTURE.md
- Community benchmarks (Banking77, multilingual en 0.875 vs pt 0.625, CPU ~140 ms/mail) — PITFALLS.md

### Tertiary (LOW confidence, needs validation at build)
- Exact cold-start seconds (~25–35 s est.) and installed GB (~1 GB est.) on the user's machine — Phase 1 spike measures
- Optimal confidence threshold default — Phase 3 pt-BR gate tunes
- `LAYA_IDLE_UNLOAD_SECONDS` residency policy — Phase 3 planning decides (default: resident)

---
*Research completed: 2026-10-10*
*Ready for roadmap: yes*
