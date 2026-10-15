# Feature Research: LLM-Assisted Replies (v1.8)

**Domain:** User-triggered LLM reply drafting in a desktop mail client
**Researched:** 2026-10-15
**Confidence:** HIGH (closest analog Thunderbird + ThunderAI/AI-Mate verified via web; generation UX patterns cross-checked across Superhuman/Shortwave/Missive; OpenAI-compatible API shape is stable standard; SGE-side dependencies read directly from `src/components/ComposeWindow.tsx`)

## Feature Landscape

### Table Stakes (Users Expect These)

Features users assume exist once a "Generate with AI" button is offered. Missing these = feature feels broken or untrustworthy.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| LLM settings section (base URL + model + API key) | Every AI-mail integration (ThunderAI, AI-Mate, Outlook add-ins) asks for endpoint + key + model; users expect BYOK, not a bundled vendor | LOW | Single settings form; base URL free-text covers OpenAI + Ollama/LM Studio/OpenRouter without per-provider code. Validate as URL on save. Test-connection button is the established pattern (ThunderAI has it) — one `models` or minimal chat call on demand. |
| API key in OS keyring, never plaintext | SGE already stores mail creds in keyring (Phase 5 pattern); users on Linux expect secrets in Secret Service, and PROJECT.md mandates it | LOW | Reuse existing keyring save/load helpers with a second service/account entry (e.g. `sge/llm-api-key`). Never log key; mask in UI (`••••`). Same seam as mail creds, not a new mechanism. |
| Single reply prompt template with placeholders | ThunderAI's core value is custom prompts with mail variables; v1.8 scope pins `{sender}`/`{subject}`/`{body}` | LOW | Store template as plain string in settings (SQLite or config file, not keyring — it is not secret). Render by string substitution before the API call. Provide a sane default template so the button works out of the box; reset-to-default affordance expected. |
| Generation defaults (temperature / max tokens / timeout) | OpenAI-compatible `chat/completions` takes these per call; ThunderAI/AI-Mate expose model params; without a timeout the UI can hang forever | LOW | Three numeric fields with sane defaults (e.g. temp 0.7, max_tokens 500–1000, timeout 30–60 s). Clamp ranges on save. Timeout enforced client-side by racing the request (ComposeWindow already has a `withTimeout` precedent for `send_pending`). |
| Generate button in Reply/Reply-All window | The unit of work is "draft a reply to THIS message"; ThunderAI/AI-Mate inject into the compose window, Superhuman puts Instant Reply on the open message | LOW | Button lives in `.compose-actions` next to Enviar, reply modes only (no new-message/forward per scope guard). Needs the original message (already BODY.PEEK-fetched for reply prefill — reuse it, no second fetch). |
| Generated text inserted above quoted original, editable | Universal pattern: AI output lands in the compose body above the `On … wrote:` quote block; user edits like their own typing (Superhuman, ThunderAI, Missive all do this) | LOW | String prepend: `generated + "\n\n" + existing body`. Must not disturb threading headers (`In-Reply-To`/`References`) or recipients — generation touches only the body textarea state. |
| Loading / cancel / retry states on the button | Generation takes seconds; every reference client shows progress + allows cancel + retry on failure (Superhuman streams with stop; Missive shows regenerate) | MEDIUM | Minimum: disabled button with "Gerando…" label + spinner, cancellable via AbortController, error notice in the existing `notice` area with a Retry action. Streaming (SSE/token-by-token) is a differentiator, not required — non-streaming single response is the simpler correct v1. |
| Plain-language errors, offline-first preserved | App works offline; LLM needs network by definition. Users expect a clear "not configured / unreachable / bad key" message, not a stack trace | LOW | Map failure classes: missing config → link to settings; 401 → wrong/revoked key; timeout/network → connectivity; 4xx/5xx → provider error text. Never include key or full body in the error or logs (PROJECT.md constraint). |
| Explicit review-before-send (no auto-send) | Trust baseline: ThunderAI/AI-Mate never send; Superhuman drafts require approval; auto-send is universally treated as the dangerous option | LOW | Architectural: generation writes to body state only; the existing `handleSend` path is untouched. No background/triggered calls — the only call site is the button handler. This is a negative requirement to preserve, not code to write. |
| Nothing leaves the machine except on button click | PROJECT.md constraint + the reason BYOK-local-endpoint (Ollama) matters; users chose SGE partly for offline posture | LOW | Guard at the call site: no prefetch, no draft-sync of generated text to server, no telemetry containing bodies. Document in settings UI ("email content is sent to your configured endpoint only when you click Generate"). |

