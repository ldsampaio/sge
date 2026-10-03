//! On-demand body fetch with cache-cap and attachment extraction (Phase 4).
//!
//! BODY.PEEK semantics are enforced by the [`SyncSession::fetch_body`]
//! implementation in [`crate::imap`] — this module never issues a bare
//! `BODY[]` that could set `\\Seen` on the server (D-flags invariant,
//! M1 read-only).
//!
//! Threat model:
//! * T-02-04 (DoS) — 30s read timeout inherited from session core.
//!   BODY_CACHE_CAP_BYTES (256 KiB) caps what is persisted; oversized
//!   bodies are parsed in-memory only and never cached.
//! * T-04-01 (Tampering) — attachment filenames reduced to basename
//!   and confined under app-data/attachments/<uidv>/<uid>/.
//! * T-04-03 (Malicious mail) — ammonia strips <script>, on* handlers,
//!   javascript: URLs, nested <iframe>; the renderer adds a sandboxed
//!   iframe wrapper separately.

use crate::imap::SyncError;
use crate::imap::SyncSession;
use crate::store::queries::AttachmentInfo;
use mail_parser::{Message, MessageParser, MimeHeaders, PartType};
use rusqlite::Connection;

/// Body cache cap — anything larger is not persisted (DoS mitigation).
pub const BODY_CACHE_CAP_BYTES: usize = 262_144; // 256 KiB

/// Cached body content ready for the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyContent {
    pub text: Option<String>,
    pub html: Option<String>,
}

/// Sanitize raw HTML for safe local rendering.
///
/// Uses ammonia with default tag/attribute/protocol allow-lists —
/// strips `<script>`, `on*` handlers, `javascript:` URLs, and bare
/// `<iframe>` (the renderer adds a sandboxed iframe wrapper separately).
pub fn sanitize_html(input: &str) -> String {
    ammonia::Builder::new().clean(input).to_string()
}

/// Extract the first `text/plain` body from parsed message parts.
pub fn extract_text(msg: &Message<'_>) -> Option<String> {
    for id in &msg.text_body {
        if let Some(part) = msg.parts.get(*id as usize) {
            if let PartType::Text(cow) = &part.body {
                return Some(cow.to_string());
            }
        }
    }
    // Fallback: scan all parts
    for part in &msg.parts {
        if let PartType::Text(cow) = &part.body {
            return Some(cow.to_string());
        }
    }
    None
}

/// Extract the first `text/html` body from parsed message parts.
pub fn extract_html(msg: &Message<'_>) -> Option<String> {
    for id in &msg.html_body {
        if let Some(part) = msg.parts.get(*id as usize) {
            if let PartType::Html(cow) = &part.body {
                return Some(cow.to_string());
            }
        }
    }
    // Fallback: scan all parts
    for part in &msg.parts {
        if let PartType::Html(cow) = &part.body {
            return Some(cow.to_string());
        }
    }
    None
}

/// Extract attachment metadata from a parsed message.
///
/// A part is considered an attachment if it has a `Content-Disposition:
/// attachment` or a filename in either Content-Disposition or Content-Type.
/// The `part_number` is the index into `msg.parts` — used by the frontend
/// to request a specific part via `save_attachment`.
pub fn extract_attachments(msg: &Message<'_>) -> Vec<AttachmentInfo> {
    msg.parts
        .iter()
        .enumerate()
        .filter_map(|(i, part)| {
            let cd = part.content_disposition();
            let is_attachment = cd.is_some_and(|ct| {
                ct.c_type.eq_ignore_ascii_case("attachment")
            });
            let has_filename = part.attachment_name().is_some();

            if !is_attachment && !has_filename {
                return None;
            }

            let name = part.attachment_name().unwrap_or("attachment").to_string();

            let content_type = part
                .content_type()
                .map(|ct| {
                    let sub = ct.c_subtype.as_deref().unwrap_or("plain");
                    format!("{}/{}", ct.c_type, sub)
                })
                .unwrap_or_else(|| "application/octet-stream".to_string());

            let size = match &part.body {
                PartType::Binary(cow) => cow.len() as u64,
                PartType::Text(cow) => cow.len() as u64,
                PartType::Html(cow) => cow.len() as u64,
                _ => 0,
            };

            Some(AttachmentInfo {
                name,
                size,
                content_type,
                part_number: i.to_string(),
            })
        })
        .collect()
}

/// Extract the decoded bytes for a specific attachment part.
///
/// `part_number` is the index into `msg.parts` (as returned by
/// [`extract_attachments`]).
pub fn extract_attachment_bytes(msg: &Message<'_>, part_number: &str) -> Option<Vec<u8>> {
    let idx: usize = part_number.parse().ok()?;
    msg.parts.get(idx).and_then(|part| match &part.body {
        PartType::Binary(cow) => Some(cow.to_vec()),
        PartType::Text(cow) => Some(cow.to_string().into_bytes()),
        PartType::Html(cow) => Some(cow.to_string().into_bytes()),
        _ => None,
    })
}

