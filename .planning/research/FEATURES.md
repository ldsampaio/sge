# Feature Research: Automatic Email Classification (SGE v1.3 Auto-Classify)

**Domain:** Desktop mail client — on-device AI email classification + taxonomy-driven folder organization
**Researched:** 2026-10-10
**Confidence:** MEDIUM (competitor UX from current web sources; Laya-specific UX inferred from SGE constraints)

## Feature Landscape

### Table Stakes (Users Expect These)

Features users assume exist once any client promises "automatic organization". Missing these = the feature feels broken or untrustworthy.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Classify-on-arrival (auto-classify during sync) | Every reference client does this: Thunderbird filters fire on Inbox delivery, Gmail filters run on new mail, Superhuman auto-labels every incoming message. Users expect new mail to already be sorted when they open the client. | MEDIUM | SGE: hook into post-sync pipeline (after headers land in SQLite). Classify headers + on-demand body snippet. Must not block first paint — classify async after list renders. Depends on existing poll/manual-refresh sync path (Phase 8). |
| Per-email confirm-before-move dialog | AI misfiles; every AI-mail product keeps a human-review gate (ThinkAutomation sends an explicit validation request and waits; Inbox Zero / Missive-style agents "surface results for review"). Moving mail physically (IMAP MOVE) is destructive-adjacent — user must see destination + reason and approve. | LOW | SGE: modal/banner in reading pane: suggested category, confidence, one-line justification, [Move] [Change] [Dismiss]. Reuses Phase 10 MOVE verb + outbox queue. |
| Override / correct a classification | Superhuman lets users define their own auto-labels; Gmail users edit filters; Shortwave splits are user-tunable. A classifier without a correction path trains distrust — users must fix a wrong filing in one click. | LOW | SGE: "wrong folder?" action → re-move + record override (SQLite table: message UID, old/new category, timestamp). Override log is the future training signal (v1.3 only records, no fine-tuning — out of scope per PROJECT.md). |
| Fallback / unclassified bucket | Gmail has Primary catch-all; every rule engine has a no-match path (Thunderbird leaves unmatched mail in Inbox). Low-confidence classifications must land somewhere reviewable, never silently misplaced. | LOW | SGE: `A Classificar` folder/bucket for low-confidence or ambiguous mail. Requires explicit review UI (list + classify actions), else it becomes a graveyard. |
| Manual per-email "classify now" | Thunderbird "Run Filters on Folder/Message", Gmail "Filter messages like these". Users expect to trigger classification on demand for a selected message, not only on arrival. | LOW | SGE: context-menu / toolbar button → same confirm dialog as auto path. Shares one classify code path with sync-time flow. |
| Batch "organize existing mail" | Gmail's "Also apply filter to N matching conversations" checkbox is the canonical pattern — users creating any organization scheme immediately want it applied retroactively. Thunderbird has Run-Now on folder. | MEDIUM | SGE: whole-account batch = same classifier, no per-email confirmation, with progress bar + final report (moved count, per-category counts, failures, low-confidence routed to `A Classificar`). Must reuse Phase 10/11 verbs (MOVE + CREATE) with the offline outbox so large runs survive disconnects. |
| Taxonomy / category editor UI | Gmail filter list (edit/delete), Thunderbird Message Filters dialog, Superhuman custom auto-label prompts, Shortwave custom splits. Users must see, add, rename, and delete categories — a hardcoded invisible taxonomy is unacceptable. | MEDIUM | SGE: settings screen listing categories (name, parent, keywords/rules), add/rename/delete with guards (cannot delete `Auto` root or non-empty without confirm — mirrors Phase 11 destructive-folder guards backend-side). Hierarchical pt-BR tree, UTFPR default shipped. |
| Confidence display + threshold | AI classifiers (Dynamics 365 Email Classification, Fyxer-style AI triage) surface "needs attention vs noise" — users calibrate trust via visible confidence. A bare label with no confidence hides the model's uncertainty. | LOW | SGE: store confidence per classification in SQLite; show badge (high/med/low) in confirm dialog and list; threshold setting routes below-threshold to `A Classificar`. |
| Classification survives restart / offline | Desktop mail is offline-first (SGE core value). Labels and folder assignments must persist in SQLite and reconcile with server on reconnect. | LOW | SGE: primary+secondary labels as SQLite columns; moves queued in existing outbox (Phase 10/13 machinery). No new infra — reuse. |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Fully offline on-device classification (Laya sidecar) | Superhuman/Shortwave/Gmail classify server-side or in-cloud — mail content leaves the machine. SGE classifies locally: privacy story nobody in the AI-mail space offers, strong fit for university (UTFPR) mail with sensitive content. | HIGH | Biggest v1.3 risk (packaging, model size, inference latency). But it IS the milestone's identity — do not compromise to a cloud API. |
| Secondary label that never moves | Gmail labels vs folders distinction, but taken further: primary = physical `Auto/` location, secondary = SQLite-only tag. Lets one email belong to two taxonomy nodes without IMAP double-filing complexity (COPY + dual-sync bookkeeping). | LOW | Cheap to build (one SQLite column + filter UI), genuinely useful for cross-cutting categories (e.g. primary `Auto/Financeiro`, secondary `Urgente`). Local-only must be clearly badged so users don't expect it on other clients. |
| Portuguese-first hierarchical taxonomy (UTFPR default, JSON import) | Competitors ship English-generic categories (Promotions/Social/Updates). A shipped pt-BR academic taxonomy (administrativo, acadêmico, eventos, financeiro…) + JSON import/export istailor-made for the actual user and portable across machines. | LOW–MEDIUM | JSON schema: name, parent, keywords, optional rules. Validate on import (cycle check, duplicate names, reserved `Auto` root). Export enables backup/sharing. |
| Justification line per classification | AI-mail agents increasingly explain ("why this label") — builds trust during the low-accuracy early period. Combined with the sensitive-data rule (never reproduce passwords/codes) it becomes a privacy-respecting explanation. | LOW | One short sentence stored alongside the label; shown in confirm dialog and message detail. Redaction must happen at generation time (prompt-level instruction + post-filter), not display time. |
| Batch report with per-category counts + failure list | Gmail's retroactive apply is fire-and-forget with no report. A real report (N moved, per-folder breakdown, K to `A Classificar`, failures with retry action) turns batch from scary to trustworthy — especially for whole-account reorganizations. | LOW | Report view persisted (SQLite run record) so users can audit after the fact. Retry-failed button reuses outbox. |
| Override log as visible history | Most clients silently accept corrections. Showing "you corrected X → Y (n times)" per category gives users a sense of control and provides the dataset for any future fine-tuning milestone. | LOW | Simple list UI over the overrides table. v1.3 records only; explicitly no learning yet. |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Auto-move without any confirmation on single emails | "Full automation" sounds efficient (some Outlook-agent case studies do this) | One wrong MOVE on important mail (boleto, matrícula) destroys trust; IMAP MOVE across folders is hard for users to audit after the fact | Keep single-email always-confirmed (already decided); unconfirmed only in explicit batch mode with report |
| Server-side rules / Sieve sync | "My rules should work in webmail too" | SGE has no Sieve support; syncing local taxonomy to server filters is a second product (protocol negotiation, capability detection). Scope explosion. | Local classification only via existing MOVE verbs (already decided, PROJECT.md out-of-scope) |
| Fine-tuning Laya on user mail in v1.3 | "It should learn from my corrections" | Training pipeline (dataset curation, eval, checkpoint versioning, rollback) dwarfs the milestone; override volume too small to matter initially | Zero-shot + keyword-assisted only; record overrides now, learn later (already decided) |
| Real-time classification of every flag/sync event | "Everything should always be instantly classified" | Reclassifying on each poll wastes inference cycles and can thrash folders (move → re-move loops) when taxonomy edits change mappings | Classify once per message (new arrivals + explicit triggers); taxonomy edits apply prospectively + optional re-run batch |
| Cloud AI fallback ("use GPT when unsure") | Higher accuracy temptation | Violates the offline constraint and leaks mail content off-machine; two code paths to maintain | Keep fully offline; low-confidence → `A Classificar` for human review |
| Auto-deleting / auto-archiving low-value mail | Superhuman auto-archives marketing; sounds tidy | Destructive-adjacent without user history; SGE has no trash-restore UX maturity for classifier-driven deletes yet | Never delete via classifier in v1.3; worst case is `A Classificar` or a low-priority `Auto/` branch |