### Differentiators (Competitive Advantage)

Not expected in v1.8, but valuable later. Pick at most one for this milestone; defer the rest.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Streaming output (token-by-token into the body) | Perceived speed: user sees progress in ~1 s instead of staring at a spinner for 10–20 s; standard in Superhuman/Shortwave | MEDIUM | Requires SSE parsing of `chat/completions` stream + incremental body updates + mid-stream cancel. Backend (Rust reqwest) or frontend fetch both viable; adds cancellation and partial-text-on-abort edge cases. Defer to v1.8.x. |
| Regenerate / tone / length options | Superhuman "Write with AI" offers length/tone variants; ThunderAI supports multiple saved prompts — users often dislike the first draft | LOW–MEDIUM | Cheap version: Regenerate button (re-call same prompt, replaces or appends). Expensive version: per-generation tone/length picker. Recommend cheap version only if time remains. |
| Local-endpoint first-run preset (Ollama) | One-click `http://localhost:11434/v1` + model preset removes the blank-page problem for privacy-sensitive users; matches "no cloud fallback" posture | LOW | Just a preset button filling base URL + model name. No extra code beyond the settings form. Good v1.8 polish candidate. |
| Per-draft prompt override (small text field in compose) | Power users tweak instructions per reply without editing the global template ("reply declining politely…") | LOW | Optional extra input above the Generate button, prepended/appended to the rendered template. Slight prompt-construction complexity; defer unless free. |
| Multi-prompt library (ThunderAI-style saved prompts) | Different templates for different reply kinds (formal, short ack, follow-up) | MEDIUM | v1.8 scope pins a single template — this directly contradicts the scope guard. Explicitly deferred. |
| Auto-draft on message open / background generation | Zero-click convenience (Superhuman Instant Reply pre-generates for every message) | HIGH | Directly violates v1.8 privacy constraint (content leaves machine without consent) and scope guard. Listed only to reject. |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Auto-send generated replies | "Inbox zero without touching anything" | Sends wrong-tone/factually-wrong mail under the user's name; destroys trust; violates milestone's core promise | Review-before-send only; generation writes body state, send stays manual via existing `queue_send` |
| Background/pre-generation for every message | Feels instant when opening compose | Sends every incoming mail body to a third party without consent; burns tokens; contradicts offline-by-default posture | Generate strictly on button click; document the privacy boundary in settings UI |
| Full-mailbox context (thread history / past sent mail as style examples) | Better-grounded, voice-matched drafts | Prompt grows unbounded (token cost, latency); pulls more PII per call; needs thread-fetch + sent-mail retrieval machinery | Single-message context (`{sender}`/`{subject}`/`{body}`) only — matches scope and keeps prompts auditable |
| Per-provider native SDKs (Anthropic/Gemini/bedrock clients) | "Support my provider natively" | N× auth shapes, N× request formats, N× error maps; maintenance burden for a single-account Linux client | One OpenAI-compatible `POST {base}/chat/completions` shape covers OpenAI, Ollama, LM Studio, OpenRouter, LiteLLM proxies |
| Saving API key in SQLite/config plaintext | Simpler code than keyring | Credential theft on disk read; violates PROJECT.md security constraint; inconsistent with mail-cred handling | Keyring entry reusing Phase 5 helpers; settings stores everything except the key |
| Generation in new-message / forward windows | Symmetry ("why not everywhere?") | Out of v1.8 scope; forward has no single `{body}` source semantics (multi-part); new-message has no source context at all | Reply/Reply-All only per scope guard; revisit when forward/new-message UX is specified |
| Logging prompts/responses for "debugging" | Easier bug reports | Mail bodies + key material in logs; violates "never log key or body verbatim" constraint | Log only status codes, latency, token counts, and redacted error classes |

