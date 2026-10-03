# Project Research Summary

**Project:** SGE — Linux IMAP Desktop Client
**Domain:** Rust + Tauri v2 + React + SQLite desktop email client (read-only M1, INBOX-only, headers-first sync)
**Researched:** 2026-10-03
**Confidence:** HIGH

## Executive Summary

SGE is a Linux-only desktop IMAP email client (Rust + Tauri v2 backend, React frontend, SQLite local cache) whose milestone 1 is a read-only, INBOX-only viewer for personal UTFPR mail. Experts build this class of app as an offline-first local cache: a single Rust-owned sync worker performs UID-based, headers-first IMAP fetch into SQLite (WAL + FTS5), bodies and attachments load on demand, and React renders exclusively from the local store via typed Tauri commands — never by talking IMAP from the UI. Progress streams over `Channel<SyncEvent>`, credentials live only in the OS keyring (Secret Service), and HTML mail is sanitized in Rust before it ever reaches the WebView.

The recommended approach is dependency-ordered: scaffold Tauri v2.12 + React 19 + TS, then build the backend sync core (async-imap 0.11, mail-parser 0.11 + full_encoding, rusqlite 0.37 bundled + rusqlite_migration 2.x, keyring 3 + sync-secret-service) before any UI, because every pane depends on the UIDVALIDITY-guarded incremental sync algorithm and the canonical SQLite schema being correct first. Frontend follows with a virtualized three-pane shell (TanStack Query + virtual list), then the sanitized reader + attachment download, and finally keyring auto-login plus pinned-baseline Linux bundling (.deb/AppImage on Ubuntu 22.04 CI).

Key risks are silent cache corruption (ignoring UIDVALIDITY), full-body-first sync that stalls on large university mailboxes, Gmail-isms that break on Dovecot/Zimbra servers, raw-HTML XSS reaching Tauri IPC, plaintext credential fallbacks on keyring-less machines, and "works in dev, won't install" bundling failures. Each has a concrete, testable mitigation (validity-bump resync test, paged UID sweep, CAPABILITY/NAMESPACE probing, ammonia + sandboxed iframe without same-origin, keyring probe + explicit encrypted fallback, clean-VM install matrix) that the roadmap bakes in as phase gates.

## Key Findings

### Recommended Stack

Tauri v2.12 (Rust crate + `@tauri-apps/api ^2.12` + CLI 2.12, capability-based IPC, `create-tauri-app 4.7.4` scaffold) with React 19 + TypeScript + Vite frontend. The data plane lives entirely in Rust: `rusqlite 0.37` with `bundled` (static SQLite incl. FTS5, WAL from day one, single-writer `Mutex<Connection>`), `rusqlite_migration 2.x` for versioned schema, `async-imap 0.11` + `async-native-tls 0.5` on a dedicated sync thread (native-tls uses the system CA store — important for institutional servers like mail.utfpr.edu.br), `mail-parser 0.11` with `full_encoding` (41 charsets incl. win-1252/iso-8859-1, zero-copy, fuzzed), and `keyring 3` with `sync-secret-service` (all calls via `spawn_blocking` / sync thread — never on a runtime thread). Frontend caching via `@tanstack/react-query 5.x` + `@tanstack/virtual`; sanitization via `ammonia` in Rust + sandboxed iframe. Build/release on frozen Ubuntu 22.04 with webkit2gtk-4.1. Details: STACK.md.

**Core technologies:**
- `tauri 2.12` (+ matching JS API/CLI): app runtime, typed `invoke` commands, Channel progress streams, Linux bundling — v2 is the only maintained line.
- `rusqlite 0.37 bundled` + `rusqlite_migration 2.x`: Rust-owned mail store (accounts, sync state, headers, FTS5); bundled SQLite guarantees identical FTS5 behavior on every distro.
- `async-imap 0.11` + `async-native-tls 0.5` (async-std worker on dedicated thread): SELECT INBOX, paged UID header fetch, on-demand body fetch; system CA store for university certs; fallback downgrade path is sync `imap 2.x` if bridging hurts.
- `mail-parser 0.11 + full_encoding`: RFC5322/MIME decode (subjects, addresses, multipart bodies, attachment sections, 41 charsets); headers-only fast path for the sweep.
- `keyring 3 + sync-secret-service`: passwords only in GNOME Keyring/KWallet Secret Service (service `sge`); host/port/username in SQLite, never the secret; explicit consent-gated fallback if no daemon, never silent plaintext.
- React 19 + TS + Vite + TanStack Query/Virtual: three-pane UI reading SQLite-via-commands; virtualized list mandatory from day one for 10k+ row mailboxes.

