# Pitfalls Research

**Domain:** Rust IMAP desktop client (Tauri v2 + SQLite, Linux, university servers)
**Researched:** 2026-10-02
**Confidence:** HIGH (RFC-backed + Tauri/official docs); MEDIUM on mail.utfpr.edu.br specifics (unverified server config)

## Critical Pitfalls

### Pitfall 1: Ignoring UIDVALIDITY — silent cache corruption

**What goes wrong:**
Local SQLite cache maps UIDs to messages. Server (mailbox rebuild, migration, Dovecot maintenance) bumps UIDVALIDITY, invalidating every cached UID. App keeps showing stale/wrong messages or silently refills with nothing (known bichon bug: incremental fetch off a pre-reset high-water mark returns 0 rows and the mailbox looks empty).

**Why it happens:**
Developers store `uid -> message` but forget to store `uidvalidity` per mailbox, or compare it only at login instead of on every SELECT. RFC 3501 STRONGLY encourages servers to bump it on reorder/recreate, and university servers do this during maintenance.

**How to avoid:**
- Store `(mailbox, uidvalidity, uidnext)` in SQLite alongside messages. Buildable task: `sync_meta` table + check on every SELECT; on mismatch, wipe that mailbox's cached rows and do a **full** `UID FETCH 1:*`, never incremental.
- Never persist high-water UID across a UIDVALIDITY change.

**Warning signs:**
Mailbox suddenly empty after server maintenance; messages with wrong bodies; duplicate rows after re-sync.

**Phase to address:**
Sync-engine / local-cache phase (first backend phase). Gate: integration test that simulates a UIDVALIDITY bump and asserts full resync.

---

### Pitfall 2: Fetching sequence numbers instead of UIDs, and full RFC822 up front

**What goes wrong:**
Using message sequence numbers (`FETCH 1:N`) breaks the moment another client expunges a message — sequence numbers shift, cache points at the wrong mail. Fetching full `RFC822` bodies for the whole INBOX on first sync hangs the UI for minutes on a 10k+ university mailbox and can take 25–90s+ per large message with attachments.

**Why it happens:**
`rust-imap` examples show `fetch("1", "RFC822")` — easy to copy. Sequence numbers feel natural; UID discipline feels like boilerplate.

**How to avoid:**
- Use `UID FETCH` exclusively; key cache on UID, order UI by INTERNALDATE/UID.
- Headers-first sync: `UID FETCH x:y (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)` paginated in chunks (e.g. 500 UIDs/window); bodies on demand via `UID FETCH uid (BODY.PEEK[])` or partial `BODY.PEEK[TEXT]<start.size>` for large parts. Buildable tasks: paginated header sync + on-demand body fetch + progress UI.

**Warning signs:**
Wrong message opens when tapped; first sync never finishes; memory spikes on large INBOX.

**Phase to address:**
Sync-engine phase. Gate: 15k-message fixture syncs headers in <10s on loopback; body fetch is lazy.

---

### Pitfall 3: Assuming Gmail-isms — SPECIAL-USE, NAMESPACE, separator

**What goes wrong:**
Hardcoding folder names (`INBOX/Sent`), `/` separator, or assuming `SPECIAL-USE` flags exist. University Dovecot/Zimbra servers often: (a) have no SPECIAL-USE mailboxes configured by default, (b) use `.` separator or `INBOX.` prefixes, (c) hide alias namespaces. M1 is INBOX-only so blast radius is small, but LIST parsing that assumes `/` breaks even INBOX display on some servers, and M2 folder support will explode without abstraction.

**Why it happens:**
Dev tests only against Gmail; Gmail normalizes everything.

**How to avoid:**
- Issue `CAPABILITY`, `NAMESPACE`, `LIST "" "*"` at connect; use returned hierarchy separator verbatim; treat SPECIAL-USE as optional (fall back to name matching). Buildable task: `mailbox_discovery` module + fixture tests with `.`-separator and no-SPECIAL-USE CAPABILITY responses.

**Warning signs:**
Folders missing/duplicated on real server but fine on Gmail; backslash-escaped names.

**Phase to address:**
Connection/discovery phase (before sync). Gate: connects to a Dovecot fixture with `.` separator and passes.

---

### Pitfall 4: IDLE without fallback — dead "new mail" on university networks

**What goes wrong:**
Relying solely on IMAP IDLE for freshness. University firewalls/NAT kill idle sockets silently; some servers don't advertise IDLE at all (client MUST NOT use it then, RFC 2177). Result: no new mail until restart, or a hung background thread holding a dead socket. `rust-imap`'s blocking IDLE also blocks the session — a second connection is needed for concurrent fetch.

