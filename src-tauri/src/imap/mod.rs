//! IMAP connection core (Phase 1, Plan 01-02).
//!
//! Three security modes over `async-imap 0.11` + `async-native-tls` (system CA
//! store, so university certs validate without custom roots), driven on a
//! dedicated blocking thread via `async_std::task::block_on` — never on the
//! Tauri tokio runtime threads.
//!
//! M1 read-only invariant (lifted in Phase 6): this module issued only
//! SELECT plus read-only probes (CAPABILITY, NAMESPACE, LIST, STATUS).
//! The first and only write verb is [`SyncSession::set_seen`] — UID STORE
//! `\Seen` behind [`manager::SessionManager`]. No full-message fetch sets
//! flags: bodies always use peek-only fetches, so background sync never
//! sets `\Seen` on the server.

pub mod bodies;
pub mod errors;
pub mod headers;
pub mod manager;
pub mod mutf7;
pub mod probe;
pub mod roles;
pub mod session;
pub mod trash;

pub use errors::ImapError;

use futures::TryStreamExt;

/// Connection (and whole-probe) timeout, seconds. Locked by CONTEXT D-timeout.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

/// IMAP transport security. Frontend `SecuritySelector` values map 1:1 to
/// these variants (`implicit_tls` / `starttls` / `plain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityMode {
    /// Implicit TLS on connect (default, port 993).
    ImplicitTls,
    /// Plain connect upgraded with STARTTLS (port 143).
    StartTls,
    /// Unencrypted, localhost only, explicit confirm required.
    PlainLocal,
}

impl SecurityMode {
    /// Parse a frontend security string. Unknown values are a protocol-level
    /// client error, never a silent default.
    pub fn parse(s: &str) -> Result<Self, ImapError> {
        match s {
            "implicit_tls" | "implicit-tls" | "ssl" => Ok(SecurityMode::ImplicitTls),
            "starttls" | "start-tls" => Ok(SecurityMode::StartTls),
            "plain" | "plain-local" => Ok(SecurityMode::PlainLocal),
            other => Err(ImapError::Protocol {
                detail: format!(
                    "unknown security mode {other:?} — expected implicit_tls, starttls, or plain"
                ),
            }),
        }
    }

    /// Canonical wire value shared with the frontend selector.
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityMode::ImplicitTls => "implicit_tls",
            SecurityMode::StartTls => "starttls",
            SecurityMode::PlainLocal => "plain",
        }
    }
}

/// True for loopback hosts allowed to use unencrypted IMAP.
///
/// Allowlist is deliberately narrow (fail-closed): only `localhost`,
/// `127.0.0.1`, and `::1` (bracketed or bare) pass. The rest of `127.0.0.0/8`,
/// `::ffff:127.0.0.1`, and dotted forms like `localhost.` are refused — a
/// stub on `127.0.0.2` must use one of the listed names. Input should already
/// be trimmed (see [`normalize_host`]).
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_matches(['[', ']']);
    h.eq_ignore_ascii_case("localhost") || h == "127.0.0.1" || h == "::1"
}

/// Normalize a user-supplied hostname before it reaches TCP dial, DNS, or
/// TLS SNI/hostname verification.
///
/// Trims surrounding whitespace and splits off a trailing `:port` the user
/// may have pasted into the server field (e.g. `mail.utfpr.edu.br:993` —
/// without this the dial layer would treat the whole string as a hostname).
/// An explicitly pasted port is dropped in favor of the separate port
/// argument rather than silently overriding it. Bare IPv6 literals (multiple
/// colons, no brackets) are left intact for [`socket_addr`]-style bracketing
/// downstream.
pub fn normalize_host(raw: &str) -> Result<String, ImapError> {
    let host = raw.trim();
    if host.is_empty() {
        return Err(ImapError::Protocol {
            detail: "invalid host: server address must not be empty".to_string(),
        });
    }
    if let Some(stripped) = host.strip_prefix('[') {
        // Bracketed literal: "[::1]" or "[::1]:993".
        match stripped.find(']') {
            Some(end) => {
                let inner = &stripped[..end];
                let rest = &stripped[end + 1..];
                if !rest.is_empty()
                    && !(rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit()))
                {
                    return Err(ImapError::Protocol {
                        detail: format!("invalid host: malformed bracketed address {raw:?}"),
                    });
                }
                if inner.is_empty() {
                    return Err(ImapError::Protocol {
                        detail: "invalid host: server address must not be empty".to_string(),
                    });
                }
                return Ok(inner.to_string());
            }
            None => {
                return Err(ImapError::Protocol {
                    detail: format!("invalid host: unbalanced bracket in {raw:?}"),
                });
            }
        }
    }
    if host.contains(':') && !host.contains("::") && host.matches(':').count() == 1 {
        let (name, port_part) = host.rsplit_once(':').expect("single colon");
        if !port_part.is_empty()
            && port_part.chars().all(|c| c.is_ascii_digit())
            && !name.trim().is_empty()
        {
            // Trailing numeric port: use the hostname half; the numeric half
            // is intentionally dropped — the separate port argument wins.
            return Ok(name.trim().to_string());
        }
    }
    Ok(host.to_string())
}

