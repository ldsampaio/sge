---
gsd_state_version: "1.0"
milestone: v1.4
milestone_name: SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules
status: starting
stopped_at: Phase 22 COMPLETE (19/19 green, live pending creds); 23 executing; 24/25 queued; 26 discuss dispatching
last_updated: "2026-10-13T14:18:40.048Z"
state_head: 95082b99cf54cf70b1da3b7165f63a8526f320ff
progress:
  total_phases: 18
  completed_phases: 20
  total_plans: 18
  completed_plans: 2
last_activity: 2026-10-11
ship_status: Milestone v1.3 complete (audit passed); v1.4 started — phases 21-27 scoped in ROADMAP.md (not yet planned per-phase)
current_phase_name: SMTP Send — Live Gate (Live-01, SANDBOXED)
---

---
# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-10 — Milestone v1.3 complete)

**Core value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.
**Current focus:** Phase 21 — SMTP Send — Live Gate (Live-01, SANDBOXED)

## Milestone Status

**v1.4 SMTP Send + ...: IN PROGRESS** — Phase 21 + 22 complete (offline green, live runs pending human creds); 23/24/25 planned (23 executing); 26/27 open. Requirements in `.planning/REQUIREMENTS.md`. Live testing vs own account under sandbox contract (2026-10-13): creds in git-ignored `src-tauri/.live-test.env`, test mail only to self, `SGE-Test-<run-id>` sandbox, post-run cleanup.
**v1.3 Auto-Classify: PASSED** — 17/17 requirements, 6/6 phases (15-20), audit `.planning/v1.3-MILESTONE-AUDIT.md` (PASSED, deferred live gates documented).

- Milestone audit: `.planning/v1.3-MILESTONE-AUDIT.md` — status: passed, scores: requirements 17/17, phases 6/6, integration 18/18 paths wired (1 blocker found + fixed in-audit)
- All phase summaries verified and complete

## Current Position

**Phase 22 COMPLETE 2026-10-13** (LIVE-02 offline scope; live run pending human creds via `scripts/live-22.sh`):

| Plan | Status | Key Deliverables |
|------|--------|-----------------|
| 22-01 Tracer | complete | Harness skeleton + pre-flight guard + GateSession + one-copy helpers, 10 offline tests |
| 22-02 Expansion | complete | Replay-order + double-sync resurrection + Seen both ways + secret scan + prefix audit + cleanup/report, 19/19 tests green |

Phase 21 likewise complete (offline green; live pending `scripts/live-21.sh`). Phase 23 executing (3 waves), 24/25 queued, 26/27 open.

All 6 phases (15-20) are **complete** as of 2026-10-10:

| Phase | Status | Key Deliverables |
|-------|--------|-----------------|
| 15. Sidecar Packaging Spike | complete | Frozen binary 294MB, 6s cold-start, CONTRACT.md verified, 13 unit tests |
| 16. Taxonomy + Store | complete | 23 ID-stable taxonomy ids, M12+ migrations, redaction sanitizer, 56 store tests |
| 17. Classify Engine (No Moves) | complete | Behind-sync suggestions, keyword veto → review bucket, 44 classify tests |
| 18. Confirm + Trust UX | complete | Confirm-gated MOVE, override/log/pin, badges, threshold, 6 filing tests |
| 19. Taxonomy Editor + Import | complete | Add/rename/delete/merge, import/export, ID-stable ops, 10 taxedit tests |
| 20. Batch Reorganization | complete | Journal/resume/undo, chunked 25, UIDVALIDITY re-check, 3 batch tests |

**Integration:** 18/18 paths wired; 1 blocker (`classify_message` orphaned from UI) found and fixed in-audit.

**Performance:** 328 Rust tests green, tsc clean, zero warnings. Frozen binary: 294MB, 6s cold-start. Bundles: .deb (sge-laya + 63 weights files) + AppImage (RC=0).

## Performance Metrics

**Velocity:**

- v1.2 shipped 2026-10-08: 5 phases (10-14), 252+ tests green
- v1.1 shipped 2026-10-05: 4 phases (6-9), 8 plans
- v1.0 shipped 2026-10-03: 5 phases (1-5)

**By Phase:** (all complete)