**Why it happens:**
IDLE demos look trivial (`idle()` + `DONE`); polling feels old-fashioned.

**How to avoid:**
- Design sync as **poll-first, IDLE-later**: M1 ships with `NOOP`/re-SELECT polling (e.g. 60–120s, manual refresh button); add IDLE post-M1 with 29-min re-issue timer, heartbeat, and poll fallback when CAPABILITY lacks IDLE. Buildable tasks: (1) periodic poll + manual refresh in M1; (2) IDLE worker on a dedicated connection with watchdog in hardening phase.

**Warning signs:**
New mail arrives on phone but never on desktop; app hangs on sleep/wake; IDLE thread never returns.

**Phase to address:**
M1: polling sync phase. Post-M1: live-update hardening phase.

---

### Pitfall 5: TLS verification corner-cutting on flaky university certs

**What goes wrong:**
`mail.utfpr.edu.br` may present expired/intermediate-missing/hostname-mismatched chains (common on university mail). Two failure modes: (a) dev adds `danger_accept_invalid_certs(true)` / custom accept-all verifier to "make it work" and ships it — full MITM exposure of password + mail; (b) strict verifier with no escape hatch — user locked out with an opaque error.

**Why it happens:**
`native-tls` vs `rustls` root-store differences (system store vs Mozilla WebPKI) produce "works on my machine" TLS; pressure to just disable verification.

**How to avoid:**
- Use platform verifier (`native-tls` on Linux honoring system CA store, or `rustls-platform-verifier`); **never** ship accept-all. Add user-configurable security mode per PROJECT.md (SSL/TLS 993 vs STARTTLS 143) + explicit "trust this certificate" flow: on verification failure, show cert fingerprint (SHA-256) and pin it in SQLite/keyring on user consent (TOFU). Buildable tasks: TLS mode selector + cert-error dialog with fingerprint pinning. Prefer `rustls-platform-verifier::Verifier::new_with_extra_roots` for pinned roots.

**Warning signs:**
`certificate verify failed` only on some distros; `danger_accept_invalid` anywhere outside a `#[cfg(test)]` fixture.

**Phase to address:**
Connection/auth phase. Gate: `grep -r danger_accept_invalid` must be empty in release profile; cert-error UX verified against a bad-cert fixture.

---

### Pitfall 6: Charset/encoding hell — mojibake subjects and panics

**What goes wrong:**
Brazilian university mail is full of `iso-8859-1`, `windows-1252`, `iso-8859-15` subjects/bodies and malformed RFC 2047 encoded-words. Naive `String::from_utf8(body).expect(...)` (as in `rust-imap` docs example) panics; `mailparse`'s Latin-1 fallback returns garbage silently; dates from "creative" servers fail `dateparse`.

**Why it happens:**
Copying the docs example's `.expect("message was not valid utf-8")`; assuming UTF-8 everywhere.

**How to avoid:**
- Use `mail-parser` (stalwart, 41 charsets, Postel-law lenient, zero-copy) over `mailparse` for header/body decoding; route all byte→str through it, never raw `from_utf8().unwrap()`. Wrap parse in `Result` with fallback row ("undecodable part — show raw"). Buildable tasks: decode pipeline on `mail-parser` + fixture corpus of win-1252/iso-8859-1/qp/broken-encoded-word mails + never-panic fuzz test.

**Warning signs:**
`Ã©`/`?` in subjects; panics on specific messages; empty bodies on multipart/alternative.

**Phase to address:**
Parse/render pipeline phase. Gate: fixture corpus (≥20 real-world Brazilian mails) renders without panic or mojibake.

---

### Pitfall 7: Rendering raw HTML email — XSS → Tauri IPC = arbitrary code execution

**What goes wrong:**
Email HTML rendered unsanitized (or `FORBID_TAGS: ['script']` only) via `dangerouslySetInnerHTML` in the React reader. Known Tauri RCE chain (CVE-2026-82642, Readest): `<iframe srcdoc>` / `<object>` / `<embed>` payload survives naive DOMPurify config, executes inside `sandbox="allow-same-origin allow-scripts"` iframe, and reaches `parent.parent.__TAURI_INTERNALS__.invoke(...)` → every permitted IPC command. Email is attacker-controlled input by definition.

**Why it happens:**
Forgetting email is untrusted; trusting "we stripped `<script>` so we're safe"; reusing the app's own origin for the reader iframe.