/// Everything needed to open one IMAP INBOX session.
///
/// `password` is held in memory only for the session: never logged (the
/// manual [`fmt::Debug`] impl redacts it), never persisted here — keyring
/// persistence lands in Plan 01-03 behind remember-me consent. WR-10: the
/// bytes are [`zeroize::Zeroizing`] so allocator memory is scrubbed on drop
/// (cheap insurance against core-dump/swap exposure). Residual risk: copies
/// briefly exist inside TLS/IMAP plumbing and in the frontend JS string —
/// both outside Rust's reach — so this narrows, not eliminates, exposure.
#[derive(Clone)]
pub struct AccountConfig {
    pub host: String,
    pub port: u16,
    pub security: SecurityMode,
    pub username: String,
    pub password: zeroize::Zeroizing<String>,
    /// One-time cert exception requested by explicit user override. The
    /// backend NEVER silently bypasses verification for this flag (see
    /// [`probe::run_probe`]): it is logged, surfaced, and refused.
    pub allow_untrusted: bool,
    /// Plain-local requires explicit confirmation (CLI flag / localhost rule).
    pub plain_local_confirmed: bool,
}

impl std::fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &"***")
            .field("allow_untrusted", &self.allow_untrusted)
            .field("plain_local_confirmed", &self.plain_local_confirmed)
            .finish()
    }
}

/// Mailbox summary returned after a successful INBOX SELECT. Field names and
/// types are the IPC contract shared with `connect_account`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxSummary {
    pub selected_mailbox: String,
    pub uid_validity: u32,
    pub exists: u32,
    pub uid_next: Option<u32>,
}

/// Information about a discovered mailbox from `LIST "" "*"`.
///
/// Returned by [`SyncSession::list_mailboxes`] and surfaced to the
/// frontend as the folder tree (FOLD-01). `delimiter` is the
/// mailbox-hierarchy delimiter (e.g. `/` or `.`); `attributes` are
/// the RFC 3501 LIST attributes (`\Marked`, `\Unmarked`, `\Noselect`,
/// `\Noinferiors`, `\All`, `\Archive`, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxInfo {
    /// Raw wire name (modified UTF-7) — the ONLY form valid for
    /// SELECT/STATUS. Never display this directly.
    pub name: String,
    /// Decoded display form for the folder tree.
    pub display_name: String,
    pub delimiter: String,
    pub attributes: Vec<String>,
}

/// A boxed async stream satisfying async-imap's transport bound —
/// `AsyncRead + AsyncWrite + Unpin + Debug + Send`.  Used to erase
/// TLS/TCP stream types behind `Client<BoxedStream>`.
///
/// Trait objects cannot contain multiple non-auto traits, so we bundle
/// the async-imap bound behind a sealed marker trait and erase to
/// `Box<dyn AsyncStream>`.
pub trait AsyncStream: futures::AsyncRead + futures::AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<T: futures::AsyncRead + futures::AsyncWrite + Unpin + std::fmt::Debug + Send> AsyncStream for T {}

/// A fully typed IMAP session over a [`BoxedStream`] — the concrete type
/// the sync engine owns and drives through [`SyncSession`].
pub type BoxedStream = Box<dyn AsyncStream>;
pub type BoxedSession = async_imap::Session<BoxedStream>;

/// Pinned boxed future alias consumed by [`SyncSession`] trait methods.
/// The trait stays object-safe because every method returns this
/// concrete `Pin<Box<dyn Future …>>` rather than `impl Future`.
pub type PinBox<'a, T> = std::pin::Pin<std::boxed::Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Errors from the sync-path IMAP layer (Phase 2+).
///
/// `SyncError` is intentionally coarse at this layer — most variants carry
/// a diagnostic string that the IPC layer maps back to a structured
/// `SyncError` enum for the frontend.
#[derive(Debug)]
pub enum SyncError {
    /// Protocol-level failure (FETCH/SELECT/LOGOUT returned an error).
    Protocol(String),
    /// I/O failure during a body fetch or attachment write.
    Io(String),
    /// Parse failure (envelope, bodystructure, or RFC822 parse).
    Parse(String),
    /// Invariants violation (e.g. missing UIDVALIDITY, unexpected state).
    State(String),
    /// Loud refusal: the op was NOT attempted because a safety
    /// precondition failed (Plan 10-02: unverifiable unmark dance).
    /// Deterministic — callers must NOT retry (a retry would re-COPY and
    /// duplicate messages only to refuse again).
    Refused(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Protocol(s) => write!(f, "IMAP protocol error: {s}"),
            SyncError::Io(s) => write!(f, "IMAP I/O error: {s}"),
            SyncError::Parse(s) => write!(f, "IMAP parse error: {s}"),
            SyncError::State(s) => write!(f, "IMAP state error: {s}"),
            SyncError::Refused(s) => write!(f, "IMAP refused: {s}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<crate::store::StoreError> for SyncError {
    fn from(e: crate::store::StoreError) -> Self {
        SyncError::Protocol(format!("store: {e}"))
    }
}

impl From<async_imap::error::Error> for SyncError {
    fn from(e: async_imap::error::Error) -> Self {
        SyncError::Protocol(format!("{e}"))
    }
}

/// Object-safe trait abstracting an IMAP folder session.
///
/// Implemented by:
/// - [`BoxedSession`] — real IMAP session (Phase 1 connection core)
/// - `MockSession` (in `sync::worker::tests` and `imap::bodies::tests`) — deterministic fixture
///
/// Reads are ENVELOPE sweeps, `BODY.PEEK[]` bodies, SELECT, LOGOUT.
/// `LIST` discovers the folder tree. The single write verb is
/// [`SyncSession::set_seen`] (UID STORE `\\Seen`, Phase 6 — ends the M1
/// read-only era). `EXPUNGE` and scoped `UID EXPUNGE` carry delete/move
/// (Phase 10); `APPEND` persists draft copies (Phase 12, DRAFT-02).
pub trait SyncSession: Unpin + Send {
    /// `SELECT <mailbox>` — selects any mailbox by name and returns
    /// UIDVALIDITY / UIDNEXT / exists counts.
    fn select_mailbox(
        &mut self,
        name: &str,
    ) -> PinBox<'_, Result<MailboxSummary, SyncError>>;

    /// `SELECT INBOX` — convenience delegating to [`select_mailbox`](Self::select_mailbox)
    /// with `"INBOX"`.
    fn select_inbox(&mut self) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
        self.select_mailbox("INBOX")
    }

    /// `UID SEARCH ALL` — returns all UIDs currently in the selected mailbox.
    fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>>;

    /// `UID FETCH <range> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)`
    /// over a range or comma-separated list of UIDs. Returns parsed headers.
    fn fetch_envelopes<'a>(
        &'a mut self,
        range: &'a str,
    ) -> PinBox<'a, Result<Vec<headers::MessageHeader>, SyncError>>;

