# Feature Research

**Domain:** Desktop IMAP email client — v1.1 triage & folders (flag sync, multi-folder browsing, refresh, UID backfill)
**Researched:** 2026-10-04
**Confidence:** HIGH (IMAP RFC 3501/9051 behavior + established desktop-client conventions: Thunderbird, eM Client, Apple Mail)

## Feature Landscape

### Table Stakes (Users Expect These)

Features users assume exist in any desktop mail client. Missing these = product feels broken.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Mark read / mark unread with server sync | Every client (Thunderbird, Apple Mail, Outlook) toggles read state and it sticks across devices; users treat read-state as mailbox truth | LOW | IMAP `STORE <seq/uid> ±FLAGS (\Seen)`. Use `UID STORE` so local UID maps 1:1. Optimistic local update + async STORE; on failure, roll back the row and surface error. Opening/reading a message in the reader auto-marks Seen (debatable delay: Thunderbird marks on open or after N sec — pick "on open", keep it simple). Explicit "mark unread" must `UID STORE -FLAGS (\Seen)`. Must never set \Seen during background sync — keep BODY.PEEK discipline from M1 for all fetch paths. |
| Manual refresh (Get Mail / F5 button) | Users distrust auto-sync; every client has an explicit refresh affordance when mail "feels stale" | LOW | Re-run incremental sync for the selected folder (UID-based, same path as poll). Must be cancellable-safe (no duplicate rows on rapid clicks — guard with in-flight flag per folder) and show spinner/disabled state while running. Cheap: reuse the poll sync path with a `manual=true` trigger. |
| Folder list shows real server tree (Sent, Drafts, Trash, custom) | Users expect their server-side folders; synthetic views (Gmail-style All Mail) confuse IMAP users | MEDIUM | `LIST "" "*"` (or `LSUB` fallback) at login + refresh; store `folders(mailbox_path, uidvalidity, uidnext, last_seen_uid)`. Per-folder sync state row — M1's single-INBOX sync state becomes per-folder. Special-use detection via `LIST-EXTENDED`/`SPECIAL-USE` (`\Sent \Drafts \Trash \Junk`) when advertised, else name-heuristic fallback (case-insensitive Sent/Sent Items, Drafts, Trash, Junk/Spam). Folder open = SELECT + incremental sync that folder. |
| Per-folder message list with unread counts | Sidebar badge counts are the primary triage signal in every three-pane client | LOW–MEDIUM | Unread count per folder: `STATUS <mailbox> (UNSEEN)` is cheapest (no SELECT needed) — refresh counts for all subscribed folders on poll tick; exact list sync only for the selected folder. Store `unseen_count` on the folder row. |
| Poll-based auto-refresh on a configurable interval | Thunderbird polls IMAP by default ("Check for new messages every N minutes", default 10); users expect fresh mail without clicking | LOW | Timer in Tauri backend (or frontend interval invoking a Tauri command): default 5–10 min, user-configurable 1–60 min + "manual only" option. Each tick: incremental UID sync of INBOX (and unseen-count STATUS for other folders). Must pause while offline and resume on reconnect; must not run concurrent syncs (serialize per folder). IDLE/push is NOT required for v1.1 — polling is the established baseline (Thunderbird itself polls IMAP; IDLE is a differentiator). |

### Differentiators (Competitive Advantage)

Features that set the product apart. Not required, but valuable.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| IDLE push for instant INBOX delivery | Mail arrives instantly like a messaging app instead of next poll tick; reduces "where is my mail" complaints | MEDIUM | RFC 2177/9051 IDLE: persistent connection on selected INBOX, server pushes EXISTS/RECENT; client then runs incremental sync. Constraints: one IDLE per folder (IDLE only watches the selected mailbox — keep it on INBOX), must re-IDLE every ~15–29 min (servers drop at 20–30 min), needs reconnect/backoff logic, falls back to polling when unsupported. Defer to post-v1.1: poll+manual is the accepted baseline and IDLE adds connection-lifecycle complexity. If added later, keep poll as fallback. |
| UID gap backfill (no silent holes) | Robustness differentiator: most lightweight clients assume contiguous UIDs and silently miss messages when EXPUNGE/fetch races create gaps; guaranteeing completeness is a trust feature | MEDIUM | See dependency notes. Implementation: persist `last_seen_uid` + UIDVALIDITY per folder; on each sync, `UID SEARCH UID <last_seen+1>:*` and compare returned UID set vs local rows — fetch any UID in range missing locally (covers gaps from partial failures, EXPUNGE shifts, concurrent clients). On UIDVALIDITY change: full resync that folder (UIDs invalidated — dump and re-fetch headers). This is the correct scope for v1.1's "no silent gaps" requirement: gap-fill within a valid UIDVALIDITY epoch, full resync on epoch change. |
| Multi-select bulk triage (mark read, flag) | Power-user triage speed: select N messages, one action syncs all | LOW | `UID STORE <uidset> +FLAGS (\Seen)` handles sets natively — one round-trip for bulk. Depends on flag sync working for single messages first. Include in v1.1 only if cheap (it is: same code path, uidset instead of single UID). |
| \Flagged (star) sync | Cheap second flag that Thunderbird/Apple Mail users expect for pinning | LOW | Same STORE path as \Seen with `\Flagged` instead. Defer unless trivial — v1.1 requirement is Seen only; adding Flagged is a one-line extension of the same command builder, so it's a natural stretch goal, not a separate phase. |

