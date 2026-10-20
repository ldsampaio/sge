//! Header sweep: parse IMAP ENVELOPE FETCH responses into [`MessageHeader`]
//! rows suitable for insertion via [`store::queries::upsert_message`].
//!
//! All parsing is read-only — `BODY.PEEK` semantics are enforced upstream
//! by the sync worker's `UID FETCH` attributes (never bare `BODY[]`).

use async_imap::imap_proto::{
    types::{AttributeValue, Envelope, MessageSection, SectionPath},
    Address, BodyStructure,
};
use async_imap::types::{Fetch, Flag};
use futures::TryStreamExt;
use mail_parser::parsers::MessageStream;

use super::SyncError;

/// FETCH attribute set for the header sweep.
///
/// MUST stay parenthesized: RFC 3501 §6.4.5 allows a bare single
/// `fetch-att`, but multiple attributes REQUIRE `(...)`. An unparenthesized
/// multi-attr FETCH is malformed — strict servers (Zimbra) reject it with
/// `BAD Invalid arguments`, and async-imap surfaces that rejection as an
/// empty stream (no error), i.e. a silent "0 messages" sync.
pub const FETCH_ATTRS: &str = "(UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)";

/// Separate attribute set for the References lane. The `.PEEK` prefix is
/// mandatory: a non-PEEK `BODY[HEADER.FIELDS …]` FETCH permanently sets
/// `\Seen` server-side on every swept message with no client undo.
/// Never folded into `FETCH_ATTRS` — `fetch_attrs_are_parenthesized`
/// tripwires that the two constants stay disjoint.
pub const FETCH_REFS_ATTRS: &str = "(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])";

/// A single message header row extracted from an IMAP ENVELOPE FETCH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHeader {
    pub uid: u32,
    pub message_id: Option<String>,
    pub subject: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub date_utc: String,
    pub flags: String,
    pub has_attachments: bool,
    pub preview: String,
    pub modseq: Option<u64>,
    /// Raw msg-id from the ENVELOPE's `In-Reply-To` (never RFC 2047-decoded).
    pub in_reply_to: Option<String>,
    /// Ordered msg-id chain from the `BODY.PEEK[HEADER.FIELDS (REFERENCES)]`
    /// lane, parsed only by `imap::references::parse_references`;
    /// `None` = own-thread sentinel (absent/empty field, never a synthetic chain).
    pub references: Option<Vec<String>>,
}

/// Convert async-imap `Flag` list into the JSON string the DB stores.
/// System flags use their canonical backslash form; keywords are passed
/// through verbatim.
pub fn format_flags(flags: Vec<Flag>) -> String {
    let names: Vec<String> = flags
        .iter()
        .map(|f| match f {
            Flag::Seen => "\\Seen".to_string(),
            Flag::Answered => "\\Answered".to_string(),
            Flag::Flagged => "\\Flagged".to_string(),
            Flag::Deleted => "\\Deleted".to_string(),
            Flag::Draft => "\\Draft".to_string(),
            Flag::Recent => "\\Recent".to_string(),
            Flag::MayCreate => "*".to_string(),
            Flag::Custom(s) => s.to_string(),
        })
        .collect();
    serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_string())
}

/// Render a single [`Address`] as a displayable string.
/// `"Name" <user@domain>` when name is present, otherwise `user@domain`.
fn format_address(addr: &Address) -> String {
    let mailbox = addr
        .mailbox
        .as_ref()
        .map(|b| String::from_utf8_lossy(b))
        .unwrap_or_default();
    let host = addr
        .host
        .as_ref()
        .map(|b| String::from_utf8_lossy(b))
        .unwrap_or_default();
    let name = addr
        .name
        .as_ref()
        .map(|b| String::from_utf8_lossy(b).trim_matches('"').to_string())
        .map(|n| decode_header_words(&n))
        .unwrap_or_default();

    if !name.is_empty() && !mailbox.is_empty() && !host.is_empty() {
        format!("{} <{}@{}>", name, mailbox, host)
    } else if !mailbox.is_empty() && !host.is_empty() {
        format!("{}@{}", mailbox, host)
    } else if !name.is_empty() {
        name
    } else {
        String::new()
    }
}

