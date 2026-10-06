# Requirements: SGE — Linux IMAP Desktop Client

**Defined:** 2026-10-06 (Milestone v1.2 Compose & Organize)
**Core Value:** Connect to an IMAP server on Linux and read your mail locally in a fast Gmail-like UI.

## v1.2 Requirements

### Envio SMTP

- [ ] **SEND-01**: User can compose a new message with To/Cc/Bcc and send via SMTP (STARTTLS, keyring credentials)
- [ ] **SEND-02**: User can reply with quote + threading headers (In-Reply-To/References per RFC 5322)
- [ ] **SEND-03**: User can forward a message with its attachments re-attached
- [ ] **SEND-04**: Outgoing mail queues durably offline and retries with backoff (no duplicate sends)
- [ ] **SEND-05**: User can attach files via picker and drag & drop, including inline images
- [ ] **SEND-06**: Sent mail is APPENDEd to the Sent folder on success

### Organização (delete/move)

- [ ] **DEL-01**: User can delete a message via move-to-Trash with undo
- [ ] **DEL-02**: User can permanently expunge with an explicit confirmation dialog
- [ ] **MOVE-01**: User can move messages between folders (UID MOVE with COPY+STORE+EXPUNGE fallback)

### Pastas

- [x] **FOLD-04**: User can create a new folder via IMAP CREATE (respects hierarchy delimiter)
- [x] **FOLD-05**: User can rename a folder (cache + outbox invalidated)
- [x] **FOLD-06**: User can delete a folder via IMAP DELETE (INBOX protected, non-empty guarded)

### Rascunhos

- [ ] **DRAFT-01**: User can save and edit drafts locally (local-first editing)
- [ ] **DRAFT-02**: Drafts persist on the server via APPEND (old copy expunged on save)
- [ ] **DRAFT-03**: Sending a draft deletes it in the same send transaction

## Future Requirements (deferred)

- IDLE push (poll stays fallback)
- CONDSTORE/QRESYNC fast path
- Remote image auto-load policy for composed mail
- Multiple SMTP identities / From aliases

## Out of Scope

- **PGP/S-MIME encryption** — key management + UX surface too large for this milestone; revisit with identities work
- **Templates/snippets** — no evidence of need yet; revisit after compose ships
- **Server-side filters/rules** — Sieve/ManageSieve is a separate protocol surface; out for v1.2

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| SEND-01 | 14 | Pending |
| SEND-02 | 14 | Pending |
| SEND-03 | 14 | Pending |
| SEND-04 | 13 | Pending |
| SEND-05 | 14 | Pending |
| SEND-06 | 13 | Pending |
| DEL-01 | 10 | Pending |
| DEL-02 | 10 | Pending |
| MOVE-01 | 10 | Pending |
| FOLD-04 | 11 | Complete |
| FOLD-05 | 11 | Complete |
| FOLD-06 | 11 | Complete |
| DRAFT-01 | 12 | Pending |
| DRAFT-02 | 12 | Pending |
| DRAFT-03 | 13 | Pending |