    /// `UID FETCH <uid> BODY.PEEK[]` — fetch the full RFC822 message
    /// bytes for a single UID. Never sets `\\Seen`.
    fn fetch_body(&mut self, uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>>;

    /// `UID STORE <uid> ±FLAGS.SILENT (\\Seen)` — set or clear the Seen
    /// flag on exactly one message by UID. UID-only addressing: sequence
    /// numbers must never reach this path (T-6-1).</
    fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>>;

    /// `UID STORE <uid> ±FLAGS.SILENT (\\Deleted)` — set or clear the
    /// Deleted flag on exactly one message by UID. UID-only addressing:
    /// sequence numbers must never reach this path (T-6-1).
    fn store_deleted(&mut self, uid: u32, deleted: bool) -> PinBox<'_, Result<(), SyncError>>;

    /// `EXPUNGE` — permanently remove all `\\Deleted` messages in the
    /// selected mailbox. Collect-and-drop: returned sequence numbers are
    /// drained to completion but never trusted as UIDs. NEVER the default —
    /// UID-scoped [`SyncSession::uid_expunge`] is the DEL-02 workhorse; bare
    /// expunge exists only for the UIDPLUS-absent fallback dance (Plan 10-02).
    fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>>;

    /// `UID EXPUNGE <set>` (RFC 4315, requires UIDPLUS) — permanently
    /// remove only `\\Deleted` messages whose UIDs are in `uid_set`.
    /// Comma-joined `"1,2,3"` shape (see [`chunk_uid_set`]).
    fn uid_expunge(&mut self, uid_set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>>;

    /// `UID COPY <set> <dest>` — copy messages to `dest` (raw wire name,
    /// never display_name). Fallback leg 1 when MOVE is unavailable.
    fn uid_copy_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>>;

    /// `UID MOVE <set> <dest>` (RFC 6851, requires MOVE capability) —
    /// server-side move as one action, flags + INTERNALDATE preserved.
    /// Preferred over COPY + STORE + EXPUNGE.
    fn uid_move_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>>;

    /// `CAPABILITY` — server capability atoms as owned strings
    /// (object-safe return; no lifetime leak from the vendored struct).
    fn capabilities(&mut self) -> PinBox<'_, Result<Vec<String>, SyncError>>;

    /// `CREATE <name>` — create one mailbox (Phase 10: Trash fallback only).
    fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>>;

    /// `RENAME <old> <new>` — rename one mailbox, subtree included
    /// (Plan 11-02). Name-only verb: no UID args, no streams to drain.
    /// Raw wire names in, never display names.
    fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>>;

    /// `DELETE <name>` — delete one (empty, childless) mailbox
    /// (Plan 11-02). Name-only verb: no UID args, no streams to drain.
    /// Raw wire name in, never display_name.
    fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>>;

    /// `APPEND <mailbox> (<flags>) <literal>` — persist one draft copy on
    /// the server (Phase 12, DRAFT-02). UID discovery is a separate
    /// [`SyncSession::uid_search_header`] step: async-imap's `append`
    /// returns `()` and swallows APPENDUID, so callers reconcile via
    /// `UID SEARCH HEADER Message-ID` instead of parsing APPENDUID.
    fn append_message(
        &mut self,
        mailbox: &str,
        flags: &str,
        bytes: &[u8],
    ) -> PinBox<'_, Result<(), SyncError>>;

    /// `UID SEARCH HEADER <field> <value>` — find server copies by header
    /// (Phase 12: reconcile the APPENDed draft via its stable Message-ID).
    /// Returns sorted UIDs: exactly one → the new copy; zero → the server
    /// didn't persist it (loud error, keep `dirty=1`); multiple → take the
    /// max (newest wins, the tracked old UID is still the expunge target).
    fn uid_search_header(
        &mut self,
        field: &str,
        value: &str,
    ) -> PinBox<'_, Result<Vec<u32>, SyncError>>;

    /// `LIST "" "*"` — discover all mailboxes on the server (FOLD-01).
    /// Returns raw LIST results with name, delimiter, and attributes.
    fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>>;

    /// `STATUS <mailbox> (UIDVALIDITY UIDNEXT UNSEEN MESSAGES)` — read-only
    /// triage signals for one folder without SELECTing it (FOLD-02).
    ///
    /// `unseen` is the count of messages without `\Seen` (STATUS UNSEEN
    /// datum), NOT the SELECT "first unseen sequence number" semantic.
    /// `messages` is the STATUS MESSAGES datum (total messages in the
    /// folder) — feeds the non-empty delete guard (Plan 11-02). Vendored
    /// mapping note: async-imap 0.11.3 has no dedicated `messages` field —
    /// `parse_status` folds `StatusAttribute::Messages` into
    /// `Mailbox.exists`, so `messages` reads from `.exists` here.
    fn mailbox_status(&mut self, name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>>;

    /// Graceful `LOGOUT`.  Idempotent on error.
    fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>>;
}

/// Read-only per-folder triage signals from `STATUS` (FOLD-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxStatus {
    pub uid_validity: u32,
    pub uid_next: Option<u32>,
    pub unseen: u32,
    /// STATUS MESSAGES datum: total messages in the folder (Plan 11-02
    /// non-empty delete guard). Sourced from the vendored `Mailbox.exists`
    /// field — see the `mailbox_status` trait doc for the mapping.
    pub messages: u32,
}

/// The UID STORE argument for a Seen toggle, in canonical form.
///
/// Exactly two variants exist — no user-input interpolation is possible
/// (the UID travels as a `u32` formatted by the caller, never parsed from
/// input). `.SILENT` suppresses the server's untagged FETCH replies.
pub fn seen_store_arg(seen: bool) -> &'static str {
    if seen {
        "+FLAGS.SILENT (\\Seen)"
    } else {
        "-FLAGS.SILENT (\\Seen)"
    }
}

/// The UID STORE argument for a Deleted toggle, in canonical form.
///
/// Mirrors [`seen_store_arg`]: exactly two variants, no user-input
/// interpolation (the UID travels as a `u32` formatted by the caller).
/// `.SILENT` suppresses the server's untagged FETCH replies.
pub fn deleted_store_arg(deleted: bool) -> &'static str {
    if deleted {
        "+FLAGS.SILENT (\\Deleted)"
    } else {
        "-FLAGS.SILENT (\\Deleted)"
    }
}