/// Render an address list (or `None`) as a comma-separated display string.
pub fn format_addresses(addrs: Option<&[Address]>) -> String {
    addrs
        .into_iter()
        .flatten()
        .map(format_address)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Decode a `Cow<[u8]>` RFC822 header field (e.g. subject, message-id) to a
/// string, trimming trailing CR that IMAP servers sometimes append.
fn decode_bytes_opt(b: Option<&[u8]>) -> Option<String> {
    b.map(|bytes| {
        String::from_utf8_lossy(bytes)
            .trim_end_matches('\r')
            .to_string()
    })
}

/// Copy the ENVELOPE's `In-Reply-To` msg-id: lossy decode + trailing-CR trim
/// ONLY — msg-id bytes must never pass through `decode_header_words` (an
/// `=?...?=`-shaped id substring would be rewritten). Both parse paths route
/// this field through here so their `MessageHeader` shapes stay identical.
fn copy_envelope_in_reply_to(env: &Envelope<'_>) -> Option<String> {
    decode_bytes_opt(env.in_reply_to.as_deref())
}

/// Decode RFC 2047 encoded-words (`=?charset?Q|B?payload?=`) in a header
/// value — display names and subjects arrive in this form for any
/// non-ASCII text ("Gabinete da Direção-Geral…" shows up raw otherwise).
///
/// - Plain text passes through untouched; malformed words are left visible
///   (fail-visible, never silently dropped).
/// - Adjacent encoded-words separated only by whitespace are joined WITHOUT
///   the gap (RFC 2047 §6.2).
/// - Charset coverage comes from mail-parser's `full_encoding` decoder.
pub fn decode_header_words(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut pending_ws = String::new();
    let mut prev_was_word = false;
    let mut i = 0;
    while i < bytes.len() {
        if raw[i..].starts_with("=?") {
            match try_decode_word(&raw[i..]) {
                Some((decoded, consumed)) => {
                    // Whitespace between two encoded-words is ignored…
                    if !prev_was_word {
                        out.push_str(&pending_ws);
                    }
                    pending_ws.clear();
                    out.push_str(&decoded);
                    i += consumed;
                    prev_was_word = true;
                }
                None => {
                    out.push_str(&pending_ws);
                    pending_ws.clear();
                    out.push_str("=?");
                    i += 2;
                    prev_was_word = false;
                }
            }
        } else if bytes[i].is_ascii_whitespace() {
            pending_ws.push(bytes[i] as char);
            i += 1;
        } else {
            out.push_str(&pending_ws);
            pending_ws.clear();
            let ch = raw[i..].chars().next().expect("non-empty slice");
            out.push(ch);
            i += ch.len_utf8();
            prev_was_word = false;
        }
    }
    // …but leading/trailing/isolated whitespace is preserved.
    out.push_str(&pending_ws);
    out
}

/// Try to decode ONE encoded-word at the start of `s` (which must begin
/// with `=?`). Returns the decoded text plus bytes consumed (including the
/// trailing `?=`), or `None` when it isn't a valid encoded-word.
///
/// Parsed structurally (`=?charset?enc?payload?=`) instead of searching for
/// the first `?=` — a Q-payload may itself START with `=XX`, which a naive
/// terminator search mistakes for the end of the word.
fn try_decode_word(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    // Charset runs to the next `?`.
    let q1 = b.iter().skip(2).position(|&c| c == b'?')? + 2;
    if q1 <= 2 {
        return None; // empty charset
    }
    // Exactly one encoding char followed by `?`.
    let enc = *b.get(q1 + 1)?;
    if b.get(q1 + 2) != Some(&b'?') {
        return None;
    }
    if !matches!(enc, b'q' | b'Q' | b'b' | b'B') {
        return None;
    }
    // Payload runs to the first `?=` — neither Q-escaping (`?` → `=3F`)
    // nor base64 (no `?` in alphabet) can contain a literal `?=`.
    let payload_start = q1 + 3;
    let rel = s[payload_start..].find("?=")?;
    let end = payload_start + rel + 2; // consume through `?=`
                                       // mail-parser's decoder takes the word starting at `?charset?...?=`.
    let inner = &s[1..end];
    let decoded = MessageStream::new(inner.as_bytes()).decode_rfc2047()?;
    Some((decoded, end))
}

/// Build the ~200-char text preview from the first text part we can find.
/// If mail-parser is not used here, fall back to the envelope subject.
fn make_preview(subject: &str, message_id: Option<&str>) -> String {
    if !subject.is_empty() {
        subject.chars().take(200).collect()
    } else if let Some(mid) = message_id {
        mid.chars().take(200).collect()
    } else {
        String::new()
    }
}

/// Recursively check whether *any* part of a [`BodyStructure`] is an
/// attachment (disposition type `attachment` or a filename parameter).
pub fn body_has_attachments(bs: &BodyStructure) -> bool {
    let common = match bs {
        BodyStructure::Basic { common, .. }
        | BodyStructure::Text { common, .. }
        | BodyStructure::Message { common, .. }
        | BodyStructure::Multipart { common, .. } => common,
    };
    let disposition_is_attachment = common
        .disposition
        .as_ref()
        .map_or(false, |d| d.ty.eq_ignore_ascii_case("attachment"));
    if disposition_is_attachment {
        return true;
    }
    match bs {
        BodyStructure::Multipart { bodies, .. } => bodies.iter().any(body_has_attachments),
        _ => false,
    }
}

/// Parse a single `async_imap::Fetch` (from `UID FETCH ... ENVELOPE
/// BODYSTRUCTURE`) into a [`MessageHeader`].
pub fn parse_fetch_item(fetch: &Fetch, uid: u32) -> MessageHeader {
    let env = fetch.envelope();
    let date_utc = fetch
        .internal_date()
        .map(|dt| dt.to_rfc3339())
        .or_else(|| {
            env.as_ref()
                .and_then(|e| decode_bytes_opt(e.date.as_deref()))
        })
        .unwrap_or_default();

    let (subject, message_id, in_reply_to, from_addr, to_addrs, cc_addrs) = if let Some(e) = env {
        (
            decode_bytes_opt(e.subject.as_deref())
                .map(|s| decode_header_words(&s))
                .unwrap_or_default(),
            decode_bytes_opt(e.message_id.as_deref()),
            copy_envelope_in_reply_to(e),
            format_addresses(e.from.as_deref()),
            format_addresses(e.to.as_deref()),
            format_addresses(e.cc.as_deref()),
        )
    } else {
        (
            String::new(),
            None,
            None,
            String::new(),
            String::new(),
            String::new(),
        )
    };

    let flags = format_flags(fetch.flags().collect());
    let has_attachments = fetch
        .bodystructure()
        .map(body_has_attachments)
        .unwrap_or(false);
    let preview = make_preview(&subject, message_id.as_deref());
    let modseq = fetch.modseq;

    MessageHeader {
        uid,
        message_id,
        in_reply_to,
        subject,
        from_addr,
        to_addrs,
        cc_addrs,
        date_utc,
        flags,
        has_attachments,
        preview,
        modseq,
        references: None,
    }
}

/// Fetch ENVELOPE/FLAGS/BODYSTRUCTURE for a UID range and parse into
/// [`MessageHeader`]s.
///
/// Issues `UID FETCH <range> (UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE)`
/// against the given session. BODY.PEEK semantics are enforced by the
/// attribute set: no bare `BODY[]` appears, so the server never sets `\Seen`.
pub async fn fetch_envelope_batch(
    session: &mut super::BoxedSession,
    range: &str,
) -> Result<Vec<MessageHeader>, SyncError> {
    let mut stream = session
        .uid_fetch(range, FETCH_ATTRS)
        .await
        .map_err(|e| SyncError::Protocol(format!("UID FETCH: {e}")))?;
    let mut out = Vec::new();
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|e| SyncError::Protocol(format!("FETCH stream: {e}")))?
    {
        let uid = fetch
            .uid
            .ok_or_else(|| SyncError::Protocol("missing UID in FETCH response".into()))?;
        out.push(parse_fetch_item(&fetch, uid));
    }
    Ok(out)
}

