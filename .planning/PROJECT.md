# SGE — Linux IMAP Desktop Client

## What This Is

SGE is a Linux desktop email client built with Rust + Tauri v2 + React + SQLite. The user logs in with username, password, and IMAP server URL (e.g. mail.utfpr.edu.br); the app copies mail from the server to a local SQLite database (messages stay on the server, IMAP semantics) and displays them in a Gmail-like three-pane interface. Milestone 1 is a read-only INBOX viewer for personal UTFPR mail use.

## Core Value

Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI — if this doesn't work, nothing else matters.

## Current Milestone: v1.1 Triage & Folders

**Goal:** User can triage mail across folders with server-synced state in the Linux desktop client.

**Target features:**
- Read/unread sync (toggle Seen flag, ends read-only era)
- Folder browsing (Sent, Drafts, custom folders, per-folder sync)
- Poll + manual refresh (periodic INBOX refresh)
- UID backfill (fill gaps missed between syncs)

## Requirements

### Validated

- ✓ User can log in with username, password, and IMAP server URL — Phase 1 (human-validated with real UTFPR account)
- ✓ App connects over configurable IMAP security (host/port, SSL/TLS or STARTTLS) — Phase 1 (993/SSL live-verified; STARTTLS path unit-tested only, server is 993-only)
- ✓ App syncs mail headers first, downloads bodies on demand into local SQLite (server copies preserved) — Phase 2 (M1 complete)
- ✓ User sees Gmail-like three-pane UI (sidebar, message list, reading pane) — Phase 3 (M1 complete)
- ✓ User can browse INBOX messages locally (offline-capable after sync) — Phase 3 (M1 complete)
- ✓ User can search/filter Inbox messages — Phase 3 (M1 complete)
- ✓ User can view attachment names and download/save attachments — Phase 4 (M1 complete)
- ✓ App remembers credentials securely via OS keyring with auto-login — Phase 5 (M1 complete)
- ✓ App ships as a Linux desktop build (Tauri v2 bundle) — Phase 5 (M1 complete)

### Active (v1.1)

- [ ] User can mark messages read/unread with Seen-flag sync to the server
- [ ] User can browse Sent, Drafts, and custom folders with per-folder sync
- [ ] App refreshes INBOX on poll interval plus manual refresh
- [ ] App backfills UIDs missed between syncs (no silent gaps)

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
| Remember-me keyring save in Phase 1 | User required saving user+password; secure path is keyring, so CONN-03 save slice moved forward (auto-connect stays Phase 5) | ✓ Good |
| imap-proto 0.16 can't parse NAMESPACE | Parser gap poisons async-imap session; profile server from CAPABILITY+LIST instead, regression tripwire pinned | ✓ Good |
| STARTTLS ships without live test | mail.utfpr.edu.br is 993-only; stub + unit coverage, residual risk documented | ⚠️ Revisit if a 143 server appears |
| App detects non-Tauri hosting | Raw `__TAURI_INTERNALS__` TypeError confused browser-URL users; guard + plain-language message added | ✓ Good |
| Cargo default-run = sge | imap_probe harness binary broke bare `cargo run` for Tauri dev | ✓ Good |

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
*Last updated: 2026-10-04 — Milestone v1.1 started*