/// The APPEND flags literal for a draft copy: `\Draft` (+`\Seen`, per
/// CONTEXT — the copy is the user's own text, already "read").
pub const DRAFT_FLAGS: &str = "(\\Draft \\Seen)";

/// Which server verb carries a move, given the advertised capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovePath {
    /// Server advertises MOVE — one-verb `UID MOVE`.
    UidMove,
    /// No MOVE — COPY + STORE + EXPUNGE fallback sequence (Plan 10-02).
    FallbackCopy,
}

/// Which expunge path is safe, given the advertised capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpungePath {
    /// Server advertises UIDPLUS — scoped `UID EXPUNGE` (the default).
    UidExpunge,
    /// No UIDPLUS — the unmark-others dance behind a plain `EXPUNGE`
    /// (Plan 10-02 executes it; see [`ExpungePath::Refuse`]).
    UnmarkDance,
    /// Loud failure when the [`ExpungePath::UnmarkDance`] preconditions
    /// can't be verified — never expunge blindly. Constructed at runtime
    /// by Plan 10-02, not by [`choose_expunge_path`].
    Refuse,
}

fn has_capability(caps: &[String], atom: &str) -> bool {
    caps.iter().any(|c| c.eq_ignore_ascii_case(atom))
}

/// Pick the move verb from a `CAPABILITY` atom list (case-insensitive).
pub fn choose_move_path(caps: &[String]) -> MovePath {
    if has_capability(caps, "MOVE") {
        MovePath::UidMove
    } else {
        MovePath::FallbackCopy
    }
}

/// Pick the expunge path from a `CAPABILITY` atom list (case-insensitive).
///
/// UIDPLUS present → scoped `UID EXPUNGE`; absent → the unmark-others
/// dance. [`ExpungePath::Refuse`] is never returned here — Plan 10-02
/// upgrades `UnmarkDance` to `Refuse` at runtime when the dance cannot
/// be verified on the live session.
pub fn choose_expunge_path(caps: &[String]) -> ExpungePath {
    if has_capability(caps, "UIDPLUS") {
        ExpungePath::UidExpunge
    } else {
        ExpungePath::UnmarkDance
    }
}

/// Max UIDs per UID-set verb call (matches the sweep batch size).
pub const UID_SET_CHUNK_SIZE: usize = 200;

