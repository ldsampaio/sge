---
gsd_state_version: "1.0"
milestone: v1.9
milestone_name: Email Threading & Interface Revamp
status: planning
last_updated: "2026-10-20T03:13:16.330Z"
last_activity: 2026-10-20
progress:
  total_phases: 0
  completed_phases: 0
  total_plans: 0
  completed_plans: 0
  percent: 0
---

# SGE — State (v1.8 LLM-Assisted Replies)

**Milestone:** v1.8 LLM-Assisted Replies (roadmap: phases 39–41, written 2026-10-15)
**Status:** Phases 39–40 executed; a post-hoc repair pass on 2026-10-18 fixed the reply/send/LLM wiring. Awaiting human UAT.
**Prev milestone:** v1.7 (phases 37–38: New Message button + Compose window — delivered, but the Reply/Reply-All half was **stub-only** and is credited to the 2026-10-18 repair, not to Phase 38)

## Current Position

Phase: Not started (defining requirements)
Plan: —
Status: Defining requirements
Last activity: 2026-10-20 — Milestone v1.9 started

## Repair Pass (2026-10-18) — why it was needed

The user reported "o botão nem existe" when trying to reply. Auditing the chain against the roadmap found **four independent breaks**, all now fixed:

1. **No UI entry point.** No button anywhere reached `open_compose_window` or `queue_send` — New Message and Reply were unreachable. `DraftEditor.tsx` held a non-functional Reply/Reply-All stub pair (see the provenance correction appended to `milestones/v1.7-phases/38-reply-reply-all-ui-ux/38-01-SUMMARY.md`).
2. **`generate_reply` ignored the opened message.** It hardcoded INBOX UID 1, so it always prompted about the wrong mail (or refused).
3. **Single-flight and cancel were no-ops.** The `RequestTracker` was constructed per call, so nothing shared state with `cancel_generate`.
4. **`queue_send` dropped threading headers and attachments.** Tauri silently ignores unknown invoke args, so `in_reply_to`/`references`/attachments never reached the renderer — replies were not threaded and attachments could not be sent.

Delivered in `dc7f8c7`: real **Responder** / **Responder a todos** buttons (`ReadingPane`) and **Nova mensagem** (`MailboxView`); `fetch_message` now returns `cc_addrs` + `message_id` from the BODY.PEEK cache; `queue_send` accepts threading headers + attachments; `render_draft_rfc5322` emits `In-Reply-To`/`References` and `multipart/mixed` (base64, `Content-Disposition: attachment`, RFC 2231 `filename*` for non-ASCII); drag-and-drop attachment staging in `ComposeWindow`; `llm_tracker` moved onto `AppState`; one retry on transient LLM failures.

## Roadmap Summary

- Phase 39: LLM Settings Foundation (LLM-01..04) — store + validation + prompt render — **executed**
- Phase 40: Generation Flow (LLM-05, LLM-07, LLM-09) — client + IPC chokepoint — **executed**
- Phase 41: Compose Integration + Privacy Audit (LLM-06 Ollama preset [UAT passed], LLM-08 threading verified, LLM-10 regenerate [UAT pending], LLM-11 privacy guardrails) — awaiting UAT

## Next

- **[Human UAT]** Open the app in a VM with your account; click Répondre on an actual INBOX message and paste `http://localhost:3000` to the LLM settings (Ollama test); confirm the reply threads correctly, send with a non-ASCII-named attachment.
- **[LLM-10 Regenerate]** Edit the generated draft, click Générer novamente to replace with a new result while preserving threading headers.
- **[Cancel-mid-generation]** Start generation and stop interaction; confirm the body stays editable, no phantom drafts appear in INBOX/Sent, cleanup is immediate.
- **[Ship after UAT]** Tag v1.7, update CHANGELOG, ship.

## Decisions (accumulated)

- 39-01: `LlmSettingsError` variants are payload-free unit variants — secrets cannot be interpolated into errors by construction (T-39-01)
- 39-01: `DEFAULT_TEMPLATE` single-line literal shared byte-identically with the M23 seed; drift guarded by `seed_defaults_equal_consts` test
- 39-01: `pub mod llm;` one-line declaration in lib.rs required for compilation (plan's don't-touch-lib.rs honored otherwise); SCHEMA_VERSION fix deferred to 39-03
- 39-02: `LlmKeyStore` trait injection (keyring + memory impls) with fail-closed presence-bool; RESEARCH regex unusable (no backrefs) so hidden-strip is a nesting-counted tag scanner; substitution is single-pass positional (chained replace re-expands values)
- **2026-10-18:** reply threading reads the local cache (`messages.message_id`, `cc_addrs`) instead of issuing new IMAP header fetches — no new BODY.PEEK request, no `references` column (absent `References`, `buildReplySeed` degrades to `References = In-Reply-To`, which still threads correctly)
- **2026-10-18:** the LLM single-flight tracker is shared state on `AppState`, never per-call — a per-call tracker makes both single-flight and `cancel_generate` no-ops
- **2026-10-18:** attachment pre-check is 18 MiB decoded against the backend's 25 MiB *rendered* cap (base64 inflates 4/3); the backend stays the authority via `send-too-large` / `send-bad-attachment`
