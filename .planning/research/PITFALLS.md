# Pitfalls Research

**Domain:** Adding on-device AI email classification + auto-filing (Laya) to an existing IMAP desktop client (SGE v1.3, Rust + Tauri v2 + React + SQLite, UTFPR pt-BR mail)
**Researched:** 2026-10-10
**Confidence:** HIGH (Laya limits from official docs + community benchmarks; IMAP/Tauri pitfalls from official docs + project history)

## Critical Pitfalls

### Pitfall 1: Flat taxonomy blows Laya's choice-option budget

**What goes wrong:**
A single `choice` question listing every leaf category (e.g. 30–77 flat labels) classifies unreliably: options share a fixed input budget, discrimination degrades, and probabilities look confident while being wrong.

**Why it happens:**
Laya's options share a fixed head/token budget; official docs advise care above ~20 options at default settings. The upstream model card explicitly warns about overconfidence and sensitivity to label budgets. A known calibration bug ships temperature 0.10 for `choice:11+` (clamped to 0.5 with a warning), which sharpens probabilities — answers with 11+ options look roughly twice as certain as the raw model is. The Banking77 (77-intent) benchmark exists precisely to demonstrate this limitation.

**How to avoid:**
Design hierarchical routing, not one flat question: stage 1 = top-level choice (≤8 coarse categories), stage 2 = child choice within the winner only. Keep every `choice` question under ~20 options, ideally <11 to dodge the miscalibrated slot. Prefer one `choice` with many options over N yes/no (`noul`) questions — 16 yes/no questions cost ~8× one forward pass, while a 40-option choice costs ~2× a 3-option one. Add a confidence threshold with escalate/abstain (Laya 0.3.21 opt-in abstention) so low-confidence mail stays in INBOX instead of misfiling.

**Warning signs:**
Accuracy on a held-out pt-BR set drops as categories are added; probability mass concentrates (>0.9) on wrong labels; the `choice:11+` temperature-clamp warning appears in sidecar logs and is ignored.

**Phase to address:**
Classifier-accuracy-gates phase (before any auto-move ships): hierarchical question design + accuracy gate on pt-BR sample + threshold tuning. Re-verify whenever the taxonomy gains options.

---

### Pitfall 2: Long emails silently truncated by the 1024-token state budget

**What goes wrong:**
Newsletters, threads, and forwarded chains get classified on their headers/first paragraphs only — the decisive content (a boleto link at the bottom, a meeting change mid-thread) is cut off, producing confident misclassifications.

**Why it happens:**
The multilingual checkpoint defaults to 1024 tokens total (~768 for the state/message); English is 512 (~320 state). The encoder supports up to 8192, but raising the limit increases latency and quality must be re-verified. Developers "drop the whole email in" and never notice truncation because Laya truncates silently.

**How to avoid:**
Build an evidence-selection step before inference: feed subject + sender + first ~N chars + extracted signals (keywords, quoted-reply stripping, attachment names) rather than raw body. Strip quoted history and signatures first — they both waste budget and confuse the classifier. If raising the context window, re-run the accuracy gate at the new setting and measure CPU latency on a low-end target machine. Never classify attachments' full content; use filename/MIME only.

**Warning signs:**
Short emails classify well, long threads don't; classification changes when you reorder (truncate head vs tail); latency spikes after bumping max-tokens without a benchmark.

**Phase to address:**
Sidecar/classifier phase: input-preparation pipeline (quote-strip → signal-extract → budget-fit) with unit tests asserting what gets truncated, plus the accuracy gate stratified by email length.

---

### Pitfall 3: Sidecar packaging explodes bundle size and breaks first-run

**What goes wrong:**
The Linux bundle balloons (Python runtime via PyInstaller + ~400M-param checkpoint ≈ 1 GB+), first launch hangs downloading/loading the model, orphan sidecar processes linger after quit, and wrong target-triple naming means Tauri can't find the binary at all.

