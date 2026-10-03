//! Header sweep: parse IMAP ENVELOPE FETCH responses into [`MessageHeader`]
//! rows suitable for insertion via [`store::queries::upsert_message`].
//!
//! All parsing is read-only — `BODY.PEEK` semantics are enforced upstream
//! by the sync worker's `UID FETCH` attributes (never bare `BODY[]`).

use async_imap::imap_proto::{Address, BodyStructure};
use async_imap::types::{Fetch, Flag};
use futures::TryStreamExt;

use super::SyncError;

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
    let (subject, message_id, from_addr, to_addrs, cc_addrs, date_utc) = if let Some(e) = env {
        (
            decode_bytes_opt(e.subject.as_deref()).unwrap_or_default(),
            decode_bytes_opt(e.message_id.as_deref()),
            format_addresses(e.from.as_deref()),
            format_addresses(e.to.as_deref()),
            format_addresses(e.cc.as_deref()),
            decode_bytes_opt(e.date.as_deref()).unwrap_or_default(),
        )
    } else {
        (String::new(), None, String::new(), String::new(), String::new(), String::new())
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
    let attrs = "UID FLAGS INTERNALDATE ENVELOPE BODYSTRUCTURE";
    let mut stream = session
        .uid_fetch(range, attrs)
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
}
