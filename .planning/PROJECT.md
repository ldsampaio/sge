# SGE — Linux IMAP Desktop Client

## What This Is

SGE is a Linux desktop email client built with Rust + Tauri v2 + React + SQLite. The user logs in with username, password, and IMAP server URL (e.g. mail.utfpr.edu.br); the app copies mail from the server to a local SQLite database (messages stay on the server, IMAP semantics) and displays them in a Gmail-like three-pane interface. Milestone 1 is a read-only INBOX viewer for personal UTFPR mail use.

## Core Value

Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI — if this doesn't work, nothing else matters.

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] User can log in with username, password, and IMAP server URL
- [ ] App connects over configurable IMAP security (host/port, SSL/TLS or STARTTLS)
- [ ] App syncs mail headers first, downloads bodies on demand into local SQLite (server copies preserved)
- [ ] User sees Gmail-like three-pane UI (sidebar, message list, reading pane)
- [ ] User can browse INBOX messages locally (offline-capable after sync)
- [ ] User can search/filter Inbox messages
- [ ] User can view attachment names and download/save attachments
- [ ] App remembers credentials securely via OS keyring with auto-login
- [ ] App ships as a Linux desktop build (Tauri v2 bundle)

### Out of Scope

- Compose/send/reply via SMTP — deferred past milestone 1 (read-only M1)
- Non-INBOX folders (Sent, Drafts, custom folders) — M1 is INBOX only
- Windows/macOS builds — Linux only for now
- OAuth / magic-link / 2FA flows — user+password auth only for M1

## Context

- Stack is fixed: Rust + Tauri v2 backend, React frontend, SQLite local store.
- Primary server example: mail.utfpr.edu.br (personal UTFPR mail use on Linux).
  - Incoming IMAP: mail.utfpr.edu.br, port 993/SSL.
  - Outgoing SMTP (reserved for post-M1 send milestone): smtp.utfpr.edu.br, port 587/STARTTLS.
- Sync strategy decided in questioning: headers-first for fast list, bodies on demand (not full bulk download up front).
- Layout decided: full three-pane Gmail look (sidebar + list + reader), not a minimal list.
- Auth UX decided: remember everything securely (OS keyring), auto-login next launch.
- M1 scope correction during questioning: user first picked "full basics" then corrected to read-only — send is explicitly out.

## Constraints

- **Tech stack**: Rust + Tauri v2 + React + SQLite — fixed by request
- **Platform**: Linux only for milestone 1
- **Protocol**: IMAP must leave mail on server; security (port/SSL-TLS/STARTTLS) user-configurable
- **Scope**: INBOX only, read-only, attachments list + download (no send, no other folders)
- **Security**: Credentials at rest must use OS keyring, never plaintext

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Headers-first sync, bodies on demand | Fast first paint on large UTFPR mailboxes | — Pending |
| Milestone 1 is read-only (no SMTP) | User correction: ship viewer first, send later | — Pending |
| INBOX only for M1 | Simplest useful slice for personal use | — Pending |
| Configurable IMAP security | Must work against mail.utfpr.edu.br and other servers | — Pending |
| Remember all credentials in OS keyring | Personal daily-use client, auto-login expected | — Pending |
| Gmail-like three-pane layout | Familiar UX target explicitly requested | — Pending |
| Linux-only ship | Explicit constraint for M1 | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-10-02 after initialization*