**How to avoid:**
- Sanitize **in Rust backend or at render boundary** with allowlist sanitizer (e.g. `ammonia` crate) that also strips `iframe/object/embed/form/base/link/meta`, all `on*` handlers, and `srcdoc`; block remote content by default (no external images — tracking pixels + mixed-content); render in sandboxed iframe **without** `allow-same-origin` (use `sandbox=""` or `allow-popups` only) so even escaped JS can't touch IPC; set Tauri CSP in `tauri.conf.json`; route link clicks through `openUrl`/shell-open allowlist. Buildable tasks: sanitizer module + CSP config + iframe sandbox + remote-content toggle + XSS fixture tests (srcdoc/iframe/object/event-handler payloads).

**Warning signs:**
`dangerouslySetInnerHTML` with unsanitized input; `allow-same-origin` on mail iframe; remote images loading by default.

**Phase to address:**
Reader-UI phase (before any real mail is displayed). Gate: OWASP-style XSS payload suite renders inert; `__TAURI_INTERNALS__` unreachable from mail iframe.

---

### Pitfall 8: Keyring absent on minimal Linux — login loop / plaintext fallback

**What goes wrong:**
`keyring`/`libsecret` Secret Service needs a running daemon (GNOME Keyring/KWallet + D-Bus). On minimal WMs (i3/sway), headless installs, or fresh logins with locked collection, every secret-store call fails → auto-login breaks, or worse, dev "temporarily" writes the password to a plaintext config file and forgets.

**Why it happens:**
Dev machine has GNOME Keyring unlocked via PAM; target machines don't. `keyring-rs` docs explicitly warn headless/secret-service is the painful case.

**How to avoid:**
- Probe keyring availability at startup; on `NoDefaultCollection`/`PlatformFailure`, show explicit setup guidance (install+autostart `gnome-keyring`, PAM unlock) and offer **explicit opt-in** encrypted-file fallback (e.g. age/XChaCha20 sealed file with user-set master password) — never silent plaintext. Buildable tasks: keyring probe + error UX + optional encrypted-file fallback behind a clear consent screen. Consider `oo7` (portal-aware, file backend for sandboxed builds) if Flatpak is ever targeted.

**Warning signs:**
Login works on dev laptop, fails on fresh VM; `secret service: no default collection` errors; any `.toml`/`.json` containing a password.

**Phase to address:**
Auth/persistence phase. Gate: tested on a minimal Linux VM without keyring daemon; no plaintext credential anywhere (`grep -ri password ~/.config/sge` empty).

---

### Pitfall 9: Tauri Linux bundling — "works in dev, won't install"

**What goes wrong:**
Tauri v2 needs `libwebkit2gtk-4.1` + GTK/AppIndicator at build **and** runtime. Building on newest Ubuntu raises minimum glibc → `.deb` fails on older targets with `GLIBC_X.XX not found`; Ubuntu 20.04 can't even install `libwebkit2gtk-4.1-dev` (not in repos); missing system deps → white screen or linker errors for users.

**Why it happens:**
`cargo tauri dev` works because dev deps are installed; bundling/runtime matrix is never tested.

**How to avoid:**
- Pin build baseline to oldest supported target (Ubuntu 22.04 / Debian 12 container or CI runner — both ship webkit2gtk-4.1); declare `depends: libwebkit2gtk-4.1-0, libgtk-3-0` in bundler config; ship `.deb` + document AppImage as fallback (bundled, ~70MB+, glibc caveat remains). Buildable tasks: Docker/CI build image pinned to 22.04 + install-matrix test (fresh 22.04 + 24.04 VMs) + `tauri info` prerequisite docs.

**Warning signs:**
White screen on launch (missing capabilities/frontend dist); `webkit2gtk-4.0 vs 4.1` linker errors from following v1 tutorials; GLIBC errors on user machines.

**Phase to address:**
Packaging/ship phase (last). Gate: clean-install test on two fresh distro versions before calling M1 done.

---

### Pitfall 10: Attachment + SQLite blowup — unbounded disk growth

**What goes wrong:**
Storing attachment blobs in SQLite, or re-downloading bodies on every view, balloons the DB (a few 30MB PDFs = 100MB+ DB) and slows FTS/list queries; no eviction → disk fills on a student laptop.

**Why it happens:**
"Cache everything in one table" is the fastest schema to write; `BODY[]` always fetches attachments with the text.

**How to avoid:**
- Metadata in SQLite, attachment bytes as files under app-data dir keyed by `(uidvalidity, uid, part-id)` with size + SHA-256; bodies via `BODYSTRUCTURE`-aware part fetch (text parts inline, attachments only on explicit download); LRU/size-cap eviction (e.g. 500MB default, user-adjustable) that deletes files but keeps metadata rows. Buildable tasks: attachment store module + quota/eviction + download-on-demand UI.

