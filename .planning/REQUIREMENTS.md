# Requirements: SGE — Linux IMAP Desktop Client

**Defined:** 2026-10-10 (Milestone v1.3 Auto-Classify)
**Core Value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.

## v1.3 Requirements

### Sidecar Engine

- [x] **SIDE-01** (Phase 15): App serves email classifications fully offline via bundled Laya sidecar (multilingual checkpoint, loopback-only, no mail content leaves the machine)
- [x] **SIDE-02** (Phase 15): Backend manages sidecar lifecycle — spawn/health/restart with backgrounded cold-start, sync and UI never block on the model
- [x] **SIDE-03** (Phase 17): User can exclude folders from auto-classify (per-folder opt-out, e.g. Sent/Drafts never auto-filed)

### Taxonomy

- [x] **TAX-01**: App ships the UTFPR default taxonomy (hierarchical, versioned, ID-stable) in the local store — Phase 16 (23 ids, v1 embedded)
- [x] **TAX-02** (Phase 19): User can add/rename/delete categories and edit keywords/rules in the options UI (ID-stable edits with migration — no orphaned labels)
- [x] **TAX-03** (Phase 19): User can import/export taxonomy JSON with validation (no cycles, no duplicate IDs, `Auto` root reserved)

### Classify Loop

- [x] **CLS-01** (Phase 17): New mail is auto-classified after sync via a behind-sync queue (sync never awaits the model)
- [x] **CLS-02** (Phase 17): User can classify/reclassify a single email manually
- [x] **CLS-03** (Phase 18): Single-email moves require explicit user confirmation — suggest → confirm → MOVE into the auto-created `Auto/` tree (root always `Auto`)
- [x] **CLS-04** (Phase 17): Low-confidence and edge mail lands in the `A Classificar` bucket for review instead of being misfiled
- [x] **CLS-05** (Phase 18): User can set the confidence threshold that routes mail to `A Classificar` (sensible default shipped)
- [x] **CLS-06**: Sensitive-data rule enforced backend-side — input redaction + output filter, justifications/logs never contain senhas/códigos/dados sigilosos, labels store pointers not content — Phase 16 (9 fixtures, schema-structural)

### Trust UX

- [x] **TRUST-01** (Phase 18): User can override a classification (one-click correct → re-move + override logged)
- [x] **TRUST-02** (Phase 18): User sees category badges in the message list (primary category per email, local-only secondary badged distinctly)
- [x] **TRUST-03** (Phase 18): User sees the (redacted) justification for a suggestion at confirm time — transient UI string, never persisted

### Batch Reorganization

- [x] **BATCH-01** (Phase 20): User can run whole-account batch classification with live progress and a persisted report (no per-email confirmation)
- [x] **BATCH-02** (Phase 20): Batch runs are journaled and resumable with undo-batch — a mid-run failure never leaves a half-filed account

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
| SIDE-01 | Phase 15 | Done |
| SIDE-02 | Phase 15 | Done |
| SIDE-03 | Phase 17 | Done |
| TAX-01 | Phase 16 | Done |
| TAX-02 | Phase 19 | Done |
| TAX-03 | Phase 19 | Done |
| CLS-01 | Phase 17 | Done |
| CLS-02 | Phase 17 | Done |
| CLS-03 | Phase 18 | Done |
| CLS-04 | Phase 17 | Done |
| CLS-05 | Phase 18 | Done |
| CLS-06 | Phase 16 | Done |
| TRUST-01 | Phase 18 | Done |
| TRUST-02 | Phase 18 | Done |
| TRUST-03 | Phase 18 | Done |
| BATCH-01 | Phase 20 | Done |
| BATCH-02 | Phase 20 | Done |

---

## v1.4 Requirements (Starting 2026-10-11; revised 2026-10-13 — LIVE testing vs own account, sandboxed)

**Milestone:** SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules
**Phases:** 21–27 (continue from 20; to be refined at plan-phase)
**Status:** Scoped — not yet planned per-phase. Phase 21 planning will split into 21 (SMTP Live Gate) / 22 (Draft/Seen Live Gate) / 23 (Search + Filters) / 24 (Auto-Archive) / 25 (Smart Rules) / 26 (Live Validation Bundle) / 27 (Verification / Audit Update).