/// Split UIDs into comma-joined `"1,2,3"` sets of at most
/// [`UID_SET_CHUNK_SIZE`] — keeps multi-message ops poll-responsive.
pub fn chunk_uid_set(uids: &[u32]) -> Vec<String> {
    uids.chunks(UID_SET_CHUNK_SIZE)
        .map(|c| {
            c.iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect()
}

/// Convert an IMAP `NameAttribute` to its canonical string form (e.g. `\Marked`).
fn name_attribute_to_string(attr: &async_imap::types::NameAttribute) -> String {
    match attr {
        async_imap::types::NameAttribute::NoInferiors => "\\\\NoInferiors".to_string(),
        async_imap::types::NameAttribute::NoSelect => "\\\\NoSelect".to_string(),
        async_imap::types::NameAttribute::Marked => "\\\\Marked".to_string(),
        async_imap::types::NameAttribute::Unmarked => "\\\\Unmarked".to_string(),
        async_imap::types::NameAttribute::All => "\\\\All".to_string(),
        async_imap::types::NameAttribute::Archive => "\\\\Archive".to_string(),
        async_imap::types::NameAttribute::Drafts => "\\\\Drafts".to_string(),
        async_imap::types::NameAttribute::Flagged => "\\\\Flagged".to_string(),
        async_imap::types::NameAttribute::Junk => "\\\\Junk".to_string(),
        async_imap::types::NameAttribute::Sent => "\\\\Sent".to_string(),
        async_imap::types::NameAttribute::Trash => "\\\\Trash".to_string(),
        async_imap::types::NameAttribute::Extension(s) => s.to_string(),
        _ => format!("{:?}", attr),
    }
}

impl SyncSession for BoxedSession {
    fn select_mailbox(
        &mut self,
        name: &str,
    ) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
        let mailbox_name = name.to_string();
        Box::pin(async move {
            let mailbox = self
                .select(&mailbox_name)
                .await
                .map_err(|e| SyncError::Protocol(format!("SELECT {mailbox_name}: {e}")))?;
            Ok(MailboxSummary {
                selected_mailbox: mailbox_name,
                uid_validity: mailbox
                    .uid_validity
                    .ok_or_else(|| SyncError::State("server did not return UIDVALIDITY".into()))?,
                uid_next: mailbox.uid_next,
                exists: mailbox.exists,
            })
        })
    }

    fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move {
            let uids_set = self
                .uid_search("ALL")
                .await
                .map_err(|e| SyncError::Protocol(format!("UID SEARCH ALL: {e}")))?;
            let mut uids: Vec<u32> = uids_set.into_iter().collect();
            uids.sort_unstable();
            Ok(uids)
        })
    }

    fn fetch_envelopes<'a>(
        &'a mut self,
        range: &'a str,
    ) -> PinBox<'a, Result<Vec<headers::MessageHeader>, SyncError>> {
        let range_owned = range.to_string();
        Box::pin(async move {
            let mut stream = self
                .uid_fetch(&range_owned, headers::FETCH_ATTRS)
                .await
                .map_err(|e| SyncError::Protocol(format!("UID FETCH: {e}")))?;
            let mut out = Vec::new();
            while let Some(fetch) = stream.try_next().await? {
                let uid = fetch
                    .uid
                    .ok_or_else(|| SyncError::Protocol("missing UID in FETCH response".into()))?;
                out.push(headers::parse_fetch_item(&fetch, uid));
            }
            Ok(out)
        })
    }

    fn fetch_body(&mut self, uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
        let uid_str = uid.to_string();
        Box::pin(async move {
            let mut stream = self
                .uid_fetch(&uid_str, "BODY.PEEK[]")
                .await
                .map_err(|e| SyncError::Protocol(format!("UID FETCH body: {e}")))?;
            while let Some(fetch) = stream.try_next().await? {
                if let Some(body) = fetch.body() {
                    return Ok(body.to_vec());
                }
            }
            Ok(Vec::new())
        })
    }

    fn set_seen(&mut self, uid: u32, seen: bool) -> PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            let stream = self
                .uid_store(uid.to_string(), seen_store_arg(seen))
                .await
                .map_err(|e| SyncError::Protocol(format!("UID STORE Seen uid {uid}: {e}")))?;
            // The STORE silently never completes unless the returned
            // response stream is drained to completion — a dropped stream
            // aborts the write, so the acknowledgement is the drain.
            let _responses: Vec<_> = stream
                .try_collect()
                .await
                .map_err(|e| SyncError::Protocol(format!("UID STORE Seen uid {uid}: {e}")))?;
            Ok(())
        })
    }

    fn store_deleted(&mut self, uid: u32, deleted: bool) -> PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            let stream = self
                .uid_store(uid.to_string(), deleted_store_arg(deleted))
                .await
                .map_err(|e| SyncError::Protocol(format!("UID STORE Deleted uid {uid}: {e}")))?;
            // Same drain-to-completion discipline as `set_seen`: a dropped
            // stream aborts the write, so the acknowledgement is the drain.
            let _responses: Vec<_> = stream
                .try_collect()
                .await
                .map_err(|e| SyncError::Protocol(format!("UID STORE Deleted uid {uid}: {e}")))?;
            Ok(())
        })
    }

    fn expunge(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        Box::pin(async move {
            let stream = self
                .expunge()
                .await
                .map_err(|e| SyncError::Protocol(format!("EXPUNGE: {e}")))?;
            // Collect-and-drop: expunged sequence numbers are drained to
            // completion but never trusted as UIDs — the local cache
            // reconciles via the next SEARCH, not via expunge responses.
            let seqs: Vec<u32> = stream
                .try_collect()
                .await
                .map_err(|e| SyncError::Protocol(format!("EXPUNGE: {e}")))?;
            Ok(seqs)
        })
    }

    fn uid_expunge(&mut self, uid_set: &str) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        let set = uid_set.to_string();
        Box::pin(async move {
            let stream = self
                .uid_expunge(&set)
                .await
                .map_err(|e| SyncError::Protocol(format!("UID EXPUNGE {set}: {e}")))?;
            let uids: Vec<u32> = stream
                .try_collect()
                .await
                .map_err(|e| SyncError::Protocol(format!("UID EXPUNGE {set}: {e}")))?;
            Ok(uids)
        })
    }

    fn uid_copy_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
        let set = uid_set.to_string();
        let dest_owned = dest.to_string();
        Box::pin(async move {
            self.uid_copy(&set, &dest_owned)
                .await
                .map_err(|e| {
                    SyncError::Protocol(format!("UID COPY {set} -> {dest_owned}: {e}"))
                })?;
            Ok(())
        })
    }

    fn uid_move_to(&mut self, uid_set: &str, dest: &str) -> PinBox<'_, Result<(), SyncError>> {
        let set = uid_set.to_string();
        let dest_owned = dest.to_string();
        Box::pin(async move {
            self.uid_mv(&set, &dest_owned)
                .await
                .map_err(|e| {
                    SyncError::Protocol(format!("UID MOVE {set} -> {dest_owned}: {e}"))
                })?;
            Ok(())
        })
    }

    fn capabilities(&mut self) -> PinBox<'_, Result<Vec<String>, SyncError>> {
        Box::pin(async move {
            let caps = self
                .capabilities()
                .await
                .map_err(|e| SyncError::Protocol(format!("CAPABILITY: {e}")))?;
            Ok(caps
                .iter()
                .map(|c| match c {
                    async_imap::types::Capability::Imap4rev1 => "IMAP4rev1".to_string(),
                    async_imap::types::Capability::Auth(mech) => format!("AUTH={mech}"),
                    async_imap::types::Capability::Atom(atom) => atom.clone(),
                })
                .collect())
        })
    }

    fn create_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
        let name_owned = name.to_string();
        Box::pin(async move {
            self.create(&name_owned)
                .await
                .map_err(|e| SyncError::Protocol(format!("CREATE {name_owned}: {e}")))?;
            Ok(())
        })
    }

    fn rename_mailbox(&mut self, old: &str, new: &str) -> PinBox<'_, Result<(), SyncError>> {
        let (old_owned, new_owned) = (old.to_string(), new.to_string());
        Box::pin(async move {
            // Vendored async-imap 0.11.3 names confirmed via `cargo fetch`
            // source inspection (Plan 11-02 compile gate): `rename(from, to)`
            // and `delete(mailbox_name)` — no delta.
            self.rename(&old_owned, &new_owned)
                .await
                .map_err(|e| SyncError::Protocol(format!("RENAME {old_owned} -> {new_owned}: {e}")))?;
            Ok(())
        })
    }

    fn delete_mailbox(&mut self, name: &str) -> PinBox<'_, Result<(), SyncError>> {
        let name_owned = name.to_string();
        Box::pin(async move {
            self.delete(&name_owned)
                .await
                .map_err(|e| SyncError::Protocol(format!("DELETE {name_owned}: {e}")))?;
            Ok(())
        })
    }

    fn append_message(
        &mut self,
        mailbox: &str,
        flags: &str,
        bytes: &[u8],
    ) -> PinBox<'_, Result<(), SyncError>> {
        let (mailbox_owned, flags_owned, bytes_owned) =
            (mailbox.to_string(), flags.to_string(), bytes.to_vec());
        Box::pin(async move {
            self.append(&mailbox_owned, Some(flags_owned.as_str()), None, &bytes_owned)
                .await
                .map_err(|e| {
                    SyncError::Protocol(format!("APPEND {mailbox_owned}: {e}"))
                })?;
            Ok(())
        })
    }

    fn uid_search_header(
        &mut self,
        field: &str,
        value: &str,
    ) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
        let (field_owned, value_owned) = (field.to_string(), value.to_string());
        Box::pin(async move {
            let query = format!("HEADER {field_owned} \"{value_owned}\"");
            let uids_set = self.uid_search(&query).await.map_err(|e| {
                SyncError::Protocol(format!(
                    "UID SEARCH HEADER {field_owned} {value_owned}: {e}"
                ))
            })?;
            let mut uids: Vec<u32> = uids_set.into_iter().collect();
            uids.sort_unstable();
            Ok(uids)
        })
    }

    fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
        Box::pin(async move {
            self.logout()
                .await
                .map_err(|e| SyncError::Protocol(format!("LOGOUT: {e}")))?;
            Ok(())
        })
    }

    fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
        Box::pin(async move {
            let mut stream = self
                .list(Some(""), Some("*"))
                .await
                .map_err(|e| SyncError::Protocol(format!("LIST: {e}")))?;
            let mut out = Vec::new();
            while let Some(name) = stream.try_next().await? {
                let raw = name.name().to_string();
                out.push(MailboxInfo {
                    display_name: mutf7::decode_modified_utf7(&raw),
                    name: raw,
                    delimiter: name
                        .delimiter()
                        .map(|d| d.to_string())
                        .unwrap_or_default(),
                    attributes: name
                        .attributes()
                        .iter()
                        .map(name_attribute_to_string)
                        .collect(),
                });
            }
            Ok(out)
        })
    }

    fn mailbox_status(&mut self, name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>> {
        let mailbox_name = name.to_string();
        Box::pin(async move {
            let mailbox = self
                .status(&mailbox_name, "(UIDVALIDITY UIDNEXT UNSEEN MESSAGES)")
                .await
                .map_err(|e| SyncError::Protocol(format!("STATUS {mailbox_name}: {e}")))?;
            Ok(MailboxStatus {
                uid_validity: mailbox.uid_validity.ok_or_else(|| {
                    SyncError::State(format!(
                        "STATUS {mailbox_name} did not return UIDVALIDITY"
                    ))
                })?,
                uid_next: mailbox.uid_next,
                // STATUS UNSEEN datum = count of messages without \Seen.
                unseen: mailbox.unseen.unwrap_or(0),
                // Vendored mapping: async-imap 0.11.3 folds the MESSAGES
                // datum into `Mailbox.exists` (parse_status) — there is no
                // dedicated field (Plan 11-02 compile-gate delta).
                messages: mailbox.exists,
            })
        })
    }
}