### Anti-Features (Commonly Requested, Often Problematic)

Features that seem good but create problems.

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Real-time sync of ALL folders (IDLE per folder / full STATUS storm every tick) | "Everything should always be fresh" | One connection per IDLE folder; STATUS-polling dozens of folders every minute hammers the server and drains battery; university servers (like UTFPR's) may throttle/rate-limit | Poll INBOX content on interval; STATUS unseen-counts for other folders on the same tick (cheap, no SELECT); full list sync only for the folder the user opens |
| Auto-mark-read on preview/selection | Feels "smart" | Users rage when skimming the list marks everything read; destroys the unread triage signal; syncs unwanted \Seen to server affecting phone/other clients | Mark Seen only when the message is actually opened in the reader pane (or explicit user action); provide "mark all as read" per folder as the bulk escape hatch |
| UID-based permanent local identity without UIDVALIDITY guard | "UIDs are stable, why check?" | UIDs are only valid within a UIDVALIDITY epoch; after server-side mailbox rebuild (migration, restore), stale UIDs silently corrupt sync state — the classic "mail disappeared / duplicated" bug | Always persist and check UIDVALIDITY per folder; on mismatch, discard UID state and full-resync (this is RFC-mandated, not optional) |
| Deleting/expunging from client in v1.1 | "Triage means delete" | EXPUNGE semantics (UID vs sequence, UIDPLUS extension variance) plus Trash-model differences across servers are a whole feature area; half-implemented delete loses mail | Keep v1.1 triage = read-state + folders only; delete/move is the next milestone with its own research (Trash semantics, UIDPLUS MOVE, expunge policy) |
| Full-body prefetch for all folders | "Offline everything" | Multiplies storage and initial sync time; M1 deliberately chose headers-first/bodies-on-demand | Keep headers-first per folder; bodies on demand everywhere; full-offline prefetch is a later opt-in setting |

## Feature Dependencies

```
[Folder browsing]
    └──requires──> [Per-folder sync state (uidvalidity, last_seen_uid, unseen_count)]
                       └──requires──> [M1 INBOX sync engine] (generalize, don't rewrite)

[Poll + manual refresh]
    └──requires──> [Incremental UID sync for one folder] (same function, two triggers)
                       └──requires──> [Per-folder sync state]

[UID gap backfill]
    └──requires──> [Incremental UID sync] (gap detection rides on the same UID-range SEARCH)
    └──requires──> [Per-folder sync state] (needs persisted last_seen_uid + UIDVALIDITY)

[Read/unread flag sync]
    └──enhances──> [Message list + reader] (M1 UI exists; add toggle affordances)
    └──requires──> [Local seen-state column writable] (M1 stored flags read-only from sync)

[Bulk triage] ──enhances──> [Read/unread flag sync] (same STORE path, uidset)
[\Flagged sync] ──enhances──> [Read/unread flag sync] (same STORE path, different flag)
[IDLE push] ──enhances──> [Poll + manual refresh] (replaces tick for INBOX; poll stays as fallback)
[Delete/move] ──conflicts──> [v1.1 scope] (deferred milestone; do not mix expunge logic into flag sync)
```

### Dependency Notes

- **Per-folder sync state is the foundation of all v1.1 work:** M1 keeps one sync cursor (INBOX UIDVALIDITY + max UID). v1.1 must promote this to a per-folder table before folder browsing, poll, or backfill can function. Do this first — it's the phase-ordering constraint everything else hangs on.
- **Poll and manual refresh are one code path:** both trigger `sync_folder(selected)`; the only difference is trigger source (timer vs user). Build the sync function once, wire two triggers. Manual refresh must reuse — not duplicate — poll logic.
- **Backfill is not a separate sync mode:** gap detection (`UID SEARCH` range vs local rows, fetch missing) runs inside every incremental sync. There is no "backfill phase" at runtime — it's a property of the sync function. Roadmap implication: don't plan backfill as a separate implementation step from incremental sync; plan it as acceptance criteria on the sync function.
- **Flag sync conflicts with nothing in M1 but ends the read-only invariant:** every fetch path must be audited to keep using BODY.PEEK (a non-PEEK FETCH implicitly sets \Seen server-side and would corrupt read state). This is the highest-risk regression of v1.1 — a fetch-path audit, not new code, is the mitigation.
- **Folder browsing enhances poll:** once per-folder state exists, poll cheaply extends to STATUS unseen-counts for background folders at marginal cost.

## MVP Definition

v1.1 scope is fixed by PROJECT.md (triage & folders). Ruthless cut below is about ordering within v1.1, not scope removal.

### Launch With (v1.1)

- [ ] Per-folder sync state table (mailbox, UIDVALIDITY, last_seen_uid, unseen_count) — foundation; nothing else works without it
- [ ] Read/unread toggle with UID STORE \Seen sync (optimistic UI + rollback) + auto-Seen on reader open — ends read-only era, the headline feature
- [ ] Folder tree from LIST + open/SELECT any folder with headers-first sync — browsing itself
- [ ] Poll timer (default 5–10 min, configurable) + manual refresh button sharing one sync path — freshness
- [ ] UID gap backfill inside incremental sync + UIDVALIDITY-mismatch full resync — no silent gaps
- [ ] BODY.PEEK audit on all fetch paths (regression guard for flag integrity) — prevents the worst v1.1 bug class

### Add After Validation (v1.1.x)

- [ ] Bulk multi-select mark read/unread — trigger: single-message flag sync proven stable against UTFPR server
- [ ] \Flagged (star) sync — trigger: STORE path generalized; nearly free once flag sync ships
- [ ] IDLE push for INBOX with poll fallback — trigger: users complain poll latency matters; needs connection-lifecycle work that doesn't fit v1.1

### Future Consideration (v2+)

- [ ] Delete/move with Trash semantics + UIDPLUS — why defer: separate protocol surface (EXPUNGE, per-server Trash models), deserves its own milestone and research
- [ ] Full-offline body prefetch per folder — why defer: storage/sync-time cost; M1's on-demand model is the right default until users ask
- [ ] \Answered / $Forwarded / custom keyword sync — why defer: compose/reply (which sets \Answered) is itself a later milestone

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Per-folder sync state | HIGH (enabler) | MEDIUM | P1 |
| Read/unread flag sync | HIGH (headline v1.1) | LOW | P1 |
| Folder tree + per-folder browse | HIGH (headline v1.1) | MEDIUM | P1 |
| Poll + manual refresh | HIGH | LOW | P1 |
| UID gap backfill | HIGH (trust) | MEDIUM | P1 |
| BODY.PEEK fetch audit | HIGH (regression guard) | LOW | P1 |
| Bulk multi-select triage | MEDIUM | LOW | P2 |
| \Flagged sync | MEDIUM | LOW | P2 |
| IDLE push | MEDIUM | MEDIUM | P3 |
| Delete/move | HIGH (later) | HIGH | P3 (next milestone) |

**Priority key:**
- P1: Must have for v1.1
- P2: Should have, add when possible (v1.1.x stretch)
- P3: Nice to have, future consideration

## Competitor Feature Analysis

| Feature | Thunderbird | Apple Mail / eM Client | Our Approach (SGE v1.1) |
|---------|-------------|------------------------|-------------------------|
| Flag sync | STORE \Seen on open/toggle, multi-device consistent | Same; auto-Seen on open | Same: UID STORE ±FLAGS (\Seen), Seen on reader open, explicit unread toggle |
| Refresh model | Poll every N min (default 10) + Get Mail button; no IMAP IDLE | IDLE push where supported + poll fallback | Thunderbird model: poll (5–10 min default) + manual refresh; IDLE deferred |
| Folders | Real LIST tree, per-folder sync, STATUS counts | Same | Same: LIST tree, per-folder headers-first sync, STATUS unseen counts |
| Gap robustness | CONDSTORE/QRESYNC (MODSEQ) on supporting servers | Same class of extensions | UID SEARCH range-diff (works on all servers incl. UTFPR's); CONDSTORE/QRESYNC is a later optimization, not v1.1 |
| Bulk triage | Multi-select + mark read/unread | Same | v1.1.x stretch: uidset STORE |

## Sources

- IMAP4rev1 / rev2 semantics (RFC 3501 / RFC 9051): STORE ±FLAGS, UID STORE, UIDVALIDITY epoch rules, BODY.PEEK vs BODY implicit-\Seen
- IDLE extension (RFC 2177): single-mailbox scope, ~30 min session limit, re-IDLE practice (15–29 min), poll fallback
- CONDSTORE/QRESYNC (RFC 4551/5162) as known-later-optimization (not v1.1)
- Client conventions verified via web: Thunderbird polls IMAP (no IDLE) with configurable interval + manual Get Mail; eM Client/Apple Mail hold per-account IDLE with poll fallback; IDLE-per-folder cost and keepalive guidance corroborated across MailBee/Limilabs/vendor docs

---
*Feature research for: SGE v1.1 Triage & Folders*
*Researched: 2026-10-04*
