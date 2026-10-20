# Feature Research — Conversation Threading UI + Interface Revamp (SGE v1.9)

**Domain:** Desktop mail client (Gmail-like three-pane, pt-BR, React 19 + plain CSS tokens, Tauri v2, bodies fetched per-message)
**Researched:** 2026-10-20
**Confidence:** MEDIUM overall — two sources fetched live and verified (Gmail Help Center official; W3C APG Listbox pattern). Client-pattern claims (Thunderbird, Geary, K-9, Evolution) come from stable, years-old, well-known behavior; live fetches of their KBs were blocked in this environment (bot-protection / 404 / timeouts), so they are tagged MEDIUM/LOW where the details are specific.

**Scope note:** every item below is a NEW v1.9 feature (backend thread lane + grouped rows + reader stack + state/keyboard revamp). Existing flows (FTS search, drafts, compose windows, undo toast, pagination 50/pg, category badges, sync status) are preserved-by-default, not re-listed as features.

---

## Existing Airfield (what the code already supports vs what it must grow)

Explicitly answer "does the existing data/commands support it?" per the downstream consumer's request:

| Capability needed by v1.9 | Already supported today? | Gap |
|---|---|---|
| Grouping messages | No — `messages` has no `references`/`in_reply_to` columns; sync lane never parses them | **Blocked on approved lane:** M26 migration (`references_json`), ENVELOPE `in_reply_to` extraction ( arrives in existing FETCH_ATTRS response), `HEADER.FIELDS (REFERENCES)` lane for new syncs, backend-computed `thread_id` |
| Thread payload for the list | Partial — `list_messages` returns flat `MessageRow[]` only | Extended payload: rows must carry `thread_id`, `thread_count`, `thread_unread`; per-member summaries the READER needs (uid, from, date, flags, preview) must come from a new thread fetch |
| Open thread and show member messages | Partial — `fetch_message(uid)` per member works as-is (BODY.PEEK contract) | Reader needs one thread-summary invoke first; member bodies still lazy per expanded message |
| Per-message unread toggle | Yes — `set_seen`, optimistic FLAG_UPDATE_EVENT wash | None — reuse verbatim |
| Read-on-open | Yes — Thunderbird-style Seen-on-open in ReadingPane | Extend to "on expanded/viewed" in the stack |
| Delete/move one message | Yes — `delete_message`/`move_message` (uid) | Conversation scope = N calls or new batch command |
| Batch delete | Yes — `expunge_messages(uids)` accepts a list (`expunge_messages` precedent) | Batch **move** and batch `set_seen` do not exist — need loop or new commands |
| Undo batch op | Partial — `undo_queued_op(uid, mailbox)` is single-uid | Conversation delete→undo = N undos or one op keyed per member set |
| Pagination | Existing, message-keyed | Conversation scope requires backend thread-keyed COUNT/OFFSET (a conversation row is not a message row) |
| HTML body render | Yes — sandboxed empty iframe per message | N iframes per open conversation is the new perf edge — lazy-create on expand |
| Refresh without duplication | Yes — `refreshKey` bump re-invoke; keys are `(mailbox, uid)` | Thread rows keyed on `thread_id` inherit the same property once grouping is backend-computed |
| Any UI state tokens/states | Ad-hoc — inline hex colors in ClassifyBadges/SuggestionBar paths, hover CSS mixed with inline `style` | Revamp lane: named tokens, shared primitives (Button/Dialog/Empty/Loading), standardized interaction states |

---

## Feature Landscape

### A. Conversation list rows

### Table Stakes (Users Expect These)

