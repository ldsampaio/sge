# FEATURES — Compose & Organize Milestone (v1.2) Research

> Question: How do compose/send, folder management, delete/move, drafts typically work in desktop mail clients? Expected behaviors, table stakes vs differentiators vs anti-features, complexity, dependencies on existing triage features.
> Date: 2026-10-06. Sources: RFC 5322 (§3.6 reply/threading headers), RFC 6851 (MOVE), RFC 4315 (UIDPLUS/COPYUID), Thunderbird account-settings + support docs, Apple Mail / Gmail IMAP behavior notes, SGE `.planning/PROJECT.md` + repo layout (`src-tauri/src/imap|sync|store`, `src/components/*`).

Already built — do NOT re-spec: read/unread sync (Seen STORE, optimistic UI, outbox), folder-tree browsing + per-folder sync + STATUS UNSEEN badges, 5-min poll + manual refresh (single-flight SyncGate), UID backfill (range-diff/tombstoning/convergence), global search, attachment list/download, keyring auth, Linux bundle.

---

## 1. Compose & Send (SMTP via `smtp.utfpr.edu.br:587/STARTTLS`)

### 1.1 Expected behaviors (table stakes)

| Behavior | Convention (TB / Apple Mail / Gmail / Outlook) | Notes for SGE |
|---|---|---|
| New compose: To/Cc/Bcc + Subject + rich/plain body | All clients; Bcc hidden but sent | Validate ≥1 recipient; empty-subject confirm dialog ("Send anyway?") |
| Reply-To honored | RFC 5322 §3.6: reply goes to `Reply-To:` if present, else `From:` | Must parse `Reply-To` from cached headers (already stored) |
| Reply headers | `In-Reply-To:` = parent `Message-ID`; `References:` = parent `References` + parent `Message-ID` (RFC 5322 §3.6.4) | Without these, threading breaks on every other client — non-negotiable |
| Reply subject | Prefix `Re:` once (don't stack `Re: Re:`) | Trivial string check |
| Reply quoting | Quote original below attribution line (`On <date>, <author> wrote:`), `> `-prefixed, original headers (From/Date/To/Subject) included; cursor above quote | Thunderbird/Apple/Gmail default = top-post + quoted history. Trim signatures (`-- `) optionally |
| Reply-All | To = original From + To, Cc = original Cc, **minus own identity** | Needs "own address" config; else self-echo loops |
| Forward semantics | New message, subject `Fwd:` once, body = forward banner + quoted/inline original (attachments re-attached, see below); **no** In-Reply-To/References threading to original | Forward-as-inline (default) vs forward-as-attachment (EML, niche) — ship inline only first |
| Attachments on send | Pick files → MIME multipart/mixed, correct Content-Type + filename*, size shown, removable pre-send; forward re-attaches original parts | SGE already downloads attachments → has MIME parse to reuse for re-attach |
| Send flow | SMTP 587/STARTTLS + same creds (keyring) → on 250 OK, APPEND copy to Sent (`\Seen`) | Gmail auto-saves Sent server-side; generic IMAP (UTFPR) does **not** — client must APPEND, else Sent folder empty |
| Offline/outbox | Send failure → queue in local outbox, retry on reconnect, surfaced in UI (not silent drop) | SGE already has durable outbox pattern for flag sync — reuse |
| Identity/From | `From:` = configured account name+address; optional display-name setting | Single identity is fine for v1.2 |

### 1.2 Differentiators (do later)

- HTML compose with sanitized paste; stationery/templates; signatures editor (per-identity).
- Send-later / scheduled send; undo-send window (5–30 s delay-hold, Gmail-style).
- Smart recipients (recent-contact ranking), contact autocomplete from history.
- Large-attachment handling (size warning >20–25 MB, the de-facto SMTP ceiling).

### 1.3 Anti-features (do NOT build)

- Read receipts / tracking pixels on send — privacy-hostile, server support spotty.
- Auto-external-recipient rewriting, plug-in-driven "AI rewrite" in the send path — scope creep, blocks the send milestone.
- POP-style "leave on server" toggles — irrelevant to IMAP+SMTP milestone.

### 1.4 Complexity & dependencies

| Item | Complexity | Depends on |
|---|---|---|
| MIME build + SMTP send (lettre crate, STARTTLS 587) | **M** — new `smtp/` module, keyring creds reuse, TLS | keyring auth ✓, attachment parse ✓ |
| Reply/Reply-All/Forward header + quote construction | **S–M** — pure function on cached headers/body | headers-first store ✓, bodies-on-demand ✓ |
| APPEND-to-Sent + Sent folder refresh | **S** — IMAP APPEND + reuse per-folder sync | folder-tree/per-folder sync ✓ (Phase 7), poll ✓ |
| Outbox queue + retry + "Sent/Failed" UI | **M** — mirrors existing flag-outbox | SyncGate/outbox pattern ✓, SyncStatus UI ✓ |
| Attachment re-attach on forward | **M** — fetch full part bytes, re-encode | attachment download ✓ (Phase 4) |

Risk: UTFPR SMTP auth policy may differ from IMAP creds (some servers require full address vs bare user) — make SMTP username configurable, default = IMAP username.

---

## 2. Folder Management (IMAP CREATE / RENAME / DELETE)

### 2.1 Expected behaviors (table stakes)

| Behavior | Convention | Notes for SGE |
|---|---|---|
| Create | `CREATE` with user-typed name under selected parent; hierarchy delimiter from LIST (`.` or `/` — **server-dependent**, must use probed delimiter, cf. imap-probe/CAPABILITY+LIST decision) | Validate against existing names; handle non-ASCII via modified-UTF7 encode (SGE already has `imap/mutf7.rs` ✓) |
| Rename | `RENAME old new`; children move with it (server-side); update local DB folder rows + cached UIDs' folder mapping | INBOX itself must be non-renamable (guard in UI) |
| Delete | `DELETE`; refuse non-empty without confirm; standard clients offer "delete folder + contents" vs "cancel" | Never delete by flagging messages — `DELETE` removes the **mailbox** |
| Special folders | INBOX, Sent, Drafts, Trash (+Junk) pinned at top, special icons, excluded from rename/delete | SGE sidebar already special-cases Sent/Drafts (Phase 7) — extend with Trash role |
| Subscribe/unsubscribe | LIST-Subscribed vs full LIST; "Show only subscribed" toggle (Thunderbird setting) | Table stakes for servers with many shared folders; cheap (LSUB/UNSUBSCRIBE) |
| Refresh after op | Re-LIST tree + STATUS on affected branch; optimistic add/rename with rollback on NO response | Reuses poll/manual-refresh single path ✓ |

### 2.2 Differentiators (later)

- Drag-drop folder reorder, folder colors/icons, per-folder retention/auto-archive rules, favorite pinning.
- Namespace-aware display stripping (`INBOX.` prefix hiding).

### 2.3 Anti-features

- Client-side-only "virtual folders" that diverge from server LIST — breaks cross-client consistency.
- Recursive expunge-on-delete ("empty folder then delete") as silent default — data-loss risk; always confirm.
- Allowing CREATE with empty name / leading-trailing spaces / delimiter collisions — server NO storms.

### 2.4 Complexity & dependencies

| Item | Complexity | Depends on |
|---|---|---|
| CREATE/RENAME/DELETE commands + error mapping | **S–M** — thin `imap/manager.rs` extension + mutf7 reuse | session manager ✓, mutf7 ✓, folder tree ✓ |
| Local DB folder-table migration (roles, parent, delimiter) | **S** — rusqlite_migration pattern exists | store/queries ✓ |
| Special-folder roles + Trash wiring (see §3) | **M** — policy decision with server variance | folder browsing ✓, STATUS badges ✓ |
| Subscribe/unsubscribe | **S** | LIST path ✓ |

Risk: UTFPR namespace/delimiter unknown until live LIST — probe at account setup and persist per-account.

---

## 3. Delete & Move

### 3.1 Expected behaviors (table stakes) — the trash-vs-expunge decision

Two models exist; **desktop default is Trash-move, not raw expunge**:

1. **Move-to-Trash (default delete):** `UID COPY`/`UID MOVE` → Trash, then flag source `\Deleted` + expunge source (RFC 6851 `UID MOVE` if server advertises MOVE capability, else COPY + STORE +FLAGS + UID EXPUNGE — the UIDPLUS fallback). Message remains recoverable in Trash.
2. **Permanent delete (Shift+Delete / "Empty Trash"):** STORE `\Deleted` + `UID EXPUNGE` (UIDPLUS) or `EXPUNGE` scoped to Trash selection. Thunderbird exposes exactly this trio: *Move to Trash / Just mark deleted / Remove immediately* + *Empty Trash on Exit* + *Compact(=expunge)*.

| Behavior | Convention | Notes for SGE |
|---|---|---|
| Del key = move to Trash | Thunderbird/Apple/Outlook default | Needs Trash folder identity (auto-detect `Trash`/`Deleted`/`Lixeira`/Gmail `[Gmail]/Trash`, else CREATE `Trash` once with confirm) |
| Undo delete (Ctrl+Z / toast "Undo") | Move back from Trash (reverse MOVE); ~5–10 s window | Only feasible with Trash model — another reason raw-expunge-as-default is wrong |
| Empty Trash = true expunge | STORE \Deleted + EXPUNGE on Trash | Confirm dialog; show count |
| Move between folders (drag-drop / "Move to" menu) | `UID MOVE` preferred; COPY+STORE+EXPUNGE fallback; COPYUID response maps old→new UIDs for local DB update | Without COPYUID, re-sync target folder (cheap since per-folder sync exists) |
| Copy (Ctrl+drag / "Copy to") | `UID COPY` without flagging source | Minor extra once move exists |
| Archive (optional sibling) | One-key move to `Archive/YYYY` | Gmail-popularized; cheap given move exists — consider bundling |
| Offline delete/move queue | Flag locally (`deleted_local` tombstone / `move_pending`), replay on reconnect — same reconcile as Seen flags | Reuses pending-wins outbox + tombstoning from Phases 6/9 |

### 3.2 Differentiators (later)

- Swipe-to-delete/archive gestures, bulk triage shortcuts (`e` archive, `#` delete à la Gmail/Superhuman), auto-empty-Trash-after-N-days setting.

### 3.3 Anti-features

- **Raw EXPUNGE as the only delete** — irreversible, diverges from every desktop client, and on Gmail only removes the label (message lingers in All Mail). PROJECT.md "expunge" line must be read as *expunge-under-the-hood*, with Trash UX on top.
- Full-mailbox `EXPUNGE` (no UID scoping) on servers without UIDPLUS — can nuke other clients' `\Deleted` messages; always prefer `UID EXPUNGE` and capability-check first.
- Silent permanent delete with no confirm and no undo — data-loss + trust-loss.

### 3.4 Complexity & dependencies

| Item | Complexity | Depends on |
|---|---|---|
| Capability probe (MOVE? UIDPLUS?) at SELECT time | **S** — CAPABILITY parse already partially exists | session/probe ✓ |
| Delete→Trash + undo + Empty Trash | **M** — reuse move path + outbox | folder tree ✓, poll ✓, outbox ✓ |
| Drag-drop / Move-to / Copy-to | **M** (UI-heavy; protocol is same COPY/MOVE) | MessageList + Sidebar DnD (new), per-folder sync ✓ |
| Tombstone + replay for offline ops | **M** — extend Phase 9 tombstoning | UID backfill/convergence ✓ |
| Archive shortcut | **S** once move exists | move path |

---

## 4. Drafts (save / edit / autosave / send)

### 4.1 Expected behaviors (table stakes)

| Behavior | Convention (TB/Apple Mail/Gmail web) | Notes for SGE |
|---|---|---|
| Autosave while composing | Every ~30–60 s + on pause; Thunderbird/Apple save to IMAP Drafts; Gmail saves continuously | Start with explicit Save + timed autosave (30 s, only-if-dirty) |
| Server copy via APPEND | Draft stored with `APPEND Drafts (\Seen \Draft)` + MIME body; each autosave **replaces** the previous server copy (APPEND new + EXPUNGE old — IMAP has no in-place edit; Thunderbird's duplicate-draft bugs are exactly failure to delete the old copy) | Track `draft_uid` per compose session; delete-then-append atomically-ish |
| Edit draft | Open from Drafts → compose prefilled; saving updates same UID chain | Drafts folder browsing already exists (Phase 7) — needs "open as editable" vs read-only distinction |
| Send draft | Normal SMTP send → delete draft copy (APPEND…no — STORE \Deleted + EXPUNGE the draft UID) → APPEND to Sent | The #1 Thunderbird complaint is orphan drafts left after send — delete-on-send must be in the send transaction |
| Discard draft | Confirm → delete draft UID | — |
| Drafts sync across clients | Draft appears in Drafts on phone/webmail (standard IMAP) | Free via APPEND; verify against UTFPR webmail |
| Offline drafts | Save to local SQLite with `sync_pending`; APPEND on reconnect | Same outbox pattern as flags/deletes |

### 4.2 Differentiators (later)

- Multi-device conflict banner ("edited elsewhere"), draft templates/snippets, resume-composer on restart (restore open compose windows).

### 4.3 Anti-features

- Saving drafts to **Sent** folder on misconfiguration (classic Thunderbird footgun from wrong Copies&Folders mapping) — hard-code Drafts role, no user-mappable folder paths in v1.2.
- Autosave creating a **new copy every tick** (Thunderbird duplicate-draft bug, Apple Mail/Gmail-bin incidents) — must track-and-replace; cap: one server copy per compose session.
- Autosave overwriting a draft the user is editing on another device without warning — last-writer-wins silently; at minimum compare `INTERNALDATE`/size before replace (banner is v-later).

### 4.4 Complexity & dependencies

| Item | Complexity | Depends on |
|---|---|---|
| Draft MIME build + APPEND + replace-old | **M** — new `drafts` sync path; MIME builder shared with compose | compose MIME builder (§1), per-folder sync ✓ |
| `draft_uid` session tracking (compose ↔ Drafts list) | **M** — frontend compose state + backend mapping | MessageList/ReadingPane, SQLite store ✓ |
| Delete-on-send transaction | **S–M** — ordering: SMTP OK → delete draft → APPEND Sent | send flow (§1), outbox ✓ |
| Autosave timer (dirty-check, 30 s) | **S** — UI timer like existing 5-min poll | poll pattern ✓ |
| Offline draft queue | **M** — same tombstone/outbox reuse | backfill/convergence ✓ |

---

## 5. Cross-cutting: table stakes / differentiators / anti-features summary

**Table stakes (ship in milestone):** SMTP send + APPEND-to-Sent; reply/reply-all/forward with correct threading headers + quoting; attachments on send/forward; trash-move delete + undo + empty-trash; drag-drop or Move-to + Copy-to; CREATE/RENAME/DELETE folders with special-folder guards + delimiter/mutf7 handling; draft autosave-replace + edit + delete-on-send; offline queue for all four (reuse flag/outbox + tombstones); own-address identity config (required by reply-all + sent-copy correctness).

**Differentiators (explicitly later):** undo-send window, send-later, signatures/templates, archive shortcut + shortcuts triage, folder colors/pinning/retention, draft conflict banners, contact ranking, send retry backoff UI.

**Anti-features (refuse):** raw-expunge-only delete; permanent-delete-without-confirm; client-only virtual folders; tracking pixels/read-receipts; Sent-folder draft misrouting; autosave-duplication; AI-rewrite in send path.

---

## 6. Suggested build order (dependency-aware)

1. **Move + Trash-delete + Empty Trash** — unlocks everything; exercises UID MOVE/COPYUID fallback, Trash detection, outbox reuse. (Needs: capability probe.)
2. **Folder CREATE/RENAME/DELETE + roles** — Trash from step 1 needs role; delimiter probe here.
3. **SMTP send + APPEND-to-Sent + MIME builder** — standalone vertical slice (new compose only).
4. **Reply/Reply-All/Forward quoting on top of (3)** — pure functions + UI; forward re-attach reuses Phase 4 parse.
5. **Drafts (save/replace/edit/delete-on-send + autosave)** — reuses MIME builder (3) + APPEND/move machinery (1); hardest state tracking, so last.
6. **Offline queue + undo toasts across all** — horizontal hardening over the outbox/tombstone pattern once paths exist.

## 7. Open questions for discuss-phase

- Trash auto-detect vs fixed name on `mail.utfpr.edu.br` (needs live LIST; fallback CREATE `Trash`?).
- Server capabilities: does UTFPR advertise MOVE / UIDPLUS / IDLE? (Probe determines fallback code needed.)
- SMTP auth identity format (bare user vs full address) + Sent auto-save by server or client-APPEND?
- `Junk` handling in scope or deferred? (Affects special-folder set.)
- Quota exposure: does server return QUOTA that compose/APPEND should surface before large sends?
