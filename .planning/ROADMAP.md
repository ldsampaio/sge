# Roadmap: SGE — Linux IMAP Desktop Client

## Overview

From an empty repo to an installed Linux mail viewer: first a buildable Tauri shell that can SELECT INBOX on mail.utfpr.edu.br, then the headers-first sync engine persisting to SQLite, then the visible three-pane UI with offline search, then the sanitized reader with attachments, and finally keyring auto-login plus a shippable Linux bundle. Each phase is a thin MVP vertical slice — backend phases demo via CLI/fixture harness, UI phases demo in the app.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 1: Scaffold + Connection** - Tauri shell that logs in and SELECTs INBOX on a real server
- [ ] **Phase 2: Sync Engine + Local Store** - Headers-first INBOX sync into SQLite with status
- [ ] **Phase 3: Mailbox UI Shell + Search** - Gmail-like three-pane UI with offline FTS search
- [ ] **Phase 4: Reader + Attachments** - Sanitized message reading with attachment download
- [ ] **Phase 5: Keyring + Packaging** - Secure auto-login and installable Linux bundle

## Phase Details

### Phase 1: Scaffold + Connection
**Goal**: User credentials open a real IMAP INBOX session against a configurable server
**Mode:** mvp
**Depends on**: Nothing (first phase)
**Requirements**: CONN-01, CONN-02
**Success Criteria** (what must be TRUE):
  1. User can enter username, password, and server (UTFPR preset mail.utfpr.edu.br) and reach INBOX SELECT successfully
  2. User can switch security modes (ImplicitTLS 993 / STARTTLS / plain-local) and get a plain-language error on failure with retry
  3. Live server probe captures CAPABILITY/NAMESPACE/LIST transcript so later fixtures match the real server
**Plans**: TBD

### Phase 2: Sync Engine + Local Store
**Goal**: INBOX headers sync incrementally into local SQLite and stay fresh across launches
**Mode:** mvp
**Depends on**: Phase 1
**Requirements**: SYNC-01, SYNC-02
**Success Criteria** (what must be TRUE):
  1. After first sync, INBOX headers are readable from local SQLite with server copies preserved (BODY.PEEK only, never sets \Seen)
  2. Second sync is incremental (UIDVALIDITY-guarded; validity-bump triggers full resync, never silent corruption)
  3. User sees sync progress (n/total), up-to-date timestamp, and an offline badge when reading from cache
**Plans**: TBD

### Phase 3: Mailbox UI Shell + Search
**Goal**: User browses INBOX in a fast Gmail-like three-pane UI and finds mail by search, offline
**Mode:** mvp
**Depends on**: Phase 2
**Requirements**: UI-01, UI-02, UI-03, SRCH-01
**Success Criteria** (what must be TRUE):
  1. User sees a three-pane layout (sidebar with INBOX node, message list, reading-pane slot) after login
  2. User can scroll a 10k+ message list (sender, subject, date, unread dot, newest-first) without jank
  3. User can type sender/subject search and get instant offline results from local FTS
  4. User sees sensible empty, loading, and error states in every pane (first-run, empty INBOX, auth/TLS failure with retry)
**Plans**: TBD
**UI hint**: yes

### Phase 4: Reader + Attachments
**Goal**: User reads full messages safely and saves attachments to disk
**Mode:** mvp
**Depends on**: Phase 3
**Requirements**: READ-01, READ-02, READ-03
**Success Criteria** (what must be TRUE):
  1. User can open a message and read RFC-decoded headers with sanitized HTML (remote images blocked) or plaintext fallback
  2. User can see attachment names/sizes and save a file to disk via picker
  3. User can tell read vs unread messages apart visually (display-only, no server flag writes)
  4. Malicious mail (script/srcdoc/object payloads) renders inert with no Tauri IPC reachability
**Plans**: TBD
**UI hint**: yes

### Phase 5: Keyring + Packaging
**Goal**: User launches straight into mail and can install the app on a clean Linux machine
**Mode:** mvp
**Depends on**: Phase 4
**Requirements**: CONN-03, SHIP-01
**Success Criteria** (what must be TRUE):
  1. User credentials persist in the OS keyring (never plaintext) and the app auto-connects on next launch
  2. User can install and run the app from a .deb or .AppImage on clean Ubuntu 22.04 and 24.04
  3. On a keyring-less machine the user gets guided setup, never a silent plaintext fallback or login loop
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Scaffold + Connection | 0/TBD | Not started | - |
| 2. Sync Engine + Local Store | 0/TBD | Not started | - |
| 3. Mailbox UI Shell + Search | 0/TBD | Not started | - |
| 4. Reader + Attachments | 0/TBD | Not started | - |
| 5. Keyring + Packaging | 0/TBD | Not started | - |