| Feature | Why Expected | Complexity | Notes & Dependencies |
|---------|--------------|------------|-----------------------|
| Related messages grouped into one row | Every mainstream client (Gmail, Outlook, TB, K-9) groups replies; SGE already SENDS threaded replies (`In-Reply-To`/`References` wired in repair pass) but can't display them — this asymmetry is the visible bug v1.9 fixes | MEDIUM | Entirely downstream of the backend thread lane (`thread_id`). All grouping logic lives backend-side per the approved decision; client never group-subject-matches |
| Count badge (number of messages, shown only when ≥2) | Gmail/Thunderbird/K-9 all show it; without it a 7-message thread is indistinguishable from 1 | LOW | Payload needs `thread_count`; badge suppressed at 1 (single-message threads render EXACTLY like today's rows — no badge, no layout shift) |
| Aggregate unread on the row (dot bolds, "N novas") | Unread triage is the list's core job; per-member unread is invisible while collapsed | LOW | Compute client-side from per-member flags carried in the payload — no new command |
| Senders displayed, deduplicated, truncated ("A · B +2") | Recipient noise drowns senders in real reply-all traffic | MEDIUM | Needs participant list in payload; parse name/addr per member; pt-BR copy for the +N suffix |
| Sender→recipient flip when the user is the author ("Eu" / "Para: …") | In Sent folders and in threads you've also replied to, showing your own name tells nothing; Gmail's pattern | MEDIUM | Needs "me" identity (already in settings/keyring) + direction knowledge per member; LOW priority only because SGE is single-hero user |
| Snippet from the LATEST message (not the thread root) | Snippet of the first reply reads stale/wrong when the thread is active | LOW | Backend already stores per-message preview; payload needs latest-message preview specifically — and today list doors only carry the row's own preview |
| Subject without Re:/header normalization, "(sem assunto)" fallback | "Re: Re: Re: Aviso" is noise; already the list's existing subject path | LOW | Minimal `Re:`-prefix trim client-side; empty-subject fallback already exists |
| Latest date on the row, right-aligned | Ordering key for triage | LOW | New field from member max; `formatRowDate` reused |
| Refresh keeps threads coherent (no dupes, no dedupe flicker) | New replies must merge INTO the existing conversation row, not spawn a second row | LOW | Free once `thread_id` is backend-stable and rows key on it — the existing guarded refresh path already covers this |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| "N não lidas" split from the count badge | Unread-count-in-thread is a real Gmail/ProtonMail pattern; lets the user decide whether to open | LOW | Same payload; two chips instead of one |
| Hover keeps existing row actions (delete/move/undo) working on the entire conversation | Preserves learned muscle memory, no flow regression | MEDIUM | Per-message vs per-convention split — anti-feature below governs the default |
| Participant avatars in the row (initial-circle cluster) | Already have per-row avatar code; clustering further differentiates M individuals | LOW | Reuses `avatarInitial`; visual only |
| Thread chips rendered candidate inline (badge on attach + unread count on the same right side) | Keeps existing attachment icon and inbox-chip behavior two-line lists near threads | LOW | Reuses CategoryBadge; low value, cheap |

### Anti-Features

| Feature | Why Seems Good | Why Problematic | Alternative |
|---------|----------------|-----------------|-------------|
| Virtualized infinite scroll on the list | "Inbox is long" | 50/page pagination already bound both fetch cost and DOM size; virtualization introduces a fixed-size recycler + a11y tradeoff (APG measured-set requirements) into one flat list. Zero measured benefit. MEDIUM-L refit cost for a non-problem | Keep `PAGE_SIZE = 50` pager; the revamp only restyles rows |
| Gmail's >100-messages split-into-new-accounts | Gmail does it | Carries complexity the backend would need (counting edge in `thread_id` assignment) for personal-mailbox volumes that never hit 100 | Leave cap unimplemented; document as future-safe |
| Client-side subject-similarity grouping as a fallback path | "Thread lock if headers missing" | Subject grouping is documented-unreliable (false joins on `Re:<same subject>`, the very reason the References lane was chosen); a fallback reintroduces the bug the design rejected | Single-message thread until its parent is fetched — honest, user-visible behavior |
| Drag reordering / manual pinning of threads | Data management feel | IMAP pages everywhere, ordering is date-keyed by design; retention has no UI to persist custom order | Keep the established newest-first list order |
| Inline expand-all / collapse-all on the LIST | Mails in a thread are multiple, so expand rows in place is discoverable — K-9's model | In-place list expansion forces nested focus inside `role="option"` rows; APG listbox does not support interactive elements INSIDE the option surface (current code already has this hazard with nested unread-dot <button> in <button>) — nesting more controls compounds it | The reader stack is the expansion surface; the list stays one-row-per-thread |

---

### B. Conversation reader

### Table Stakes

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Chronological message stack (oldest at top, newest at bottom) | Gmail Help Center states it verbatim ("latest email at the bottom"); every conversation-view client does oldest→newest | LOW | Reader becomes a `MessageView[]` column; the current single-message rendering becomes the per-member card body |
| Per-message collapse/expand of the members | Collapsed history + focused latest is how Thunderbird Conversations/Geary/Outlook.com all do the reader | MEDIUM | Header-args come from the thread-summary payload; bodies lazy-fetch via existing `fetch_message` per EXPAND (never BLOCK flow) |
| Sensible expansion defaults: latest and first-unread expanded, history collapsed | Opening a 20-message thread must not auto-open 20 bodies (fetch cost, i-frame FTE); unexpanded history until UNREAD messages is the user's actual question | MEDIUM | Default rule proposed: messages expanded (in order) up to and including the first-unread; if no unread, only the last message. Matches TB Conversations "focus the last read" behavior |
| Read-on-expand (per message) | Read semantics are per-message, not per-thread, in Thunderbird/Geary; SGE's set_seen is already per-message | LOW | Fire existing `set_seen` + FLAG_UPDATE_EVENT when an expanded card's body is (fully) actually rendered — same seen-on-open logic reused |
| Per-message Reply / Reply-All | Users reply to the exact message, mid-thread — SGE COMPOSE already carries threading headers | LOW | Move existing reader's `handleReply` into the per-card action row (which keeps `openComposeWindow` seeding from that card's uid) |
| Per-message mark unread | Core triage affordance preserved | LOW | Reuses reader toggle + list FLAG_UPDATE_EVENT listener |
| Quoted text shown as-is inside the card | The inline `> quoted` lines are already how upstream mail arrives (duplicate MST quotes) | LOW | No trimming, minimal differentiator trimmed below |
| Long threads paginate or cap inside the reader naturally: "Mostrar N mensagens anteriores" is a thin expansion button that fetches headers lazily. | A 40-message stack of IFrames-expanded headers degrades scroll and render | LOW | Header-only list from the payload, fold into a disclosure above the first expanded card |
| Loading/error/empty/refresh states preserved per-card, not per-conversation | Whole-panel replacement flickers on background refresh; per-card deferred states keep position | LOW | Bootstrap from the reader's existing status machine; each card's body section independent (skeleton / retry button / empty-state) |