## Feature Dependencies

```
Batch whole-account classification
    └──requires──> Per-email classify + confirm pipeline (same classifier, minus dialog)
                        └──requires──> Phase 10 MOVE verb + offline outbox
                        └──requires──> Phase 11 CREATE verb (Auto/ tree on demand)
    └──requires──> Taxonomy store (SQLite) + shipped UTFPR default
    └──requires──> A Classificar fallback bucket

Override / correction UI
    └──requires──> Per-email classify pipeline (reuses confirm dialog)
    └──enhances──> Override log table (future training data)

Secondary local-only label
    └──requires──> SQLite label columns (no IMAP dependency)
    └──conflicts──> Server-side folder expectations (must badge as local-only)

Taxonomy JSON import/export
    └──requires──> Taxonomy store + editor UI
    └──requires──> Validation (cycles, duplicates, reserved Auto root)

Sensitive-data redaction in justifications/logs
    └──requires──> Justification generation (prompt-level + post-filter)
    ──cross-cuts──> ALL classification UI and logs (confirm dialog, report, SQLite)
```

### Dependency Notes

- **Batch requires the single-email pipeline first:** build classify → confirm → MOVE once, then batch is "same loop, dialog off, progress on". Do not build two classifiers.
- **Everything classification touches needs Phase 10/11 verbs:** MOVE (Phase 10) and CREATE (Phase 11) are the only IMAP surface the classifier needs — no new protocol work, but those verbs' offline queue + UIDVALIDITY gating must be respected (large batches amplify edge cases: expunged UIDs mid-run, folder created mid-run).
- **Taxonomy store is the foundation:** the SQLite taxonomy table (categories, parents, keywords) must exist before classify-on-sync, editor UI, import, or batch. It is the natural Phase 1 of the milestone.
- **Redaction cross-cuts everything:** the sensitive-data rule is not a feature but a constraint on every surface that displays or logs classification output — design it once (shared sanitizer in Rust backend), apply everywhere.

