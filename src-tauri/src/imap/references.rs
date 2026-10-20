//! IMAP `References:` header — thread-chain extraction (RFC 5322 §4.3).
//!
//! Grammar: `references-list = reference *(WSP reference)`; each `reference`
//! is a message id, normally `<...>`-framed, but some servers emit ids bare,
//! without angle brackets (`bare_and_case` guards that quirk). Headers arrive
//! folded: the spec's fold width is column 998, but real servers (Zimbra on
//! the UTFPR wire) break long fields at narrower widths with CRLF+WSP
//! continuations, so this parser unfolds BEFORE tokenizing and never assumes
//! a fold position — ids can never carry CR/LF bytes into the store.
//!
//! Chain rules (locked by CONTEXT D-04/D-05, mirrored here for the 42-03
//! lane that calls this exactly once at fetch time):
//! - Absent/empty field → `None`: the own-thread sentinel. The store writes
//!   NULL `references_json` and never a synthetic empty chain.
//! - Bare tokens are normalized to `<...>` framing only — case preserved
//!   exactly (msg-ids are case-significant; joins are exact-match).
//! - Duplicates dedupe order-preservingly, first occurrence kept.
//! - Caps: [`MAX_CHAIN_IDS`] ids per chain and [`MAX_RAW_BYTES`] of raw
//!   input; when either guard fires, `truncated` records it, bounding
//!   graph-walk blowup (threat T-42-02).
//!
//! Degradation policy: malformed fragments drop, parsing never fails — a
//! broken `References:` header must never break the mail row behind it.
//!
//! WHY-NOT the house decoder: `headers.rs`'s `decode_header_words` runs the
//! RFC 2047 display pass, and msg-ids legally contain substrings shaped
//! exactly like encoded-words (`<X=?utf-8?Q?=42?=@host>`). Routing msg-id
//! bytes through it would rewrite them and silently break exact-match joins,
//! so tokens pass through VERBATIM; the contrast probe test below keeps that
//! proof pinned against future refactors.
//!
//! This parser owns no I/O and no SQL — the 42-03 fetch lane calls
//! [`parse_references`] once at fetch time and the store persists the
//! normalized ids verbatim as JSON.

/// Caps on one parsed chain. Beyond id count or raw size, the graph walk
/// behind a single row blows up — both guards bound it (threat T-42-02).
pub const MAX_CHAIN_IDS: usize = 100;
pub const MAX_RAW_BYTES: usize = 32 * 1024;

/// A parsed thread chain: message ids in References order, deduped, each
/// normalized to `<...>` framing with case preserved.
pub struct ThreadChain {
    /// Ordered ids — never empty (an empty chain is `None` upstream).
    pub ids: Vec<String>,
    /// Set when the 100-id or 32 KiB guard dropped part of the field.
    pub truncated: bool,
}

/// Parse the raw value of a `References` field — the bytes that already
/// went through `headers.rs::decode_bytes_opt` semantics (lossy decode,
/// trailing-CR trim, NO RFC 2047 pass). `None` = absent/empty field: the
/// own-thread sentinel, so the store writes NULL `references_json`.
pub fn parse_references(raw: &str) -> Option<ThreadChain> {
    let mut truncated = false;

    // 32 KiB raw guard, floored to a char boundary so the cut cannot split
    // a multibyte char from a lossy decode.
    let window = if raw.len() > MAX_RAW_BYTES {
        truncated = true;
        let mut cut = MAX_RAW_BYTES;
        while !raw.is_char_boundary(cut) {
            cut -= 1;
        }
        &raw[..cut]
    } else {
        raw
    };

    // Unfold CRLF+WSP continuation FIRST (fixture folded_two_line): message
    // ids never contain CR/LF, so stripping the fold's newline leaves the
    // continuation WSP as a plain separator.
    let unfolded = unfold(window);
    let bytes = unfolded.as_bytes();
    let n = bytes.len();

    let mut ids: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        if bytes[i] == b'<' {
            // Framed id: consume to the next '>'. A msg-id cannot legally
            // carry '>', so the first match closes the token.
            match unfolded[i + 1..].find('>') {
                Some(off) => {
                    i += off + 2;
                }
                None => {
                    // Unterminated '<' — degrade: swallow to the next WSP,
                    // then the well-formedness check below drops it.
                    while i < n && bytes[i] != b' ' && bytes[i] != b'\t' {
                        i += 1;
                    }
                }
            }
        } else {
            // Bare token (bracket-less quirk): run to the next WSP.
            while i < n && bytes[i] != b' ' && bytes[i] != b'\t' {
                i += 1;
            }
        }
        let token = &unfolded[start..i];
        let framed = bytes[start] == b'<';

        let well_formed = if framed {
            // Interior non-empty, properly closed, no nested '<'. The
            // framing ASCII bounds make the interior slice safe.
            token.len() >= 3 && token.ends_with(">") && !token[1..token.len() - 1].contains('<')
        } else {
            // A bare token may not smuggle brackets — that would alias.
            !token.contains('<') && !token.contains('>')
        };
        if !well_formed {
            continue;
        }

        // Frame-only normalization, case preserved exactly (msg-ids are
        // case-significant; joins are exact-match).
        let normalized: String = if framed {
            token.to_string()
        } else {
            format!("<{}>", token)
        };

        if ids.iter().any(|existing| *existing == normalized) {
            continue; // order-preserving first-occurrence dedupe
        }
        if ids.len() == MAX_CHAIN_IDS {
            // First unseen id past the cap: record and stop (T-42-02).
            truncated = true;
            break;
        }
        ids.push(normalized);
    }

    if ids.is_empty() {
        return None; // own-thread sentinel — never a synthetic empty chain
    }
    Some(ThreadChain { ids, truncated })
}