/// Append-only command/response transcript for the probe fixture.
///
/// Secrets never enter the transcript (only CAPABILITY/NAMESPACE/LIST/STATUS/
/// SELECT exchanges are echoed), and [`Transcript::render`] redacts any
/// accidental occurrence of the username/password as belt and suspenders.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Vec<String>,
}

impl Transcript {
    pub fn new() -> Self {
        Transcript { lines: Vec::new() }
    }

    /// Client-side note (connect line, warnings). Must never contain secrets.
    pub fn note(&mut self, line: impl Into<String>) {
        self.lines.push(format!("# {}", line.into()));
    }

    /// Echo of a client command (never a LOGIN line — auth is issued inside
    /// async-imap and never echoed here).
    pub fn client(&mut self, line: impl Into<String>) {
        self.lines.push(format!("C: {}", line.into()));
    }

    /// One server response line.
    pub fn server(&mut self, line: impl Into<String>) {
        self.lines.push(format!("S: {}", line.into()));
    }

    /// Render with password occurrences replaced by `***`.
    ///
    /// Only the password is scrubbed: it is the actual secret, and it never
    /// enters the transcript by construction (LOGIN is issued inside
    /// async-imap and never echoed), so this is belt and suspenders. The
    /// username is intentionally left intact — short usernames would
    /// annihilate readable text, and the username already appears in
    /// user-facing error strings by design. Passwords shorter than 3 chars
    /// are skipped to avoid the same annihilation problem.
    pub fn render(&self, cfg: &AccountConfig) -> String {
        let mut out = self.lines.join("\n");
        if cfg.password.len() >= 3 {
            out = out.replace(cfg.password.as_str(), "***");
        }
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_constant_is_30s() {
        assert_eq!(CONNECT_TIMEOUT_SECS, 30);
    }

    #[test]
    fn security_mode_roundtrips() {
        for mode in [
            SecurityMode::ImplicitTls,
            SecurityMode::StartTls,
            SecurityMode::PlainLocal,
        ] {
            assert_eq!(SecurityMode::parse(mode.as_str()).unwrap(), mode);
        }
    }

    #[test]
    fn security_mode_rejects_junk() {
        assert!(SecurityMode::parse("ssl-tls-maybe").is_err());
        assert!(SecurityMode::parse("").is_err());
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("LOCALHOST"));
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("::1"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("mail.utfpr.edu.br"));
        assert!(!is_loopback(""));
    }