### Differentiators

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Smooth reliable auto-scroll on open to the first-unread / last expanded card | Geary's break-through UX; saves the scroll-to-find repetition. (Smooth-scroll `prefers-reduced-motion` respected) | MEDIUM | Ref/anchor measurement after the stack renders; must be test-stable |
| Quoted-text trimmer for plain-text bodies (collapse the inline quote blob) | The #1 body-noise pattern for triage emails; TB Conversations did this | MEDIUM-HIGH | Heuristic (`line begins with >`, `"On <date> X wrote:"` split) — fragile against non-quoted formats; plain-text ONLY (HTML quotes stay un-guided), default ON with per-card retry |
| Thread participants line in the reader header (To + other participants) | In user-controlled context for "who else seen this whole conversation" | LOW | Pure payload rendering |
| Full-thread mark-read when reaching the end | matches Gmail mental model once user scrolls to the end | LOW | IntersectionObserver on the newest card; optional |

### Anti-Features

| Feature | Why Seems Good | Why Problematic | Alternative |
|---------|-----------------|-----------------|-------------|
| All-expanded-by-default reader for multi-message threads | "No click to read" | Auto-fetches EVERY member body per open and instantiates one sandboxed set of Niframes; heavy IO on 4G/unsynced bodies, and documents the collapse-management problem v1.9 exists to keep literal | Default: latest(+ first-unread-range) expanded; history collapsed to headers |
| Gmail's title-level threading (message only in reader) + hiding per-message seen — "whole conversation is one read" model | Familiar Gmail shape | Conflicts directly with SGE's per-message set_seen and per-message UID contract; wrong model when bodies are fetched on demand, and undo "mark-read-conversation" churns more server flags than expand-as-read | Keep per-message read semantics (Thunderbird/Geary model) |
| In-card rich-text editing / reply box inline in the stack | Faster reply | The compose window is an established mode with attachment staging, drafts, undo, LLM Generate — duplicating the flow inline duplicates every guarantee | Existing `openComposeWindow("reply"/"reply-all")`, seeded from that card |
| Email thread headers visualized as a tree/timeline (jwz "references graph") | Interesting look | No reference data quality to draw from (References chain often sparse), and the vertical chronology is the known-good visual | Flat chronology only |
| "Load all bodies" button in the reader |就地 Offline reading | User-triggered fetch-a-flood — optimization that would change the BODY.PEEK contrast outside the approved lane (bodies are fetched per-message on demand in the current always) | Open and read cards one at a time; that's the lane scope |

