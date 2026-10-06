//! IMAP modified UTF-7 decoding (RFC 3501 §5.1.3) for mailbox names.
//!
//! `LIST` returns international mailbox names in a modified UTF-7 encoding
//! (e.g. `Orienta&AOcA9Q-es` for `Orientações`). The wire form must be kept
//! for `SELECT`/`STATUS` (protocol), while the UI shows the decoded form.
//! This module only decodes; encoding is never needed (the client never
//! creates folders in v1.x).

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
}
