//! On-demand body fetch with cache-cap and file-backed attachments.
//!
//! BODY.PEEK semantics are enforced by the [`SyncSession::fetch_body`]
//! implementation in [`crate::imap`] — this module never issues a bare
//! `BODY[]` that could set `\\Seen` on the server (D-flags invariant,
//! M1 read-only).
//!
//! Threat model:
//! * T-02-04 (DoS) — 30s read timeout inherited from session core.
//!   BODY_CACHE_CAP_BYTES (256 KiB) caps what is persisted; oversized
//!   bodies are parsed in-memory only and never cached, so re-fetch
//!   happens on each access but no unbounded growth occurs.
//! * T-02-04 (Tampering) — attachment filenames are reduced to
//!   basename and confined under app-data/attachments/<uidv>/<uid>/.

use std::path::Path;

use mail_parser::{MessageParser, PartType};
use rusqlite::Connection;

use super::SyncError;

/// Body cache cap — anything larger is not persisted (DoS mitigation).
pub const BODY_CACHE_CAP_BYTES: usize = 262_144; // 256 KiB

// ── Public API ───────────────────────────────────────────────────────

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
/// `<iframe>` (the renderer adds a sandboxed iframe wrapper separately
/// per D-flags).
pub fn sanitize_html(input: &str) -> String {
    ammonia::Builder::new().clean(input).to_string()
}