## MVP Definition (v1.3 scope)

### Launch With (v1.3)

Minimum for "classifica e organiza com confirmação":

- [ ] Taxonomy store in SQLite + shipped UTFPR default taxonomy — nothing works without it
- [ ] Laya sidecar serving classifications to Rust backend (offline) — the milestone's identity
- [ ] Classify-on-sync + manual per-email classify, both with confirm-before-move dialog — the core loop
- [ ] `Auto/` tree auto-created on demand (Phase 11 CREATE reuse) — physical organization
- [ ] Override UI (correct + log) — trust repair, one click
- [ ] `A Classificar` fallback bucket + review list — safety net for low confidence
- [ ] Primary + secondary SQLite labels (secondary never moves) — local organization model
- [ ] Sensitive-data redaction across justifications/logs — privacy constraint, non-negotiable
- [ ] Batch whole-account run with progress + report (no per-email confirm) — the "reorganize everything" payoff

### Add After Validation (v1.3.x)

- [ ] Taxonomy JSON import/export — ship with default first; import when users ask to share/tweak at scale (editor UI covers basic tweaks)
- [ ] Confidence threshold setting — ship with sensible default; expose tuning after observing real confidence distributions
- [ ] Override history view — log data from day one, build the visible UI once corrections accumulate
- [ ] Batch retry-failed + scheduled/dry-run batch — after first real whole-account runs prove the happy path

### Future Consideration (v2+)

