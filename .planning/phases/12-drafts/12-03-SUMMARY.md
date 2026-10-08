---
phase: 12-drafts
plan: 12-03
subsystem: ui
tags: [react, tauri-invoke, drafts, autosave, pt-BR]

# Dependency graph
requires:
  - phase: 11-folder-crud
    provides: create_folder command + confirm-then-create flow reused for drafts-missing
  - phase: 12-drafts (12-01)
    provides: DraftRow store shape mirrored in types.ts
provides:
  - DraftEditor.tsx (editor form + explicit save/discard + 30s dirty-only autosave)
  - DraftSaveResult + DraftRow + DiscardResult shared shapes (types.ts)
  - Drafts-folder wiring (Novo rascunho, row→editor resume, post-save refresh)
affects: [13-send (DRAFT-03 send-transaction consumes server_uid + dirty), 14-mime]

# Actuals (#2632) — session diff only (chars/4 over the 4 files this plan touched)
actuals:
  tokens: 6824
  tasks: 3
  commits: 4
plan_head_before: 0149657b076f2f073969749958acbf0969860103

# Tech tracking
tech-stack:
  added: []
  patterns: [snapshot-diff-dirty-tracking, live-mirror-autosave-closure, session-uid-to-compose-id-map, frozen-contract-defensive-invoke]

key-files:
  created:
    - src/components/DraftEditor.tsx
  modified:
    - src/types.ts
    - src/components/MailboxView.tsx
    - src/components/MailboxView.css

key-decisions:
  - "List rows carry no compose-session id, so row→editor uses a two-path open: session-saved UIDs resolve via get_draft, all other rows seed a fresh session from fetch_message plain-text only"
  - "Autosave interval is mount-once with a live field mirror: the same doSave path as the button, dirty-gated, in-flight-guarded, cleared on unmount"
  - "Unknown-command rejections get their own inline copy so the UI compiles and renders even if the 12-02 backend has not landed"

patterns-established:
  - "Draft dirty tracking as pure snapshot diff: typing never touches invoke, save resets the baseline"
  - "drafts-missing create-then-retry runs exactly once per save behind an inline confirm reusing the expunge-modal skeleton"

requirements-completed: [DRAFT-01, DRAFT-02]

coverage:
  - id: D1
    description: "User can save a draft explicitly and resume editing it later from the Drafts folder with no network wait"
    requirement: "DRAFT-01"
    verification:
      - kind: other
        ref: "tsc --noEmit (clean, 3/3 task checkpoints)"
        status: pass
    human_judgment: true
    rationale: "Typecheck proves the wiring compiles; the type→Save→Salvo→close→reopen-intact walkthrough needs a human in the Tauri window (no automated UI harness in repo)"
  - id: D2
    description: "30 s autosave fires only when dirty; clean ticks issue no commands"
    requirement: "DRAFT-01"
    verification:
      - kind: other
        ref: "tsc --noEmit (clean)"
        status: pass
    human_judgment: true
    rationale: "Zero-invoke-on-clean-tick and exactly-one-save-on-dirty-tick need a human (or invoke spy) observing the dev window across a 30 s tick"
  - id: D3
    description: "Discard removes the draft behind a confirm only when dirty content exists; missing Drafts folder offers create-behind-confirmation"
    requirement: "DRAFT-02"
    verification:
      - kind: other
        ref: "tsc --noEmit (clean)"
        status: pass
    human_judgment: true
    rationale: "Confirm-vs-no-confirm branching and the create-and-retry path need a human driving both variants in the Tauri window"

# Metrics
duration: ~45min
completed: 2026-10-08
status: complete
---

# Phase 12 Plan 03: Drafts UI Summary

**Local-first draft composer with dirty-only 30 s autosave, Drafts-folder resume, and guarded discard — all against the frozen save/get/discard command contract**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-10-08T (plan-head `0149657`)
- **Completed:** 2026-10-08
- **Tasks:** 3 / 3
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments

- `DraftEditor.tsx`: To/Cc/Bcc + Subject + `<textarea>` body form with pure snapshot-diff dirty tracking (typing issues zero invokes), explicit Guardar/Descartar/Fechar, `drafts-missing` → inline "criar?" confirm → `create_folder` → exactly-once retry, and SyncStatus-tone indicator (Salvando… / Não salvo / Não salvo — sem conexão / Salvo)
- Drafts view wiring in `MailboxView.tsx`: "Rascunhos" folder reuses the message list, "Novo rascunho" opens a blank editor, row click resumes via `get_draft` (session-known) or a text-only `fetch_message` seed (fresh session), post-save list refresh with no full sync
- 30 s dirty-only autosave: single mount-once interval per open editor through a live field mirror, same save path as the button, in-flight-guarded, cleared on unmount/editor-switch
- `tsc --noEmit` clean after every task; no backend files touched

## Task Commits

Each task was committed atomically:

