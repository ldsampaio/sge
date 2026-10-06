//! IMAP modified UTF-7 codec (RFC 3501 §5.1.3) for mailbox names.
//!
//! `LIST` returns international mailbox names in a modified UTF-7 encoding
//! (e.g. `Orienta&AOcA9Q-es` for `Orientações`). The wire form must be kept
//! for `SELECT`/`STATUS`/`CREATE`/`RENAME`/`DELETE` (protocol), while the UI
//! shows the decoded form.
//!
//! Wire/display split: [`decode_modified_utf7`] turns wire names into UI
//! display names; [`encode_modified_utf7`] turns user-typed leaves back
//! into wire names before any verb. The UI never encodes — the folder
//! commands (`commands::sync`) own the encode step, after [`validate_leaf`]
//! rejects empty, delimiter-carrying, and INBOX-variant leaves.

/// Decode an IMAP modified-UTF-7 mailbox name to display form.
///
/// Rules: printable ASCII (except `&`) passes through; `&-` is a literal
/// `&`; `&<modified-base64>-` decodes as UTF-16BE (`,` stands in for `/`).
/// Malformed shifts degrade to U+FFFD rather than failing — a folder name
/// must never break the tree render.
pub fn decode_modified_utf7(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'&' {
            // LIST names are ASCII outside shifts; anything else degrades.
            out.push(if b >= 0x20 && b < 0x7f {
                b as char
            } else {
                '\u{FFFD}'
            });
            i += 1;
            continue;
        }
        // Shift sequence: find the terminating '-'.
        let rest = &name[i + 1..];
        match rest.find('-') {
            None => {
                // Unterminated shift — literalize the rest and stop.
                out.push('\u{FFFD}');
                break;
            }
            Some(end) => {
                let chunk = &rest[..end];
                if chunk.is_empty() {
                    out.push('&'); // `&-` — literal ampersand.
                } else {
                    out.push_str(&decode_shift(chunk));
                }
                i += 1 + end + 1;
            }
        }
    }
    out
}

/// Encode a display-form mailbox name to IMAP modified UTF-7 wire form.
///
/// Inverse grammar of [`decode_modified_utf7`]: printable ASCII (except
/// `&`) passes through byte-identical — so hierarchy delimiters (`/`, `.`)
/// are never encoded; `&` becomes `&-`; maximal non-ASCII runs become
/// UTF-16BE → modified base64 (`,` for `/`, `=` padding stripped) wrapped
/// in `&…-`. Callers must run [`validate_leaf`] first: the encoder never
/// makes hierarchy decisions, it only encodes what it is given.
pub fn encode_modified_utf7(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut run: Vec<u16> = Vec::new();
    let flush = |out: &mut String, run: &mut Vec<u16>| {
        if run.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(run.len() * 2);
        for u in run.drain(..) {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        let b64 = base64_encode(&bytes);
        // Modified base64: `/` → `,`, padding stripped.
        let b64: String = b64.trim_end_matches('=').replace('/', ",");
        out.push('&');
        out.push_str(&b64);
        out.push('-');
    };
    for c in name.chars() {
        if c == '&' {
            flush(&mut out, &mut run);
            out.push_str("&-");
        } else if c.is_ascii() && (c as u8) >= 0x20 && (c as u8) < 0x7f {
            flush(&mut out, &mut run);
            out.push(c);
        } else {
            // Non-ASCII (or control/DEL): UTF-16BE shift run, surrogates
            // included for astral chars.
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                run.push(*u);
            }
        }
    }
    flush(&mut out, &mut run);
    out
}

/// Why a folder leaf was rejected before ever reaching the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderNameError {
    /// Empty after trimming whitespace.
    Empty,
    /// Carries the hierarchy delimiter — would create nesting the user
    /// did not pick (hierarchy escape T-11-02).
    ContainsDelimiter(char),
    /// Case-insensitive `INBOX` — a reserved server folder.
    ReservedInbox,
}

impl std::fmt::Display for FolderNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FolderNameError::Empty => write!(f, "folder name is empty"),
            FolderNameError::ContainsDelimiter(d) => {
                write!(f, "folder name contains hierarchy delimiter {d:?}")
            }
            FolderNameError::ReservedInbox => write!(f, "INBOX is a reserved folder"),
        }
    }
}

impl std::error::Error for FolderNameError {}

/// Validate a user-typed folder leaf BEFORE encoding or joining.
///
/// Pure, no I/O. `delimiter` is the parent's cached LIST delimiter (`""`
/// disables the delimiter check — top-level creates join nothing).
/// Case-insensitive `inbox` is reserved on any delimiter.
pub fn validate_leaf(leaf: &str, delimiter: &str) -> Result<(), FolderNameError> {
    let trimmed = leaf.trim();
    if trimmed.is_empty() {
        return Err(FolderNameError::Empty);
    }
    if !delimiter.is_empty() {
        if let Some(d) = trimmed.chars().find(|c| delimiter.contains(*c)) {
            return Err(FolderNameError::ContainsDelimiter(d));
        }
    }
    if trimmed.eq_ignore_ascii_case("inbox") {
        return Err(FolderNameError::ReservedInbox);
    }
    Ok(())
}

