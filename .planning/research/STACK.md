# Stack Research

**Domain:** Email conversation threading (RFC 5322 References lane) + shared UI primitives (v1.9 milestone scope only)
**Researched:** 2026-10-20
**Confidence:** HIGH (versions verified against crates.io/npm registry APIs + library source pulled and inspected; integration points verified against SGE source)

Supersedes the v1.9-relevant sections of the prior STACK.md. Everything not listed here is **unchanged** — the M1 baseline pins stay exactly as-is (see "Baselines That Do NOT Change").

## Baselines That Do NOT Change (verified in lockfile 2026-10-20)

| Package | Lockfile version | Verdict | Why |
|---------|-----------------|---------|-----|
| async-imap | 0.11.3 (imap-proto 0.16.7 transitively) | **Keep** | Everything v1.9 needs is already in 0.11.3 — verified directly from crate source (below). Do **not** take 0.12.0: it swaps imap-proto to 0.17, moves to Rust 2024 edition, and reshuffles the futures dep — pure regression risk against the single-session SessionManager with zero features v1.9 needs. Historical precedent (Key Decisions: imap-proto parser gaps poisoned sessions) says lockstep parser upgrades must carry a functional reason. |
| mail-parser | 0.11.9 (`full_encoding`) | **Keep** | 0.11.9 is the latest 0.11 release (2026-09-09). References parsing does not need mail-parser at all — that lane is raw IMAP header bytes, not a MIME tree. |
| rusqlite | 0.37.0 bundled | **Keep** | M26 is plain `ALTER TABLE` SQL. crates.io shows 0.40.2 as latest stable, but migration semantics don't change and 0.38+ would touch every `queries.rs` signature for zero benefit. |
| rusqlite_migration | 2.3.0 (latest 2.6.0) | **Keep** | 2.3.0 behavior (`M::up(sql)` → `Migrations::to_latest`) is what M26 needs. No bump mid-milestone. |
| tauri | 2.12.1 | **Keep** | CSP is the only v1.9 surface; config semantics verified against v2 docs (below). |
| react / react-dom | ^19.1.0 (resolves 19.3.1) | **Keep** | caret already flattens to latest 19.3.x at install; `ref` as regular prop, hooks for primitives — all React 19. |
| tools (next: vite@8.3.4, @vitejs/plugin-react@6.1.2, typescript ~6.0.3 tilde-pinned) | latest-registry-2026-10-20 | **Keep** | npm registry verified: `react@19.3.1`, `vite@8.3.4`, `@vitejs/plugin-react@6.1.2`, `typescript@7.0.2` exist — the tilde pin on typescript deliberately stays off the 7.x line mid-milestone. |

## Recommended Stack — NEW capabilities only

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `mail-threading` (crates.io) | 0.1.3 (pub. 2026-06-04, MIT/Apache-2.0) | JWZ / RFC 5256 `THREAD=REFERENCES` algorithm → flat thread membership → backend-computed `thread_id` | **The one justified new Rust dep.** Implements JWZ lineage with RFC 5256 semantics: Message-ID duplicate policy, phantom (missing-ancestor) containers with pruning, subject-prefix normalization for fallback — exactly the edge cases where hand-rolled JWZ goes wrong. Ships a shared JSON conformance corpus (`testdata/conformance`) + RFC coverage matrix so our own regression claims can point at real fixtures. API fits SGE precisely: caller passes `{id (our `messages.id` PK), message_id, in_reply_to, references, date, subject}` → `Vec<Thread>` with `root_message_id` + `messages: Vec<caller_id>`. Linear hash-map complexity. Per user decision, configure `subject_merge: false` (headerless @subject grouping was explicitly rejected as unreliable). |
| chrono (already in tree) | ("0.4" present) | `Message.date` field for `mail-threading` input | Already a direct SGE dependency; `mail-threading` also depends on chrono → **zero marginal crates** unless serde feature enabled (better: map `Thread` output manually in `store/threads.rs`, skip the `serde` feature so no new derive surface). |
| In-house msg-id/RFC 2047 tooling (no crate) | — | `references.rs`: parse `BODY[HEADER.FIELDS (REFERENCES)]` raw header bytes → normalized `Vec<String>`; extract `envelope.in_reply_to`: | `use-email-id` (0.1.0, 157 downloads) is not traction-justified. The existing house pattern (`decode_header_words`, `decode_bytes_opt` in `imap/headers.rs`) is the right home: References values are pre-encoded msg-ids (no 2047 in msg-id context). Keep same fail-visible style + tests. |

