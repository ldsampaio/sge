# Feature Research

**Domain:** Linux desktop IMAP email client (read-only M1, Gmail-like three-pane)
**Researched:** 2026-10-03
**Confidence:** HIGH

## Feature Landscape

### Table Stakes (Users Expect These)

Features users assume exist. Missing these = product feels incomplete.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| IMAP login form (username, password, server URL) — **table-stakes** | Every desktop client opens with account setup; no login = no product. | LOW | Three fields + validation; prefill port from security mode; test-connection button before save. Thunderbird/Geary both lead with this. |
| Configurable IMAP security (host/port, SSL/TLS vs STARTTLS) — **table-stakes** | University/corporate servers (e.g. mail.utfpr.edu.br) vary ports and modes; hardcoded 993/TLS fails on real servers. | MEDIUM | Offer presets (993/SSL-TLS, 143/STARTTLS, 143/plain-local-only with warning); verify TLS cert; surface handshake errors in plain language. |
| INBOX folder sidebar — **table-stakes** | Three-pane Gmail/Thunderbird/Geary layout is the requested mental model; a list without sidebar feels broken. | LOW | M1 shows single INBOX node (+ optional Unread/Special views); component must accept multi-folder data later without rewrite. |
| Virtualized message list (sender, subject, date, unread dot, sort desc) — **table-stakes** | Users judge "fast" in the first 2 seconds; rendering 10k rows without virtualization janks or OOMs. | MEDIUM | Use TanStack Virtual / react-virtuoso; row height fixed or measured; selection + keyboard nav (j/k, arrows, Enter). |
| Reading pane with From/To/Date/Subject header — **table-stakes** | Thunderbird/Geary/Gmail all show a header block above body; missing headers destroy trust in what's displayed. | LOW | Render RFC-decoded headers; show address not just display-name; date in local tz. |
| Plain-text body rendering — **table-stakes** | Fallback for every multipart/legacy message; HTML-only readers show blank on text-only mail. | LOW | Wrap, preserve quoted `>` blocks; linkify URLs. |
| HTML body rendering with sanitization — **table-stakes** | Most real mail is HTML; rendering raw HTML enables XSS/tracking (cf. OWA CVE-2026-42897 half-click precedent). | MEDIUM | DOMPurify-equivalent allowlist (strip script/event handlers/javascript: URIs/forms) + sandboxed iframe + block remote images by default. This is table-stakes *security*, not polish. |
| Headers-first sync into SQLite, bodies on demand — **table-stakes** | Core Value promise: fast first paint on large UTFPR mailboxes; bulk full-download stalls startup (proton-mail-mcp pattern validates this). | MEDIUM | ENVELOPE/FETCH headers → list immediately; lazy BODY[] fetch + cache; UIDVALIDITY/UID tracking for incremental sync. |
| Local search/filter (sender/subject via SQLite FTS5) — **table-stakes** | Thunderbird Quick Filter bar sets the expectation: type-to-filter must work offline and instantly. | MEDIUM | FTS5 over from/subject (+ cached body); debounce input; rank by BM25; show "searching local cache" hint when bodies not yet fetched. |
| Attachment names list + download/save — **table-stakes** | PROJECT.md Active requirement; users must at least see filenames/sizes and save files. | LOW | Parse MIME structure; show icon + size; Save-As dialog; stream to disk, never load whole file into memory. |
| Sync status indicator + offline badge — **table-stakes** | Offline-capable-after-sync is promised; without a visible badge users can't tell stale from live. | LOW | Status bar/pill: Syncing (progress n/total), Up-to-date (timestamp), Offline (cached data), Error (retry). |
| Empty / loading / error states for every pane — **table-stakes** | First-run (no account), empty INBOX, slow UTFPR server, wrong password all occur; blank panes read as "crashed". | LOW | Skeleton list rows; empty-INBOX illustration; per-pane error card with retry; auth errors name the field (host vs credentials vs TLS). |
| Read/unread state display — **table-stakes** | Unread bolding/dots are universal email grammar; users triage by scan. | LOW | M1 may be display-only (no server flag write) — but visual distinction is still required. Document whether toggle writes back (M1: read-only, so persist locally or defer toggle). |
| Secure credential remember + auto-login (OS keyring) — **table-stakes** | PROJECT.md mandates keyring + auto-login for daily personal use; plaintext or retype-every-launch fails the brief. | MEDIUM | Secret Service API via keyring crate; store password + server; auto-connect on launch; lock-screen semantics respected. |
| Linux desktop bundle (Tauri v2) + system integration — **table-stakes** | "Ships as Linux build" is an Active requirement; a dev-only `cargo run` is not shippable. | MEDIUM | .deb/.AppImage via tauri-bundler; window state persistence; sane HiDPI fonts. |

