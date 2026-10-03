# Requirements: SGE — Linux IMAP Desktop Client

**Defined:** 2026-10-02
**Core Value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.

## v1 Requirements

### Connection

- [ ] **CONN-01**: User can log in with username, password, and IMAP server URL (UTFPR preset: mail.utfpr.edu.br)
- [ ] **CONN-02**: User can configure IMAP security (host/port, SSL/TLS on 993 or STARTTLS) and test the connection with plain-language errors
- [ ] **CONN-03**: App remembers credentials securely in the OS keyring and auto-connects on next launch

### Sync & Store

- [ ] **SYNC-01**: App syncs INBOX headers first into local SQLite and downloads bodies on demand (server copies preserved), with UIDVALIDITY-guarded incremental refresh
- [ ] **SYNC-02**: User sees sync status (progress n/total, up-to-date timestamp) and an offline badge when reading from cache

### Mailbox UI

- [ ] **UI-01**: User sees a Gmail-like three-pane layout with an INBOX sidebar node
- [ ] **UI-02**: User can browse a virtualized INBOX list (sender, subject, date, unread dot, sorted newest-first) that stays fast at 10k+ messages
- [ ] **UI-03**: User sees empty, loading, and error states for every pane (first-run login, empty INBOX, auth/TLS errors with retry)

### Reader & Attachments

- [ ] **READ-01**: User can read messages with RFC-decoded headers (From/To/Date/Subject) and sanitized HTML plus plaintext fallback (remote images blocked by default)
- [ ] **READ-02**: User can see attachment names/sizes and download/save them to disk
- [ ] **READ-03**: User can distinguish read vs unread messages visually (display-only in M1, no server flag writes)

### Search

- [ ] **SRCH-01**: User can search/filter local messages by sender/subject (SQLite FTS5) with instant offline results

### Packaging

- [ ] **SHIP-01**: User can install and run the app as a Linux desktop bundle (Tauri v2 .deb/.AppImage)

## v2 Requirements

### Fast follows (v1.x)

- **FF-01**: User can allow remote images per sender (persisted allowlist)
- **FF-02**: User can triage by keyboard (j/k navigation, / search, shortcut overlay)
- **FF-03**: User can quick-look/open attachments via xdg-open
- **FF-04**: User can view connection diagnostics (host:port/mode, last error, copy debug info)
- **FF-05**: App refreshes INBOX on poll interval plus manual refresh
- **FF-06**: User can browse Sent/Drafts/custom folders in the sidebar
- **FF-07**: User can mark read/unread with server flag sync (ends read-only era)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Compose/send/reply via SMTP (smtp.utfpr.edu.br:587/STARTTLS reserved for M2) | Explicitly deferred past M1 after user correction; needs MIME build, drafts, send queue |
| OAuth / 2FA / magic-link login | M1 is user+password; per-provider project of its own |
| Threading / conversation view | Needs References reconstruction; list-first is fine for M1 |
| Windows/macOS builds | Linux-only constraint for M1 |
| Full-mailbox bulk body download up front | Stalls first paint, bloats SQLite; violates headers-first decision |
| Auto-load remote images by default | Tracking-pixel leak; contradicts sanitization posture |
| Real-time IDLE push (M1) | Reconnect complexity; poll/manual suffices to validate viewer |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| CONN-01 | TBD | Pending |
| CONN-02 | TBD | Pending |
| CONN-03 | TBD | Pending |
| SYNC-01 | TBD | Pending |
| SYNC-02 | TBD | Pending |
| UI-01 | TBD | Pending |
| UI-02 | TBD | Pending |
| UI-03 | TBD | Pending |
| READ-01 | TBD | Pending |
| READ-02 | TBD | Pending |
| READ-03 | TBD | Pending |
| SRCH-01 | TBD | Pending |
| SHIP-01 | TBD | Pending |

**Coverage:**
- v1 requirements: 13 total
- Mapped to phases: 0
- Unmapped: 13 ⚠️

---
*Requirements defined: 2026-10-02*
*Last updated: 2026-10-02 after initial definition*