## Feature Dependencies

```
Generate-with-LLM button
    └──requires──> Reply prefill (original MessageView in memory)
    │                  └──requires──> ComposeWindow reply modes (phases 37-38) ✅ built
    └──requires──> LLM settings (base URL + model + template + defaults)
    │                  └──requires──> Settings UI surface (account-setup-adjacent menu)
    └──requires──> Keyring API-key entry
    │                  └──requires──> Phase 5 keyring helpers ✅ built (new entry, same mechanism)
    └──requires──> Chat-completions HTTP call (reqwest or frontend fetch)
                       └──conflicts──> Offline-first guarantee (call site must be button-only)

Generated-text insertion ──enhances──> Existing body/quote state (prepend above quote)
Send (queue_send) ──independent──> Generation (untouched path; review gap between them is the feature)
```

### Dependency Notes

- **Generate button requires reply prefill:** the `{body}` placeholder is filled from the already-fetched original (`fetch_message` result used by `buildReplySeed`). No second IMAP fetch — read `body` state/seed, strip the quote attribution if needed, substitute into template.
- **Generate button requires LLM settings + keyring key:** button must degrade gracefully when unconfigured (disabled with tooltip, or click → notice directing to settings). Config-load failure modes mirror the existing `load_credentials` pattern.
- **Insertion enhances existing quote state:** generation only sets body text; `In-Reply-To`/`References`/recipients/subject set by reply prefill stay untouched. No dependency on `queue_send` internals.
- **Send path is independent by design:** `handleSend` → `queue_send` → `send_pending` is unchanged. The review gap (user reads/edits generated text, then decides to send) is enforced by doing nothing — no code connects generation output to automatic enqueue.
- **HTTP client conflicts with offline posture:** the only network call outside IMAP/SMTP sync; isolate in one Tauri command (e.g. `generate_reply`) so audit can verify a single call site, and so the frontend never sees the API key (key stays in Rust backend: load from keyring → POST → return text).

## MVP Definition

### Launch With (v1.8)

- [ ] LLM settings section — base URL + model + API key (keyring) + prompt template with `{sender}`/`{subject}`/`{body}` + defaults (temperature/max tokens/timeout) + test-connection — without this nothing else works
- [ ] Generate-with-LLM button in Reply/Reply-All — non-streaming `chat/completions` call, AbortController cancel, loading state — the milestone's core value
- [ ] Insert-above-quote + full editability — the review mechanism; zero extra UI beyond textarea behavior
- [ ] Plain-language error states (unconfigured / 401 / timeout / provider error) in existing `notice` area with retry — without this every failure looks like an app bug
- [ ] Privacy boundary: button-only network call, key never logged, bodies never logged verbatim — the trust contract, mostly preserved by architecture + one audit pass

### Add After Validation (v1.8.x)

- [ ] Regenerate button — trigger: users report first drafts often miss and re-clicking Generate is awkward
- [ ] Streaming output — trigger: latency complaints on long generations (local Ollama on CPU is slow)
- [ ] Ollama/localhost preset + default template tuning — trigger: real users struggle with blank base-URL field
- [ ] Per-draft instruction override — trigger: users ask to adjust tone per reply without editing global template

### Future Consideration (v2+)

