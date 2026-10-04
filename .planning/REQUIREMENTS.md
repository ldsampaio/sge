# Requirements: SGE — Linux IMAP Desktop Client

**Defined:** 2026-10-04 (Milestone v1.1 Triage & Folders)
**Core Value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.

## v1.1 Requirements

### Flag Sync (ends read-only era)

- [x] **FLAG-01**: User can mark a message read/unread with the Seen flag synced to the server (optimistic UI, reconciled on sync)
- [x] **FLAG-02**: Flag toggles made while offline queue durably in SQLite and replay on reconnect

### Folders

- [ ] **FOLD-01**: User can browse the server folder tree (Sent, Drafts, custom folders) in the sidebar
- [ ] **FOLD-02**: User can open a folder and browse its messages from local cache
- [ ] **FOLD-03**: User sees unread counts per folder

### Refresh & Backfill

- [ ] **SYNC-03**: App refreshes mail on a poll interval plus a manual refresh trigger (one shared code path, no overlapping syncs)
- [ ] **SYNC-04**: App backfills UIDs missed between syncs with no silent gaps (gap detection on every incremental sync)

## v1 Requirements (shipped)

All Milestone 1 requirements delivered (5/5 phases, verified 2026-10-03). Retained here for traceability.

### Connection

- [x] **CONN-01**: User can log in with username, password, and IMAP server URL (UTFPR preset: mail.utfpr.edu.br)
- [x] **CONN-02**: User can configure IMAP security (host/port, SSL/TLS on 993 or STARTTLS) and test the connection with plain-language errors
- [x] **CONN-03**: App remembers credentials securely in the OS keyring and auto-connects on next launch

### Sync & Store

- [x] **SYNC-01**: App syncs INBOX headers first into local SQLite and downloads bodies on demand (server copies preserved), with UIDVALIDITY-guarded incremental refresh
- [x] **SYNC-02**: User sees sync status (progress n/total, up-to-date timestamp) and an offline badge when reading from cache

### Mailbox UI

- [x] **UI-01**: User sees a Gmail-like three-pane layout with an INBOX sidebar node
- [x] **UI-02**: User can browse a virtualized INBOX list (sender, subject, date, unread dot, sorted newest-first) that stays fast at 10k+ messages
- [x] **UI-03**: User sees empty, loading, and error states for every pane (first-run login, empty INBOX, auth/TLS errors with retry)

### Reader & Attachments

- [x] **READ-01**: User can read messages with RFC-decoded headers (From/To/Date/Subject) and sanitized HTML plus plaintext fallback (remote images blocked by default)
- [x] **READ-02**: User can see attachment names/sizes and download/save them to disk
- [x] **READ-03**: User can distinguish read vs unread messages visually (display-only in M1, no server flag writes)

### Search

- [x] **SRCH-01**: User can search/filter local messages by sender/subject (SQLite FTS5) with instant offline results

### Packaging

- [x] **SHIP-01**: User can install and run the app as a Linux desktop bundle (Tauri v2 .deb/.AppImage)

## Future Requirements

Deferred past v1.1. Tracked but not in the current roadmap.

### Fast follows (v1.x)

- **FF-01**: User can allow remote images per sender (persisted allowlist)
- **FF-02**: User can triage by keyboard (j/k navigation, / search, shortcut overlay)
- **FF-03**: User can quick-look/open attachments via xdg-open
- **FF-04**: User can view connection diagnostics (host:port/mode, last error, copy debug info)

### Later (v2+)

- **V2-01**: User can compose/send/reply via SMTP (smtp.utfpr.edu.br:587/STARTTLS)
- **V2-02**: User can delete/move messages with server expunge
- **V2-03**: App receives real-time IDLE push (poll stays as fallback)
- **V2-04**: App syncs via CONDSTORE/QRESYNC MODSEQ deltas (needs server capability probe)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Compose/send/reply via SMTP (smtp.utfpr.edu.br:587/STARTTLS reserved) | Explicitly deferred past v1.1; needs MIME build, drafts, send queue — own milestone |
| Delete/expunge/move messages | Destructive server ops; whole next milestone, not triage (research anti-feature for v1.1) |
| Real-time IDLE push | Reconnect complexity; poll/manual suffices for v1.1, poll stays as fallback later |
| CONDSTORE/QRESYNC optimization | Needs UTFPR capability probe; plain SEARCH diff suffices at v1.1 scale |
| Bulk triage / multi-select / Flagged keywords | Stretch beyond table stakes; v1.1.x candidates |
| Answered flags / threading / conversation view | Needs References reconstruction; list-first is fine |
| OAuth / 2FA / magic-link login | M1 is user+password; per-provider project of its own |
| Windows/macOS builds | Linux-only constraint stands |
| Full-mailbox bulk body download up front | Stalls first paint, bloats SQLite; violates headers-first decision |
| Auto-load remote images by default | Tracking-pixel leak; contradicts sanitization posture |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| FLAG-01 | Phase 6 | Complete |
| FLAG-02 | Phase 6 | Complete |
| FOLD-01 | Phase 7 | Pending |
| FOLD-02 | Phase 7 | Pending |
| FOLD-03 | Phase 7 | Pending |
| SYNC-03 | Phase 8 | Pending |
| SYNC-04 | Phase 9 | Pending |
| CONN-01 | Phase 1 (M1) | Complete |
| CONN-02 | Phase 1 (M1) | Complete |
| CONN-03 | Phase 5 (M1) | Complete |
| SYNC-01 | Phase 2 (M1) | Complete |
| SYNC-02 | Phase 2 (M1) | Complete |
| UI-01 | Phase 3 (M1) | Complete |
| UI-02 | Phase 3 (M1) | Complete |
| UI-03 | Phase 3 (M1) | Complete |
| READ-01 | Phase 4 (M1) | Complete |
| READ-02 | Phase 4 (M1) | Complete |
| READ-03 | Phase 4 (M1) | Complete |
| SRCH-01 | Phase 3 (M1) | Complete |
| SHIP-01 | Phase 5 (M1) | Complete |

**Coverage:**

- v1.1 requirements: 7 total
- Mapped to phases: 7 (Phases 6-9)
- Unmapped: 0 ✓

---
*Requirements defined: 2026-10-02 (M1)*
*Last updated: 2026-10-04 milestone v1.1 definition*