**Threading lane integration points (verified from source):**

- ` envelopes` land free: `imap-proto 0.16.7` `Envelope` already has `pub in_reply_to: Option<Cow<[u8]>>` (verified `src/types.rs:619`) — extend `parse_fetch_item` / `parse_fetch_attributes` in `imap/headers.rs` to copy it into `MessageHeader.in_reply_to`. **FETCH_ATTRS stays byte-identical** — no new wire bytes, no new lanes for the headers-first sweep.
- New-lane fetch for full References chains (new messages only, optional per-folder backfill): emit `UID FETCH <range> (UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])`. **Verified end-to-end against 0.11.3 source:** imap-proto 0.16.7 parses the response into `AttributeValue::BodySection { section: Some(SectionPath::Full(MessageSection::Header)), data: Some(bytes) }` (parser `rfc3501/body.rs:22-29` maps both `HEADER` and `HEADER.FIELDS` to `MessageSection::Header`; parser test `test_header_fields` at `src/parser/tests.rs:553` uses this exact response shape), and async-imap `Fetch::section(&SectionPath::Full(MessageSection::Header))` at `src/types/fetch.rs:179` is the public accessor. **Invariant push-down:** `.PEEK` keeps the read-only `\Seen` invariant; the `peek_audit` test in `headers.rs` must be extended to enumerate this new fetch path (path 5) — the test’s `fetch_attrs_are_parenthesized` guard also rejects bare `BODY.PEEK[HEADER` in `FETCH_ATTRS`, so the new lane keeps its attr set in a separate named constant at the top of `headers.rs` (parenthesized: RFC 3501 §6.4.5 multipart rule applies).
- Wire cost: References-only sections are tiny (`References: <a@x> <b@y> ...` ≈ tens-~hundreds B/msg for short chains; Zimbra on UTFPr sends exactly the requested field + CRLF separator). Batched by UID range, same shape as `fetch_envelope_batch`. The expensive `BODY[HEADER.FIELDS (…)]` fetch failure mode is server dependency variance, cheap enough to verify at Phase 42's live gates.
- Thread computation lives in a new small module (e.g. `imap/threads.rs` or `store/threads.rs`): load `SELECT id, message_id, in_reply_to, references_json, date_utc, subject FROM messages (per mailbox)` → `mail_threading::thread_messages_with(&msgs, &ThreadingOptions { subject_merge: false, ..default })` → `UPDATE messages SET thread_id $WHERE id $IN …`. Computed at sync-end + after backfill; `thread_id` is an INTEGER column value (e.g. root message `messages.id`), never a key — matches schema.sql's "Message-ID is a nullable display field, never a key (T-02-01)".

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| None new — no frontend additions | — | Shared UI primitives (Button, Modal/Dialog, Empty/Loading) | Plain-CSS-token codebase: new `src/components/ui/` folder + a third stylesheet (`ui-primitives.css`, imported after `tokens.css` in main.tsx) covering shared primitives + standard interaction states (hover/focus-visible/disabled/aria-busy/empty/loading). React 19: native `ref` prop, no `forwardRef`; semantic `<button>`/`<dialog>`-pattern HTML + `aria-*`; shorthand data-state attributes (`data-state="open|closed|loading"`) instead of a CSS-in-JS lib. React 19 gives `useRef`/`useImperativeHandle` no better option here. |

**Frontend integration points (verified against current tree):** `src/styles/tokens.css` (font tokens already reference the right families — see "Fonts" below), `src/components/MessageList.tsx` (conversation grouping rendering; row = `data-conversation-id`, aggregate unread badge via existing `--badge-*` tokens), `src/components/ReadingPane.tsx` (expand/collapse message stack — `<details>`-free custom pattern, no ARIA anti-pattern, per-message vs per-conversation killed-action mapping keyed on the existing `(mailbox, uid)` surfaces), and `src/components/ui/*` for shared primitives consumed by new panels first, legacy panels opportunistically (revamp sweep limit).