### Expected Features

M1 is a read-only INBOX viewer; send, multi-folder, OAuth, IDLE push, and threading are explicitly deferred. All ten P1 table-stakes items map 1:1 to the PROJECT.md Active requirements. Details: FEATURES.md.

**Must have (table stakes):**
- IMAP login form + configurable security (host/port, ImplicitTLS 993 / STARTTLS 143 / plain-local) with test-connection — nothing connects without this.
- Headers-first INBOX sync into SQLite + incremental refresh (ENVELOPE/BODYSTRUCTURE pages, UIDVALIDITY-guarded) — the Core Value mechanism.
- Gmail-like three-pane shell (INBOX sidebar + virtualized list + reader) with keyboard nav.
- Sanitized HTML + plaintext reader (allowlist sanitize, sandboxed iframe, remote images blocked by default) — security, not polish.
- Local FTS5 search/filter (subject/from/snippet, BM25, offline) — retention hook.
- Attachment names list + save-to-disk (streamed, never whole-file-in-memory).
- Sync status pill + offline badge (Syncing n/total, Up-to-date, Offline, Error+retry).
- Empty/loading/error states per pane incl. actionable auth-error copy.
- Keyring remember + auto-login.
- Linux Tauri bundle (.deb + AppImage).

**Should have (competitive):**
- Sub-second cold start to readable list (instrument time-to-first-list < 2s) — the "wow".
- Search-as-you-type with highlight snippets over 10k+ messages — nearly free once FTS5 exists.
- Remote-image per-sender allowlist; attachment quick-look/open-with via xdg-open; keyboard-first triage (j/k, /, ?) + shortcut overlay; connection diagnostics screen (last IMAP error, host:port/mode, cert summary, copy-debug-info).

**Defer (v2+):**
- Compose/send/reply via SMTP (explicitly out of M1), multi-folder sidebar, server flag writes (mark read/delete), IDLE push (M1 = poll + manual refresh), OAuth/2FA, threading/conversation view, full-body backfill index, Windows/macOS builds.

### Architecture Approach

SQLite is the single source of truth; React never touches IMAP. One SyncWorker owns one IMAP session, runs the UIDVALIDITY-guarded incremental algorithm (full resync on mismatch, batched 200-UID header windows with per-batch commit + Channel progress, expunge diff, on-demand `BODY[]` fetch), writes through a centralized `store/queries.rs`, and the UI observes via TanStack Query invalidation. Commands stay thin (validation + delegate), MIME parse flows `mail-parser → ammonia → store sanitized-only`, attachments persist as files with metadata rows. Canonical M1 schema: `mailboxes` (uid_validity, uid_next, highest_modseq, last_sync_at folded in), `messages` keyed `(mailbox_id, uid)` with INTERNALDATE `date_utc`, `message_bodies` (sanitized only, `body_complete` flag), `attachment_parts` (part_number/section for targeted fetch), `messages_fts` FTS5 + triggers. Details: ARCHITECTURE.md.

**Major components:**
1. SyncWorker (tokio task, one IMAP session, Channel<SyncEvent> progress, cancellation token) — owns all IMAP; poll-first in M1, IDLE later on a dedicated connection.
2. ImapConn (session.rs / headers.rs / bodies.rs) — TLS/STARTTLS connect, SELECT, paged header sweep, on-demand body/part fetch; BODY.PEEK only (never set \Seen in read-only M1).
3. MailStore (rusqlite + WAL, schema.sql, queries.rs) — migrations, UIDVALIDITY-guarded upserts, FTS triggers, single reviewable SQL module.
4. BodyFetch + sanitizer (bodies.rs + ammonia) — on-demand fetch, charset decode, sanitize-before-store, attachment part fetch + save-to-disk streaming.
5. CredentialVault (creds.rs via keyring) — Secret Service only; startup probe + guided setup on headless/locked machines.
6. React query layer + Reader (useMessages/useMessageBody/useSync hooks, virtualized list, sandboxed `sandbox=""` iframe reader) — renders exclusively from store; offline = cache still resolves.

### Critical Pitfalls

Top items from PITFALLS.md (10 critical documented; 5 below are M1-fatal, rest phase-gated):