/// Minimal base64 encoder (standard alphabet with `=` padding; the
/// caller adapts it to the modified alphabet).
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHA[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHA[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Decode one `&...-` payload (modified base64 → UTF-16BE → String).
fn decode_shift(chunk: &str) -> String {
    // Modified base64 uses ',' where standard base64 uses '/'.
    let std_b64: String = chunk.chars().map(|c| if c == ',' { '/' } else { c }).collect();
    // Payloads omit padding; restore it to a multiple of 4.
    let padded = match std_b64.len() % 4 {
        0 => std_b64,
        r => std_b64 + &"=".repeat(4 - r),
    };
    let bytes = match base64_decode(&padded) {
        Some(b) => b,
        None => return "\u{FFFD}".to_string(),
    };
    if bytes.len() % 2 != 0 {
        return "\u{FFFD}".to_string();
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

/// Minimal base64 decoder (standard alphabet + `=` padding only).
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0;
    for &b in s.as_bytes() {
        if b == b'=' {
            break;
        }
        buf = (buf << 6) | val(b)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8 & 0xff);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_user_reported_folder() {
        // Real UTFPR folder name from the field report.
        assert_eq!(decode_modified_utf7("Orienta&AOcA9Q-es"), "Orientações");
    }

    #[test]
    fn ascii_passthrough_and_literal_ampersand() {
        assert_eq!(decode_modified_utf7("INBOX"), "INBOX");
        assert_eq!(decode_modified_utf7("Caixa &- Arquivo"), "Caixa & Arquivo");
        assert_eq!(decode_modified_utf7("Sent"), "Sent");
    }

    #[test]
    fn decodes_accented_prefix_and_suffix() {
        // "&AOk-" = U+00E9 (é); trailing shift after ASCII.
        assert_eq!(decode_modified_utf7("&AOk-cole"), "école");
        assert_eq!(decode_modified_utf7("Caf&AOk-"), "Café");
    }

    #[test]
    fn malformed_shifts_degrade_gracefully() {
        assert!(decode_modified_utf7("A&").contains('\u{FFFD}'));
        assert!(decode_modified_utf7("A&!!!-B").contains('B'));
    }

    #[test]
    fn encode_matches_known_wire_form() {
        // Real UTFPR folder name: the encoder must reproduce the exact
        // wire bytes the server's LIST returns.
        assert_eq!(encode_modified_utf7("Orientações"), "Orienta&AOcA9Q-es");
        assert_eq!(encode_modified_utf7("INBOX"), "INBOX");
        assert_eq!(encode_modified_utf7("A&B"), "A&-B");
        assert_eq!(encode_modified_utf7("Café"), "Caf&AOk-");
    }

    #[test]
    fn encode_roundtrips_through_decode() {
        for name in [
            "Projetos",
            "Lixeira & Cia",
            "日本語",
            "&-",
            "&",
            "A&B&C",
            "école",
            "Pai/Filho",
            "a.b",
            "100%",
            "Grüße aus München",
        ] {
            assert_eq!(
                decode_modified_utf7(&encode_modified_utf7(name)),
                name,
                "roundtrip failed for {name:?}"
            );
        }
    }

    #[test]
    fn encode_never_emits_bare_ampersand() {
        for name in ["&", "A&B", "Lixeira & Cia", "&&&"] {
            let wire = encode_modified_utf7(name);
            // Every `&` in the output belongs to `&-` or an `&…-` shift:
            // strip shifts, then no `&` may remain.
            let mut stripped = String::new();
            let mut rest = wire.as_str();
            while let Some(i) = rest.find('&') {
                stripped.push_str(&rest[..i]);
                let tail = &rest[i + 1..];
                match tail.find('-') {
                    Some(end) => rest = &tail[end + 1..],
                    None => panic!("bare & in {wire:?}"),
                }
            }
            stripped.push_str(rest);
            assert!(!stripped.contains('&'), "bare & in {wire:?}");
            assert_eq!(decode_modified_utf7(&wire), name);
        }
    }

    #[test]
    fn encode_leaves_delimiters_byte_identical() {
        assert_eq!(encode_modified_utf7("Pai/Filho"), "Pai/Filho");
        assert_eq!(encode_modified_utf7("a.b.c"), "a.b.c");
    }

    #[test]
    fn validate_leaf_rejects_and_accepts() {
        assert_eq!(validate_leaf("", "/"), Err(FolderNameError::Empty));
        assert_eq!(validate_leaf("   ", "/"), Err(FolderNameError::Empty));
        assert_eq!(
            validate_leaf("A/B", "/"),
            Err(FolderNameError::ContainsDelimiter('/'))
        );
        assert_eq!(
            validate_leaf("a.b", "."),
            Err(FolderNameError::ContainsDelimiter('.'))
        );
        // Empty delimiter disables the hierarchy check.
        assert_eq!(validate_leaf("A/B", ""), Ok(()));
        for inbox in ["inbox", "INBOX", "Inbox", "  InBox  "] {
            assert_eq!(
                validate_leaf(inbox, "/"),
                Err(FolderNameError::ReservedInbox),
                "{inbox:?} must be reserved"
            );
        }
        assert_eq!(validate_leaf("A&B", "/"), Ok(()));
        assert_eq!(validate_leaf("Projetos", "/"), Ok(()));
        assert_eq!(validate_leaf("Inbox Zero", "/"), Ok(()));
    }
}
