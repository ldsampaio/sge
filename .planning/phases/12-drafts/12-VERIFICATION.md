---
phase: 12-drafts
verified: 2026-10-08T00:00:00Z
status: human_needed
score: 3/3 must-haves verified (automated); 1 live gate pending human
human_verification:
  - test: "Live drafts round-trip against mail.utfpr.edu.br (UTFPR creds + network)"
    expected: "Create draft in Drafts folder, save twice, confirm exactly one server copy (APPEND-new + expunge-old); delete test drafts after. Verifies quoted HEADER SEARCH (MN-01) against a strict server."
    why_human: "Needs real UTFPR credentials + network; no credentials in this runtime. Deferred per 12-CONTEXT specifics + 12-02 D5 to /gsd-verify-work 12."
---

# Phase 12: Drafts Verification Report

**Phase Goal:** User can save and edit drafts locally and trust exactly one server copy exists per compose session
**Verified:** 2026-10-08
**Status:** human_needed (all automated checks green; only the deferred live gate remains)
**Re-verification:** No — initial verification (review fix notes in 12-REVIEW.md verified in code)

## Goal Achievement

### Observable Truths (from ROADMAP Phase 12 success criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | User can save a draft explicitly (plus 30 s dirty-only autosave) and resume editing later from Drafts, local-first with no network wait | ✓ VERIFIED | `save_draft` upserts `dirty=1` before any network (sync.rs:836-858); CR-01 fix confirmed — cached-tree wire resolve (sync.rs:822-834) precedes local write, LIST only on cold cache. 30 s dirty-gated interval in DraftEditor.tsx:250 (`setInterval`, dirty-gated, in-flight-guarded, cleared on unmount). Resume via `get_draft` / seeded open in MailboxView. Tests: `commands::` draft tests + `store::` M9 tests green. |
| 2 | Exactly one server copy per compose session — APPEND-new + expunge-old with `draft_uid` tracked, never a copy per tick | ✓ VERIFIED | `save_draft_copy_in` (manager.rs:359): APPEND → SEARCH-reconcile (exactly-one → use, zero → loud Refused, multi → max) → mark + scoped expunge-old; stable per-session Message-ID (upsert never overwrites, sync.rs:848-851); MJ-03 retry-resume via `search_draft_uid` + `expunge_old_only` (manager.rs:436-484) confirmed in code. Tests: `imap::manager::` save/discard + retry-resume tests green. |
| 3 | Offline edits queue and sync on reconnect | ✓ VERIFIED | Offline save persists `dirty=1`, `acked=false` (save_draft failure path stays dirty); `replay_dirty_drafts` (worker.rs:359) flushes mailbox-filtered dirty rows pre-sweep + empty-branch (worker.rs:730,776), per-row ack, failure stays dirty. Tests: `replay_dirty_drafts_acks_and_cleans`, `append_failure_stays_dirty`, `reconciles_each_row_by_message_id`, epoch-bump test — all green. |

**Score:** 3/3 truths verified (automated)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src-tauri/src/drafts.rs` | Pure RFC 5322 renderer + Message-ID helper | ✓ VERIFIED | Round-trip + injection tests green; MN-02 validation fix confirmed |
| `src-tauri/src/store/` M9 `drafts` table + queries | 12-col table, CRUD + dirty queue | ✓ VERIFIED | Preserve-rows + schema-version tests green |
| `imap/manager.rs` save/discard fns | APPEND-new + expunge-old, epoch guards, resume-retry | ✓ VERIFIED | Code read (save_once/search/expunge_old_only/discard_server_copy_in); MJ-03 + MJ-04 fixes present |
| `commands/sync.rs` save/get/discard | Frozen UI contract, local-first ordering | ✓ VERIFIED | Code read; CR-01 + MN-03 fixes present; commands registered in lib.rs |
| `sync/worker.rs` replay flush | Reconnect flush + pending depth | ✓ VERIFIED | Call sites + 6 flush tests green |
| `src/components/DraftEditor.tsx` | Form + save/discard + autosave | ✓ VERIFIED | Code read; MJ fixes (session key, onSavedRef, pristine-disable, lossy-seed notice) present |

### Key Link Verification

| From | To | Via | Status |
|------|----|-----|--------|
| DraftEditor → save_draft | Tauri command, frozen contract | invoke save/get/discard | WIRED (tsc clean) |
| save_draft → store | upsert dirty=1 before network | spawn_blocking upsert | WIRED |
| save_draft → manager | save_draft_copy_in APPEND/SEARCH/expunge | lease-scoped verbs | WIRED |
| worker → replay_dirty_drafts | pre-sweep + empty-branch flush | save_draft_on_session reuse | WIRED |
| MailboxView → DraftEditor | per-open session key, resume paths | DraftEditorPane | WIRED |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Backend suite green | `cargo test -p sge` (src-tauri/) | 226 passed, 0 failed, 1 ignored | ✓ PASS |
| Frontend typecheck | `npx tsc --noEmit` | exit 0, no errors | ✓ PASS |

### Requirements Coverage

| Requirement | Description | Status | Evidence |
|-------------|-------------|--------|----------|
| DRAFT-01 | Local-first save/edit/resume + autosave | ✓ SATISFIED | Truth 1; 12-03 claims DRAFT-01 in requirements-completed |
| DRAFT-02 | Exactly-one server copy + discard | ✓ SATISFIED | Truth 2 + discard epoch-gated path; 12-03 claims DRAFT-02 |

(DRAFT-03 send-transaction is Phase 13 scope per CONTEXT; handoff state `server_uid` + `dirty` exposed.)

### Anti-Patterns Found

None. Review's 11 findings (1 critical, 4 major, 6 minor) all carry fix commits (d6f10f7, fc32e74, bd754c7, d2ad356, 13819d8, 23651d7, 68fca05, c9d248f, 44d9773, 8b40a52) and each fix was spot-confirmed in current code. No stub patterns introduced.

### Human Verification Required

1. **Live drafts round-trip (deferred live gate).** Against `mail.utfpr.edu.br` with UTFPR credentials: create draft → save twice → assert exactly one server copy; discard → assert server copy gone; cleanup test drafts. Also exercises MN-01 quoted SEARCH against a strict server. Deferred per 12-CONTEXT ("one live gate") and 12-02 D5 (human_judgment, no creds in scope) → `/gsd-verify-work 12` follow-up.

### Gaps Summary

No code gaps. The only open item is the credential-gated live round-trip, which is a human-run gate, not a code defect — hence `human_needed`, not `gaps_found`.

---
_Verified: 2026-10-08_
_Verifier: the agent (gsd-verifier)_