1. **Ignoring UIDVALIDITY → silent cache corruption** — store `(mailbox, uidvalidity, uidnext)`; check on every SELECT; mismatch → wipe mailbox rows + full `UID FETCH`, never incremental. Gate: validity-bump simulation test.
2. **Sequence numbers + full RFC822 up front → wrong messages + hung first sync** — `UID FETCH` exclusively, key cache on UID, order by INTERNALDATE/UID; headers-first pages (100–500 UIDs/window), bodies via `BODY.PEEK[]` on demand. Gate: 15k-fixture header sync fast, bodies lazy.
3. **Rendering raw HTML → XSS-to-Tauri-IPC RCE (CVE-2026-82642 pattern)** — sanitize in Rust (ammonia, strip script/iframe/object/embed/form/on*/srcdoc), render in `sandbox=""` iframe (no allow-same-origin), Tauri CSP, remote content blocked, links via shell-open allowlist. Gate: XSS payload suite inert, `__TAURI_INTERNALS__` unreachable.
4. **TLS corner-cutting (`danger_accept_invalid_certs`) or strict-only lockout** — platform verifier (native-tls system store), user-configurable security mode, TOFU fingerprint-pinning UX on failure. Gate: `grep -r danger_accept_invalid` clean in release; bad-cert fixture shows fingerprint dialog.
5. **Keyring absent on minimal Linux → login loop / plaintext fallback** — probe at startup, guided gnome-keyring setup UX, explicit opt-in encrypted-file fallback only with consent. Gate: minimal-VM test, no secret bytes outside keyring/encrypted store.
6. **Also phase-gated:** Gmail-isms (probe CAPABILITY/NAMESPACE/LIST, use server separator, SPECIAL-USE optional — Dovecot fixture test); IDLE-without-fallback (M1 = 60–120s poll + manual refresh, IDLE post-M1 with watchdog); attachments-as-BLOBs (files on disk keyed by uidvalidity/uid/part + SHA-256, 500MB LRU cap); `from_utf8().unwrap()` on mail bytes (all decode through mail-parser + ≥20 PT-BR fixture corpus, never-panic fuzz); newest-Ubuntu bundling (pinned 22.04 CI image, `.deb` declares webkit2gtk-4.1 deps, clean-install on 22.04 + 24.04).

## Implications for Roadmap

Suggested phase structure (dependency order; each phase demoable; mirrors ARCHITECTURE.md §Phase-Shaped Boundaries):

### Phase 1: Project scaffold + connection core
**Rationale:** Nothing exists without a buildable Tauri shell and a working IMAP SELECT against real university servers (Gmail-isms and TLS kill projects here, not in the UI).
**Delivers:** `create-tauri-app` React-TS skeleton, `tauri.conf.json` Linux bundle sketch, `imap/session.rs` (ImplicitTLS/STARTTLS/plain, 10s connect / 30s read timeouts, CAPABILITY/NAMESPACE/LIST probing, TLS-mode selector), login command wired to a CLI/fixtures harness.
**Addresses:** Login form backend, configurable security, connection diagnostics foundation.
**Avoids:** Gmail-isms pitfall, TLS pitfalls (strict verifier + TOFU UX from the start).
**Uses:** tauri 2.12, async-imap 0.11 + async-native-tls, thiserror typed errors.

### Phase 2: Sync engine (headers-first, UID-disciplined)
**Rationale:** The Core Value mechanism; every later phase (store, list, search) consumes its output. Must land before schema-dependent UI.
**Delivers:** `imap/headers.rs` + normative 7-step incremental algorithm (SELECT → compare UIDVALIDITY → full-or-incremental → 200-UID batched sweep → expunge diff → write sync_state → Channel Finished), poll + manual refresh, per-batch progress events.
**Addresses:** Headers-first sync, sync status events.
**Avoids:** UIDVALIDITY corruption, sequence-number/fetch-everything traps, IDLE-without-fallback (poll-first by design).
**Implements:** SyncWorker + ImapConn.

### Phase 3: Local store + FTS search backend
**Rationale:** SQLite is the source of truth; UI reads must resolve offline before any pane is built. Schema mistakes here corrupt everything downstream.
**Delivers:** `schema.sql` (canonical M1 DDL), `queries.rs` (all SQL centralized), WAL, migrations v1, FTS5 + triggers, `list_messages`/`search`/`sync_state` commands; header sweep persists 10k fixture rows with <50ms list/search.
**Addresses:** SQLite cache, offline-capable reads, FTS5 search backend.
**Avoids:** Attachment-blob bloat (metadata-only schema from day one), FTS write-amplification (single-transaction full resync path).
**Uses:** rusqlite 0.37 bundled + rusqlite_migration 2.x.

