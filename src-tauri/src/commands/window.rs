//! Window lifecycle commands: the standalone Compose window (Thunderbird
//! style) and the LLM settings window.
//!
//! The frontend owns everything inside a window: `App.tsx` routes
//! `#compose?...` / `#llm-config` *before* the login gate (a secondary window
//! shares this process's backend — in-memory account, keyring, SQLite — so it
//! must not render the login shell). The backend therefore owns only the
//! window itself: a unique label, the URL hash, the title and the size.
//!
//! Reply modes carry `uid` + `mailbox` in that hash so the new window can
//! `BODY.PEEK`-fetch the original and prefill recipients/subject/quote and
//! threading headers itself (M1 read-only invariant preserved — the prefetch
//! never sets `\Seen`).
//!
//! Labels are chosen to match the `windows` glob in
//! `capabilities/default.json` (`compose-*`, `llm-config`): a window that
//! matches no capability cannot call a single `invoke`, so the labels and the
//! capability list are one contract.

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Reply/new compose windows are labeled `compose-<n>` (capability glob
/// `compose-*`). Tauri refuses a duplicate label, and a *closed* label must
/// never be reused silently — two reply windows are two different windows —
/// so the suffix is a monotonic counter, never a hash of the source message.
const COMPOSE_LABEL_PREFIX: &str = "compose-";

/// Exactly one settings window: a second click focuses the open one instead
/// of stacking duplicates.
const LLM_CONFIG_LABEL: &str = "llm-config";

static COMPOSE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Compose mode vocabulary shared with the frontend
/// (`ComposeMode` in `src/types.ts`, `parseComposeHash` in `src/lib/compose.ts`).
fn normalize_mode(mode: &str) -> Result<&'static str, String> {
    match mode {
        "new" => Ok("new"),
        "reply" => Ok("reply"),
        "reply-all" => Ok("reply-all"),
        other => Err(format!(
            "unknown compose mode '{other}' — expected new, reply or reply-all"
        )),
    }
}

/// Percent-encode one query component (RFC 3986 unreserved set kept verbatim).
///
/// Mailbox wire names reach this unchanged from the frontend and may contain
/// `/`, `&`, spaces or modified UTF-7 (`&AOk-`) — anything that would break
/// out of the query string is encoded here, and `URLSearchParams` on the
/// other side decodes it back.
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `#compose?mode=...&uid=...&mailbox=...` — the exact shape
/// `parseComposeHash` parses. `mailbox` is always present (defaults to INBOX
/// on both sides) so a reply window never silently prefills from the wrong
/// folder.
fn compose_hash(mode: &str, uid: Option<u32>, mailbox: Option<&str>) -> String {
    let mut hash = format!("#compose?mode={mode}");
    if let Some(uid) = uid {
        hash.push_str(&format!("&uid={uid}"));
    }
    hash.push_str(&format!(
        "&mailbox={}",
        encode_component(mailbox.unwrap_or("INBOX"))
    ));
    hash
}

/// Open the Compose window: blank for a new message, prefilled by the
/// answered message for reply / reply-all.
///
/// `uid`/`mailbox` are reply-only inputs; a call with `mode="new"` ignores
/// them (nothing to fetch). Window creation is dispatched to the UI thread by
/// Tauri, so this command stays async deliberately.
#[tauri::command]
pub async fn open_compose_window(
    app: AppHandle,
    mode: String,
    uid: Option<u32>,
    mailbox: Option<String>,
) -> Result<(), String> {
    let mode = normalize_mode(&mode)?;
    let label = format!(
        "{COMPOSE_LABEL_PREFIX}{}",
        COMPOSE_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let title = match mode {
        "new" => "Nova mensagem",
        "reply" => "Responder",
        _ => "Responder a todos",
    };
    let url = WebviewUrl::App(
        format!("index.html{}", compose_hash(mode, uid, mailbox.as_deref())).into(),
    );
    WebviewWindowBuilder::new(&app, &label, url)
        .title(title)
        .inner_size(760.0, 620.0)
        .min_inner_size(520.0, 420.0)
        .build()
        .map_err(|e| format!("could not open the compose window: {e}"))?;
    Ok(())
}

/// Open the LLM settings window (`#llm-config`), or focus the open one.
#[tauri::command]
pub async fn open_llm_config_window(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(LLM_CONFIG_LABEL) {
        // Already open: raise it instead of stacking a second settings window
        // (two windows editing one keyring entry would race on save).
        let _ = existing.unminimize();
        existing
            .set_focus()
            .map_err(|e| format!("could not focus the settings window: {e}"))?;
        return Ok(());
    }
    WebviewWindowBuilder::new(
        &app,
        LLM_CONFIG_LABEL,
        WebviewUrl::App("index.html#llm-config".into()),
    )
    .title("Configuração de LLM")
    .inner_size(720.0, 640.0)
    .min_inner_size(520.0, 460.0)
    .build()
    .map_err(|e| format!("could not open the settings window: {e}"))?;
    Ok(())
}

/// Close the calling window.
///
/// The Compose window calls this so a successful send can dismiss itself
/// without depending on the frontend `close()` permission path (`invoke`
/// round-trips through the backend, which owns the window). Only the window
/// that issued the call is closed — never another compose window.
#[tauri::command]
pub async fn close_compose_window(window: WebviewWindow) -> Result<(), String> {
    window
        .close()
        .map_err(|e| format!("could not close the window: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_hash_carries_mode_uid_and_encoded_mailbox() {
        assert_eq!(
            compose_hash("new", None, None),
            "#compose?mode=new&mailbox=INBOX"
        );
        assert_eq!(
            compose_hash("reply-all", Some(42), Some("Pastas/Arquivadas")),
            "#compose?mode=reply-all&uid=42&mailbox=Pastas%2FArquivadas"
        );
        // Modified UTF-7 wire names must survive the round-trip.
        assert_eq!(
            compose_hash("reply", Some(7), Some("&AOk-")),
            "#compose?mode=reply&uid=7&mailbox=%26AOk-"
        );
    }

    #[test]
    fn unknown_modes_are_refused_by_name() {
        let err = normalize_mode("forward").unwrap_err();
        assert!(err.contains("forward"), "error names the bad value: {err}");
        assert_eq!(normalize_mode("reply").unwrap(), "reply");
    }

    #[test]
    fn compose_labels_are_unique_and_capability_scoped() {
        let a = format!(
            "{COMPOSE_LABEL_PREFIX}{}",
            COMPOSE_SEQ.fetch_add(1, Ordering::SeqCst)
        );
        let b = format!(
            "{COMPOSE_LABEL_PREFIX}{}",
            COMPOSE_SEQ.fetch_add(1, Ordering::SeqCst)
        );
        assert_ne!(a, b);
        assert!(
            a.starts_with("compose-"),
            "capability glob compose-* covers {a}"
        );
        assert_eq!(LLM_CONFIG_LABEL, "llm-config");
    }
}