- [ ] Multi-prompt library — why defer: v1.8 scope pins a single template; needs prompt-CRUD UI
- [ ] Forward/new-message generation — why defer: out of scope; different context semantics need their own spec
- [ ] Voice matching from sent mail — why defer: PII expansion + retrieval machinery; conflicts with minimal-context posture

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| LLM settings + keyring key + template + defaults | HIGH | LOW | P1 |
| Generate button (non-streaming) + cancel | HIGH | LOW–MEDIUM | P1 |
| Insert above quote, editable | HIGH | LOW | P1 |
| Plain-language errors + retry | HIGH | LOW | P1 |
| Test-connection in settings | MEDIUM | LOW | P1 |
| Button-only privacy boundary (audit) | HIGH | LOW | P1 |
| Regenerate | MEDIUM | LOW | P2 |
| Ollama preset | MEDIUM | LOW | P2 |
| Streaming output | MEDIUM | MEDIUM | P2 |
| Per-draft override | LOW | LOW | P3 |
| Multi-prompt library | MEDIUM | MEDIUM | P3 |
| Auto-send / background generation | LOW (negative trust) | HIGH (risk) | Never |

**Priority key:**
- P1: Must have for launch
- P2: Should have, add when possible
- P3: Nice to have, future consideration

## Competitor Feature Analysis

| Feature | Thunderbird + ThunderAI (closest analog) | Superhuman / Shortwave / Missive | Our Approach (v1.8) |
|---------|------------------------------------------|----------------------------------|---------------------|
| Endpoint config | OpenAI-compatible base URL + key + model, per-addon settings; custom prompts with variables | Bundled vendor model, no endpoint config | Any OpenAI-compatible base URL (covers local Ollama too); single template, not a prompt library |
| Key storage | Browser-extension storage (Thunderbird profile) | Server-side / OAuth session | OS keyring via existing Phase 5 helpers — stronger than extension storage, consistent with mail creds |
| Generation trigger | Toolbar/menu action in compose, user-triggered | Instant Reply drafts, one-click accept, some auto-draft | Strictly button-click in Reply/Reply-All; no auto/background per constraint |
| Insertion point | Into compose body | Inline draft replacing/augmenting compose | Prepend above quoted original; threading headers untouched |
| Loading/cancel/retry | Modal/progress, re-run prompt | Streaming with stop + regenerate + tone/length | Non-streaming v1: spinner + AbortController cancel + retry via notice; streaming deferred |
| Review gate | Manual (compose is never auto-sent) | Approve/edit before send; some auto-send modes | Manual always; generation and `queue_send` paths fully disconnected |
| Privacy posture | Sends selected mail to configured endpoint | Cloud processing by default | Button-only exfiltration; offline default preserved; local-endpoint friendly |

## Sources

- Thunderbird ThunderAI setup + OpenAI-compatible config (Regolo guide, Oct 2025) and custom-prompt tutorial (micz.it) — closest feature analog: HIGH confidence for settings/prompt-table-stakes
- Thunderbird AI-Mate / AI-reply add-on listings (addons.thunderbird.net, GitHub EnriqueITE/AI-Mate) — compose-window injection + API key + model settings pattern: MEDIUM-HIGH
- Superhuman Write-with-AI / Instant Reply / Personalization docs; Shortwave vs Superhuman comparisons (Zapier, Jotform) — generation UX (streaming, regenerate, tone) and review-gate patterns: MEDIUM
- OpenAI-compatible `chat/completions` API shape (CometAPI explainer, OpenRouter base-URL docs, LangChain ChatOpenAI params) — base URL + model + temperature + max_tokens standard: HIGH
- SGE codebase: `src/components/ComposeWindow.tsx` (reply prefill via BODY.PEEK, `queue_send` send path, `withTimeout` precedent, `notice` error area) — dependency mapping: HIGH (primary source)

---
*Feature research for: v1.8 LLM-assisted replies (user-triggered drafting only)*
*Researched: 2026-10-15*