### Phase 4: Three-pane UI shell
**Rationale:** First visible value; depends on store commands existing. Virtualization and sanitizer-before-first-render must be in from day one (both are rewrites if retrofitted).
**Delivers:** Sidebar (static INBOX), virtualized MessageList (selection, keyboard nav, stable UID-keyed reconciliation), SyncStatus bar (Channel wiring, throttled invalidation ≤1×/500ms), empty/loading/error states, TanStack Query hooks.
**Addresses:** Three-pane layout, virtualized list, sync badge, pane states.
**Avoids:** Non-virtualized list rewrite, stale-list UX (preserve selection across background resync), blocking-first-sync (progressive paint).
**Implements:** React shell + hooks; no IMAP logic in frontend.

### Phase 5: Reader + attachments (sanitized, on-demand)
**Rationale:** Completes the "read" loop; needs store (Phase 3) + shell selection (Phase 4). Highest security sensitivity — gates the milestone.
**Delivers:** `imap/bodies.rs` + `fetch_body`/`list_parts`/`save_attachment` commands, mail-parser decode pipeline, ammonia sanitize-before-store, sandboxed iframe reader, plaintext fallback, attachment download streaming, PT-BR charset corpus green, XSS suite inert.
**Addresses:** Body rendering, attachment list + download.
**Avoids:** Unsanitized-HTML RCE, charset mojibake/panics, eager-attachment DB bloat, remote-image tracking (blocked by default).
**Uses:** mail-parser 0.11 + full_encoding, ammonia, file-backed attachment store + quota.

### Phase 6: Auth persistence + Linux packaging (ship gate)
**Rationale:** Last because auto-login and bundles can only be verified against the complete app on clean machines; keyring behavior differs between dev laptop and target.
**Delivers:** CredentialVault (keyring get/set/delete, startup probe, locked-collection UX, consented encrypted fallback), auto-login cold start, pinned 22.04 CI image, `.deb` + AppImage, install-matrix test (fresh 22.04 + 24.04), `cargo-deny`/`cargo-audit` gate.
**Addresses:** Keyring remember + auto-login, shippable Linux build.
**Avoids:** Plaintext credential fallback, glibc/webkit install failures.
**Uses:** keyring 3 sync-secret-service, tauri-bundler.

### Phase Ordering Rationale

- **Dependency order:** connection → sync algorithm → store schema → UI shell → reader → auth/packaging; no phase consumes something not yet built (frontend never precedes the commands it invokes).
- **Risk-front-loaded:** the four M1-fatal pitfalls (UIDVALIDITY, UID discipline, TLS, Gmail-isms) all land in Phases 1–2 where fixtures make them cheap to test, not in UI phases where they masquerade as rendering bugs.
- **Security-in-depth at the boundary:** sanitizer lands in Phase 5 before any real mail is displayed, and bundling/keyring land last where clean-VM verification is meaningful — matching the PITFALLS.md "looks done but isn't" checklist order.
- **Demoable slices:** CLI SELECT → fixture sweep → sqlite3-inspectable DB → offline UI → full read → installed app; each phase has a gateable artifact.

### Research Flags

Phases likely needing deeper research during planning (`--research-phase` recommended):
- **Phase 1 (connection):** STARTTLS-vs-implicit-TLS handshake details for async-imap + async-native-tls pairing and TOFU pinning crate choice (`rustls-platform-verifier::Verifier::new_with_extra_roots` vs native-tls custom roots) — MEDIUM confidence on exact API shape; spike against mail.utfpr.edu.br early.
- **Phase 5 (reader):** ammonia allowlist tuning (cid: URIs, table fidelity vs strictness) + iframe sandbox + Tauri CSP interaction — verify against the CVE-2026-82642 srcdoc/object payload family, not just `<script>` removal.
- **Phase 6 (packaging):** webkit2gtk-4.1 runtime dependency set for .deb vs AppImage on 22.04/24.04 targets — confirm empirically on clean VMs.

Phases with standard patterns (skip research-phase):
- **Phase 3 (store):** rusqlite + WAL + FTS5 + migrations are textbook; canonical DDL already specified in STACK.md/ARCHITECTURE.md.
- **Phase 4 (shell):** TanStack Query + virtual list three-pane is a commodity pattern; only project-specific bit is Channel→invalidate wiring (documented in ARCHITECTURE.md Pattern 1).
- **Phase 2 (sync):** IMAP UID/UIDVALIDITY algorithm is RFC-normative with steps spelled out; only server-specific validation (real Dovecot responses) needs fixture capture, not API research.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Pins verified against Tauri official releases (2.12.0 Sep 2026), crates.io/docs.rs (mail-parser 0.11.9, keyring 3.5, rusqlite 0.37); only async-imap minor is MEDIUM (`cargo add` at scaffold is source of truth). |
| Features | HIGH | Grounded in Thunderbird/Geary docs, AgentMail sanitization guidance, SQLite FTS5 docs, and prior-art (proton-mail-mcp/mail-memex) precedents; M1 scope confirmed by PROJECT.md correction history. |
| Architecture | HIGH | Tauri Channel-vs-events from official docs, sync rules from RFC 3501 + crate docs, schema + algorithm normative with sizing notes; component boundaries map to phases. |
| Pitfalls | HIGH (MEDIUM on server specifics) | RFC/TAURI/CVE-backed with gates; mail.utfpr.edu.br TLS/CAPABILITY specifics unverified — must probe the live server in Phase 1. |