    #[test]
    fn normalize_host_trims_and_strips_pasted_port() {
        // WR-03: the value reaching dial/TLS SNI must be clean.
        assert_eq!(
            normalize_host("  mail.utfpr.edu.br  ").unwrap(),
            "mail.utfpr.edu.br"
        );
        assert_eq!(
            normalize_host("mail.utfpr.edu.br:993").unwrap(),
            "mail.utfpr.edu.br"
        );
        assert_eq!(
            normalize_host("  mail.utfpr.edu.br:993  ").unwrap(),
            "mail.utfpr.edu.br"
        );
        assert_eq!(normalize_host("[::1]").unwrap(), "::1");
        assert_eq!(normalize_host("[::1]:143").unwrap(), "::1");
        // Bare IPv6 is left intact for downstream bracketing.
        assert_eq!(normalize_host("::1").unwrap(), "::1");
        assert!(normalize_host("   ").is_err());
        assert!(normalize_host("[::1").is_err());
    }

    #[test]
    fn transcript_redacts_secrets() {
        let cfg = AccountConfig {
            host: "mail.example".into(),
            port: 993,
            security: SecurityMode::ImplicitTls,
            username: "user1".into(),
            password: zeroize::Zeroizing::new("s3cret-pw".to_string()),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let mut t = Transcript::new();
        t.server("* OK ready");
        // Simulate a leaked line; render must still scrub the password.
        // (The username is intentionally NOT scrubbed: short names would
        // annihilate readable text, and it appears in error strings anyway.)
        t.server("leaked s3cret-pw here");
        let rendered = t.render(&cfg);
        assert!(
            !rendered.contains("s3cret-pw"),
            "password leaked: {rendered}"
        );
        assert!(rendered.contains("***"));
    }

    #[test]
    fn account_debug_redacts_password() {
        let cfg = AccountConfig {
            host: "h".into(),
            port: 993,
            security: SecurityMode::ImplicitTls,
            username: "u".into(),
            password: zeroize::Zeroizing::new("pw123".to_string()),
            allow_untrusted: false,
            plain_local_confirmed: false,
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("pw123"), "password in Debug: {dbg}");
    }

    #[test]
    fn set_seen_store_arg_spelling() {
        // T-6-01: UID-only STORE with the silent Seen literals — the exact
        // wire spelling the mock-based worker tests assert against.
        assert_eq!(seen_store_arg(true), "+FLAGS.SILENT (\\Seen)");
        assert_eq!(seen_store_arg(false), "-FLAGS.SILENT (\\Seen)");
    }

    #[test]
    fn set_seen_store_arg_never_sequence_addressed() {
        // The argument carries no identifier at all — the UID travels as
        // the separate uid_set parameter, so a sequence number can never
        // be smuggled into the flag expression.
        for seen in [true, false] {
            let arg = seen_store_arg(seen);
            assert!(!arg.contains("UID"), "flag arg must not name identifiers: {arg}");
            assert!(arg.ends_with("(\\Seen)"), "canonical backslash-Seen form: {arg}");
        }
    }

    #[test]
    fn deleted_store_arg_spelling() {
        // T-10-01: UID-only STORE with the silent Deleted literals — the
        // exact wire spelling the mock-based worker tests assert against.
        assert_eq!(deleted_store_arg(true), "+FLAGS.SILENT (\\Deleted)");
        assert_eq!(deleted_store_arg(false), "-FLAGS.SILENT (\\Deleted)");
    }

    #[test]
    fn deleted_store_arg_never_sequence_addressed() {        // Same purity contract as `seen_store_arg`: no identifier in the
        // flag expression, canonical backslash-Deleted form.
        for deleted in [true, false] {
            let arg = deleted_store_arg(deleted);
            assert!(!arg.contains("UID"), "flag arg must not name identifiers: {arg}");
            assert!(arg.ends_with("(\\Deleted)"), "canonical backslash-Deleted form: {arg}");
        }
    }

    #[test]
    fn draft_flags_spelling() {
        // T-12: APPEND carries the exact (\Draft \Seen) literal — the
        // wire spelling the fake-based draft tests assert against.
        assert_eq!(DRAFT_FLAGS, "(\\Draft \\Seen)");
    }

    fn caps(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn move_path_capability_matrix() {
        // MOVE + UIDPLUS → one-verb UID MOVE.
        assert_eq!(
            choose_move_path(&caps(&["IMAP4rev1", "UIDPLUS", "MOVE"])),
            MovePath::UidMove
        );
        // Neither → COPY + STORE + EXPUNGE fallback.
        assert_eq!(
            choose_move_path(&caps(&["IMAP4rev1"])),
            MovePath::FallbackCopy
        );
        // Mixed: MOVE without UIDPLUS still moves in one verb.
        assert_eq!(
            choose_move_path(&caps(&["IMAP4rev1", "MOVE"])),
            MovePath::UidMove
        );
        // Mixed: UIDPLUS without MOVE still falls back to COPY.
        assert_eq!(
            choose_move_path(&caps(&["IMAP4rev1", "UIDPLUS"])),
            MovePath::FallbackCopy
        );
        // Atom matching is case-insensitive (servers vary).
        assert_eq!(choose_move_path(&caps(&["move"])), MovePath::UidMove);
    }

    #[test]
    fn expunge_path_capability_matrix() {
        // UIDPLUS (± MOVE) → scoped UID EXPUNGE.
        assert_eq!(
            choose_expunge_path(&caps(&["IMAP4rev1", "UIDPLUS", "MOVE"])),
            ExpungePath::UidExpunge
        );
        assert_eq!(
            choose_expunge_path(&caps(&["IMAP4rev1", "UIDPLUS"])),
            ExpungePath::UidExpunge
        );
        // No UIDPLUS → unmark-others dance (Plan 10-02 executes it).
        assert_eq!(
            choose_expunge_path(&caps(&["IMAP4rev1", "MOVE"])),
            ExpungePath::UnmarkDance
        );
        assert_eq!(
            choose_expunge_path(&caps(&["IMAP4rev1"])),
            ExpungePath::UnmarkDance
        );
        // Case-insensitive.
        assert_eq!(
            choose_expunge_path(&caps(&["uidplus"])),
            ExpungePath::UidExpunge
        );
        // Refuse is the runtime loud-failure upgrade, never the pure pick.
        assert_ne!(choose_expunge_path(&caps(&["IMAP4rev1"])), ExpungePath::Refuse);
    }

    #[test]
    fn chunk_uid_set_shapes() {
        assert!(chunk_uid_set(&[]).is_empty());
        assert_eq!(chunk_uid_set(&[7]), vec!["7".to_string()]);
        assert_eq!(chunk_uid_set(&[1, 2, 3]), vec!["1,2,3".to_string()]);
        // 450 UIDs → 3 chunks of ≤200.
        let uids: Vec<u32> = (1..=450).collect();
        let chunks = chunk_uid_set(&uids);
        assert_eq!(chunks.len(), 3);
        for c in &chunks {
            assert!(c.split(',').count() <= UID_SET_CHUNK_SIZE);
        }
        assert_eq!(chunks[0].split(',').count(), 200);
        assert_eq!(chunks[1].split(',').count(), 200);
        assert_eq!(chunks[2].split(',').count(), 50);
        assert!(chunks[0].starts_with("1,2,3"));
        assert!(chunks[2].starts_with("401,"));
        assert!(chunks[2].ends_with(",450"));
    }
}