> **Live testing vs own account (2026-10-13): the user provides real credentials** (`src-tauri/.live-test.env`, git-ignored, never committed) so LIVE-* gates run against the real `mail.utfpr.edu.br` / `smtp.utfpr.edu.br:587`. **Sandbox safety contract (structural):** every live test creates one sandbox folder `SGE-Test-<run-id>` at start; ALL APPEND / MOVE / STORE / EXPUNGE ops are scoped to that folder; test mail is sent ONLY to the user's own address (never to other accounts); existing mail and existing folders are never touched (no moves/flags/expunges outside the sandbox, no CREATE/RENAME/DELETE of non-sandbox folders); a pre-flight guard aborts the run if any op targets outside the sandbox; the run ends with cleanup (sandbox folder + test mails removed). Offline doubles (MockSession/FakeSession, record-replay SMTP transport) remain the fallback for CI and for failure-mode coverage too dangerous to run live.

### Live Validation Gates (real server, sandboxed — creds in git-ignored `src-tauri/.live-test.env`)
- [ ] **LIVE-01** (Phase 21): End-to-end SMTP send against `smtp.utfpr.edu.br:587/STARTTLS` verified — test message sent ONLY to the user's own address, arrives, no duplicate (Message-ID check), Sent copy present, retry/backoff surfaces cleanly, keyring creds reused, no secret leakage in logs. Sandbox contract holds (no other recipient, no existing mail touched).
- [ ] **LIVE-02** (Phase 22): Draft APPEND inside the `SGE-Test-<run-id>` sandbox leaves exactly one server copy per session (`draft_uid` tracked); edit offline + reconnect survives; delete/move replay from `imap_outbox` scoped to sandbox UIDs with no silent resurrection by backfill; Seen-flag round-trip survives reconnect. Pre-flight guard + post-run cleanup verified.
- [ ] **LIVE-03** (Phase 26): All deferred `/gsd-verify-work` gates (6/7/8 for v1.1, 12/13 for v1.2, confirm→MOVE for v1.3) executed against the real server under the sandbox contract; results recorded; any remaining deferrals documented with reason.

### Search + Filters
- [ ] **SRCH-01** (Phase 23): User searches local SQLite by subject, body snippet, sender, attachment name, folder scope; results ranked; multi-folder selectable.
- [ ] **SRCH-02** (Phase 23): User saves, edits, deletes filter queries; filters applied at sync time or query-time; state persisted across restarts.
- [ ] **SRCH-03** (Phase 23): Search state (query + folder scope + sort) survives app restart and reconnect.

### Auto-Archive
- [ ] **ARCH-01** (Phase 24): User configures auto-archive age rule (e.g., >30 days in folder → `Auto/Archive/`); batch job journaled per message with resume/undo; uses `batch_classify` machinery with archive-specific threshold.
- [ ] **ARCH-02** (Phase 24): Archive never deletes — only MOVE; user can review archive folder; undo-batch restores originals.

### Smart Folder Rules
- [ ] **RULE-01** (Phase 25): User defines rule (keyword / sender pattern / subject regex); rule validated (no empty patterns, no reserved folder names); conflicts with existing taxonomy resolved by user confirmation reusing `confirm_suggestion`.
- [ ] **RULE-02** (Phase 25): Rule applies at sync or on-demand to new/incoming mail; moves use UID-only + UIDVALIDITY gating; rule persists in SQLite; survives taxonomy version bumps (ID-stable references).
- [ ] **RULE-03** (Phase 25): User can list/edit/delete rules; rules never auto-delete messages (only MOVE to user-selected folder); sensitive-data redaction applies to any rule justification/logs.

### Scope / Constraints (carried forward)
- Body.PEEK-only reads; no silent expunge; UID-only addressed with UIDVALIDITY epoch gating; keyring-only creds; Linux-only bundle; no cloud/Laya fallback.