- [ ] Learning from overrides (fine-tune or keyword auto-suggest) — needs accumulated override dataset; explicitly out of scope now
- [ ] Server-side (Sieve) rule export — second product, revisit only if webmail parity demanded
- [ ] Cross-device taxonomy sync — no sync story exists in SGE at all yet

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Laya sidecar classification service | HIGH | HIGH | P1 |
| Taxonomy store + UTFPR default | HIGH | MEDIUM | P1 |
| Classify-on-sync + confirm dialog | HIGH | MEDIUM | P1 |
| Manual per-email classify | HIGH | LOW | P1 |
| Auto/ tree auto-creation | HIGH | LOW (reuse) | P1 |
| Override + log | HIGH | LOW | P1 |
| A Classificar bucket + review | HIGH | LOW | P1 |
| Primary/secondary SQLite labels | MEDIUM | LOW | P1 |
| Sensitive-data redaction | HIGH | LOW–MEDIUM | P1 |
| Batch run + progress + report | HIGH | MEDIUM | P1 |
| Taxonomy editor UI | HIGH | MEDIUM | P1 |
| JSON import/export | MEDIUM | LOW | P2 |
| Confidence threshold setting | MEDIUM | LOW | P2 |
| Override history view | LOW–MEDIUM | LOW | P2 |
| Batch retry / dry-run | MEDIUM | LOW–MEDIUM | P2 |

**Priority key:**
- P1: Must have for v1.3 (the milestone goal already commits to all of these)
- P2: Should have, add when possible (v1.3.x)
- P3: Nice to have, future consideration (v2+)

## Competitor Feature Analysis

| Feature | Thunderbird | Gmail | Superhuman / Shortwave | SGE v1.3 Approach |
|---------|-------------|-------|------------------------|-------------------|
| Auto-organize new mail | Filters fire on delivery (header/sender rules only) | Filters run on arrival (header criteria only) | Auto-labels / AI filters on content (cloud) | Content classification on-device (Laya), confirm before move |
| Rule/taxonomy management | Message Filters dialog per account | Filters tab in settings | Custom auto-label prompts / custom splits | Hierarchical pt-BR taxonomy editor + JSON import, UTFPR default |
| Retroactive apply | Run Filters on Folder (manual, no report) | "Also apply to matching conversations" checkbox (no report) | N/A (labels are views, not moves) | Whole-account batch with progress + persisted report + retry |
| Correction path | Edit filter, re-run | Edit filter, re-apply | Retune prompt / split definition | One-click override (re-move + log), no filter editing needed |
| Unclassifiable mail | Left in Inbox | Left in Inbox / Primary | N/A | Explicit `A Classificar` bucket with review UI |
| Multi-label | Tags + folders (both server-side) | Multiple labels (server-side, no move needed) | Labels as views | Primary (physical Auto/ folder) + secondary (SQLite-only label) |
| Privacy model | Local rules, no content leaves | Server-side, content mined | Cloud AI reads mail | Fully offline — content never leaves machine |
| Explanation | None (deterministic rules) | None | Minimal | One-line justification per classification, redacted |

## Sources

- Thunderbird message filters (support.mozilla.org — organize-your-messages-using-filters; Fedora Magazine intro; msgFilterRules.dat per-account storage)
- Gmail filters (support.google.com/mail/answer/6579; "Also apply filter to matching conversations" retroactive pattern)
- Superhuman auto labels + split inbox + auto archive (superhuman.com; tutorial coverage of custom auto-label prompts)
- Shortwave AI filters in plain English + splits/bundles (shortwave.com; Zapier comparison 2026)
- AI triage with human review gate (ThinkAutomation validation-request pattern; Missive 9-best-AI-assistants 2026 roundup; Inbox Zero agent-on-existing-inbox)
- AI pre-filing evaluation + irrelevant filtering (Dynamics 365 Email Classification, GA 2026)
- SGE PROJECT.md v1.3 scoping decisions (confirm-except-batch, Auto/ root, secondary local-only, offline, sensitive-data rule)

---
*Feature research for: SGE v1.3 Auto-Classify (classification UX)*
*Researched: 2026-10-10*
