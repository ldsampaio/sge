---
phase: 12-drafts
reviewed: 2026-10-08T00:00:00Z
depth: standard
files_reviewed: 12
files_reviewed_list:
  - src-tauri/src/drafts.rs
  - src-tauri/src/store/mod.rs
  - src-tauri/src/store/queries.rs
  - src-tauri/src/imap/mod.rs
  - src-tauri/src/imap/manager.rs
  - src-tauri/src/imap/bodies.rs
  - src-tauri/src/commands/sync.rs
  - src-tauri/src/sync/worker.rs
  - src-tauri/src/bin/sync_demo.rs
  - src-tauri/src/lib.rs
  - src/components/DraftEditor.tsx
  - src/components/MailboxView.tsx
  - src/types.ts
findings:
  critical: 1
  major: 4
  minor: 6
  total: 11
status: issues_found
---

# Phase 12 (Drafts): Code Review Report

**Reviewed:** 2026-10-08
**Depth:** standard (full read of all Rust + TS changes in `7e18eec..HEAD`, cross-checked against 12-01/12-02/12-03 PLANs)
**Files Reviewed:** 12 source files (+ `src/types.ts`, `MailboxView.css` skimmed for contract surface)
**Status:** issues_found — 1 critical, 4 major, 6 minor

## Summary

Phase 12 delivers a coherent local-first drafts slice: M9 store + pure RFC 5322
renderer + APPEND/SEARCH-HEADER verbs (12-01), save/discard commands + reconnect
flush (12-02), and editor + Drafts-folder wiring + dirty-only autosave (12-03).
Renderer injection guards, the stable-Message-ID reconcile rule, and the
scoped-expunge discipline are all genuinely in place and tested.

The headline defect is that `save_draft` is **not actually local-first**: it
resolves the Drafts wire name over the network (`LIST`) *before* the local
upsert, so every offline save fails without persisting anything — the exact
scenario DRAFT-01 and the plan's own acceptance criteria require to work.
A second user-visible defect blanks the composer right after the first save of
a new draft (remount on key change with a stale seed). The remaining majors
are a stale-seed collision between consecutively opened server drafts, a
retry path that re-APPENDs (duplicating the server copy), and a discard path
with no UIDVALIDITY stale guard. Minors are protocol-robustness and hygiene
items. No hardcoded secrets, no `eval`/unsafe patterns, no SQL injection
(all queries parameterized), no draft-body XSS (textarea-only editing).

## Critical Issues

### CR-01: `save_draft` fails offline before the local-first write — local-first ordering violated

**File:** `src-tauri/src/commands/sync.rs:781-794`
**Issue:** `save_draft` calls `resolve_drafts_wire(&manager, mailbox).await?`
(line 794) *before* the `upsert_draft … dirty=1` local write. With the default
UI path, `mailbox` is always `None` (DraftEditor's invoke at
`DraftEditor.tsx` passes only `{id, subject, body, to, cc, bcc}` — no
`mailbox`), so resolution falls through to `manager.list_mailboxes()`, which
is `lease_for("INBOX")` → `connect_sync` when no session exists
(`imap/manager.rs:498-501`). Offline, the connect fails and `save_draft`
returns `Err` having persisted nothing. The plan's acceptance criterion
("offline manager error → row persisted `dirty=1`, `acked=false`") is therefore
unreachable: the offline failure happens one step earlier than the tested
path, and the command's own doc comment ("the call returns even offline",
line ~786) is false. This breaks DRAFT-01's core promise and also means every
*online* save pays a `LIST` round-trip before touching the store.
**Fix:**
```rust
// In save_draft: do the local-first upsert FIRST using a cached wire name,
// and only then attempt the network legs.
let wire = match &mailbox {
    Some(w) => w.clone(),
    None => find_drafts_wire_cached(&store)  // roles cached from last LIST
        .ok_or_else(|| "drafts-missing: ...".to_string())?,
};
// 1. upsert dirty=1 (returns even offline) ...
// 2. online attempt save_draft_copy_in; on connectivity error -> acked=false.
```
At minimum, move the `upsert_draft` block above `resolve_drafts_wire`, and
resolve the wire from the locally cached folder tree (store `mailboxes`
table) with `LIST` only as a fallback/refresh — never as a gate on the
local write.

## Major Issues

### MJ-01: First save of a new draft remounts the editor and blanks the form