---

### C. Per-message vs per-conversation actions

### Table Stakes

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Per-message actions inside the stack (reply, reply-all, delete, move, mark unread, classify) | Relevant actions differ by message in a conversation | LOW | All six already exist for the single-message reader; re-homing into per-card rows |
| Delete conversation (whole thread → Trash) from list hover + reader header | Core triage action scales its scope with the grouping major break | MEDIUM | Client loops existing `delete_message(uid)` per member TODAY (feasible inside lane); better = one new batch command that reuses `delete_message`'s validation copy; undo speculative — needs rework |
| Move conversation | Same scope-value | MEDIUM | Reuses the existing MoveMenu; same batch shape as conversation delete |
| Mark-all-read conversation ("outra linha") | The list unread-version anchors to a thread | LOW | Batch `set_seen` loop over members, existing optimistic wash per uid |

### Differentiators

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Conversation bulk undo entry (one "Desfazer" toast that restores the whole thread) | Batch delete without undo is exactly the trap v1.2's restore flow was built to avoid | MEDIUM | `undo_queued_op` is uid-shaped — extend to accept the member list at once (backend contract expansion needed — NOT in the currently-approved lane; re-confirm) |
| Conversation chip labor safety: batch ops confirm before N messages are affected | expunge already has a confirm-gate modal; scale it to "apagar N mensagens" | LOW | Reuses ExpungeModal pattern — this is the antifragile move |

### Anti-Features

| Feature | Why Seems Good | Why Problematic | Alternative |
|---------|-----------------|-----------------|-------------|
| Multi-select toolbar (checkboxes, select-all, shift-selection) | Gmail parity深层 | No checkbox precedent anywhere in the rows today; listbox APG redirects gear-pattern multi-select to grids; the revamp already has 3 lanes of work. Not one-row-of-scope creep | Stay single-thread-row action scope; revisit if user-arrival |
| Per-conversation Send/Reply composition (compose once to all-thread participants) | One window per message → huge | Reuse the REPLY-ALL compose built in v1.7; thread-scoped restatement adds nothing | Reply to the specific card you mean |
| Undo as batch server-side transaction | "One command" | IMAP flag-store has no transaction; a cluster of UID STOREs is the protocol reality, that's fine. Bull async work | Do batch = N real STORE calls, undo = re-enqueue the inverse N (both existing pathways) |

---

### D. State discipline + keyboard/accessibility revamp