**Warning signs:**
DB file >500MB after a week; list scrolling janks; re-download on every open (check network log).

**Phase to address:**
Cache/storage phase + attachment UI phase. Gate: 100-mail × 5MB-attachment fixture keeps DB <50MB with lazy attachments.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Sequence numbers instead of UIDs | Less code | Wrong-message bugs after any expunge | Never |
| Skip UIDVALIDITY column | Simpler schema | Full cache corruption on server maintenance | Never |
| `danger_accept_invalid_certs` to "fix" TLS | Unblocks dev | Credential-stealing MITM in prod | Never (test fixtures only) |
| Raw HTML into `dangerouslySetInnerHTML` | Reader works day 1 | XSS→IPC RCE | Never |
| Plaintext credential file "for now" | Login persists without keyring setup | Credential theft; hard to migrate later | Never — use explicit encrypted fallback |
| Poll-only, no IDLE | Ships faster | Stale inbox, user complaints | M1 only (IDLE is a hardening phase) |
| Attachments as SQLite BLOBs | One-table schema | DB bloat, slow queries | Never for M1+ (files + metadata) |
| `unwrap()`/`expect()` on MIME decode | Less error plumbing | Panic on real-world mail | Never on parse path |
| Build `.deb` on latest Ubuntu | Fastest CI | GLIBC breakage on older targets | Never — pin 22.04 baseline |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| University IMAP (Dovecot/Zimbra) | Assume Gmail CAPABILITY set (IDLE, SPECIAL-USE, QRESYNC) | Probe `CAPABILITY`/`NAMESPACE`; degrade gracefully; fixture-test non-Gmail responses |
| TLS (native-tls vs rustls) | Assume same root store everywhere | Use platform verifier; test on target distros; support TOFU pinning UX |
| Secret Service keyring | Assume daemon + unlocked collection | Probe, guide setup, explicit encrypted-file fallback |
| WebKit2GTK runtime | Assume user has webkit installed | Declare `.deb` deps; test fresh installs; offer AppImage |
| SQLite + Tauri IPC | Large blobs over IPC / main thread | Stream/paginate; binary via files, metadata via IPC; `rusqlite` + WAL on backend thread |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Full-body sync of INBOX | First sync takes forever, UI frozen | Headers-first + paged UID windows + bodies on demand | ~2k+ messages or any 10MB+ mail |
| Unchunked `UID FETCH 1:*` on 15k mailbox | Timeout / OOM | Page by UID ranges (e.g. 500), commit per page, progress bar | Large UTFPR alumni/staff inboxes |
| Loading remote images by default | Slow render, tracking leaks | Block remote content; click-to-load per sender | Any newsletter-heavy inbox |
| FTS without index / LIKE on body | Search takes seconds | SQLite FTS5 on subject/from + snippet; bodies indexed lazily | ~5k+ cached messages |
| IDLE on UI thread (blocking rust-imap) | UI freezes during idle | Dedicated sync thread/connection; poll fallback with timer | Immediately under real use |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Accept-all TLS verifier shipped | Password + mail MITM on campus Wi-Fi | Platform verifier + TOFU pin UX; audit for `danger_accept_invalid` |
| Unsanitized mail HTML in same-origin iframe | XSS → Tauri IPC → arbitrary code execution | Allowlist sanitize (strip iframe/object/embed/srcdoc/on*), `sandbox` without same-origin, CSP |
| Plaintext credential storage | Local credential theft | OS keyring primary; explicit encrypted fallback only with consent |
| Auto-loading remote images/trackers | Read-receipt tracking, IP leak, mixed-content | Block by default; per-sender allowlist |
| Attachment auto-open / preview without sandbox | Malicious file execution | Download-to-disk + open via system handler only on user action; never auto-execute |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| No offline/error state for dead IDLE/socket | Inbox silently stale; user distrusts app | Connection status indicator + "last synced Xm ago" + manual refresh |
| Opaque TLS/keyring errors | User can't log in, gives up | Actionable messages ("install gnome-keyring", "cert expired — review fingerprint") |
| Blocking first sync with no progress | App looks hung on large inbox | Headers-first + progress bar + usable list ASAP |
| Mojibake subjects (encoding bugs) | App looks broken for Portuguese mail (ç, ã, é) | Correct charset pipeline; test with PT-BR corpus |
| Losing scroll/selection on background resync | Disorienting list jumps | Stable UID-keyed list reconciliation; preserve selection |

## "Looks Done But Isn't" Checklist