### Differentiators (Competitive Advantage)

Features that set the product apart. Not required, but valuable.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Sub-second cold start to readable list (headers-first + virtualized + cached) — **differentiator** | Thunderbird on huge IMAP boxes feels heavy; instant-open is the "wow" that validates Core Value. | MEDIUM | Measure: time-to-first-list < 2s on 10k-message INBOX; lazy bodies keep it there. Worth instrumenting in M1. |
| Search-as-you-type with highlighting over 10k+ local messages — **differentiator** | FTS5 + BM25 ranking + highlight snippets feels like Gmail, not like a thin IMAP wrapper. | MEDIUM | `highlight()` snippets in results; prefix queries; exact-phrase support. Cheap because FTS5 is already there. |
| Remote-image blocking with per-sender allow — **differentiator** | Privacy win over naive renderers; blocks open-tracking pixels by default while staying one click from full fidelity. | LOW | Banner "Images blocked — Show once / Always from sender"; persisted allowlist in SQLite. |
| Attachment quick-look (size + MIME icon + open-with) — **differentiator** | Listing names is expected; one-click preview/open is what makes attachments feel done. | LOW | xdg-open integration; warn before executing risky types. |
| Keyboard-first triage (j/k/n/p, / to search, r-reload) — **differentiator** | Power-user speed that Gmail/Thunderbird users bring as muscle memory; trivial cost, high delight. | LOW | Add a `?` shortcut overlay; ensure list↔reader focus model is sane. |
| Reading-pane density / font-size control — **differentiator** | Accessibility + personal-client fit for daily UTFPR use; almost free in React. | LOW | Comfortable/compact toggle; base font scale; respects system font. |
| Connection diagnostics screen (log + "copy debug info") — **differentiator** | University mail servers have quirky TLS/certs; self-serve diagnostics cut support to zero for a personal tool. | LOW | Show last IMAP error, resolved host:port/mode, cert summary; one-click copy for bug reports. |

### Anti-Features (Commonly Requested, Often Problematic)

Features that seem good but create problems.

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Compose / send / reply via SMTP in M1 | "It's an email client, of course it sends" | Doubles scope (SMTP, drafts, quoting, MIME build, sent-state); PROJECT.md explicitly defers past M1 after user correction | Ship read-only M1; add SMTP as milestone 2 with its own send-queue design |
| Full-mailbox bulk body download up front | "True offline" completeness | Stalls first paint for minutes on large boxes; huge SQLite bloat; violates headers-first decision | Lazy bodies + incremental backfill idle task post-M1 |
| Auto-load remote images by default | Prettier newsletters | Tracking pixels leak IP/open events; XSS-adjacent surface; contradicts sanitization posture | Block by default + per-sender allowlist (differentiator above) |
| Multi-folder / Sent / Drafts in M1 | "Sidebar looks empty with one folder" | Each folder needs sync state, UID tracking, semantics; explodes testing matrix for INBOX-only milestone | Single INBOX node; architect sidebar data-driven so folders plug in later |
| OAuth / 2FA / magic-link in M1 | Modern auth expectation | Server-specific (Gmail vs generic IMAP); PROJECT.md scopes M1 to user+password; OAuth is a project of its own | Defer; keep auth module trait-based so OAuth fits later |
| Message flag writes (mark read/delete/archive) in M1 | Triage feels read-write | IMAP flag STORE + UID expunge + conflict handling contradicts read-only scope; half-done writes corrupt trust | Display-only unread state in M1 (or local-only cache flag, clearly labeled); server writes in M2 |
| Real-time IDLE push sync in M1 | "Live inbox" feel | Connection lifecycle + reconnect + battery/complexity; polling/manual refresh suffices to validate viewer | Manual refresh + poll interval; IDLE as post-validation enhancement |