### Table Stakes (confidence MEDIUM — W3C APG fetched; the state-matrix itself is unrefetchable in this env)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Named, token-driven interaction states on every touched surface (default/hover/focus-visible/active/selected/disabled + loading/empty/error/sync-wash) | Today: inline hex colors (`#4ade80`, `#94a3b8`) and inline conditional `style`; hover lives only in CSS; severity labels drift between surfaces | MEDIUM | Token layer extension (`--color-state-*`), every touched component declares states via classes, no inline conditional styling; part of the "revamp sweep" |
| Typography fixed ON-disk: embed Baloo 2 / Nunito woff2 locally (no CDN) and a normalized fallback stack | Decided (Key Decisions 2026-10-20) — tokens reference fonts that don't ship — every element renders in the system fallback today | LOW | OFL-licensed woff2 vendored; `@font-face` + fallback normalization |
| Correct Listbox semantics: NO interactive element nested INSIDE the `role="option"` surface | APG explicit: the listbox interaction model does not support controls inside an option; today the unread-dot is a `<button>` nested inside the row `<button>` (invalid HTML + broken roving semantics) | MEDIUM | Move the row to `role="option"` div with the whole-row activation, keep the unread-dot + hover actions OUTSIDE the option's activation surface OR move the row to the Grid pattern; must fix, not walk around |
| Keyboard navigation: Up/Down Arrow move focus between row options, Home/End, Enter opens, selection is distinct from focus, focus lands on the runtime-selected row | APG listbox keyboard contract; the pager and row buttons are currently the only keyboard-points | MEDIUM | Roving index maintained in the list component; Home/End powered by a `rowListRef`; validation-fitted to existing `role="listbox" aria-selected` markup |
| `:focus-visible`-only focus rings (keyboard-only), no ring on mouse click | WCAG-visible + user-experience standard; today focus styling is indistinguishable between click and Tab | LOW | CSS-only: `:focus-visible` selectors per token family; regression-testable via classes |
| Loading as skeleton / reserved region (not layout-shifting spinner-on-top), error + retry as the established `.btn`, `role="alert"` obligations | Consistency and layout-shift avoidance are the revamp's raison d'être | MEDIUM | New `EmptyState`/`LoadingState`/`ErrorState` shared primitives; ALL list/reader/sidebar/modal paths consume them |
| Buttons consolidated to one `Button` primitive (primary/ghost/destructive variants, size = touch target at least) | Today `className` Variant CSS strings appear in five uncoordinated places | LOW | New `Button.tsx`; `btn/*` variant Classes as a public surface |
| Modal/Dialog shared primitive (Focus-trap + Esc, currently per-modal bespoke) | ExpungeModal, FolderDialog, FolderDeleteModal re-implement to three different closing/focusing behaviors | MEDIUM | One Dialog primitive consumed by all three; must preserve ALL existing backend contract copy (backend rejects carry UI text, e.g. `need_typed_confirm`) |
| `prefers-reduced-motion` honored (auto-scroll, wash-a bug alert) | Accessibility standard that's invisible in testing until violated | LOW | CSS media query + skip smooth-scroll when asserted |

### Differentiators

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Type-ahead jump-to-Sender on the list | APG-recommended for lists > 7 items; partially replaces search for a friend's mail | LOW-MEDIUM | Boundary: generic flat-mail tail-of-list; skip |
| Component catalog eslint ban (no new inline `style={{color:*}}` in touched files) | Token regression prevention over new visuals | LOW | Lint rule + code-review check; keeps the surfaces clean AFTER the revamp, otherwise the sweep decays |
| Reader stack i-frame on-demand with analytics-free height stabilization | Every HTML message is one sandboxed empty iframe; N iframes at once is fine per-page for 1; the conversation reader makes it N — lazy-mount keeps the fast path | MEDIUM | Mount body only when the card is expanded; keep placeholder hero + fallback |

### Anti-Features

| Feature | Why Seems Good | Why Problematic | Alternative |
|---------|-----------------|-----------------|-------------|
| Dark theme | "everyone wants it" | Explicitly out of v1.9 scope (PROJECT.md scope guard); token revamp is LIGHT-theme-only — tearing everything into a dual-theme plan doubles QA | Ship-light-theme-authoritative clean tokens; the work deliberately makes dark theme CHEAP LATER |
| Star / \Flagged actions | Mail client tradition | Explicitly out of v1.9 scope; undeveloped backend flag plumbing | Defer to a future milestone alongside the smart-folder rules |
| Full legacy-panel rewrites (sidebar, settings, compose window) while touching shared primitives | "New primitives mean rename everything" | Explicitly out-of-scope (visual rewrite of untouched surfaces) + UAT blast radius; the v1.8 closeout already deferred enough | Use primitives ONLY in touched surfaces; legacy panels adopt them opportunistically later |
| New UI dependencies (Radix, Headless UI, DND kit, etc.) | Design-system shortcuts | Explicitly out-of-scope (`no new UI dependencies`); hand-rolled primitives stay small by discipline | Plain JSX + CSS tokens |
| aria-activedescendant basework for the list in the SAME phase as row revamp | "SSR-safe fancy focus" | Two focus models migrating at once — pick roving tab-index first, cancel activedescendant to a documented future | Roving focus now |
| `aria-live` chatter on every state transition ("3 mensagens apagadas · carregando · focado") | Broadcast empathy | Screen-reader delay eating >simplest state every IPC shake | Targeted `aria-live="polite"` on the established list-state + undo toast only — the code already does this, PRESERVE it |