/// Read cached body text/html from `message_bodies` row.
pub fn read_cached_body(conn: &Connection, message_id: u64) -> Result<BodyContent, SyncError> {
    let (text, html) = conn
        .query_row(
            "SELECT body_text, body_html FROM message_bodies WHERE message_id = ?1",
            rusqlite::params![message_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .map_err(|e| SyncError::Io(format!("cached body read: {e}")))?;
    Ok(BodyContent { text, html })
}

/// Detail returned by the on-demand message reader: body text/HTML plus
/// attachment metadata.
#[derive(Debug, Clone, Default)]
pub struct MessageDetail {
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Fetch (or retrieve cached) body + attachment metadata for a message.
///
/// Mirrors [`fetch_body_message`] semantics but also extracts attachments.
/// Cache-hit path returns cached text/html + SQLite-stored attachment metadata.
pub async fn fetch_message_detail(
    session: &mut dyn SyncSession,
    conn: &Connection,
    message_id: u64,
    uid: u32,
) -> Result<MessageDetail, SyncError> {
    // Cache hit → return cached body + attachment metas from SQLite.
    if crate::store::queries::body_is_complete(conn, message_id)? {
        let cached = read_cached_body(conn, message_id)?;
        let attachments = crate::store::queries::list_attachments(conn, message_id)
            .map_err(|e| SyncError::Io(format!("cached attachments: {e}")))?;
        return Ok(MessageDetail {
            text: cached.text,
            html: cached.html,
            attachments,
        });
    }

    // Cache miss → fetch body via BODY.PEEK[] (never sets \Seen).
    let raw = session.fetch_body(uid).await?;
    let msg = MessageParser::new()
        .parse(&raw)
        .ok_or_else(|| SyncError::Parse("mail-parser returned None".into()))?;

    let text = extract_text(&msg);
    let html = extract_html(&msg).map(|h| sanitize_html(&h));
    let attachments = extract_attachments(&msg);

    // Cache body if within cap.
    let total = text.as_ref().map(|s| s.len()).unwrap_or(0)
        + html.as_ref().map(|s| s.len()).unwrap_or(0);
    if total <= BODY_CACHE_CAP_BYTES {
        crate::store::queries::insert_body(
            conn,
            message_id,
            text.as_deref(),
            html.as_deref(),
        )?;
        for att in &attachments {
            let _ = crate::store::queries::insert_attachment_meta(
                conn,
                message_id,
                &att.part_number,
                &att.name,
                &att.content_type,
                att.size,
            );
        }
    }

    Ok(MessageDetail {
        text,
        html,
        attachments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MSG: &str = "From: alice@example.com\r\nTo: bob@example.com\r\nSubject: Test message\r\nDate: Thu, 03 Oct 2024 12:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"BOUND\"\r\n\r\n--BOUND\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello plain text world.\r\n\r\n--BOUND\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Hello <b>HTML</b></p></body></html>\r\n\r\n--BOUND\r\nContent-Type: application/pdf; name=\"doc.pdf\"\r\nContent-Disposition: attachment; filename=\"doc.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQKJcOkw7zDtsOf\r\n\r\n--BOUND--\r\n";

    #[test]
    fn sanitize_strips_script_and_iframe() {
        let input =
            r#"<script>alert(1)</script><p>hello</p><iframe srcdoc="evil"></iframe>"#;
        let out = sanitize_html(input);
        assert!(!out.contains("<script"), "<script> tag should be stripped");
        assert!(!out.contains("iframe"), "<iframe> tag should be stripped");
        assert!(out.contains("hello"), "text content should be preserved");
    }

    #[test]
    fn sanitize_keeps_safe_tags() {
        let input = r#"<p>hello <b>world</b></p>"#;
        let out = sanitize_html(input);
        assert!(out.contains("hello"), "text preserved");
        assert!(out.contains("<b>"), "safe tag preserved");
    }

    #[test]
    fn sanitize_strips_event_handlers_and_js_urls() {
        let input = r#"<p onclick="evil()">click</p><a href="javascript:alert(1)">link</a><img src=x onerror="alert(1)">"#;
        let out = sanitize_html(input);
        assert!(!out.contains("onclick"), "event handler should be stripped");
        assert!(!out.contains("javascript:"), "javascript: URL should be stripped");
        assert!(!out.contains("onerror"), "onerror handler should be stripped");
    }

    #[test]
    fn extract_attachments_finds_pdf() {
        let msg = MessageParser::new()
            .parse(SAMPLE_MSG)
            .expect("sample message must parse");
        let attachments = extract_attachments(&msg);
        assert_eq!(attachments.len(), 1, "should find 1 attachment");
        assert_eq!(attachments[0].name, "doc.pdf");
        assert_eq!(attachments[0].content_type, "application/pdf");
        assert!(!attachments[0].part_number.is_empty());
    }

    #[test]
    fn extract_attachment_bytes_returns_decoded() {
        let msg = MessageParser::new()
            .parse(SAMPLE_MSG)
            .expect("sample message must parse");
        let attachments = extract_attachments(&msg);
        let part_number = &attachments[0].part_number;
        let bytes = extract_attachment_bytes(&msg, part_number);
        assert!(bytes.is_some(), "should extract attachment bytes");
    }

    #[test]
    fn extract_text_and_html_from_multipart() {
        let msg = MessageParser::new()
            .parse(SAMPLE_MSG)
            .expect("sample message must parse");
        let text = extract_text(&msg);
        assert_eq!(text.as_deref().map(|s| s.trim_end()), Some("Hello plain text world."));
        let html = extract_html(&msg);
        assert_eq!(
            html.as_deref().map(|s| s.trim_end()),
            Some("<html><body><p>Hello <b>HTML</b></p></body></html>")
        );
    }
}