## Feature Dependencies

```
[Three-pane UI shell]
    └──requires──> [IMAP login + security config]
                       └──requires──> [Headers-first sync → SQLite]
                                          └──requires──> [Message list (virtualized)]
                                                             └──requires──> [Reading pane + sanitization]
                                                                                └──enhances──> [Attachment list + download]
                                                                                └──enhances──> [FTS5 search/filter]

[Credential remember + auto-login] ──enhances──> [IMAP login + security config]
[Sync status / offline badge] ──enhances──> [Headers-first sync → SQLite]
[Empty/loading/error states] ──enhances──> [Three-pane UI shell]
[Attachment download] ──conflicts──> [Compose/send] (no conflict technically — excluded by scope, not incompatibility)
[Server flag writes] ──conflicts──> [Read-only M1 scope]
```

### Dependency Notes

- **Three-pane shell requires login+security config:** no connection, nothing to display; first-run state routes to login form.
- **Login requires headers-first sync → SQLite:** connection alone isn't shippable; list must populate from local DB (DB-first reads, IMAP fallback only pre-sync).
- **Message list requires virtualization from day one:** retrofitting virtualization after a naive `.map()` list is a rewrite of selection/scroll/keyboard logic.
- **Reading pane requires sanitizer before first HTML render:** never render unsanitized mail even in dev; sandbox + CSP from the start.
- **Attachment download enhances reader:** MIME structure parsing is shared; download is a small step once structure is parsed.
- **FTS5 search enhances headers-first sync:** FTS index populates from the same sync pipeline; search over un-synced bodies must be labeled "headers only".
- **Server flag writes conflict with M1 scope:** excluded deliberately; adding STORE/EXPUNGE mid-milestone breaks the read-only test story.

## MVP Definition

### Launch With (v1 = M1 read-only viewer)

Minimum viable product — what's needed to validate the concept.

- [ ] IMAP login form + configurable security (host/port, SSL-TLS/STARTTLS) — without this nothing connects
- [ ] Headers-first INBOX sync into SQLite + incremental refresh — the Core Value mechanism
- [ ] Three-pane shell: INBOX sidebar + virtualized list + reader — the requested UX
- [ ] Sanitized HTML + plaintext body rendering — safe display is non-negotiable
- [ ] Local FTS5 search/filter — offline triage is the retention hook
- [ ] Attachment list + download — Active requirement, completes "read" loop
- [ ] Sync status + offline badge — makes offline-capable legible
- [ ] Empty/loading/error states incl. auth-error copy — first-run and failure UX
- [ ] Keyring remember + auto-login — daily-use expectation
- [ ] Linux Tauri bundle — shippable artifact

### Add After Validation (v1.x)

Features to add once core is working.

- [ ] Remote-image per-sender allowlist — trigger: users complain newsletters look broken
- [ ] Keyboard shortcuts + shortcut overlay — trigger: user does daily triage and wants speed
- [ ] Background IDLE/poll refresh — trigger: manual refresh feels stale in daily use
- [ ] Multi-folder sidebar (Sent, custom) — trigger: INBOX-only feels limiting after a week
- [ ] Read/unread + flag server writes — trigger: triage actions requested; marks end of read-only era

### Future Consideration (v2+)

Features to defer until product-market fit is established.

