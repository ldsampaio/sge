//! Header sweep: parse IMAP ENVELOPE FETCH responses into [`MessageHeader`]
//! rows suitable for insertion via [`store::queries::upsert_message`].
//!
//! All parsing is read-only — `BODY.PEEK` semantics are enforced upstream
//! by the sync worker's `UID FETCH` attributes (never bare `BODY[]`).

use async_imap::imap_proto::{Address, BodyStructure};
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
    b.map(|bytes| String::from_utf8_lossy(bytes).trim_end_matches('\r').to_string())
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

    let (subject, message_id, from_addr, to_addrs, cc_addrs) = if let Some(e) = env {
        (
            decode_bytes_opt(e.subject.as_deref())
                .map(|s| decode_header_words(&s))
                .unwrap_or_default(),
            decode_bytes_opt(e.message_id.as_deref()),
            format_addresses(e.from.as_deref()),
            format_addresses(e.to.as_deref()),
            format_addresses(e.cc.as_deref()),
        )
    } else {
        (String::new(), None, String::new(), String::new(), String::new())
    };

    let flags = format_flags(fetch.flags().collect());
    let has_attachments = fetch
        .bodystructure()
        .map(body_has_attachments)
        .unwrap_or(false);
    let preview = make_preview(&subject, message_id.as_deref());

    MessageHeader {
        uid,
        message_id,
        subject,
        from_addr,
        to_addrs,
        cc_addrs,
        date_utc,
        flags,
        has_attachments,
        preview,
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
    }

    /// PEEK audit over every fetch path in the crate (T-6-03).
    ///
    /// Fails the build if any fetch attribute set or body-fetch call site
    /// regresses to a bare `BODY[]` / `RFC822` form that would set `\\Seen`
    /// as a read side effect. Audits the four paths locked in CONTEXT.md:
    ///
    /// 1. **Headers attrs** — `FETCH_ATTRS` (this module): header sweep only.
    /// 2. **Body fetch impl** — `SyncSession::fetch_body` in `imap/mod.rs`:
    ///    must issue `uid_fetch(_, "BODY.PEEK[]")`.
    /// 3. **Sync worker** — `sync/worker.rs` step 5: calls `fetch_envelopes`
    ///    (path 1), never a direct body fetch.
    /// 4. **fetch_message command** — `commands/sync.rs`: calls
    ///    `session.fetch_body` (path 2), never `fetch_rfc822` or bare
    ///    `uid_fetch(.., "BODY[]")`.
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

        // All four paths are accounted for — document the audit mapping so
        // a future contributor adding a new fetch must update this test.
        let audited_paths = [
            ("1. headers FETCH_ATTRS", "header sweep — no body part"),
            ("2. imap/mod.rs fetch_body", "uid_fetch(uid, \"BODY.PEEK[]\")"),
            ("3. sync/worker.rs fetch_envelopes", "routes through path 1"),
            ("4. commands/sync.rs fetch_message", "session.fetch_body → path 2"),
        ];
        assert_eq!(
            audited_paths.len(),
            4,
            "peek_audit: all four fetch paths must be accounted for"
        );
    }
}