**Why it happens:**
Common, well-documented Tauri sidecar failure modes: (a) binary must be named with the exact `-TARGET_TRIPLE` suffix (`laya-sidecar-x86_64-unknown-linux-gnu`) or bundling silently misses it; (b) PyInstaller `--onefile` without `collect_dynamic_libs`/`collect_data_files` builds fine but crashes loading the model; (c) the app must kill the child process on exit or it orphans; (d) model load takes seconds during which inference requests can't be served — requests during warmup fail; (e) port conflicts if the sidecar is an HTTP server.

**How to avoid:**
Prefer a small native sidecar (ONNX runtime via Laya's ONNX surface, or a tiny Rust/Python IPC bridge over stdin/stdout — not HTTP, dodges port conflicts) over a full Python+torch bundle. Ship the checkpoint as a separate first-run download with progress + checksum, not inside the installer. Implement sidecar supervision in Rust: health-check, warmup gate (queue classify requests until model-ready), kill-on-exit, crash-restart with backoff. Pin and CI-test the exact triple-suffixed binary name; smoke-test `tauri build` output contains the sidecar.

**Warning signs:**
`.deb`/AppImage size jumps 10×; cold-start classify calls time out; `ps` shows sidecar processes after app quit; "sidecar not found" only on the packaged build, never in dev.

**Phase to address:**
Sidecar-packaging phase (first v1.3 phase): packaging spike must prove bundle size, cold-start latency, and process lifecycle before any classification UX is built.

---

### Pitfall 4: Auto-MOVE files mail into the wrong folder with no way back

**What goes wrong:**
A misclassified payslip lands in `Auto/Newsletters`, the user can't find it, panics, and loses trust in the whole feature. Worse: a MOVE is a server-side mutation — the mail left INBOX on the server, so "just re-run" doesn't restore the original state.

**Why it happens:**
Zero-shot accuracy on real multilingual mail is ~0.6–0.7 per community benchmarks (e.g. 0.625 pt on a small multilingual test) — nowhere near "safe to file silently." Developers test on 20 clean samples, see 90%, and ship unconfirmed moves. They also conflate "label" with "move": a wrong label is one click to fix, a wrong MOVE is a lost-email scare.

**How to avoid:**
Enforce the milestone's trust gates structurally, not as UI polish: single-email flow ALWAYS user-confirmed before MOVE (suggest-then-confirm, never silent); secondary category is SQLite-local-only and never moves anything; batch mode is an explicit user-initiated op with per-message report. Build override as a first-class path: correct classification = MOVE back + record override (override log doubles as future accuracy data). Consider a "review queue" (pending suggestions list) as the default surface instead of immediate moves. Set the abstain threshold so uncertain mail simply stays put.

**Warning signs:**
No override/correction UI in the plan; accuracy measured on English samples only; confirmation treated as a "nice to have" that batch mode skips without a report; no way to list "what did the classifier move today."

**Phase to address:**
Move-safety/undo phase (with Phase 10 MOVE machinery reuse): confirm-gate + override + move-report are success criteria, not stretch goals. Accuracy gate must pass on pt-BR UTFPR-like mail before confirm-gate is ever relaxed.

---

### Pitfall 5: Batch reorganization corrupts state mid-run (UIDVALIDITY, throttling, partial moves)

**What goes wrong:**
A 3,000-message batch MOVE run half-completes: server throttles/disconnects, a folder's UIDVALIDITY changes mid-run, UIDs cached at scan time no longer address the right messages, and the user ends up with mail split across INBOX and `Auto/` with no record of what moved.

**Why it happens:**
IMAP UIDs are only valid within a (mailbox, UIDVALIDITY) epoch — any renumbering invalidates cached UIDs, and cross-mailbox MOVEs assign new UIDs in the destination (the SGE codebase already gates on UIDVALIDITY epochs for exactly this reason). Bulk ops also hit server rate limits/connection drops (university servers like UTFPR are particularly throttling-prone). Developers write the batch as a fire-and-forget loop with no checkpointing.

**How to avoid:**
Treat batch as a resumable job, not a loop: (1) snapshot (mailbox, UIDVALIDITY, UID list) per folder before starting; re-validate UIDVALIDITY before every chunk and abort-to-resync on change; (2) chunk MOVEs (e.g. 25–50/chunk) with progress persistence after each chunk so a crash resumes, not restarts; (3) address everything UID-only, re-resolve UIDs per chunk, never cache sequence numbers; (4) create the full `Auto/` tree up front (CREATE phase) so a mid-run failure never leaves a half-created taxonomy; (5) write a per-message move journal (SQLite: message-id, from-folder, to-folder, UIDVALIDITY epoch, status) enabling both resume and a full "undo batch" operation; (6) reuse the proven Phase 10 verb pattern (lease + reconnect-retry + drain) rather than a new ad-hoc IMAP path.

**Warning signs:**
Batch plan has no resume story; UIDs fetched once at start and used throughout; no UIDVALIDITY re-check inside the loop; no move journal; testing only against 50 local messages, never thousands over a real throttling connection.

**Phase to address:**
Batch-discipline phase: chunking + journal + resume + undo-batch are the phase's success criteria; live-gate against the real server with a large test folder before calling it done.

---

### Pitfall 6: Sensitive data leaks into justifications, logs, and stored labels

**What goes wrong:**
A classification justification quotes a password-reset code, a boleto linha digitável, or salary figures; that string lands in SQLite, app logs, and the batch report — plaintext PII/sensitive-data persistence that violates the milestone's own sensitive-data rule and turns every log file into a credential leak.

**Why it happens:**
Laya justifications/scores echo input evidence; developers log full inference inputs/outputs for debugging and never remove it. Prompt/context assembly pulls sender+subject+body into one string that gets logged by default (a 2024 study found 8.5% of production LLM prompts contain sensitive data, mostly via default logging). The sensitive-data rule ("never reproduce senhas/códigos/dados sigilosos") is treated as a prompt instruction rather than an enforced output filter — and system-prompt restrictions alone are documented as unreliable mitigation (OWASP LLM02:2025).

**How to avoid:**
Defense in depth, enforced in code: (1) redact before inference — mask passwords, codes, numbers (CPF, boleto lines, tokens) with typed placeholders in the classifier input; classification doesn't need them; (2) output filter — regex/blocklist scan on every justification before display/storage, refusing to persist on match; (3) never log full email content or raw inference payloads — log message-IDs, category IDs, and scores only; (4) store labels (primary/secondary IDs), never justification text, in SQLite; justifications are transient UI strings; (5) backend-side enforcement, not UI-only (same principle as the destructive-folder guards: direct IPC invoke must also refuse).

**Warning signs:**
Logs contain email bodies; justification strings persisted in SQLite schema; no redaction step in the inference pipeline; "we'll tell the model not to quote secrets" as the only mitigation.

**Phase to address:**
Privacy phase (cross-cutting, but must land before any justification is displayed or logged): redaction + output filter + log hygiene with a test that feeds a password-reset email through the full pipeline and asserts no secret survives in DB/logs.

---

### Pitfall 7: Taxonomy edits silently invalidate stored labels and filed mail

**What goes wrong:**
User renames/merges a category; hundreds of messages carry the old label ID, filed mail sits in a renamed folder, and the next sync either orphans them or reclassifies everything from scratch — destroying manual overrides.

**Why it happens:**
Labels stored as category *names* (not stable IDs), no taxonomy versioning, no migration path. The `Auto/` tree mirrors the taxonomy, so a taxonomy edit is simultaneously a folder-rename problem (IMAP RENAME semantics, Phase 11 machinery) and a label-remapping problem (SQLite).

**How to avoid:**
Stable category IDs (UUIDs) independent of display names; taxonomy JSON versioned; stored labels reference IDs; on taxonomy import/edit, run an explicit migration: renamed → RENAME folder + relabel (ID unchanged); merged → MOVE mail to surviving folder + remap IDs; deleted → mail stays, label marked orphaned, user prompted. Overrides must survive reclassification (override = pinned label, never auto-overwritten). Reserve the `Auto` root name so user folders can never collide with classifier output.

**Warning signs:**
Schema stores category name strings; taxonomy import overwrites without diff; no migration step in the edit flow; overrides stored in the same column as auto-labels with no pin flag.

**Phase to address:**
Taxonomy/UI phase (import + edit): ID-stable schema + migration logic are part of the phase, tested with rename/merge/delete scenarios before batch mode exists to multiply the damage.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Flat single-question taxonomy ("ship now, hierarchize later") | Faster first demo | Accuracy cliff past ~20 options + miscalibrated confidence; rewrite of question layer | Never for ship; OK for a packaging spike with synthetic labels |
| Log full inference payloads for debugging | Easy debugging this week | PII in logs forever; violates offline-privacy promise | Never — log IDs/scores only from day one |
| Batch as fire-and-forget loop, no journal | Less code | Unresumable, un-undoable half-runs on real accounts | Never |
| Store category names instead of IDs | Simpler schema | Every rename breaks labels; migration rewrite | Never — IDs cost one column |
| Skip pt-BR accuracy gate ("multilingual checkpoint handles it") | Skip eval work | Community benchmarks show non-English accuracy far below English (0.625 vs 0.875); silent misfiling on the actual user base | Never — gate on pt-BR UTFPR-like mail |
| No abstain threshold (classify everything) | Higher "coverage" metric | Low-confidence mail misfiled; trust collapse | Only in suggest-only UI where every suggestion is confirmed |
| Fine-tune the checkpoint on user mail in v1.3 | Tempting accuracy boost | Breaks offline/simplicity scope; training-data PII retention; explicitly out of scope | Never in v1.3 (out of scope: zero-shot + keyword-assisted only) |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Tauri sidecar (`externalBin`) | Binary without exact `-TARGET_TRIPLE` suffix; works in dev, missing in bundle | Name `laya-sidecar-x86_64-unknown-linux-gnu`; CI-assert bundled artifact contains it |
| Sidecar IPC | HTTP server on fixed port; conflicts + firewall prompts | stdin/stdout JSON-lines or Tauri shell Spaß; no fixed ports |
| Sidecar lifecycle | Fire-and-forget spawn; orphans + warmup races | Supervise: health-check, warmup gate, kill-on-exit, crash-restart w/ backoff |
| IMAP MOVE reuse | New ad-hoc MOVE path for classifier | Reuse Phase 10 verb pattern (lease + reconnect-retry + drain), UID-only, UIDVALIDITY-gated |
| `Auto/` tree creation | CREATE folders lazily mid-batch; half-created tree on failure | CREATE full tree up front; reserve `Auto` root; leaf-only modified-UTF-7 encoding (existing decision) |
| Laya `noul` questions | Binary yes/no per category; option labels dominate input, outputs look "stuck" | Prefer `choice`; if binary needed, use 2-option `choice`, measured — never assume |
| Laya `score` questions | Fine-grained ordinal levels (1–5 priority) expected to work | Scores are a known weak spot; use coarse buckets or choice instead, and measure |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Classify full bodies one-by-one on CPU | Sync + classify takes minutes; UI stalls; fan spins | Evidence-selection (budget-fit input) + batch questions in one forward pass + background queue off the sync path | Mailboxes >500 messages; long threads |
| N yes/no questions per email | 8× latency vs one choice; batch mode crawls | One hierarchical `choice` per level; batch multiple emails per inference call where the binding supports it | Any batch run; large taxonomies |
| Raising context to 8192 without measuring | Slower inference, unverified quality | Benchmark latency + accuracy at each context setting on target hardware | Low-end Linux desktops (the actual SGE user base) |
| Blocking sync on classification | INBOX list waits for classifier; fast-first-paint regresses | Classify async after headers-first sync; show mail immediately, labels arrive progressively | First sync of a large UTFPR mailbox |
| Checkpoint download inside installer | 1 GB+ installer; failed installs on slow links | Installer stays lean; first-run model download with progress + resume + checksum | Any metered/slow university network |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Justification echoes secrets (passwords, codes, boleto lines) | Credential/PII persistence in DB, logs, reports | Input redaction + output filter + store label IDs only (see Pitfall 6) |
| Full email content in app logs/traces | Every log file becomes a mailbox copy; offline-privacy promise broken | Log message-IDs + category IDs + scores only; audit logs for body text |
| Sidecar runs unsandboxed with app privileges | Malicious/buggy sidecar = full user privileges (documented Tauri behavior) | Bundle only self-built sidecar; no remote model/config fetch at runtime except pinned first-run download with checksum |
| Override/correction log stores email text | Correction dataset becomes PII store | Overrides store (message-id, old-id, new-id, timestamp) — never content |
| Taxonomy JSON import executes blindly | Malicious taxonomy file could inject folder names/paths | Validate + sanitize imported taxonomy (name charset, depth limit, `Auto`-root enforcement) before CREATE |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Silent auto-filing (no confirmation, no notice) | "Where did my email go?" panic; trust collapse | Confirm-gate single moves; batch shows progress + per-message report + undo |
| Confirmation fatigue (confirm every one of 3,000) | User blindly clicks OK; gate becomes theater | Single-flow confirms; batch is explicit + unconfirmed-by-design with report + undo instead of 3,000 dialogs |
| No "what did it do" surface | User can't audit or learn the system | "Recently classified" view: message, from→to folder, confidence, one-click undo |
| Correction requires manual folder digging | Fixing a mistake is harder than the feature saves | One-click override in the reading pane: re-move + pin + record, no folder navigation |
| Overconfident labels shown as fact | User trusts a 0.51 guess like a 0.99 one | Show confidence qualitatively (sure/unsure) or not at all; route unsure mail to review queue, not to folders |
| `Auto/` folders indistinguishable from user folders | User edits/deletes classifier folders; taxonomy and tree diverge | `Auto` root always present, visually distinct (icon/prefix); guard against user filing into `Auto/` manually |

## "Looks Done But Isn't" Checklist

- [ ] **Sidecar packaging:** Bundled installer tested — verify sidecar present in built `.deb`/AppImage, cold-start latency measured, no orphans after quit, triple-suffix exact
- [ ] **Classifier accuracy:** Gated on pt-BR mail sample — verify accuracy reported per hierarchy level and per email-length bucket, not one aggregate number
- [ ] **Truncation behavior:** Evidence-selection tested — verify a long thread with decisive content at the bottom classifies correctly; assert what gets cut
- [ ] **Confirm gate:** Single-email MOVE impossible without confirmation — verify via direct IPC invoke, not just UI click-path (backend enforcement)
- [ ] **Batch resume:** Kill mid-batch — verify resume completes without duplicates or skips (journal-driven), and undo-batch restores original folders
- [ ] **UIDVALIDITY safety:** Change UIDVALIDITY mid-batch (test double) — verify abort-to-resync, no cross-epoch UID reuse
- [ ] **Privacy:** Password-reset email through full pipeline — verify no secret in SQLite, logs, justifications, or batch report
- [ ] **Taxonomy edit:** Rename + merge + delete categories — verify filed mail, stored labels, and overrides all survive with correct remapping
- [ ] **Override path:** Correct a misclassification — verify MOVE-back + pin + no re-auto-move on next sync
- [ ] **Secondary label:** Classify with secondary category — verify zero IMAP traffic for the secondary label (SQLite-only)

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Flat-taxonomy accuracy cliff | MEDIUM | Re-split into hierarchical questions; re-run accuracy gate; reclassify (labels only, no moves) — no server state harmed if moves were gated |
| Silent truncation misfiles | LOW | Fix evidence-selection; reclassify affected length bucket; override moves back (journal/override log) |
| Bloated/broken sidecar bundle | MEDIUM | Split checkpoint to first-run download; fix triple naming; re-ship installer; existing installs self-heal via update |
| Wrong-folder misfiles (confirmed flow) | LOW | Per-message override (MOVE back + pin); trust preserved because user was in the loop |
| Wrong-folder misfiles (batch, no journal) | HIGH | Manual hunt via search; no systematic undo — this is the disaster case the journal exists to prevent |
| Half-completed batch | MEDIUM with journal / HIGH without | Resume from journal; UIDVALIDITY re-check; undo-batch for already-moved; without journal: manual reconciliation via Message-ID diff |
| Secret leaked into DB/logs | HIGH | Purge justification/log storage; rotate exposed credentials (user action); add redaction + filter; treat stored copies as compromised until wiped |
| Taxonomy edit orphaned labels | MEDIUM | Remap IDs via migration; orphaned labels surface in review queue for manual assignment; re-file affected folders |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| P1 flat-taxonomy budget | Classifier-accuracy-gates phase | pt-BR accuracy gate passes per level; every choice <20 options; threshold + abstain tuned |
| P2 silent truncation | Sidecar/classifier phase | Length-stratified accuracy; truncation unit tests; bottom-loaded thread test |
| P3 sidecar packaging | Sidecar-packaging phase (first) | Bundle contains sidecar; cold-start timed; no orphans; installer size acceptable |
| P4 unconfirmed MOVE harm | Move-safety/undo phase | IPC-level confirm enforcement test; override e2e; secondary-label-zero-IMAP test |
| P5 batch corruption | Batch-discipline phase | Kill-mid-batch resume test; UIDVALIDITY-change abort test; undo-batch e2e; live large-folder gate |
| P6 sensitive-data leakage | Privacy phase (before any display/log) | Password-reset pipeline test: no secret in DB/logs/UI/report |
| P7 taxonomy-edit invalidation | Taxonomy/UI phase | Rename/merge/delete migration tests; override survival test |
| Confirm-fatigue UX | UX/batch-report phase | Batch unconfirmed-by-design + report + undo; no per-message dialogs in batch |
| `noul`/`score` misuse | Classifier phase | Question-type choice documented + measured; no per-category yes/no in shipped design |

## Sources

- Laya official docs via community synthesis: choice option budget (~20), multilingual 1024-token context (~768 state), temperature miscalibration for `choice:11+`, `noul` label-domination failure mode, score weakness — Runware "Jev, Laya, and Decision Models Explained"; MarkTechPost "A Developer's Guide to Laya" (2026-10-06); DEV "Not Another LLM: I Tried Laya" (Banking77 test); Medium Tuhin Sharma "Laya System 1 AI" (multilingual benchmark: en 0.875 vs pt 0.625); Wilson Wu "Jev vs Laya" (context/option decision tree); Zima Store Laya guide (hierarchical routing pattern); HF blog Laya model-selection guide (overconfidence + label-budget limits)
- Tauri sidecar: official docs "Embedding External Binaries" (`externalBin` + target-triple suffix + kill-on-exit ownership); techXcelerate sidecar guide (unsandboxed privileges); AI Echoes "Building Production-Ready Desktop LLM Apps" (PyInstaller spec, port conflicts, model-load failures); MClare blog pandas sidecar (Python-specific Tauri pitfalls); Tauri discussion #15339 (multi-GB Python sidecars in production `.dmg`)
- IMAP: Jevid "UIDVALIDITY & IMAP MOVE Integrity"; Aurinko "IMAP integration challenges" (UID instability); Nylas "Troubleshoot UIDVALIDITY errors"; RFC 4549 (disconnected-client sync); SGE project history (UIDVALIDITY epoch gating, Phase 10 verb pattern, leaf-only modified-UTF-7)
- Auto-categorization UX: Fyxer (in-inbox vs separate-app friction); Gmail tabs ML classification signals + user-correction training; Outlook Focused Inbox/rules auto-categorization complaints (misclassification discovery); Dynamics 365 email-classification (human-overridable AI categories + feedback loop)
- PII/privacy: Gravitee (gateway PII redaction patterns); Kong PII sanitization (typed-token vault); Tianpan "PII in LLM Pipelines" (8.5% prompts contain sensitive data; default-logging trap); OWASP GenAI LLM02:2025 (system-prompt restrictions unreliable; output filtering required); Openlayer (blocking gate vs observation-only logging); PMC review (remove/mask prompt-privacy techniques)

---
*Pitfalls research for: SGE v1.3 Auto-Classify (Laya on-device classification + auto-filing on IMAP desktop client)*
*Researched: 2026-10-10*