**Overall confidence:** HIGH

### Gaps to Address

- **Live server profile (mail.utfpr.edu.br):** CAPABILITY set (IDLE? SPECIAL-USE? CONDSTORE?), separator, TLS mode/certs unknown — handle in Phase 1 by capturing a real CAPABILITY/NAMESPACE/LIST transcript and TLS handshake result; adjust fixtures accordingly.
- **async-imap 0.11 exact minor + async-std/async-native-tls interop:** resolve via `cargo add` at scaffold; if executor bridging hurts, documented downgrade to sync `imap 2.x` on a std::thread (no UI impact — SyncWorker boundary absorbs it).
- **KDE/KWallet vs GNOME Keyring behavior:** passwords are UTF-8 so the known binary-secret limit doesn't bite, but verify auto-login on a KDE session in Phase 6; fallback is `oo7 0.4` portal API.
- **Attachment quota default (500MB) + eviction UX:** placeholder value from research; confirm against real student-laptop constraints during Phase 5 planning.
- **Poll interval (60–120s) + batch size (200):** sensible defaults from research; tune against measured UTFPR latency in Phase 2 (slow-server partial-results behavior matters more than the number).

## Sources

### Primary (HIGH confidence)
- Tauri v2 official docs — Calling Frontend (Channels vs events), Calling Rust (async commands), Debian/AppImage bundling + webkit2gtk-4.1 + oldest-baseline rule; release page 2.12.0 Sep 2026 — stack pins, IPC pattern, packaging.
- RFC 3501 (IMAP4rev1) UID/UIDVALIDITY semantics; RFC 2177 (IDLE 29-min rule); RFC 5162 (QRESYNC) — sync algorithm, poll-first decision.
- crates.io + docs.rs: `mail-parser` 0.11.9 (41 charsets, zero-copy, fuzzed), `keyring` 3.5.0 + `dbus-secret-service-keyring-store` (sync-secret-service feature, blocking-call rule), `tauri-plugin-rusqlite2` deps (`rusqlite ^0.37`, `rusqlite_migration ^2`) — crate selection.
- SQLite FTS5 official docs (MATCH, BM25, highlight, prefix/phrase) — search design.

### Secondary (MEDIUM confidence)
- `async-imap` / `imap-proto` crate lineage + `io-imap` 0.6 newcomer signal — async client choice (minor version via `cargo add`).
- Dovecot docs (namespaces, SPECIAL-USE defaults, extensions); Thunderbird panes/Quick-Filter docs; GNOME Geary feature set — Gmail-isms avoidance, table-stakes scope.
- AgentMail "Rendering Email Safely" (DOMPurify + sandboxed iframe + CSP); Email Markup Consortium sanitization survey; Dovecot FTS cost notes; proton-mail-mcp / mail-memex / GiraffeMail SQLite+headers-first precedents — reader security, search scope.
- `rustls-platform-verifier`, `rustls-native-certs` docs; `keyring-rs`/`oo7` issues #95/#133 (headless Secret Service); ForwardEmail SECURITY.md (Tauri iframe/CSP gotchas) — TLS/keyring/sandbox specifics.
- TanStack Query/DB offline-first + local-first (powersync/expo) docs; stalwart mail-parser vs mailparse charset comparison — store-as-truth pattern, parser choice.

### Tertiary (LOW confidence — validate in Phase 1)
- mail.utfpr.edu.br live behavior (CAPABILITY, separator, TLS chain, latency) — assumed Dovecot-like university server; needs a live transcript before Phase 2 fixtures are trusted.
- `bichon` issue #297 UIDVALIDITY post-mortem; `rust-imap` docs `fetch("1","RFC822")` example as anti-pattern source — cited as cautionary evidence, not API contracts.

---
*Research completed: 2026-10-03*
*Ready for roadmap: yes*