**File:** `src/components/MailboxView.tsx:149-153` + `:480`, with `src/components/DraftEditor.tsx:75-79`
**Issue:** `handleDraftSaved` updates the open editor to
`{...ed, draftId: result.id}` but keeps the old `initial` (undefined for a
new draft). `DraftEditorPane` keys the editor by
`key={draftEditor.draftId ?? "new"}` (line 480), so the key flips `"new"` →
`"<uuid>"` and React remounts `DraftEditor`. Field state is seeded once via
`useState(seed.to)` etc. (lines 75-79) from the stale `initial`, so the
composer the user just typed into re-renders **empty** immediately after a
successful save (`savedRef` also resets to the empty seed; the indicator then
reads "Salvo" on a blank form). Content is safe in the store, but the user
sees their text vanish.
**Fix:**
```tsx
// Propagate the saved snapshot into the seed on save:
setDraftEditor((ed) => (ed === null ? ed : {
  ...ed,
  draftId: result.id,
  initial: { ...(ed.initial ?? blankDraftRow(result.id)), /* saved fields */ },
}));
```
or keep the editor key stable across the first save (`key={stableSessionKey}`
minted when the editor opens, not the post-save id).

### MJ-02: Opening two non-session server drafts in a row shows stale content

**File:** `src/components/MailboxView.tsx:480`, `src/components/DraftEditor.tsx:69-79`
**Issue:** Rows without a session-map entry seed a *fresh* session
(`draftId: null`, `openDraftFromRow`). Two such rows opened consecutively both
produce `key="new"`, so no remount occurs and `useState(seed…)` keeps the
*first* row's To/Subject/body while the header/context implies the second row.
Saving then APPENDs the wrong content as a new server copy (the unedited
second row's copy becomes an orphan). Same root cause family as MJ-01: the
key does not identify what is being edited.
**Fix:** Key the pane by the opened row identity, e.g.
`key={draftEditor.draftId ?? `seed-${seedUid}`}` where the seed carries the
source `server_uid`, and/or add a `useEffect` syncing field state when
`initial` identity changes.

### MJ-03: Reconnect retry re-runs APPEND after a post-reconcile failure → duplicate server copies

**File:** `src-tauri/src/imap/manager.rs:354-377` (`save_draft_copy_in`)
**Issue:** The single retry wraps the *whole* `save_once` (APPEND → SEARCH →
mark + scoped-expunge). If APPEND + SEARCH succeed but the expunge-old leg
fails (e.g. transient `STORE`/`EXPUNGE` error), the retry APPENDs a *second*
copy and reconciles to its UID, expunging only the tracked `old_uid` — the
first new copy is orphaned. The summaries acknowledge this window
("carried-forward limitation"), but it directly weakens the exactly-one-copy
invariant (D1/DRAFT-02) on exactly the flaky-network path the retry exists
for. Convergence on the *next* save (max-UID + tracked-old expunge) only
helps if the user saves again; an idle client keeps two live copies.
**Fix:** Thread the reconciled UID through the retry: on retry, SEARCH first
for `msg_id`; if a hit newer than `old_uid` already exists, skip APPEND and
resume at the expunge-old leg.

### MJ-04: `discard_server_copy_in` expunges with no UIDVALIDITY stale guard

**File:** `src-tauri/src/imap/manager.rs:414-441`, caller `src-tauri/src/commands/sync.rs:975-1010`
**Issue:** The save path drops a stale `old_uid` on UIDVALIDITY mismatch
(`save_once`, T-12-04 guard), but the discard path takes a bare `uid: u32`
with no epoch comparison. If the Drafts folder's generation changed between
the save that recorded `server_uid` and the discard, `store_deleted(uid,
true)` + scoped expunge addresses a numeric UID in the *new* generation —
potentially flagging/expunging an unrelated message. `discard_draft` does not
even read the sync-state epoch, so the guard cannot be applied at the call
site.
**Fix:** Read the Drafts epoch in `discard_draft` (same `get_sync_state`
lookup `save_draft` uses) and pass it into `discard_server_copy_in`; skip the
server leg (log + rely on sweep expunge-diff, the already-documented orphan
posture) when the SELECT-time validity disagrees.

## Minor Issues

### MN-01: `UID SEARCH HEADER` value sent unquoted — unverified against strict servers

**File:** `src-tauri/src/imap/mod.rs:768`
**Issue:** `format!("HEADER {field_owned} {value_owned}")` interpolates the
Message-ID raw. `<…@sge.local>` happens to be atom-legal, so fakes and lenient
servers accept it, but RFC 3501 `header-fld-name header-list` values of this
shape belong in a quoted `astring`, and strict servers may reject or
misparse unquoted `<`/`>`/`@`. The live gate (the only check that would catch
this) is deferred, so this ships unverified.
**Fix:** `format!("HEADER {field_owned} \"{value_owned}\"")` (values are
internally generated and quote-free, so no escaping hazard).

### MN-02: `new_message_id` sanitizes only control chars

**File:** `src-tauri/src/drafts.rs:27-34`
**Issue:** `compose_id` arrives over IPC as an arbitrary string; spaces, `>`,
non-ASCII, `@`, or an empty string all pass through into
`<{clean}@sge.local>`. A malformed Message-ID breaks the SEARCH-reconcile the
whole phase keys on (zero-hit → permanent `dirty=1`, repeated APPENDs), and an
empty id yields `<@sge.local>`. The UI always sends a UUID today, so this is
latent — but the trust boundary (IPC argument → wire header) should not rely
on one frontend call site.
**Fix:** Validate `compose_id` (e.g. allow `[A-Za-z0-9_-]`, cap length,
fallback to a generated uuid on mismatch) instead of filtering controls only.

### MN-03: `get_draft` server fallback parses `From` then discards it (dead code)

**File:** `src-tauri/src/commands/sync.rs:941-949` (`let _ = from;`)
**Issue:** The fallback fetches and parses the sender address and then
explicitly drops it. Harmless (the row has no From column by design), but the
dead binding plus `let _` silencer signals unfinished handling and will
confuse the Phase 13 send-transaction reader about whether From is available.
**Fix:** Delete the `from` extraction or record why it is intentionally
unused in the comment (one line).

### MN-04: Fresh-seed resume drops Cc and HTML-only bodies

**File:** `src/components/MailboxView.tsx:125-143`
**Issue:** The fresh-session seed hardcodes `cc: "", bcc: ""`
(`MessageView` carries no Cc — contract gap, not UI sloppiness) and
`body: view.text ?? ""`, so resuming a Cc'd or HTML-only draft authored by
another client silently loses recipients/body text; the subsequent save then
persists the lossy copy as a new server version.
**Fix:** Short-term: surface a "conteúdo parcial (Cc/anexos não incluídos)"
notice on seeded opens; Phase 14: extend the fetch contract with Cc/Bcc and
an HTML-or-text body choice.

### MN-05: `Guardar` is enabled on a pristine empty form → junk server copies

**File:** `src/components/DraftEditor.tsx` (save button, ~line 340-350)
**Issue:** Nothing disables Save when the form is pristine-empty, so one
click APPENDs a zero-recipient, empty-subject/body copy to the server and
creates a store row. Autosave is correctly dirty-gated; the manual path is
not. Low harm (valid per "zero-recipient drafts must APPEND" decision) but it
is one click away from Drafts-folder clutter.
**Fix:** `disabled={phase === "saving" || (!dirty && !everSavedRef.current)}`
(or equivalent pristine check) on the submit button.

### MN-06: Autosave tick closes over first-render `doSave` (works by accident)

**File:** `src/components/DraftEditor.tsx:243-252`
**Issue:** The mount-once interval captures the first render's `doSave`
(suppressed with `eslint-disable-next-line`), so `onSaved` is permanently the
first render's closure. It is behaviorally correct today only because
`handleDraftSaved` touches refs + stable setters, and field freshness leans
entirely on `liveRef`. Any future parent callback with real state capture
will silently go stale.
**Fix:** Mirror the callback in a ref (`onSavedRef.current = onSaved`) and
call through it, or include `doSave` in deps with proper memoization.

---

## What was verified good (no finding)

- **Header-injection guard** (`drafts.rs:36-40`, `sanitize_header` on every
  header incl. `Date`/`Message-ID`): hostile `Subject\r\nBcc:` collapses to a
  single header; round-trip test pins it.
- **Stable-Message-ID discipline**: `upsert_draft` deliberately does *not*
  overwrite `message_id` on conflict; command reads the existing row first.
- **No bare `expunge()` on draft paths**; non-UIDPLUS falls back to the
  single-UID unmark dance; `old == new` never self-expunges.
- **Zero-hit reconcile is `Refused` (never retried)** — no blind re-APPEND
  from the reconcile step itself.
- **SQL fully parameterized**; M9 forward-only with preserve-rows test;
  `dirty_draft_count_for` correctly scopes `pending_depth` per folder.
- **No XSS surface**: editor is `<textarea>`-only; Drafts reading reuses the
  sanitized ReadingPane; no `dangerouslySetInnerHTML` added.

_Reviewed: 2026-10-08_
_Reviewer: the agent (gsd-code-reviewer)_
_Depth: standard_