## Feature Dependencies

```
[Backend thread lane: M26 + references lane + thread_id (APPROVED)]
        └──requires-by-all-meaning──> [Grouped list rows (badge/unread/send)]
        └──requires──> [Thread-summary payload: per-member bodies for the reader]
                    └──requires──> [Reader message stack (chronology + per-card expand)]
                                        └──requires──> [Per-message re-homed actions]
                                        └──requires──> [per-card lazy iframe mount]
        └──requires──> [Conversation batch actions (delete/move/mark-read)]
                    └──requires──> [Batch command or member-loop + undo batch shape]

[Token/primitive foundation: Button, Dialog, Empty/Loading/Error, states, woff2 fonts]
        └──enhances──> [Everyone: rows, cards, modals, pager, badges]
        └──conflicts──> [adopting new UI dependency kits] (explicitly banned)

[Keyboard navigation: roving arrows + :focus-visible]
        └──requires-by-meaning──> [Listbox semantics fix (no nested buttons)]
        └──requires──> [row-state tokens (focus-visible ring definitions)]

[Refresh without duplication] ──enhances──> [Grouped list rows]  (free: key on thread_id via existing guarded refresh)
```

### Dependency Notes

- **Every display feature requires the backend lane:** `thread_id` + `references_json` is the approved user decision (subject-grouping explicitly rejected as unreliable). No list or reader feature can ship without it.
- **Reader needs a thread-summary payload, not only thread_id:** the two existing conversation APIs (`fetch_message`, single) and `list_messages` payload (per-row) do not carry *members* — a new thread fetch returning one row per member (uid/from/date/flags/preview) is the minimal expansion within the lane's spirit; flag it at review for explicit approval.
- **Per-conversation delete/move need their batched undo shape reworked:** `undo_queued_op(uid)` is single-member — expansion IS a backend change; must be confirmed in plan review, not waved through.
- **The state-revamp lane has no backend dependency** and can proceed in parallel once tokens land — it is the only lane that never blocks on M26.
- **Keyboard navigation and listbox semantics conflict with the CURRENT DOM shape:** the nested-button-in-button row must be fixed before/with arrow roving, not after — otherwise the revamp ships with a known-broken semantic surface.

## MVP Definition — prioritization matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Backend thread lane (M26, references, thread_id) | HIGH | HIGH | **P1** |
| Grouped list rows (badge, agg unread, senders, latest date/snippet) | HIGH | MEDIUM | **P1** |
| Reader stack: chronology + per-message expand + seen-on-expand + lazy bodies | HIGH | MEDIUM-HIGH | **P1** |
| Per-message actions re-homed to cards; reply/reply-all preserved | HIGH | LOW | **P1** |
| Conversation batch actions (delete/move/mark-all-read) | HIGH | MEDIUM | **P2** |
| Batch undo shape for conversations | MEDIUM | MEDIUM (backend contract) | **P2** — confirm scope at plan review |
| Token + primitive foundation (fonts, Button/Dialog/Empty/Loading, states) | HIGH (enabler) | MEDIUM | **P1** (parallel lane) |
| Listbox semantics fix + keyboard roving + :focus-visible | HIGH (a11y) | MEDIUM | **P2** — must precede grouped-row shipping |
| Auto-scroll to first unread | MEDIUM | MEDIUM | **P3** |
| Quoted-text trimmer (plain text) | MEDIUM | MEDIUM-HIGH fragile | **P3** |
| "N não lidas" chip + avatar cluster + participants line | LOW-MEDIUM | LOW | **P3** |
| In-reader "Mostrar N anteriores" long-thread fold | LOW | LOW | **P3** |
| Type-ahead on the list | LOW | LOW-MEDIUM | **P3** (defer unless trivial) |

### Definition of "threading done" (MVP gate)

