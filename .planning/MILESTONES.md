# Milestones

## v1.8 LLM-Assisted Replies (In progress — repaired 2026-10-18)

**Phases:** 39 LLM Settings Foundation, 40 Generation Flow, 41 Compose Integration + Hardening/UAT

- LLM settings (OpenAI-compatible base URL + model + prompt template + generation defaults), API key in the OS keyring only, never plaintext; `LlmSettingsError` variants are payload-free so secrets cannot leak into an error by construction.
- Fenced prompt renderer: hidden HTML stripped by a nesting-counted tag scanner, body flattened and truncated at 8000 chars, original message fenced as untrusted data; single-pass positional substitution so a value containing `{subject}` cannot re-expand.
- User-triggered generation from the Reply window: paste-above-quote, review, cancel. No auto-send, no background calls.
- **Repaired 2026-10-18 (`dc7f8c7`):** `generate_reply` hardcoded INBOX UID 1 (prompted about the wrong message); the `RequestTracker` was built per call so single-flight and `cancel_generate` were no-ops. Both fixed; added one retry on transient failure (rate limit / timeout / 5xx), releasing the slot only when still owner.

**Deferred / not proven:** no real endpoint has ever been called with a real key. Offline suite only.

## v1.7 Compose UX + Reply/Reply-All + New Message (Shipped 2026-10-15 — ⚠️ over-claimed, corrected 2026-10-18)

**Phases:** 37 Compose UI (New Message button + window), 38 Reply / Reply-All UI/UX

- **Phase 37 — real:** New Message button + compose window wired toward `queue_send`.
- **Phase 38 — overstated.** The summary claimed working Reply/Reply-All buttons displaying `In-Reply-To`/`References`, and cited a `commands/send.rs` that does not exist. Actual state: display-only stubs, no handler, no UI entry point — reply was unreachable ("o botão nem existe"). Verified against code on 2026-10-18.
- **The real feature was delivered by the 2026-10-18 repair (`dc7f8c7`):** Reply / Reply-All / New Message entry points, `In-Reply-To`/`References` on the wire, `multipart/mixed` attachments (base64, `Content-Disposition: attachment`, RFC 2231 filenames) with drag-and-drop staging.

**Corrected provenance:** reply + threading headers belong to `dc7f8c7`, not to Phase 38. See the provenance note appended to `milestones/v1.7-phases/38-reply-reply-all-ui-ux/38-01-SUMMARY.md`.

**Deferred / not proven:** no real reply has been observed threading in a recipient's client; no attachment has completed a real send.

## v1.4 SMTP Send + Search/Filters + Live Validation + Auto-Archive + Smart Folder Rules (Shipped: 2026-10-13)

**Phases completed:** 6 phases (21–26), 13 plans

**Key accomplishments:**

- SMTP send live gate: recipient-guarded self-send tracer (arrival + verbatim `\Seen` Sent copy + double-enqueue single-delivery) plus failure-mode legs (offline flush backoff, DATA-timeout uncertain reconcile, kill-mid-sending recovery), binary draft-send verdict, five-rule send audit, in-bin full-DB secret scan — offline green, live run pending human via `scripts/live-21.sh` (Phase 21).
- Draft & Seen round-trip live gate: sandbox-scoped one-copy draft proof, replay-order + double-sync resurrection guards, Seen both-directions reconcile, secret scan + prefix audit — 19/19 offline green, live run pending human via `scripts/live-22.sh` (Phase 22).
- SQLite full-text search + saved filters: M17 FTS rebuild (body_text + attachment_names) with BM25+date ranked scoped search (200 cap, redacted snippets), M18 saved_filters + CRUD with backend pt-BR validation, 8 search commands, search-state save/restore across restarts; SearchScopeSelector + SavedFiltersPanel UI, tsc clean (Phase 23).
- Age-based auto-archive: `archive_run` on verbatim batch machinery (kind tag, journal/resume), 90d configurable rule, MOVE-only structural guarantee, ArchivePanel rule editor + manual trigger with kind-aware resume/undo — 472 tests green, scheduler structurally absent by design (Phase 24).
- Smart folder rules: folder_rules M19 + AND matcher over cached text, backend validation, post-sync hook through `confirm_suggestion` (zero new IMAP verbs), first-match conflicts with BOTH-shown dialog, ID-stable taxonomy evolution, journaled bulk apply + RulesPanel — 392 lib green incl. 37 rule tests (Phase 25).
- Live validation bundle: live_gate_26 STEPs 1-15 (v1.1 read-side legs + v1.3 confirm→MOVE→undo/RENAME/DELETE/batch probe + 12/13 citation rows + BUNDLE-REPORT) — bin 15/15, safety 48/48 green, live pending human via `scripts/live-26.sh` (Phase 26).
- Suite: 541 green carried (392 lib + 40 bin + 109 live-gate safety), tsc clean, zero warnings claimed.

**Deferred:** 3 live-server runs (LIVE-01/02/03, standing user_setup — exit-2 SKIP contracts verified, creds file never existed in this env); draft-send STEP expected FAIL BY DESIGN on human run → gap closure; human click-through UAT open (search/filter restore, archive trigger→undo, rules tracer). Node-18 `vite` bundle caveat — retry on Node 20+ host. See `.planning/v1.4-MILESTONE-AUDIT.md`.

## v1.1 Triage & Folders (Shipped: 2026-10-05)

**Phases completed:** 4 phases (6–9), 8 plans, ~24 tasks

**Key accomplishments:**

- Read/unread triage with server-synced Seen flags: optimistic toggle, UID-only STORE through a single-session SessionManager, durable SQLite outbox with RFC 4549 replay, pending-wins reconcile, BODY.PEEK audit (Phase 6).
- Multi-folder client: LIST-discovered folder tree, per-folder sync state + STATUS UNSEEN caching (M3), mailbox-aware commands, unread badges with server-datum fallback, per-folder UIDVALIDITY isolation (Phase 7).
- Freshness without overlap: 5-min poll timer + manual refresh through one shared entry, backend single-flight SyncGate, honest reconnecting state with auto-retry, cooperative cancel (Phase 8).
- No silent gaps: in-pass range-diff re-fetch, 3-strike tombstoning with prune/recovery, double-poll-zero-FETCH convergence with periodic full sweep (Phase 9).
- Suite: 102 backend tests green (63 → 102 across the milestone), tsc + vite build clean.

**Deferred:** 6 live-validation items (Seen round-trip, offline replay, folder tree/​badges vs webmail, poll arrival, reconnect recovery) — resume with `/gsd-verify-work N`. See `.planning/v1.1-MILESTONE-AUDIT.md`.

## v1.0 Read-only Viewer (Shipped: 2026-10-03)

Gmail-like three-pane INBOX viewer: scaffold + connection, sync engine + local store, mailbox UI + search, reader + attachments, keyring + Linux bundle.
