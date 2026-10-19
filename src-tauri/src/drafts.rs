//! Draft RFC 5322 renderer (Phase 12, Plan 12-01).
//!
//! Pure codec: [`DraftFields`] in, raw message bytes out. The output is
//! minimal (plain text + headers) — full lettre MIME ships in Phase 14 —
//! but must round-trip through `mail-parser` (subject, recipients, body).
//!
//! Header-injection guard (T-12-02): CR/LF are stripped from every header
//! field before emission, so a hostile `"Subject\r\nBcc: evil"` can never
//! smuggle a second header onto the wire.

use serde::{Deserialize, Serialize};

/// One attachment to embed in a rendered message (`multipart/mixed`).
///
/// `data_base64` is standard base64 (padded) of the raw file bytes — the
/// IPC/`queue_send` shape, produced by the compose window's
/// `FileReader.readAsDataURL(...)`. It may carry a `data:<mime>;base64,`
/// prefix (only the payload is used) and may contain line breaks; the
/// renderer strips whitespace and re-wraps at 76 columns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInput {
    /// Filename shown to the recipient (CR/LF stripped before emission).
    pub filename: String,
    /// MIME type; blank becomes `application/octet-stream`.
    pub content_type: String,
    /// Base64 payload — bare base64 or a `data:` URL.
    pub data_base64: String,
}

/// Composed draft content for one save (mirrors the `drafts` row fields).
///
/// The threading (`in_reply_to`/`references`) and `attachments` fields
/// default to "none": a caller that only knows the drafts-row columns
/// renders the exact Phase-12 single-part text message (byte-identical
/// output), while a reply/send caller opts in to `In-Reply-To`,
/// `References` and `multipart/mixed`.
#[derive(Debug, Clone, Default)]
pub struct DraftFields {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub message_id: String,
    /// RFC 822 Message-ID of the message being replied to → `In-Reply-To`.
    pub in_reply_to: Option<String>,
    /// Space-separated ancestor Message-IDs → `References` chain.
    pub references: Option<String>,
    /// Attachments; empty keeps the legacy single-part `text/plain` body.
    pub attachments: Vec<AttachmentInput>,
}

/// Stable Message-ID for one compose session: generated once at row
/// creation and kept across re-saves, so the post-APPEND
/// `UID SEARCH HEADER Message-ID` reconcile always finds the same key
/// (a fresh ID per save would orphan the previous server copy, T-12-03).
///
/// `compose_id` arrives over IPC as an arbitrary string (MN-02), so it is
/// restricted to Message-ID-safe atoms (`[A-Za-z0-9_.-]`, capped at 128
/// chars): spaces, `>`, non-ASCII, or an empty string would otherwise break
/// the SEARCH-reconcile the whole phase keys on (zero-hit → permanent
/// `dirty=1`, repeated APPENDs). A fully-rejected id falls back to a
/// deterministic `rejected-<hash>` atom — stable per input, distinct per
/// distinct input — instead of yielding the unsearchable `<@sge.local>`.
pub fn new_message_id(compose_id: &str) -> String {
    let clean: String = compose_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(128)
        .collect();
    if clean.is_empty() {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        compose_id.hash(&mut hasher);
        format!("<rejected-{:x}@sge.local>", hasher.finish())
    } else {
        format!("<{clean}@sge.local>")
    }
}

/// Strip CR/LF from a header field value (T-12-02 injection guard).
fn sanitize_header(s: &str) -> String {
    s.chars().filter(|c| *c != '\r' && *c != '\n').collect()
}

/// Minimal base-64 encoder (standard alphabet with `=` padding).
///
/// Hand-rolled so the renderer needs no new crate: only used for
/// `=?UTF-8?B?...?=` encoded-words of non-ASCII header words.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut n: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            n |= (b as u32) << (16 - 8 * i);
        }
        let pad = 3 - chunk.len();
        for i in 0..4 - pad {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
        }
        for _ in 0..pad {
            out.push('=');
        }
    }
    out
}