/// RFC 5322 unfolding. CR/LF is never valid inside a References value: it is
/// either the newline of a CRLF+WSP fold (dropped; its WSP survives as the
/// token separator) or protocol noise (also dropped). Either way the result
/// carries no CR/LF byte at all — that is the `folded_two_line` guarantee.
fn unfold(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        let b = bytes[i];
        if b == b'\r' {
            // Drop a CRLF pair or a lone CR; a fold's WSP continuation
            // survives and becomes the token separator.
            i += if i + 1 < n && bytes[i + 1] == b'\n' {
                2
            } else {
                1
            };
            continue;
        }
        if b == b'\n' {
            i += 1;
            continue;
        }
        // Copy one char (may be multibyte from the lossy decode upstream).
        let ch = s[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_folded_two_line() {
        // RESEARCH corpus: Zimbra folds long fields mid-header. After
        // unfold the ids must be clean, ordered, and untruncated.
        let raw = "<a@utfpr.example>\r\n <b@utfpr.example>\r\n <c@utfpr.example>";
        let chain = parse_references(raw).expect("three ids");
        assert_eq!(
            chain.ids,
            [
                "<a@utfpr.example>",
                "<b@utfpr.example>",
                "<c@utfpr.example>",
            ]
        );
        assert!(!chain.truncated);
        for id in chain.ids {
            assert!(!id.contains('\r') && !id.contains('\n'));
        }
    }

    #[test]
    fn references_framed_multi() {
        // Ordered References are consumed as-is by the crate.
        let chain = parse_references("<r1@x> <r2@x> <m1@x>").expect("trio");
        assert_eq!(chain.ids, ["<r1@x>", "<r2@x>", "<m1@x>"]);
        assert!(!chain.truncated);
    }

    #[test]
    fn references_bare_and_case() {
        // Bracket-less quirk: frame-only normalization, case preserved
        // exactly — msg-ids are case-significant (exact-match joins).
        let chain = parse_references("a@x B@Y <C@Z>").expect("trio");
        assert_eq!(chain.ids, ["<a@x>", "<B@Y>", "<C@Z>"]);
        assert!(!chain.truncated);
    }

    #[test]
    fn references_malformed_or_empty() {
        // Own-thread sentinel: empty/absent parses to None — the store
        // writes NULL references_json, never a synthetic empty chain.
        assert!(parse_references("").is_none());
        assert!(parse_references("   ").is_none());
        // Malformed fragments degrade to nothing rather than failing.
        assert!(parse_references("<unclosed").is_none());
        assert!(parse_references("<> <>").is_none());
    }

    #[test]
    fn references_duplicate_ids() {
        let chain = parse_references("<a@x> <b@y> <a@x> <b@y>").expect("pair");
        assert_eq!(chain.ids, ["<a@x>", "<b@y>"]);
        assert!(!chain.truncated);
    }

    #[test]
    fn references_chain_cap_overflow() {
        // 150 well-formed ids: first 100 kept, truncation recorded
        // (PITFALLS #2 — recompute cost stays bounded). ~2.4 KB raw: the
        // 100-id cap is what fires here, not the 32 KiB guard.
        let mut id_vec: Vec<String> = Vec::new();
        for n in 1..151 {
            id_vec.push(format!("<id{:03}@cap>", n));
        }
        let raw = id_vec.join(" ");
        let chain = parse_references(&raw).expect("capped chain");
        assert_eq!(chain.ids.len(), MAX_CHAIN_IDS);
        assert_eq!(chain.ids[0], "<id001@cap>");
        assert_eq!(chain.ids[MAX_CHAIN_IDS - 1], "<id100@cap>");
        assert!(chain.truncated);
        assert!(raw.len() < MAX_RAW_BYTES);
    }

    #[test]
    fn references_encoded_shaped_ids() {
        // VERBATIM pass-through: an encoded-word-shaped msg-id substring is
        // NOT rewritten (the RFC 2047 display pass is forbidden on msg-ids).
        let chain = parse_references("<X=?utf-8?Q?=42?=@host> <p@q>").expect("pair");
        assert_eq!(chain.ids, ["<X=?utf-8?Q?=42?=@host>", "<p@q>"]);
        assert!(!chain.truncated);
    }

    // Contrast probe: proof (not prose) that the house RFC 2047 decoder
    // must never run on msg-id bytes — it rewrites an encoded_shaped_ids-
    // style token into a shorter corrupted string, breaking joins.
    #[test]
    fn references_decoder_mangles_ids_contrast_probe() {
        let token = "<X=?utf-8?Q?=42?=@host>";
        let mangled = crate::imap::headers::decode_header_words(token);
        assert_eq!(mangled, "<XB@host>");
        assert!(mangled.len() < token.len());
    }
}