- [ ] Compose/send/reply via SMTP — why defer: explicitly out of M1; needs MIME build, drafts, error handling
- [ ] OAuth / app-password flows — why defer: M1 is user+password; per-provider work
- [ ] Threading/conversation view — why defer: needs References/In-Reply-To reconstruction; list-first is fine for M1
- [ ] Full-text body index backfill + global search — why defer: needs idle backfill infra first
- [ ] Windows/macOS builds — why defer: Linux-only constraint for M1

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| IMAP login + security config | HIGH | MEDIUM | P1 |
| Headers-first sync → SQLite | HIGH | MEDIUM | P1 |
| Virtualized message list | HIGH | MEDIUM | P1 |
| Sanitized HTML + plaintext reader | HIGH | MEDIUM | P1 |
| FTS5 local search/filter | HIGH | MEDIUM | P1 |
| Attachment list + download | HIGH | LOW | P1 |
| Sync status + offline badge | HIGH | LOW | P1 |
| Empty/loading/error states | HIGH | LOW | P1 |
| Keyring remember + auto-login | HIGH | MEDIUM | P1 |
| Linux bundle | HIGH | MEDIUM | P1 |
| Remote-image allowlist | MEDIUM | LOW | P2 |
| Keyboard-first triage | MEDIUM | LOW | P2 |
| Attachment quick-look/open-with | MEDIUM | LOW | P2 |
| Connection diagnostics screen | MEDIUM | LOW | P2 |
| Density/font control | LOW | LOW | P3 |
| IDLE push sync | MEDIUM | HIGH | P3 |
| Multi-folder support | MEDIUM | HIGH | P3 |
| Compose/send (SMTP) | HIGH (later) | HIGH | P3 (v2) |

**Priority key:**
- P1: Must have for launch
- P2: Should have, add when possible
- P3: Nice to have, future consideration

## Competitor Feature Analysis

| Feature | Thunderbird | Geary (GNOME) | Our Approach (SGE M1) |
|---------|-------------|---------------|----------------------|
| Layout | Folder + list + message panes, multiple view modes | Conversation-centric three-pane, minimal chrome | Gmail-like three-pane fixed for M1; Thunderbird parity on panes, Geary-like simplicity |
| Account setup | Autoconfig + manual IMAP/SMTP, cert exceptions | Simple service presets + custom IMAP | Manual host/port/security form (university-server-first, no magic autoconfig in M1) |
| Sync model | Full sync + offline store, heavy on huge boxes | Lazy/efficient IMAP, fast feel | Headers-first + lazy bodies (fastest first paint; validated by proton-mail-mcp pattern) |
| Search | Quick Filter bar + global index | Simple search with scope operators | FTS5 local search, Quick-Filter-like UX; body search labeled when uncached |
| HTML display | Sanitized render, remote content blocked by default | HTML render with conservative defaults | DOMPurify-class sanitize + sandboxed iframe + remote-image block (match best practice) |
| Attachments | Full list/preview/save/detach | List + save/open | M1: list + save (match); preview/open-with as fast-follow P2 |
| Offline | Offline mode toggle + status | Transparent local cache | Sync-status pill + offline badge fed by SQLite cache state |
| Auth persist | Password manager + master password | GNOME Keyring integration | OS keyring (Secret Service) + auto-login, same model as Geary |

## Sources

- Thunderbird main-window/panes documentation (support.mozilla.org — Folder/Message-List/Message panes, Quick Filter bar, Views)
- GNOME Geary feature set (IMAP client, HTML read, starring/archive, Gmail/Yahoo/IMAP compat)
- AgentMail engineering: Rendering Email Safely (DOMPurify + sandboxed iframe + CSP pattern)
- Email Markup Consortium vision (per-client sanitization divergence; iframe embedding contexts)
- Dovecot FTS plugin docs (header vs body search cost distinction)
- SQLite FTS5 official docs (MATCH, BM25 ranking, highlight snippets, prefix/phrase queries)
- proton-mail-mcp / mail-memex / GiraffeMail (SQLite + FTS5 + headers-first/lazy-body + incremental sync precedent)

---
*Feature research for: Linux desktop IMAP email client (read-only M1)*
*Researched: 2026-10-03*