- [ ] **Sync:** Looks done (messages listed) but missing UIDVALIDITY check — verify by simulating a validity bump
- [ ] **Sync:** Looks done but fetches by sequence number — verify expunge from another client doesn't shift the list
- [ ] **TLS:** Looks done (connects) but ships accept-all verifier — verify `grep danger_accept_invalid` is clean in release
- [ ] **Reader:** Looks done (HTML renders) but unsanitized — verify srcdoc/iframe/object payload suite is inert
- [ ] **Auth:** Looks done (remembers login) but plaintext — verify no secret bytes outside keyring/encrypted store
- [ ] **Keyring:** Looks done on dev machine — verify on minimal VM without keyring daemon
- [ ] **Attachments:** Looks done (names shown) but eagerly downloaded — verify DB stays small with large-attachment fixture
- [ ] **Search:** Looks done (filters current page) but not FTS — verify offline search across full cache
- [ ] **Bundle:** Looks done (`dev` runs) but never installed fresh — verify `.deb` on clean 22.04 + 24.04 VMs
- [ ] **Charset:** Looks done with ASCII test mail — verify PT-BR corpus (latin-1/win-1252 subjects) renders correctly

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| UIDVALIDITY invalidation shipped without handling | MEDIUM | Ship fix that wipes+resyncs on mismatch; users re-sync once, no data loss (server is source of truth) |
| Accept-all TLS shipped | HIGH | Emergency release removing it; rotate password guidance; add pinning UX |
| XSS via mail reader | HIGH | Emergency sanitizer fix + CSP tightening; audit IPC permissions (least-privilege capabilities) |
| Plaintext credentials shipped | HIGH | Migrate into keyring on next launch, shred file, disclose to user |
| SQLite blob bloat | LOW–MEDIUM | Migration: extract blobs to files, add quota/eviction; one-time vacuum |
| Wrong-arch/glibc bundle | LOW | Rebuild on pinned 22.04 image; re-release |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| UIDVALIDITY handling | Sync-engine / cache phase (early backend) | Validity-bump simulation test → full resync |
| UID-only + paged headers-first fetch | Sync-engine phase | 15k-fixture header sync fast; lazy bodies |
| NAMESPACE/separator/SPECIAL-USE probing | Connection/discovery phase | Dovecot `.`-separator + no-SPECIAL-USE fixtures pass |
| Poll-first, IDLE later | M1 sync phase → post-M1 live-update hardening | M1 manual+periodic refresh works; IDLE watchdog post-M1 |
| TLS strict + TOFU pinning | Connection/auth phase | Bad-cert fixture shows fingerprint UX; no accept-all in release |
| Charset pipeline (mail-parser) | Parse/render phase | PT-BR corpus renders, no panics (fuzz clean) |
| HTML sanitization + iframe sandbox + CSP | Reader-UI phase | XSS payload suite inert; IPC unreachable |
| Keyring probe + fallback UX | Auth/persistence phase | Minimal-VM test; no plaintext secrets |
| Attachment file-store + quota | Cache/storage + attachment UI phase | Large-attachment fixture keeps DB small |
| Pinned-baseline bundling + install matrix | Packaging/ship phase (last) | Clean-install on 22.04 + 24.04 |

## Sources

- RFC 3501 (IMAP4rev1) §2.3.1.1 UIDVALIDITY semantics; RFC 2177 (IDLE, 29-min rule); RFC 5162 (QRESYNC validity-mismatch behavior)
- `rust-imap` docs.rs (UID type docs, `fetch("1","RFC822")` example as anti-pattern source); bichon issue #297 (UIDVALIDITY resync bug post-mortem)
- Dovecot docs (namespaces, SPECIAL-USE not configured by default, IMAP extensions)
- `mail-parser` (stalwart) vs `mailparse` docs.rs (charset coverage, Latin-1 fallback behavior)
- Tauri v2 docs (CSP, Debian/AppImage bundling, webkit2gtk-4.1 + glibc baseline); tauri-apps issues #9039 (compat), #12758 (20.04 missing 4.1)
- ForwardEmail SECURITY.md (Tauri CSP/iframe isolation gotchas); CVE-2026-82642 / Readest (DOMPurify `srcdoc` bypass → Tauri IPC RCE chain)
- `keyring-rs` / `oo7` docs.rs + GitHub issues #95, #133 (headless/no-daemon Secret Service failures)
- `rustls-platform-verifier`, `rustls-native-certs` docs (platform root store handling)

---
*Pitfalls research for: SGE — Linux IMAP desktop client (Rust + Tauri v2 + SQLite)*
*Researched: 2026-10-02*