1. **Task 1: DraftEditor form + explicit save/discard** - `9ebbbbb` (feat)
2. **Task 2: Drafts view wiring (list reuse + open/resume + Novo rascunho)** - `84c642c` (feat)
3. **Task 3: 30 s dirty-only autosave tick + pending indicator** - `2e0fb4e` (feat)

**Plan metadata:** this SUMMARY (`12-03-SUMMARY.md`, commit follows)

_Note: the measured `commits: 4` range includes `8f166f2` (12-02, landed by the parallel agent mid-run); 3 of the 4 are this plan's._

## Files Created/Modified

- `src/components/DraftEditor.tsx` (created) - Composer form, save/discard flows, autosave tick, both confirm dialogs
- `src/types.ts` (modified) - Added `DraftRow`, `DraftSaveResult`, `DiscardResult` mirroring the Rust shapes
- `src/components/MailboxView.tsx` (modified) - `isDraftsFolder`, `openDraftFromRow`, `DraftEditorPane` slot, Novo rascunho + post-save refresh
- `src/components/MailboxView.css` (modified) - `.draft-editor` / `.draft-fields` stacked form styles reusing folder-dialog tokens

## Decisions Made

- List rows carry no compose-session id and `fetch_message` returns no Message-ID header, so row→editor cannot always call `get_draft(id)`: rows saved by this session resolve through a `server_uid → id` session map to `get_draft`; every other row seeds a fresh session from the fetched plain text (`view.text ?? ""`, HTML never injected). Saving the seeded draft APPENDs a new copy; the superseded server copy is an orphan for the backend sweep (same posture as `discard_draft`'s documented best-effort leg).
- Autosave reuses the button's `doSave` verbatim (read through a live mirror, not duplicated logic); the mount-once interval is safe because the parent keys the editor per session (`key={draftId ?? "new"}`), so editor-switch remounts and the cleanup clears the old tick.
- `onClose` prop added beyond the plan's prop list (small, justified: the parent needs a close path for pristine-never-saved discards and the Fechar button).
- `cargo test -p sge` intentionally not run: this plan touches zero backend files, and the tree carries 12-02's uncommitted `sync/worker.rs` work — running the suite would verify their scope, not mine. Flagged for the phase verifier instead.

## Deviations from Plan

### Auto-fixed Issues

None - plan executed exactly as written, except the two adaptive decisions above (row→session mapping, onClose prop), which stay inside the plan's acceptance criteria and disjoint-files rule.

**Total deviations:** 0 auto-fixed (2 documented adaptive decisions, no scope creep)
**Impact on plan:** None — all must_haves truths hold at the code level; behavior walkthroughs route to human UAT (coverage D1–D3).

## Issues Encountered

- Parallel-agent overlap: 12-02 committed `8f166f2` (draft command registration) and holds uncommitted `sync/worker.rs` changes in the shared tree. Disjoint-files rule held — staged per-file (`git add <file>`, never `git add .`), verified each commit's file list; 12-02's files never entered a 12-03 commit.
- Committing on `main`: allowed — `.planning/config.json` sets `git.allow_default_branch_commits: true`, and the SDK protected-branch check returned `false` for this branch.

## Known Stubs

None introduced. Scanned created/modified files for stub patterns (`=[]`, `TODO`/`FIXME`, placeholder copy, unwired props): no hits. Seeded `DraftRow` defaults (`mailbox_id: 0`, `message_id: ""`) are transient editor seeds, never persisted — the first save mints a real uuid and the backend generates the Message-ID.

## Threat Flags

None — no new network surface. Draft body edits in `<textarea>` only (never `dangerouslySetInnerHTML`, T-12-06); Drafts-folder reading reuses the sanitized ReadingPane path untouched; autosave is a single dirty-gated, in-flight-guarded interval per editor (no concurrent `save_draft` overlap, T-12-07).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- DRAFT-01 user-visible slice complete at UI level; DRAFT-02 needs no UI beyond the saved indicator (present).
- Phase 13 handoff ready: saves surface `server_uid` + `dirty`/`acked` through `onSaved`, and the session map keeps `server_uid → id` for the send-transaction to consume.
- Phase verifier should run the dev walkthrough (coverage D1–D3, no credentials needed) plus `cargo test -p sge` once 12-02 lands, since this plan skipped the backend suite deliberately.

## Self-Check: PASSED

- `src/components/DraftEditor.tsx`, `src/types.ts` (DraftRow/DraftSaveResult/DiscardResult), `src/components/MailboxView.tsx` (DraftEditorPane/Novo rascunho), `src/components/MailboxView.css` (draft-editor styles) — all FOUND on disk with expected content
- Commits `9ebbbbb`, `84c642c`, `2e0fb4e` — all FOUND in `git log`; each commit's file list verified disjoint from 12-02's files
- `tsc --noEmit` — clean (exit 0) after all three tasks

---
*Phase: 12-drafts*
*Completed: 2026-10-08*