| Phase | Plans | Status |
|-------|-------|--------|
| 15. Sidecar Packaging Spike | 3/3 | Complete | 2026-10-10 |
| 16. Taxonomy + Store | 3/3 | Complete | 2026-10-10 |
| 17. Classify Engine (No Moves) | 2/2 | Complete | 2026-10-10 |
| 18. Confirm + Trust UX | 2/2 | Complete | 2026-10-10 |
| 19. Taxonomy Editor + Import | 2/2 | Complete | 2026-10-10 |
| 20. Batch Reorganization | 2/2 | Complete | 2026-10-10 |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table. v1.3 structural decisions (from research SUMMARY.md, FINAL):

- Sidecar: PyInstaller-frozen `laya[serve]` 0.4.2 speaking `POST /v1/systemone` (+ `/batch`), CPU-only torch, multilingual checkpoint only, weights as Tauri `resources`, binary `sge-laya-<target-triple>` via `externalBin`. Lean ONNX/IPC is a deferred fallback with an explicit trigger (size/RAM blowout), never parallel.
- Build order: packaging spike → taxonomy/store → suggestions-without-moves → confirm/MOVE → editor/import → batch.
- Trust: single-email always user-confirmed (IPC-enforced); batch unconfirmed with journal + undo; worst case `A Classificar`, never delete; secondary label local-only.
- Privacy: redaction sanitizer designed once in Phase 16, reused everywhere; labels store pointers, never content.
- [Phase ?]: 21-02: draft-send STEP verdict binary (FAIL BY DESIGN on any orphan, zero harness discard) routing to gap closure
- [Phase ?]: 21-02: secret scan in-bin over full temp-DB dump before cleanup (SCAN-DB line), wrapper re-checks log + SCAN-DB

### Pending Todos

- None — all phases complete, milestone passed
- Validate live when convenient: `/gsd-verify-work 6/7/8` (v1.1) + `/gsd-verify-work 12/13` (v1.2) — standing deferred items

### Blockers/Concerns

- Bundle size (~1 GB est.) and cold-start seconds — measured: 294MB, 6s ship fine
- Confidence threshold default tuned on representative pt-BR mail (Phase 17 default holds)
- `Auto`-root collision resolved: user already owns `Auto` → confirm-then-nest dialog (Phase 18 planning)

## Deferred Items

Items acknowledged and deferred at milestone close, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| Validation | SMTP 587 live send + draft round-trip | Deferred | v1.2 close | v1.2 |
| Validation | Seen round-trip + offline replay vs live server | Deferred | v1.1 close | v1.1 |
| Validation | Folder tree/browse/badges vs webmail | Deferred | v1.1 close | v1.1 |
| Validation | Poll arrival + reconnect recovery live | Deferred | v1.1 close | v1.1 |
| Feature | IDLE push (poll stays fallback) | Deferred | M1 close | v1.1 |
| Feature | CONDSTORE/QRESYNC fast path | Deferred | M1 close | v1.1 |
| Feature | Learning/fine-tuning Laya from overrides (log from day one) | Deferred | v1.3 scoping | v1.3 |
| Feature | Batch dry-run + retry-failed | Deferred | v1.3 scoping | v1.3 |
| Feature | Override history view | Deferred | v1.3 scoping | v1.3 |

## Session Continuity

**Stopped at:** 22-01 SUMMARY written + verified (10/10 offline green, SKIP exit-2); 22-02 executing in background agent; 23 smart-discuss in background agent (2026-10-13)

**Operator repairs 2026-10-13 (autonomous run pre-flight):**
- `38bd940` restored 6 product files wrongly deleted by `04fe8ca` cleanup (`sync.rs` 2773 lines, `filing.rs`, MailboxView, TaxonomyEditor, package.json/lock) — tsc clean after restore
- `fc2cd74` added `scripts/build-env.sh` (REQUIRED source before any cargo cmd: flatpak-SDK pkg-config + /tmp/opencode stub .pc/.so + host-only linking; never export LD_LIBRARY_PATH globally — segfaults cargo; sidecar binary path holds Phase-15 dev shim, weights dir empty stub, both git-ignored)

Last session: 2026-10-13T14:18:40.036Z
Milestone: v1.3 Auto-Classify complete
Resume file: scripts/live-21.sh

## Operator Next Steps

- Milestone v1.3 complete — no further agent actions required; work archived
- Future v1.4 planning may reference deferred items above