### Vintage Fonts: Bundle Baloo 2 + Nunito woff2 locally (OFL)

**Verified facts:** `tokens.css` already declares `--font-display: "Baloo 2"…` and `--font-body: "Nunito"…` but no font files exist — the app silently uses fallbacks (matches PROJECT.md's stated bug). Both fonts are OFL-1.1 on Google Fonts (Baloo 2 has `wght` variable 400–800 + Devanagari; Nunito is `wght` 200–1000 + italic; we need latin subset only).

**Plan (no npm dep):**

1. Vendor static woff2 latin subset files into `src/assets/fonts/` — one per family+weight actually used by tokens (Baloo 2 400/600/700; Nunito 400/600/700 + one italic if used). Sources: official `google/fonts` GitHub repo or fontsource CDN snapshot **at vendoring time** (not runtime). Note Baloo 2's raw OFL `fonts.google.com` download provides both static + variable woff2.
2. Optional-but-cheaper alternative: variable single-file woff2 (`Baloo2[wght].woff2`, `Nunito[wght].woff2`) — 2 files total, weights come free. Recommend this (one file/face, ~60–150 KB each, no per-weight @font-face duplication).
3. New `src/styles/fonts.css`: `@font-face` rules with `format('woff2-variations')` (variable) or `format('woff2')` (static), `font-display: swap` (matches tokens-first posture), latin subset `unicode-range: U+0000-00FF, …` (paste from fontsource's generated per-family css — copy verbatim to avoid mistakes in subsetting), `src: url('./../assets/fonts/<file>.woff2')` — **no `data:` base64 embedding** (bloats app.css and pollutes diff reviews).
4. Import order in `main.tsx`: `fonts.css` **before** `tokens.css` (or put `@import url("./fonts.css");` as line 1 of tokens.css — Vite requires @import first). `@font-face` must be registered before rules they feed.
5. License hygiene: include `OFL.txt` for each family inside `src/assets/fonts/` and a note in the CSS header. OFL redistribution as regular font files (not embedded in zipped software-token RHAs) requires license text visible — file-in-repo + vendor header satisfies the standard many-projects-fail case of over-defensiveness.
6. `.gitignore` rule: woff2 assets may NOT be gitignored (check the repo's asset ignore rules before vendoring).

**CSP consequence (verified against tauri.conf.json + Tauri v2 CSP docs):** current CSP is `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src ipc: http://ipc.localhost; frame-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none';` — **no explicit `font-src`** → falls back to `default-src 'self'`, which **already allows** same-origin woff2 from the bundled dist (prod `tauri://localhost`, dev `localhost:1420`). **No CSP change strictly required.** Recommend adding explicit `font-src: 'self';` (defensive, makes intent obvious — future CSP tightening won't silently break fonts). **Do not** add `'self' https://fonts.gstatic.com` style hole-outs — violates egress/offline posture and this codebase's own 1.9 scope guard. Note: the sandboxed reading-pane iframe inherits `'self'` too, but mail HTML should never load remote fonts (egress rule already in place; keep it).

## Installation

```bash
# Rust: single new direct dependency (chrono already present)
cargo add mail-threading --package sge  # "0.1", default features (no serde; chrono comes transitively anyway)
# → src-tauri/Cargo.toml pins line:
#   mail-threading = "0.1"   # JWZ/RFC5256 threading for thread_id (v1.9, Phase 42)
# Optionally pin exact: note in comment like lettre's Phase 13 record:
#   # v1.9 pin record: mail-threading resolves to 0.1.3 (dual MIT/Apache-2.0,
#   # conformance corpus verified against fixtures on 2026-10-20 → see research/STACK.md).

# Frontend: NOTHING new.
# Fonts: 2 files vendored, no npm install:
#   src/assets/fonts/Baloo2-latin-[wght].woff2  (+ OFL.txt)
#   src/assets/fonts/Nunito-latin-[wght].woff2  (+ OFL.txt)
```

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| `mail-threading` crate | In-house JWZ (~250 LOC in threads.rs) | If the user vetoes the one new dep at plan time, in-house is genuinely feasible — datograph containers, prune phantoms with subject-merge off. Deficit: no conformance corpus, so the reliability bar the user set (backend References lane "explicitly approved as reliable") must be re-earned with hand-written fixture tests. Cost estimate overshoots because dup-Message-ID policies and phantom handling are the hard 20%. |
| `mail-threading` | IMAP server-side `UID THREAD REFERENCES` (Zimbra supports it) | Never for v1.9: user already approved the offline lane (Key Decision 2026-10-20); also would need `IMAPTHREAD` capability probe + async-imap doesn't wrap THREAD responses (imap-proto parses them but `Session::uid_thread` isn't exposed in 0.11 — would need raw-response plumbing), plus offline-first sync semantics conflict with an online-only grouping. |
| Vendored woff2 + own `@font-face` | `@fontsource/baloo-2` + `@fontsource-variable/nunito` npm packages | Never for v1.9 — scope guard explicitly excludes "new UI dependencies"; vendor files instead with licenses documented above. @fontsource would normally win for auto-subsetting + license-file shipping, but the posture wins here. |
| Backend `thread_id` per RFC 5256 strict | Frontend grouping by `in_reply_to` (client-side regroup) | Never — user rejected subject-only grouping as unreliable and explicitly approved backend references lane. |
| Unity on `messages.thread_id` NULL = own-thread | Google-style "thread cache" table (separate `threads` table + FK) | v1.9 is read-only threading (`mail-threading` computed inline). Separate `threads` table only if per-thread mutable data ever needs to exist (e.g. aggregate read state materialization) — defer. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| email-threading / jwz / mail-thread (crates.io) | Don't exist / not found on crates.io search 2026-10-20 | `mail-threading` |
| `use-email-id` crate | 0.1.0, 157 downloads, single-author — no community signal for a security-adjacent parsing job | In-house `references.rs` parser following house `decode_header_words` fail-visible pattern |
| virtualization libs (react-window, react-virtual, tanstack-virtual) | Scope guard: no new UI dependencies; MessageList already renders full mailbox at acceptable perf | Collapse message stacks to one conversation row (aggregate count) — the grouping thin-out is the perf fix; revisit virtualization only in a future perf milestone with measurements |
| Radix / shadcn / Headless UI / MUI / Tailwind | Violates zero-new-deps + will mangle the clay token system (three-layer cascade would need re-merging) | Hand-rolled `src/components/ui/` primitives over existing tokens |
| CSS-in-JS, styled-vanilla, panda | Project is plain `*.css` co-located per component + `tokens.css` cascade | New plain-CSS `ui-primitives.css` + data-state attributes |
| async-imap 0.12.0 (latest, 2026-09-30) | Feature-free for v1.9; pulls imap-proto 0.17 + Rust 2024 edition migration in-place; historically, async-imap/imap-proto lockstep upgrades bit us (NAMESPACE tripwire decision) | Stay on `0.11.3`/`imap-proto 0.16.7`; record the tripwire decision |
| rusqlite 0.40.2 / migration bump mid-milestone | all `queries.rs` signatures tie to 0.37 API-surface (bundled sqlite version differences are real but pointless now) | keep 0.37.0 + 2.3.0; bump only at M+1 boundaries with CI live-gates rerun |
| New Table for threading precomputed at insert | Thread membership changes when any member arrives (late root arrival, dedup policy, phantom pruning) — precomputing at insert yields transient wrong groups | Recompute per-folder at sync-end / after backfill / after send (batched UPDATE); per-folder scope keeps cost linear-in-folder-messages |
| Base64-embedding fonts in CSS (`data:` URL) | 15% size bloat + review quality worse + lazy `@font-face` swap semantics | Same-origin file URL, `font-display: swap`, with `unicode-range` latin subsetting |

## Stack Patterns by Variant

**If `mail-threading` is rejected at plan time (user nod):**
- Hand-roll `store/threads.rs` JWZ: ~250 LOC + fixture tests mirrored from the `mail-threading` conformance corpus (its fixtures are public JSON — we can absorb the data even if we don't take the crate).
- Same schema and same integration points otherwise — zero migration cost this way.

**If Zimbra refuses `HEADER.FIELDS (REFERENCES)`** (new wire path, verified parser-side but not live-verified at UTFPr):
- Fallback: reuse the 2026-10-18 send-path convention — References = In-Reply-To only when the field is absent, which still threads correctly for shallow chains in practice; document + record as a v1.9 Known Gap instead of blocking the phase.

**If fonts render with FOUT on slower disks (woff2 > swap window):**
- `font-display: block` only for `--font-display` headers (Baloo 2 = title logo, non-critical path), keep `swap` everywhere else; preload the two variable files only if LCP moves — measure first, nothing else.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| mail-threading 0.1.3 | chrono 0.4, Rust ≥1.88 | env rustc 1.99.0 (2026-09-28) → OK. Direct dep list: chrono only (serde optional; we opt out) — verified from crates.io dependency manifest. |
| async-imap 0.11.3 | imap-proto 0.16.7 | `Envelope.in_reply_to` + `Fetch::section()` verified present at exactly these versions — no resolution drift risk (both are in our lockfile already). |
| rusqlite 0.37.0 + rusqlite_migration 2.3.0 | M26 plain SQL | `ALTER TABLE ADD COLUMN` (nullable/text-or-int) + `CREATE INDEX` are within 2.3.0's supported feature set; house preserve-rows regression-test pattern (tests at `store/mod.rs` `migrations_through_m22` precedent) extends as M26 case. |
| React 19.3.x (caret-resolved) | React DOM 19.3.x, vite 8.3.x | No new hooks beyond current usage; `ref`-as-prop is 19-only — do not use it expecting back-compat guidance if we ever downgrade. |

## Sources

- crates.io API (`/api/v1/crates/<n>/versions`, User-Agent'd, dates verified 2026-10-20): async-imap (0.12.0 / 0.11.3), rusqlite (0.40.2 latest; 0.37.0 pinned), rusqlite_migration (2.6.0 latest; 2.3.0 pinned), mail-parser (0.11.9), imap-proto (0.16.7), mail-threading (0.1.3).
- npm registry `registry.npmjs.org/<p>/latest` (2026-10-20): react@19.3.1, react-dom@19.3.1, vite@8.3.4, @vitejs/plugin-react@6.1.2, typescript@7.0.2 (kept off — tilde).
- async-imap 0.11.3 + imap-proto 0.16.7 crate sources pulled via `crates.io …/download` and inspected directly: `src/parser/rfc3501/body.rs` (HEADER.FIELDS→MessageSection::Header), `src/parser/tests.rs:553` (`test_header_fields` — the exact response shape we'll emit), `src/types/fetch.rs:179` (`Fetch::section`). Confidence HIGH (primary source).
- async-imap GitHub `CHANGELOG.md` (master, 2026-10-20): 0.12.0 scope (imap-proto 0.17, futures reshuffle, edition 2024, MSRV 1.88) — grounds the "do not upgrade" verdict.
- `mail-threading` GitHub README (`github.com/planetaryescape/mail-threading`, fetched 2026-10-20): API shape, `ThreadingOptions { subject_merge, prune_phantoms, subject_prefixes }`, conformance corpus, MSRV 1.88, chrono-only dependency claim.
- Tauri v2 CSP docs (`v2.tauri.app/security/csp/` + `/reference/config/`, fetched 2026-10-20): directive fallback to `default-src`, `font-src` semantics, dev-server/prod precedence.
- Project-local verified integration points: `src-tauri/Cargo.toml` + `Cargo.lock` (pin record + lockfile versions), `src-tauri/src/store/mod.rs` (SCHEMA_VERSION 25, M-list shape, preserve-rows test precedent), `src-tauri/src/imap/headers.rs` (FETCH_ATTRS, `parse_fetch_attributes`, `peek_audit`), `src-tauri/tauri.conf.json` (live CSP string), `src/styles/tokens.css` (font tokens), `src-tauri/src/send_queue.rs` / `src-tauri/src/drafts.rs` (existing in_reply_to/references Features, `new_message_id`, `sanitize_header` — the v1.9 References lane reuses these helpers).

---
*Stack research for: SGE v1.9 — Email Threading + Interface Revamp*
*Researched: 2026-10-20*