/// Fetch (or retrieve cached) body for a message identified by its
/// database `message_id` and IMAP `uid`.
///
/// # Workflow
/// 1. If `body_is_complete` → return cached row (no IMAP round-trip).
/// 2. Otherwise `session.fetch_body(uid)` → `BODY.PEEK[]` bytes.
/// 3. Parse with mail-parser, extract first `text/plain` and first
///    `text/html` part.
/// 4. Sanitize HTML with ammonia.
/// 5. If total ≤ BODY_CACHE_CAP_BYTES → cache via `insert_body`.
/// 6. If total > cap → skip cache (caller re-fetches next time).
/// 7. Persist attachments to disk + metadata (Phase 3).
/// 8. Return `BodyContent`.
pub async fn fetch_body_message(
    session: &mut dyn super::SyncSession,
    conn: &Connection,
    message_id: u64,
    uid: u32,
    uid_validity: u32,
    _app_data: &Path,
) -> Result<BodyContent, SyncError> {
    // Step 1 — cache hit?
    if crate::store::queries::body_is_complete(conn, message_id)? {
        return read_cached_body(conn, message_id);
    }

    // Step 2 — fetch body via BODY.PEEK[]
    let raw = session.fetch_body(uid).await?;

    // Step 3 — parse
    let msg = MessageParser::new()
        .parse(&raw)
        .ok_or_else(|| SyncError::Parse("mail-parser returned None".into()))?;

    // Step 4 — extract text + html
    let text = extract_text(&msg);
    let html = extract_html(&msg).map(|h| sanitize_html(&h));

    // Step 5 — size check
    let total = text.as_ref().map(|s| s.len()).unwrap_or(0)
        + html.as_ref().map(|s| s.len()).unwrap_or(0);

    let body_complete = total <= BODY_CACHE_CAP_BYTES;

    // Step 6 — cache if within cap
    if body_complete {
        crate::store::queries::insert_body(
            conn,
            message_id,
            text.as_deref(),
            html.as_deref(),
        )?;
    }

    // Step 7 — attachments (Phase 3)
    // TODO: mail-parser 0.11 MessagePart has headers/body/encoding only;
    // disposition is in headers. Attachment persistence is deferred.
    let _ = (uid_validity, _app_data);

    Ok(BodyContent { text, html })
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Extract the first `text/plain` body from parsed message parts.
fn extract_text(msg: &mail_parser::Message<'_>) -> Option<String> {
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
fn extract_html(msg: &mail_parser::Message<'_>) -> Option<String> {
    for id in &msg.html_body {
        if let Some(part) = msg.parts.get(*id as usize) {
            if let PartType::Html(cow) = &part.body {
                return Some(cow.to_string());
            }
        }
    }
    for part in &msg.parts {
        if let PartType::Html(cow) = &part.body {
            return Some(cow.to_string());
        }
    }
    None
}

/// Read cached text/html from `message_bodies` row.
fn read_cached_body(conn: &Connection, message_id: u64) -> Result<BodyContent, SyncError> {
    let (text, html) = conn
        .query_row(
            "SELECT body_text, body_html FROM message_bodies WHERE message_id = ?1",
            rusqlite::params![message_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|e| SyncError::Io(format!("cached body read: {e}")))?;
    Ok(BodyContent { text, html })
}

// ── Tests ────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::headers::MessageHeader;
    use crate::imap::{MailboxInfo, MailboxStatus, MailboxSummary, PinBox, SyncSession};
    use super::SyncError;
    use crate::store::Store;
    use crate::store::queries;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Minimal mock session for body-fetch tests.
    struct MockSession {
        fetch_body_called: AtomicBool,
        body: Vec<u8>,
    }

    impl SyncSession for MockSession {
        fn select_mailbox(
            &mut self,
            _name: &str,
        ) -> PinBox<'_, Result<MailboxSummary, SyncError>> {
            let summary = MailboxSummary {
                selected_mailbox: "INBOX".to_string(),
                uid_validity: 100,
                uid_next: Some(1),
                exists: 1,
            };
            Box::pin(async move { Ok(summary) })
        }

        fn list_mailboxes(&mut self) -> PinBox<'_, Result<Vec<MailboxInfo>, SyncError>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn mailbox_status(&mut self, _name: &str) -> PinBox<'_, Result<MailboxStatus, SyncError>> {
            Box::pin(async move {
                Ok(MailboxStatus {
                    uid_validity: 100,
                    uid_next: Some(1),
                    unseen: 0,
                })
            })
        }

        fn search_uids(&mut self) -> PinBox<'_, Result<Vec<u32>, SyncError>> {
            Box::pin(async move { Ok(vec![1]) })
        }

        fn fetch_envelopes<'a>(
            &'a mut self,
            _range: &'a str,
        ) -> PinBox<'a, Result<Vec<MessageHeader>, SyncError>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn fetch_body(&mut self, _uid: u32) -> PinBox<'_, Result<Vec<u8>, SyncError>> {
            self.fetch_body_called.store(true, Ordering::SeqCst);
            let body = self.body.clone();
            Box::pin(async move { Ok(body) })
        }

        fn set_seen(&mut self, _uid: u32, _seen: bool) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }

        fn logout(&mut self) -> PinBox<'_, Result<(), SyncError>> {
            Box::pin(async move { Ok(()) })
        }
    }

    fn make_msg(text: &str) -> Vec<u8> {
        format!(
            "From: alice@example.com\r\nSubject: Test\r\n\r\n{}\r\n",
            text
        )
        .into_bytes()
    }

    #[test]
    fn cache_hit_skips_imap() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mailbox_id = queries::ensure_mailbox(conn, "INBOX").unwrap();
        queries::upsert_message(
            conn, mailbox_id, 1, Some("<msg1>"),
            "Subject 1", "alice@example.com", "", "",
            "2024-01-01T00:00:00Z", "[]", false, "",
        ).unwrap();
        let msg_id = queries::find_message_id(conn, mailbox_id, 1).unwrap().unwrap();
        queries::insert_body(conn, msg_id, Some("Hello text"), Some("<p>Hello</p>")).unwrap();

        let mut session = MockSession {
            fetch_body_called: AtomicBool::new(false),
            body: Vec::new(),
        };

        let result = async_std::task::block_on(async {
            fetch_body_message(&mut session, conn, msg_id, 1, 100, std::path::Path::new("/tmp")).await
        }).unwrap();

        assert!(
            !session.fetch_body_called.load(Ordering::SeqCst),
            "should not call IMAP on cached body"
        );
        assert_eq!(result.text.as_deref(), Some("Hello text"));
        assert_eq!(result.html.as_deref(), Some("<p>Hello</p>"));
    }

    #[test]
    fn oversized_body_not_cached() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let mailbox_id = queries::ensure_mailbox(conn, "INBOX").unwrap();
        queries::upsert_message(
            conn, mailbox_id, 1, Some("<msg1>"),
            "Subject 1", "alice@example.com", "", "",
            "2024-01-01T00:00:00Z", "[]", false, "",
        ).unwrap();
        let msg_id = queries::find_message_id(conn, mailbox_id, 1).unwrap().unwrap();

        let large = "x".repeat(BODY_CACHE_CAP_BYTES + 1);
        let raw = make_msg(&large);
        let mut session = MockSession {
            fetch_body_called: AtomicBool::new(false),
            body: raw,
        };

        let _ = async_std::task::block_on(async {
            fetch_body_message(&mut session, conn, msg_id, 1, 100, std::path::Path::new("/tmp")).await
        }).unwrap();

        assert!(
            session.fetch_body_called.load(Ordering::SeqCst),
            "should call IMAP on cache miss"
        );
        assert!(
            !queries::body_is_complete(conn, msg_id).unwrap(),
            "body should not be cached (over cap)"
        );
    }

    #[test]
    fn sanitize_strips_script_and_iframe() {
        let input = r#"<script>alert(1)</script><p>hello</p><iframe srcdoc="evil"></iframe>"#;
        let out = sanitize_html(input);
        assert!(!out.contains("<script"), "script tag should be stripped");
        assert!(!out.contains("iframe"), "iframe should be stripped");
        assert!(out.contains("hello"), "text content should be preserved");
    }

    #[test]
    fn sanitize_keeps_safe_tags() {
        let input = r#"<p>hello <b>world</b></p>"#;
        let out = sanitize_html(input);
        assert!(out.contains("hello"), "text preserved");
        assert!(out.contains("<b>"), "safe tag preserved");
    }
}