- [ ] A real References-linked pair on the UTFPR mail server shows as ONE row with count badge, aggregate unread, latest-date/snippet
- [ ] Single-message threads render byte-compatible with today's row (no badge, no regression)
- [ ] Opening a 3+ message thread shows the stack; unread expanded and only those bodies fetched
- [ ] Reply/Reply-All from a specific card keeps threading headers (v1.7 contract unchanged)
- [ ] Delete/move/mark-read on the whole conversation works with one undo entry
- [ ] Refresh/reconnect never duplicates a conversation row
- [ ] Keyboard: arrows walk the list, Enter opens, focus rings only on keyboard

## Competitor Feature Analysis

| Feature | Gmail (web) | Thunderbird (TB Conversations / Threaded View) | Geary | K-9 | SGE v1.9 Approach |
|---------|-------------|------------------------------------------------|-------|-----|-------------------|
| Grouping model | Backend (server-computed subjects+reference headers, subject-split, 100-cap) | Threaded View = IMAP tree display; TB Conversations add-on = client-computed References | Client References grouping | Optional client "Group messages" | **Backend-computed `thread_id`** (References-based; mandated) |
| List-row expansion | None (row opens the reader) | Threaded view expands in place | None | List expands in place (its signature difference) | None — reader stack is the expansion surface (anti-feature above) |
| Reader default expansion | All expanded; latest at bottom | TB Conversations: latest expanded, old collapsed | Last-read focused, earlier collapsed | N/A (list-expansion model) | **Latest + first-unread-range expanded, history collapsed** (bodies lazy) |
| Read semantics | Whole-conversation read on open | Per-message read on open | Per-message | Per-message | **Per-message seen on expand** (fits BODY.PEEK contract) |
| Sender display | All participants, "Eu" for self | Senders; Twisted+ alias ellipsis | Participants line | Senders | Senders, deduped, "+N"; self-flip P3 |
| Keyboard | j/k arrows + Enter (web) | Interviews + arrows (focus) | Arrows | Arrows | Roving arrow nav per APG, no j/k mnemonics v1.9 |
| Unread aggregation | Thread-unread bold + count | Per-thread unread count | Per-thread | Per-thread | Client-side aggregate from member flags |

## Sources

- **Gmail Help Center — "Group emails into conversations"** (support.google.com/mail/answer/5900) — fetched live 2026-10-20. Key content: latest message at bottom, subject-change split, 100-message cap, reference-headers + recipient/subject + 7-day automated grouping. Confidence HIGH (venue official; content brief).
- **W3C ARIA Authoring Practices Guide — Listbox pattern** (w3.org/WAI/ARIA/apg/patterns/listbox/) — fetched live 2026-10-20. Key content: arrow/Home/End/type-ahead keyboard contract, focus-vs-selection distinction, aria-activedescendant alternative, nested-interactive-in-option prohibition.  Confidence MEDIUM (authoritative, etiquette of the row→grid migration mirrors as actionable rules).
- **Thunderbird Conversation View / Thunderbird Conversations add-on behavior** — stable training knowledge; live KB fetches blocked by cloud protectors in this env (Anubis / DRM 403/406 wall). Quoted descriptions current as of years-old engines. Confidence MEDIUM.
- **Geary handbook** — GNOME pages unreachables (406/transport). Content from training knowledge of the stable reader contract (chronology ascending, collapsed history). Confidence LOW-MEDIUM.
- **K-9 Mail threading** — documentation hosts unverifiable in this env (404s). Behavior from training knowledge: opt-in list expansion + thread count badge + per-thread unread. Confidence MEDIUM.
- **jwz threading ("threading luck"); jwz.org/doc/threading.html** — timed out (this is a web-unstable source). Content quoted from canonical web knowledge supporting the decision-albeit evidence rationale: References-chain primary, subject fallback unreliable (jwz's critique). Confidence MEDIUM, credibility VERY HIGH historically.
- **Existing codebase audit** (first-hand reads, 2026-10-20): `MessageList.tsx` (row anatomy, pagination, undo wash, nested-button hazard), `ReadingPane.tsx` (fetch-on-select, seen-on-open, action rows, suggestion bar, iframe sanitization), `MailboxView.tsx` (pane state flow, refreshKey contract, dialog patterns), `.planning/PROJECT.md` v1.9 scope + Key Decisions. Confidence HIGH (direct source).

---
*Feature research for: SGE v1.9 — conversation threading + interface revamp*
*Researched: 2026-10-20*
