# Requirements: SGE — Linux IMAP Desktop Client

**Defined:** 2026-10-10 (Milestone v1.3 Auto-Classify)
**Core Value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.

## v1.3 Requirements

### Sidecar Engine

- [ ] **SIDE-01**: App serves email classifications fully offline via bundled Laya sidecar (multilingual checkpoint, loopback-only, no mail content leaves the machine)
- [ ] **SIDE-02**: Backend manages sidecar lifecycle — spawn/health/restart with backgrounded cold-start, sync and UI never block on the model
- [ ] **SIDE-03**: User can exclude folders from auto-classify (per-folder opt-out, e.g. Sent/Drafts never auto-filed)

### Taxonomy

- [ ] **TAX-01**: App ships the UTFPR default taxonomy (hierarchical, versioned, ID-stable) in the local store
- [ ] **TAX-02**: User can add/rename/delete categories and edit keywords/rules in the options UI (ID-stable edits with migration — no orphaned labels)
- [ ] **TAX-03**: User can import/export taxonomy JSON with validation (no cycles, no duplicate IDs, `Auto` root reserved)

### Classify Loop

- [ ] **CLS-01**: New mail is auto-classified after sync via a behind-sync queue (sync never awaits the model)
- [ ] **CLS-02**: User can classify/reclassify a single email manually
- [ ] **CLS-03**: Single-email moves require explicit user confirmation — suggest → confirm → MOVE into the auto-created `Auto/` tree (root always `Auto`)
- [ ] **CLS-04**: Low-confidence and edge mail lands in the `A Classificar` bucket for review instead of being misfiled
- [ ] **CLS-05**: User can set the confidence threshold that routes mail to `A Classificar` (sensible default shipped)
- [ ] **CLS-06**: Sensitive-data rule enforced backend-side — input redaction + output filter, justifications/logs never contain senhas/códigos/dados sigilosos, labels store pointers not content

### Trust UX

- [ ] **TRUST-01**: User can override a classification (one-click correct → re-move + override logged)
- [ ] **TRUST-02**: User sees category badges in the message list (primary category per email, local-only secondary badged distinctly)
- [ ] **TRUST-03**: User sees the (redacted) justification for a suggestion at confirm time — transient UI string, never persisted

### Batch Reorganization

- [ ] **BATCH-01**: User can run whole-account batch classification with live progress and a persisted report (no per-email confirmation)
- [ ] **BATCH-02**: Batch runs are journaled and resumable with undo-batch — a mid-run failure never leaves a half-filed account

## Future Requirements (deferred)

- Override history view (log from day one in v1.3, UI later)
- Learning/fine-tuning Laya from overrides
- Batch dry-run + retry-failed
- Cross-device taxonomy sync
- Per-category auto-confirm ("always file X without asking")

## Out of Scope

- **Cloud AI fallback** — mail content never leaves the machine; violates the offline constraint
- **Auto-delete/auto-archive via classifier** — worst case is `A Classificar`, never delete
- **Unconfirmed single-email moves** — always-confirmed is structural, not polish
- **Server-side (Sieve) rule export** — a second product
- **GPU builds of the sidecar** — CPU-only for v1.3
- **Laya checkpoint fine-tuning** — zero-shot + keyword-assisted only

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| SIDE-01 | TBD | Pending |
| SIDE-02 | TBD | Pending |
| SIDE-03 | TBD | Pending |
| TAX-01 | TBD | Pending |
| TAX-02 | TBD | Pending |
| TAX-03 | TBD | Pending |
| CLS-01 | TBD | Pending |
| CLS-02 | TBD | Pending |
| CLS-03 | TBD | Pending |
| CLS-04 | TBD | Pending |
| CLS-05 | TBD | Pending |
| CLS-06 | TBD | Pending |
| TRUST-01 | TBD | Pending |
| TRUST-02 | TBD | Pending |
| TRUST-03 | TBD | Pending |
| BATCH-01 | TBD | Pending |
| BATCH-02 | TBD | Pending |