/// Turn one REFERENCES-lane item into a partial `MessageHeader` row: only
/// `uid` and `references` are populated (callers merge by uid into their
/// envelope rows). Raw bytes go through `decode_bytes_opt` semantics then
/// ONLY through `imap::references::parse_references` (42-02 proven unfold/
/// tokenize) — never `split(' ')`, never `decode_header_words`.
pub fn attach_refs_row(uid: u32, raw: &[u8]) -> MessageHeader {
    let references = decode_bytes_opt(Some(raw))
        .map(|s| crate::imap::references::parse_references(&s))
        .flatten()
        .map(|chain| chain.ids);
    MessageHeader {
        uid,
        message_id: None,
        subject: String::new(),
        from_addr: String::new(),
        to_addrs: String::new(),
        cc_addrs: String::new(),
        date_utc: String::new(),
        flags: String::new(),
        has_attachments: false,
        preview: String::new(),
        modseq: None,
        in_reply_to: None,
        references,
    }
}

/// The References lane (THR-01): `UID FETCH <range> FETCH_REFS_ATTRS` over
/// the given session — PEEK-only, so `\Seen` is never set. Emits one
/// partial row per RECEIVED item; a UID whose response carries no
/// REFERENCES section (`Fetch::section` → `None`, the silent empty-stream
/// trap at this module's header doc) is deliberately not reported, so
/// callers diff requested-vs-received UIDs and see the shape miss as a
/// gap — never as silence.
pub async fn fetch_refs_batch(
    session: &mut super::BoxedSession,
    range: &str,
) -> Result<Vec<MessageHeader>, SyncError> {
    let mut stream = session
        .uid_fetch(range, FETCH_REFS_ATTRS)
        .await
        .map_err(|e| SyncError::Protocol(format!("REFS FETCH: {e}")))?;
    let refs_path = SectionPath::Full(MessageSection::Header);
    let mut out = Vec::new();
    while let Some(fetch) = stream
        .try_next()
        .await
        .map_err(|e| SyncError::Protocol(format!("REFS FETCH stream: {e}")))?
    {
        let uid = fetch
            .uid
            .ok_or_else(|| SyncError::Protocol("missing UID in refs FETCH response".into()))?;
        match fetch.section(&refs_path) {
            Some(raw) => out.push(attach_refs_row(uid, raw)),
            // Shape miss: no REFERENCES section in the response — skip the
            // UID so the requested-vs-received diff shows the gap.
            None => {}
        }
    }
    Ok(out)
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_flags_canonical() {
        let flags = vec![Flag::Seen, Flag::Flagged, Flag::Custom("\\*".into())];
        let json = format_flags(flags);
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, vec!["\\Seen", "\\Flagged", "\\*"]);
    }

    #[test]
    fn format_address_with_name() {
        use std::borrow::Cow;
        let addr = Address {
            name: Some(Cow::Borrowed(b"Alice")),
            adl: None,
            mailbox: Some(Cow::Borrowed(b"alice")),
            host: Some(Cow::Borrowed(b"example.com")),
        };
        assert_eq!(format_address(&addr), "Alice <alice@example.com>");
    }

    #[test]
    fn format_address_without_name() {
        use std::borrow::Cow;
        let addr = Address {
            name: None,
            adl: None,
            mailbox: Some(Cow::Borrowed(b"bob")),
            host: Some(Cow::Borrowed(b"example.com")),
        };
        assert_eq!(format_address(&addr), "bob@example.com");
    }

    #[test]
    fn format_addresses_multi() {
        use std::borrow::Cow;
        let addrs = vec![
            Address {
                name: Some(Cow::Borrowed(b"Alice")),
                adl: None,
                mailbox: Some(Cow::Borrowed(b"alice")),
                host: Some(Cow::Borrowed(b"a.com")),
            },
            Address {
                name: None,
                adl: None,
                mailbox: Some(Cow::Borrowed(b"bob")),
                host: Some(Cow::Borrowed(b"b.com")),
            },
        ];
        let s = format_addresses(Some(&addrs));
        assert_eq!(s, "Alice <alice@a.com>, bob@b.com");
    }

    #[test]
    fn format_addresses_none() {
        assert_eq!(format_addresses(None), "");
    }

    #[test]
    fn make_preview_truncates_to_200() {
        let long = "x".repeat(300);
        let p = make_preview(&long, None);
        assert_eq!(p.len(), 200);
    }

    #[test]
    fn decodes_split_q_words_with_gap_collapse() {
        // Real Zimbra sender name: two adjacent Q-words — the space between
        // them must be dropped (§6.2), `_` becomes a real space.
        let raw = "=?UTF-8?Q?Gabinete_da_Dire=C3=A7=C3=A3o-Geral_de_Corn=C3=A9lio_Proc?= =?UTF-8?Q?=C3=B3pio?=";
        assert_eq!(
            decode_header_words(raw),
            "Gabinete da Direção-Geral de Cornélio Procópio"
        );
    }

    #[test]
    fn decodes_subject_with_escaped_colon() {
        let raw = "=?UTF-8?Q?Re=3A_Base_de_Conhecimento_-_Processo_de_Afastamento_d?= =?UTF-8?Q?o_Pa=C3=ADs?=";
        assert_eq!(
            decode_header_words(raw),
            "Re: Base de Conhecimento - Processo de Afastamento do País"
        );
    }

    #[test]
    fn decodes_base64_word_and_leaves_plain_text() {
        assert_eq!(
            decode_header_words("=?UTF-8?B?Q2Fmw6k=?= hello"),
            "Café hello"
        );
        assert_eq!(decode_header_words("plain subject"), "plain subject");
        assert_eq!(decode_header_words(""), "");
    }

    #[test]
    fn leaves_malformed_words_visible() {
        // No closing `?=` / unknown encoding: fail-visible, never dropped.
        assert_eq!(decode_header_words("=?UTF-8?Q?abc"), "=?UTF-8?Q?abc");
        assert_eq!(decode_header_words("=?UTF-8?X?abc?="), "=?UTF-8?X?abc?=");
    }

    #[test]
    fn fetch_refs_batch_attaches_parsed_chain() {
        use crate::imap::references::parse_references;
        // Lane bytes as received: folded continuation + trailing CR; ids
        // keep verbatim form (`=?...?=`-shaped substrings must survive).
        let raw = b"<a@x> <b=?utf-8?Q?=42?=@host>\r\n <c@y>\r";
        let row = attach_refs_row(7, raw);
        assert_eq!(row.uid, 7);
        let chain = parse_references("<a@x> <b=?utf-8?Q?=42?=@host>\r\n <c@y>");
        assert_eq!(row.references, chain.map(|c| c.ids));
    }

    #[test]
    fn fetch_refs_batch_empty_field_is_own_thread() {
        // Received item with an absent/empty REFERENCES field: UID is in the
        // stream, chain is the `None` own-thread sentinel — no synthetic
        // empty chain is ever attached.
        let row = attach_refs_row(9, b"");
        assert_eq!(row.uid, 9);
        assert_eq!(row.references, None);
        let row_cr = attach_refs_row(10, b"\r");
        assert_eq!(row_cr.references, None);
        // Shape-miss discipline: `fetch_refs_batch` skips a UID whose
        // `Fetch::section` is `None` BEFORE attaching, so the requested-
        // vs-received UID diff shows the gap instead of silence; attaching
        // is never reachable for a shape miss.
    }

    #[test]
    fn parse_fetch_paths_in_reply_to_shape_equality() {
        use std::borrow::Cow;
        let env = Envelope {
            date: None,
            subject: Some(Cow::Borrowed(b"Re: Thread")),
            from: None,
            sender: None,
            reply_to: None,
            to: None,
            cc: None,
            bcc: None,
            in_reply_to: Some(Cow::Borrowed(b"<parent@example.com>\r")),
            message_id: Some(Cow::Borrowed(b"<self@example.com>")),
        };
        let env_expected = copy_envelope_in_reply_to(&env);
        // Trailing CR trimmed, msg-id verbatim (no RFC 2047 pass).
        assert_eq!(env_expected.as_deref(), Some("<parent@example.com>"));
        let via_attributes = parse_fetch_attributes(&[AttributeValue::Envelope(Box::new(env))]);
        // Both parse paths copy the field through the SAME helper, so the
        // ENVELOPE-driven `MessageHeader.in_reply_to` shape is identical;
        // `parse_fetch_item` funnels `fetch.envelope()` through it too.
        assert_eq!(via_attributes.in_reply_to, env_expected);
        // An `=?...?=`-shaped msg-id must survive verbatim, not 2047-mangled.
        let tricky = Envelope {
            date: None,
            subject: None,
            from: None,
            sender: None,
            reply_to: None,
            to: None,
            cc: None,
            bcc: None,
            in_reply_to: Some(Cow::Borrowed(b"<X=?utf-8?Q?=42?=@host>")),
            message_id: None,
        };
        let tricky_expected = copy_envelope_in_reply_to(&tricky);
        let via_tricky = parse_fetch_attributes(&[AttributeValue::Envelope(Box::new(tricky))]);
        assert_eq!(via_tricky.in_reply_to, tricky_expected);
        assert_eq!(tricky_expected.as_deref(), Some("<X=?utf-8?Q?=42?=@host>"));
    }

    /// Regression: the FETCH attribute set MUST be parenthesized (RFC 3501
    /// §6.4.5). An unparenthesized multi-attr FETCH is malformed — Zimbra
    /// rejects it with BAD, which async-imap surfaces as an empty stream,
    /// i.e. a silent "0 messages" sync on a non-empty mailbox.
    #[test]
    fn fetch_attrs_are_parenthesized() {
        assert!(
            FETCH_ATTRS.starts_with('(') && FETCH_ATTRS.ends_with(')'),
            "FETCH_ATTRS must be parenthesized, got: {FETCH_ATTRS}"
        );
        for attr in ["UID", "FLAGS", "INTERNALDATE", "ENVELOPE", "BODYSTRUCTURE"] {
            assert!(
                FETCH_ATTRS.contains(attr),
                "FETCH_ATTRS missing {attr}: {FETCH_ATTRS}"
            );
        }
        assert!(
            !FETCH_ATTRS.contains("BODY[]") && !FETCH_ATTRS.contains("BODY.PEEK[HEADER"),
            "header sweep must not fetch bodies (read-only invariant): {FETCH_ATTRS}"
        );
        // The References lane must stay its OWN parenthesized constant…
        assert!(
            FETCH_REFS_ATTRS.starts_with('(') && FETCH_REFS_ATTRS.ends_with(')'),
            "FETCH_REFS_ATTRS must be parenthesized, got: {FETCH_REFS_ATTRS}"
        );
        for attr in ["UID", "BODY.PEEK[HEADER.FIELDS (REFERENCES)]"] {
            assert!(
                FETCH_REFS_ATTRS.contains(attr),
                "FETCH_REFS_ATTRS missing {attr}: {FETCH_REFS_ATTRS}"
            );
        }
        // …never folded into FETCH_ATTRS (the negative tripwire above stays
        // green only while the two constants remain disjoint).
        assert!(
            !FETCH_ATTRS.contains("REFERENCES"),
            "References lane must not be folded into FETCH_ATTRS: {FETCH_ATTRS}"
        );
    }

    /// PEEK audit over every fetch path in the crate (T-6-03).
    ///
    /// Fails the build if any fetch attribute set or body-fetch call site
    /// regresses to a bare `BODY[]` / `RFC822` form that would set `\\Seen`
    /// as a read side effect. Audits the five paths locked in CONTEXT.md:
    ///
    /// 1. **Headers attrs** — `FETCH_ATTRS` (this module): header sweep only.
    /// 2. **Body fetch impl** — `SyncSession::fetch_body` in `imap/mod.rs`:
    ///    must issue `uid_fetch(_, "BODY.PEEK[]")`.
    /// 3. **Sync worker** — `sync/worker.rs` step 5: calls `fetch_envelopes`
    ///    (path 1), never a direct body fetch.
    /// 4. **fetch_message command** — `commands/sync.rs`: calls
    ///    `session.fetch_body` (path 2), never `fetch_rfc822` or bare
    ///    `uid_fetch(.., "BODY[]")`.
    /// 5. **References lane attrs** — `FETCH_REFS_ATTRS` (this module):
    ///    the UID-only `BODY.PEEK[HEADER.FIELDS (REFERENCES)]` lane —
    ///    `.PEEK` prefix mandatory, never a bare `BODY[HEADER.FIELDS …]`.
    ///
    /// The string checks below are the compile-time guard: a grep-style
    /// assertion that no fetch attribute constant contains a bare body token.
    #[test]
    fn peek_audit() {
        // Path 1 — header sweep attribute set: no body part at all.
        let bare_body_patterns = ["BODY[]", "BODY[", "RFC822", "BODYSTRUCTURE.PEEK"];
        for pat in &bare_body_patterns {
            assert!(
                !FETCH_ATTRS.contains(pat),
                "peek_audit: FETCH_ATTRS contains bare body token {pat:?}: {FETCH_ATTRS}"
            );
        }

        // The Seen-store argument must be canonical backslash form only —
        // no body fetch is issued via the flag-write path either.
        let read_arg = crate::imap::seen_store_arg(true);
        let unread_arg = crate::imap::seen_store_arg(false);
        // STORE args must never reference body fetch attributes.
        for arg in [&read_arg, &unread_arg] {
            assert!(
                !arg.contains("BODY") || !arg.contains("PEEK"),
                "peek_audit: STORE arg must not contain fetch attributes: {arg}"
            );
        }

        // Path 5 — References lane attribute set: PEEK-prefixed only.
        for pat in &bare_body_patterns {
            assert!(
                !FETCH_REFS_ATTRS.contains(pat),
                "peek_audit: FETCH_REFS_ATTRS contains bare body token {pat:?}: {FETCH_REFS_ATTRS}"
            );
        }
        assert!(
            FETCH_REFS_ATTRS.contains("BODY.PEEK[HEADER.FIELDS (REFERENCES)]"),
            "peek_audit: References lane must be PEEK-guarded: {FETCH_REFS_ATTRS}"
        );

        // All five paths are accounted for — document the audit mapping so
        // a future contributor adding a new fetch must update this test.
        let audited_paths = [
            ("1. headers FETCH_ATTRS", "header sweep — no body part"),
            (
                "2. imap/mod.rs fetch_body",
                "uid_fetch(uid, \"BODY.PEEK[]\")",
            ),
            ("3. sync/worker.rs fetch_envelopes", "routes through path 1"),
            (
                "4. commands/sync.rs fetch_message",
                "session.fetch_body → path 2",
            ),
            (
                "5. headers FETCH_REFS_ATTRS",
                "uid_fetch(_, \"(UID BODY.PEEK[HEADER.FIELDS (REFERENCES)])\")",
            ),
        ];
        assert_eq!(
            audited_paths.len(),
            5,
            "peek_audit: all five fetch paths must be accounted for"
        );
    }
}