/// Encode one header value with RFC 2047 encoded-words where needed.
///
/// ASCII-only input passes through unchanged; otherwise each
/// whitespace-separated word containing non-ASCII bytes becomes
/// `=?UTF-8?B?<base64>?=` (word-boundary granularity keeps ASCII words
/// readable and matches what `mail-parser` decodes).
pub fn encode_word(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    s.split_whitespace()
        .map(|word| {
            if word.is_ascii() {
                word.to_string()
            } else {
                format!("=?UTF-8?B?{}?=", base64_encode(word.as_bytes()))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize every line break to CRLF (MIME requires CRLF; the compose
/// window's textarea produces bare LF).
fn crlf(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                out.push_str("\r\n");
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => out.push_str("\r\n"),
            other => out.push(other),
        }
    }
    out
}

/// Drop the `data:<mime>;base64,` prefix (when present) and all whitespace
/// from a base64 payload, so both `readAsDataURL` output and pre-wrapped
/// base64 bodies normalize to one canonical form.
fn base64_payload(raw: &str) -> String {
    let after_prefix = match raw.find("base64,") {
        Some(i) if raw[..i].starts_with("data:") => &raw[i + "base64,".len()..],
        _ => raw,
    };
    after_prefix
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// True when `raw` is a well-formed padded base64 payload. Refused at
/// enqueue instead of emitting a corrupt attachment the recipient can
/// never open (see `send-bad-attachment`).
pub fn is_valid_base64(raw: &str) -> bool {
    let clean = base64_payload(raw);
    if clean.is_empty() || clean.len() % 4 != 0 {
        return false;
    }
    let body = clean.trim_end_matches('=');
    if clean.len() - body.len() > 2 {
        return false;
    }
    body.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// Wrap a base64 payload at 76 columns with CRLF (RFC 2045 section 6.8).
fn wrap_base64(raw: &str) -> String {
    let clean = base64_payload(raw);
    let mut out = String::with_capacity(clean.len() + clean.len() / 76 + 2);
    for (i, chunk) in clean.as_bytes().chunks(76).enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        // base64 is ASCII, so slicing bytes is UTF-8 safe.
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    out
}

/// Percent-encode per RFC 2231 (unreserved set kept literal).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Render one MIME header parameter (`name="..."`), adding the RFC 2231
/// `name*=UTF-8''...` form when the value is non-ASCII (a raw quoted-string
/// cannot carry non-ASCII; `mail-parser` decodes both forms).
fn mime_param(name: &str, value: &str) -> String {
    let ascii: String = value
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    if value.is_ascii() {
        format!("{name}=\"{ascii}\"")
    } else {
        format!(
            "{name}=\"{ascii}\"; {name}*=UTF-8''{}",
            percent_encode(value)
        )
    }
}

/// Deterministic MIME boundary, bumped until it collides with nothing in
/// the body or the base64 payloads (a boundary appearing in content would
/// split the message wrongly).
fn pick_boundary(message_id: &str, body: &str, attachments: &[&AttachmentInput]) -> String {
    let stem: String = message_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let stem = if stem.is_empty() {
        "sge".to_string()
    } else {
        stem
    };
    let mut n = 0u32;
    loop {
        let candidate = format!("----=_sge_{stem}_{n}");
        let clash = body.contains(&candidate)
            || attachments
                .iter()
                .any(|a| a.data_base64.contains(&candidate));
        if !clash {
            return candidate;
        }
        n += 1;
    }
}

/// Render a draft as raw RFC 5322 bytes (CRLF line endings).
///
/// `date_rfc2822` is supplied by the caller (e.g. `chrono::Utc::now()`
/// formatted) so the renderer stays pure and testable. Empty To/Cc/Bcc
/// lists omit the header (a zero-recipient draft must still APPEND —
/// send-time validation is Phase 13/14). `In-Reply-To`/`References` are
/// emitted only when supplied; attachments switch the body to
/// `multipart/mixed` (base64 parts with `Content-Disposition: attachment`),
/// and an attachment-free render stays byte-identical to Phase 12.
pub fn render_draft_rfc5322(d: &DraftFields, date_rfc2822: &str) -> Vec<u8> {
    let mut out = String::new();
    let from = sanitize_header(&d.from);
    if !from.is_empty() {
        out.push_str(&format!("From: {}\r\n", encode_word(&from)));
    }
    for (name, list) in [("To", &d.to), ("Cc", &d.cc), ("Bcc", &d.bcc)] {
        let addrs: Vec<String> = list
            .iter()
            .map(|a| encode_word(&sanitize_header(a)))
            .filter(|a| !a.is_empty())
            .collect();
        if !addrs.is_empty() {
            out.push_str(&format!("{name}: {}\r\n", addrs.join(", ")));
        }
    }
    let subject = sanitize_header(&d.subject);
    out.push_str(&format!("Subject: {}\r\n", encode_word(&subject)));
    out.push_str(&format!("Date: {}\r\n", sanitize_header(date_rfc2822)));
    out.push_str(&format!(
        "Message-ID: {}\r\n",
        sanitize_header(&d.message_id)
    ));
    // Threading (RFC 5322 3.6.4): only present on replies.
    if let Some(irt) = d.in_reply_to.as_deref() {
        let v = sanitize_header(irt);
        if !v.is_empty() {
            out.push_str(&format!("In-Reply-To: {v}\r\n"));
        }
    }
    if let Some(refs) = d.references.as_deref() {
        let v = sanitize_header(refs);
        if !v.is_empty() {
            out.push_str(&format!("References: {v}\r\n"));
        }
    }
    out.push_str("MIME-Version: 1.0\r\n");

    let parts: Vec<&AttachmentInput> = d
        .attachments
        .iter()
        .filter(|a| !base64_payload(&a.data_base64).is_empty())
        .collect();
    if parts.is_empty() {
        out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
        out.push_str("\r\n");
        out.push_str(&d.body);
        return out.into_bytes();
    }

    let boundary = pick_boundary(&d.message_id, &d.body, &parts);
    out.push_str(&format!(
        "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n"
    ));
    out.push_str("\r\n");
    out.push_str("This is a multi-part message in MIME format.\r\n");
    out.push_str(&format!("--{boundary}\r\n"));
    out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    out.push_str("Content-Transfer-Encoding: 8bit\r\n");
    out.push_str("\r\n");
    out.push_str(&crlf(&d.body));
    out.push_str("\r\n");
    for att in parts {
        let filename = sanitize_header(&att.filename);
        let filename = if filename.trim().is_empty() {
            "anexo".to_string()
        } else {
            filename
        };
        let ct = sanitize_header(&att.content_type);
        let ct = if ct.trim().is_empty() {
            "application/octet-stream".to_string()
        } else {
            ct
        };
        out.push_str(&format!("--{boundary}\r\n"));
        out.push_str(&format!(
            "Content-Type: {ct}; {}\r\n",
            mime_param("name", &filename)
        ));
        out.push_str("Content-Transfer-Encoding: base64\r\n");
        out.push_str(&format!(
            "Content-Disposition: attachment; {}\r\n",
            mime_param("filename", &filename)
        ));
        out.push_str("\r\n");
        out.push_str(&wrap_base64(&att.data_base64));
        out.push_str("\r\n");
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::{MessageParser, MimeHeaders};

    const DATE: &str = "Tue, 06 Oct 2026 10:00:00 +0000";

    fn ascii_fields() -> DraftFields {
        DraftFields {
            from: "user@utfpr.edu.br".to_string(),
            to: vec!["friend@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "Hello".to_string(),
            body: "plain body".to_string(),
            message_id: new_message_id("compose-1"),
            ..Default::default()
        }
    }

    fn parse(bytes: &[u8]) -> mail_parser::Message<'_> {
        MessageParser::new()
            .parse(bytes)
            .expect("rendered bytes must parse")
    }

    #[test]
    fn renders_ascii_simple_round_trip() {
        let d = ascii_fields();
        let raw = render_draft_rfc5322(&d, DATE);
        let msg = parse(&raw);
        assert_eq!(msg.subject(), Some("Hello"));
        assert_eq!(msg.body_text(0).unwrap().to_string(), "plain body");
        let to: Vec<String> = msg
            .to()
            .map(|a| {
                a.iter()
                    .filter_map(|addr| addr.address().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(to, vec!["friend@example.com".to_string()]);
        assert_eq!(msg.message_id(), Some("compose-1@sge.local"));
    }

    #[test]
    fn renders_non_ascii_subject_and_display_name() {
        let mut d = ascii_fields();
        d.subject = "Assunto com acentuação".to_string();
        d.to = vec!["Lixeira & Cia <lixeira@example.com>".to_string()];
        let raw = render_draft_rfc5322(&d, DATE);
        assert!(
            raw.windows(10).any(|w| w == b"=?UTF-8?B?"),
            "non-ASCII words must be encoded-word encoded"
        );
        let msg = parse(&raw);
        assert_eq!(msg.subject(), Some("Assunto com acentuação"));
        let to: Vec<String> = msg
            .to()
            .map(|a| {
                a.iter()
                    .map(|addr| {
                        format!(
                            "{} <{}>",
                            addr.name().unwrap_or(""),
                            addr.address().unwrap_or("")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(to, vec!["Lixeira & Cia <lixeira@example.com>".to_string()]);
    }

    #[test]
    fn renders_multi_recipients() {
        let mut d = ascii_fields();
        d.to = vec!["a@example.com".to_string(), "b@example.com".to_string()];
        d.cc = vec!["c@example.com".to_string()];
        d.bcc = vec!["d@example.com".to_string()];
        let raw = render_draft_rfc5322(&d, DATE);
        let msg = parse(&raw);
        let collect = |a: Option<&mail_parser::Address<'_>>| -> Vec<String> {
            a.map(|addr| {
                addr.iter()
                    .filter_map(|e| e.address().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
        };
        assert_eq!(
            collect(msg.to()),
            vec!["a@example.com".to_string(), "b@example.com".to_string()]
        );
        assert_eq!(collect(msg.cc()), vec!["c@example.com".to_string()]);
        assert_eq!(collect(msg.bcc()), vec!["d@example.com".to_string()]);
    }

    #[test]
    fn empty_to_omits_header_body_intact() {
        let mut d = ascii_fields();
        d.to = vec![];
        let raw = render_draft_rfc5322(&d, DATE);
        let text = String::from_utf8_lossy(&raw);
        assert!(
            !text.lines().any(|l| l.starts_with("To:")),
            "empty To must omit the header"
        );
        let msg = parse(&raw);
        assert_eq!(msg.body_text(0).unwrap().to_string(), "plain body");
    }

    #[test]
    fn header_injection_newlines_stripped() {
        // T-12-02: a hostile subject must not smuggle a second header.
        let mut d = ascii_fields();
        d.subject = "Hello\r\nBcc: evil@example.com".to_string();
        d.to = vec!["victim@example.com\r\nTo: evil2@example.com".to_string()];
        let raw = render_draft_rfc5322(&d, DATE);
        let msg = parse(&raw);
        assert_eq!(msg.subject(), Some("HelloBcc: evil@example.com"));
        let bcc_count = msg.bcc().map(|a| a.iter().count()).unwrap_or(0);
        assert_eq!(bcc_count, 0, "injected Bcc header must not survive");
    }

    #[test]
    fn message_id_stable_per_compose_session() {
        assert_eq!(
            new_message_id("compose-1"),
            "<compose-1@sge.local>".to_string()
        );
    }

    #[test]
    fn message_id_rejects_unsafe_atoms() {
        // MN-02: spaces, `>`, and non-ASCII must not reach the wire header.
        assert_eq!(new_message_id("a b>c@d"), "<abcd@sge.local>".to_string());
        let long = "x".repeat(200);
        let id = new_message_id(&long);
        assert!(id.len() < 150, "capped at 128 atoms + domain");
        assert!(id.starts_with('<') && id.ends_with('>'));
    }

    #[test]
    fn message_id_empty_falls_back_to_deterministic_atom() {
        // Empty / fully-rejected input never yields `<@sge.local>`.
        let a = new_message_id("");
        let b = new_message_id("   ");
        assert!(a.starts_with("<rejected-") && a.ends_with("@sge.local>"));
        assert_eq!(a, new_message_id(""), "stable per input");
        assert_ne!(a, b, "distinct per distinct input");
    }

    // ---- Phase 38 reply threading + attachments ----

    #[test]
    fn reply_threading_headers_round_trip() {
        let mut d = ascii_fields();
        d.subject = "Re: Hello".to_string();
        d.in_reply_to = Some("<original@sge.local>".to_string());
        d.references = Some("<root@sge.local> <original@sge.local>".to_string());
        let raw = render_draft_rfc5322(&d, DATE);
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("In-Reply-To: <original@sge.local>\r\n"),
            "In-Reply-To must reach the wire: {text}"
        );
        assert!(
            text.contains("References: <root@sge.local> <original@sge.local>\r\n"),
            "References chain must reach the wire: {text}"
        );
        let msg = parse(&raw);
        assert_eq!(msg.in_reply_to().as_text(), Some("original@sge.local"));
        assert_eq!(msg.subject(), Some("Re: Hello"));
    }

    #[test]
    fn absent_threading_headers_leave_output_identical() {
        // Regression guard: a plain (non-reply) render must stay byte-identical
        // to the Phase 12 single-part output.
        let d = ascii_fields();
        let raw = render_draft_rfc5322(&d, DATE);
        let expected = concat!(
            "From: user@utfpr.edu.br\r\n",
            "To: friend@example.com\r\n",
            "Subject: Hello\r\n",
            "Date: Tue, 06 Oct 2026 10:00:00 +0000\r\n",
            "Message-ID: <compose-1@sge.local>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "plain body",
        );
        assert_eq!(String::from_utf8_lossy(&raw), expected);
    }

    #[test]
    fn threading_header_newlines_stripped() {
        // Same injection guard as the other headers (T-12-02 discipline).
        let mut d = ascii_fields();
        d.in_reply_to = Some("<a@sge.local>\r\nBcc: evil@example.com".to_string());
        let raw = render_draft_rfc5322(&d, DATE);
        let msg = parse(&raw);
        assert_eq!(
            msg.bcc().map(|a| a.iter().count()).unwrap_or(0),
            0,
            "injected Bcc must not survive"
        );
    }

    #[test]
    fn attachments_render_multipart_mixed_with_decodable_part() {
        let mut d = ascii_fields();
        d.attachments = vec![AttachmentInput {
            filename: "relatorio.txt".to_string(),
            content_type: "text/plain".to_string(),
            data_base64: "aGVsbG8gYXR0YWNobWVudA==".to_string(),
        }];
        let raw = render_draft_rfc5322(&d, DATE);
        let msg = parse(&raw);
        assert_eq!(msg.body_text(0).unwrap().to_string(), "plain body");
        let atts: Vec<_> = msg.attachments().collect();
        assert_eq!(atts.len(), 1);
        let names: Vec<String> = atts
            .iter()
            .map(|a| a.attachment_name().unwrap_or("").to_string())
            .collect();
        assert_eq!(names, vec!["relatorio.txt".to_string()]);
        let decoded = atts[0].contents();
        assert_eq!(decoded, b"hello attachment");
    }

    #[test]
    fn attachment_data_url_prefix_and_whitespace_are_normalized() {
        let mut d = ascii_fields();
        d.attachments = vec![AttachmentInput {
            filename: "a.bin".to_string(),
            content_type: String::new(),
            data_base64: "data:application/octet-stream;base64,AAEC\nAw==".to_string(),
        }];
        let raw = render_draft_rfc5322(&d, DATE);
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("application/octet-stream"),
            "blank type defaults"
        );
        assert!(
            !text.contains("data:") && !text.contains("\nAw"),
            "the data: prefix and line breaks must be stripped: {text}"
        );
        let msg = parse(&raw);
        let atts: Vec<_> = msg.attachments().collect();
        assert_eq!(atts[0].contents(), &[0u8, 1, 2, 3][..]);
    }

    #[test]
    fn attachment_empty_payload_keeps_single_part_body() {
        let mut d = ascii_fields();
        d.attachments = vec![AttachmentInput {
            filename: "vazio.txt".to_string(),
            content_type: "text/plain".to_string(),
            data_base64: "   ".to_string(),
        }];
        let raw = render_draft_rfc5322(&d, DATE);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(!text.contains("multipart/mixed"));
    }

    #[test]
    fn boundary_collision_with_body_is_avoided() {
        let mut d = ascii_fields();
        // Craft a body that contains the first candidate boundary.
        d.body = "----=_sge_compose1sgelocal_0".to_string();
        d.attachments = vec![AttachmentInput {
            filename: "x.txt".to_string(),
            content_type: "text/plain".to_string(),
            data_base64: "aGk=".to_string(),
        }];
        let raw = render_draft_rfc5322(&d, DATE);
        let msg = parse(&raw);
        assert_eq!(
            msg.body_text(0).unwrap().to_string(),
            "----=_sge_compose1sgelocal_0"
        );
        let atts: Vec<_> = msg.attachments().collect();
        assert_eq!(atts[0].attachment_name(), Some("x.txt"));
    }

    #[test]
    fn non_ascii_attachment_name_uses_rfc2231() {
        let mut d = ascii_fields();
        d.attachments = vec![AttachmentInput {
            filename: "relatório ção.txt".to_string(),
            content_type: "text/plain".to_string(),
            data_base64: "aGk=".to_string(),
        }];
        let raw = render_draft_rfc5322(&d, DATE);
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("filename*=UTF-8''"),
            "non-ASCII name must use the RFC 2231 form: {text}"
        );
        let msg = parse(&raw);
        let atts: Vec<_> = msg.attachments().collect();
        let name = atts[0].attachment_name().unwrap_or("");
        assert!(
            name.contains("relat") && name.contains(".txt"),
            "decoded name should survive: {name}"
        );
    }

    #[test]
    fn base64_validation_accepts_padded_and_rejects_junk() {
        assert!(is_valid_base64("aGVsbG8="));
        assert!(is_valid_base64("data:text/plain;base64,aGVsbG8="));
        assert!(is_valid_base64("aGVs\nbG8="), "whitespace is tolerated");
        assert!(!is_valid_base64(""));
        assert!(!is_valid_base64("not base64!!"));
        assert!(!is_valid_base64("aGVsbG8"), "missing padding is refused");
    }
}
