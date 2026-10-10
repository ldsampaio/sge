//! Draft RFC 5322 renderer (Phase 12, Plan 12-01).
//!
//! Pure codec: [`DraftFields`] in, raw message bytes out. The output is
//! minimal (plain text + headers) — full lettre MIME ships in Phase 14 —
//! but must round-trip through `mail-parser` (subject, recipients, body).
//!
//! Header-injection guard (T-12-02): CR/LF are stripped from every header
//! field before emission, so a hostile `"Subject\r\nBcc: evil"` can never
//! smuggle a second header onto the wire.

/// Composed draft content for one save (mirrors the `drafts` row fields).
#[derive(Debug, Clone)]
pub struct DraftFields {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub message_id: String,
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
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

/// Render a draft as raw RFC 5322 bytes (CRLF line endings).
///
/// `date_rfc2822` is supplied by the caller (e.g. `chrono::Utc::now()`
/// formatted) so the renderer stays pure and testable. Empty To/Cc/Bcc
/// lists omit the header (a zero-recipient draft must still APPEND —
/// send-time validation is Phase 13/14). No threading headers (CONTEXT).
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
    out.push_str(&format!(
        "Date: {}\r\n",
        sanitize_header(date_rfc2822)
    ));
    out.push_str(&format!(
        "Message-ID: {}\r\n",
        sanitize_header(&d.message_id)
    ));
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    out.push_str("\r\n");
    out.push_str(&d.body);
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MessageParser;

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
            raw.windows(10)
                .any(|w| w == b"=?UTF-8?B?"),
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
        assert_eq!(
            new_message_id("a b>c@d"),
            "<abcd@sge.local>".to_string()
        );
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
}