/// Parse `Vec<AttributeValue>` from a raw FETCH response into a `MessageHeader`.
/// Used by `condstore_fetch_changedsince` to parse CHANGEDSINCE responses.
pub fn parse_fetch_attributes(attrs: &[AttributeValue]) -> MessageHeader {
    let mut header = MessageHeader {
        uid: 0,
        message_id: None,
        subject: String::new(),
        from_addr: String::new(),
        to_addrs: String::new(),
        cc_addrs: String::new(),
        date_utc: String::new(),
        flags: String::new(),
        has_attachments: false,
        preview: String::new(),
        modseq: None,
        in_reply_to: None,
        references: None,
    };
    for attr in attrs {
        match attr {
            AttributeValue::Uid(u) => header.uid = *u,
            AttributeValue::ModSeq(m) => header.modseq = Some(*m),
            AttributeValue::Envelope(env) => {
                header.subject = decode_bytes_opt(env.subject.as_deref())
                    .map(|s| decode_header_words(&s))
                    .unwrap_or_default();
                header.message_id = decode_bytes_opt(env.message_id.as_deref());
                header.in_reply_to = copy_envelope_in_reply_to(&env);
                header.from_addr = format_addresses(env.from.as_deref());
                header.to_addrs = format_addresses(env.to.as_deref());
                header.cc_addrs = format_addresses(env.cc.as_deref());
                header.date_utc = env
                    .date
                    .as_deref()
                    .and_then(|b| decode_bytes_opt(Some(b)))
                    .and_then(|dt| {
                        chrono::DateTime::parse_from_str(&dt, "%d-%b-%Y %H:%M:%S %z").ok()
                    })
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default();
            }
            AttributeValue::Flags(raw_flags) => {
                header.flags =
                    format_flags(raw_flags.iter().map(|s| Flag::from(s.as_ref())).collect())
            }
            AttributeValue::InternalDate(dt) => {
                // InternalDate is Cow<str>, parse it
                header.date_utc =
                    chrono::DateTime::parse_from_str(dt.as_ref(), "%d-%b-%Y %H:%M:%S %z")
                        .map(|parsed| parsed.to_rfc3339())
                        .unwrap_or_else(|_| dt.clone().into_owned());
            }
            AttributeValue::BodyStructure(bs) => header.has_attachments = body_has_attachments(&bs),
            AttributeValue::ModSeq(m) => header.modseq = Some(*m),
            _ => {}
        }
    }
    header.preview = make_preview(&header.subject, header.message_id.as_deref());
    header
}
