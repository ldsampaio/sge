# Phase 1: Scaffold + Connection - Context

**Gathered:** 2026-10-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Buildable Tauri v2 shell with a login UI that opens a real IMAP INBOX session against a configurable server (UTFPR: mail.utfpr.edu.br:993/SSL). Includes minimal OS-keyring password save behind remember-me (CONN-03 slice pulled forward from Phase 5 by user decision). Auto-connect stays in Phase 5.

</domain>

<decisions>
## Implementation Decisions

### Scaffold & Toolchain
- Frontend: React 19 + TypeScript (strict) + Vite, managed with npm
- Lint/format from day one: ESLint + Prettier
- Scaffold via official create-tauri-app (React-TS preset)
- App identity: id `br.edu.utfpr.sge`, name `SGE`, window title `SGE`
- Rust baseline: Edition 2021, Tauri 2.12; pin async-imap/mail-parser/rusqlite/keyring per research STACK.md via `cargo add` at scaffold
- Repo hygiene: MIT license + standard Rust/Node gitignore from day one
- CI from day one: GitHub Actions Linux build + check

### Login UI
- Server field pre-filled `mail.utfpr.edu.br`, editable
- Security selector: SSL/TLS 993 (default) / STARTTLS 143 / Plain-local with warning; port auto-fills from mode, editable under an Advanced row
- Show/hide password toggle (eye icon, hidden by default)
- Single Connect button: validates, connects, SELECTs INBOX; no separate Test button
- Failure UX: plain-language error naming the failing part (host vs credentials vs TLS) + manual Retry button (no auto-retry)
- Remember-me checkbox saves username + password to OS keyring (secure); without it, credentials are memory-only for the session

### Connection Core
- IMAP via `async-imap 0.11` + `async-native-tls` on a dedicated sync thread (system CA store for university certs); fallback `imap 2.x` if bridging hurts
- Default connection timeout: 30 seconds
- Untrusted/self-signed cert: hard fail by default with an advanced one-time exception (explicit user override with warning, never silent)
- Save live probe transcript (CAPABILITY + NAMESPACE + LIST + SELECT/STATUS) to phase fixtures dir for later test fixtures
- Demo proof via CLI/fixture harness (backend phase demos via harness); UI shows real connection state

### the agent's Discretion
None — all grey areas resolved with the user.

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- None — greenfield repo (only .planning/ + AGENTS.md exist at discuss time)

### Established Patterns
- None yet — this phase establishes them (commands/events bridge, Channel<SyncEvent> for later sync progress)

### Integration Points
- N/A greenfield; later phases integrate against the connection module + probe fixtures from this phase

</code>

<specifics>
## Specific Ideas

- UTFPR server facts (user-supplied): incoming IMAP mail.utfpr.edu.br port 993/SSL; outgoing SMTP smtp.utfpr.edu.br port 587/STARTTLS (SMTP reserved for post-M1 send milestone)
- Remember-me must save user AND password (user requirement) — satisfied via OS keyring, never plaintext
- Target user: the owner themselves, personal daily UTFPR mail use on Linux

</specifics>

<deferred>
## Deferred Ideas

- Auto-connect on launch → stays in Phase 5 (Keyring + Packaging)
- Compose/send via SMTP → post-M1 milestone
- OAuth / 2FA → out of scope for M1

</deferred>
