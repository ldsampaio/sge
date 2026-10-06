//! Trash auto-detect (Phase 10, Plan 10-02).
//!
//! Layers, in order (RESEARCH §6): 1. SPECIAL-USE `\Trash` LIST attribute
//! via the existing attribute mapping; 2. case-insensitive name match
//! (reusing the Sidebar `SYSTEM_ORDER` rank-4 list plus Gmail-style and
//! PT/ES variants); 3. `Missing` — the caller prompts for confirmation and
//! then [`SessionManager::create_trash`](super::manager::SessionManager::create_trash).
//! Silent CREATE behind a differently-named Trash would twin trash folders.
//!
//! Always returns the RAW wire name (modified UTF-7) — the only form valid
//! for SELECT. The resolved name is cached per account in
//! `AppState::trash_cache` and re-detected on LIST refresh (Phase 11 owns
//! the general roles schema — nothing is invented here).

use super::MailboxInfo;

/// Where a delete should land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashResolution {
    /// Raw wire name of the Trash folder.
    Found(String),
    /// No Trash on the server — caller prompts confirm, then `create_trash()`.
    Missing,
}

/// Full wire-name candidates (compared lowercased). Mirrors the Sidebar
/// `SYSTEM_ORDER` rank-4 list (`trash/deleted/deleted messages/lixeira`)
/// plus Gmail-style and ES variants from the live-LIST probe plan.
const TRASH_FULL_NAMES: &[&str] = &[
    "trash",
    "deleted",
    "deleted items",
    "deleted messages",
    "lixeira",
    "papelera",
    "[gmail]/trash",
    "[gmail]/lixeira",
];

/// Last-segment candidates for nested hierarchies (`Archive/Trash`).
const TRASH_LEAF_NAMES: &[&str] = &["trash", "deleted", "lixeira", "papelera"];

/// Hierarchy placeholders can never be SELECTed — never a Trash target.
fn is_noselect(mb: &MailboxInfo) -> bool {
    mb.attributes.iter().any(|a| a.contains("NoSelect"))
}

/// Resolve the Trash folder from a LIST result, layers in order.
pub fn detect_trash(mailboxes: &[MailboxInfo]) -> TrashResolution {
    // Layer 1: SPECIAL-USE `\Trash` attribute (RFC 6154). Backslashes are
    // stripped so one- and two-backslash spellings both match.
    for mb in mailboxes {
        if is_noselect(mb) {
            continue;
        }
        if mb
            .attributes
            .iter()
            .any(|a| a.trim_matches('\\').eq_ignore_ascii_case("trash"))
        {
            return TrashResolution::Found(mb.name.clone());
        }
    }
    // Layer 2a: full-name match on wire or display form (non-ASCII names
    // differ between the two — either may carry the recognizable spelling).
    for mb in mailboxes {
        if is_noselect(mb) {
            continue;
        }
        let wire = mb.name.to_lowercase();
        let display = mb.display_name.to_lowercase();
        if TRASH_FULL_NAMES.contains(&wire.as_str())
            || TRASH_FULL_NAMES.contains(&display.as_str())
        {
            return TrashResolution::Found(mb.name.clone());
        }
    }
    // Layer 2b: last hierarchy segment (`/` and `.` delimiters).
    for mb in mailboxes {
        if is_noselect(mb) {
            continue;
        }
        let leaf_of = |s: &str| {
            s.split(['/', '.'])
                .next_back()
                .unwrap_or(s)
                .to_lowercase()
        };
        let wire_leaf = leaf_of(&mb.name);
        let display_leaf = leaf_of(&mb.display_name);
        if TRASH_LEAF_NAMES.contains(&wire_leaf.as_str())
            || TRASH_LEAF_NAMES.contains(&display_leaf.as_str())
        {
            return TrashResolution::Found(mb.name.clone());
        }
    }
    // Layer 3 is the caller's job (confirm → CREATE): report Missing.
    TrashResolution::Missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mb(name: &str, attributes: &[&str]) -> MailboxInfo {
        MailboxInfo {
            name: name.to_string(),
            display_name: name.to_string(),
            delimiter: "/".to_string(),
            attributes: attributes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn trash_attribute_wins_over_name() {
        let folders = vec![
            mb("Trash", &[]),
            mb("Archive", &["\\Trash"]),
            mb("INBOX", &[]),
        ];
        assert_eq!(
            detect_trash(&folders),
            TrashResolution::Found("Archive".to_string())
        );
    }

    #[test]
    fn trash_double_backslash_attribute_matches() {
        // `name_attribute_to_string` spells it with two backslashes.
        let folders = vec![mb("Arquivo", &["\\\\Trash"]), mb("INBOX", &[])];
        assert_eq!(
            detect_trash(&folders),
            TrashResolution::Found("Arquivo".to_string())
        );
    }

    #[test]
    fn trash_pt_lixeira_match() {
        let folders = vec![mb("INBOX", &[]), mb("Lixeira", &[])];
        assert_eq!(
            detect_trash(&folders),
            TrashResolution::Found("Lixeira".to_string())
        );
    }

    #[test]
    fn trash_gmail_style_match() {
        let folders = vec![
            mb("INBOX", &[]),
            mb("[Gmail]/Lixeira", &["\\HasNoChildren"]),
        ];
        assert_eq!(
            detect_trash(&folders),
            TrashResolution::Found("[Gmail]/Lixeira".to_string())
        );
    }

    #[test]
    fn trash_case_insensitive_and_deleted_items() {
        let folders = vec![mb("INBOX", &[]), mb("Deleted Items", &[])];
        assert_eq!(
            detect_trash(&folders),
            TrashResolution::Found("Deleted Items".to_string())
        );
        let upper = vec![mb("TRASH", &[])];
        assert_eq!(
            detect_trash(&upper),
            TrashResolution::Found("TRASH".to_string())
        );
    }

    #[test]
    fn trash_nested_leaf_match() {
        let folders = vec![mb("INBOX", &[]), mb("Archive/Trash", &[])];
        assert_eq!(
            detect_trash(&folders),
            TrashResolution::Found("Archive/Trash".to_string())
        );
    }

    #[test]
    fn trash_noselect_placeholder_skipped() {
        // A `\NoSelect` "Trash" cannot be SELECTed — never a target.
        let folders = vec![mb("Trash", &["\\NoSelect"]), mb("INBOX", &[])];
        assert_eq!(detect_trash(&folders), TrashResolution::Missing);
    }

    #[test]
    fn trash_missing_when_no_candidate() {
        let folders = vec![mb("INBOX", &[]), mb("Sent", &["\\Sent"])];
        assert_eq!(detect_trash(&folders), TrashResolution::Missing);
        assert_eq!(detect_trash(&[]), TrashResolution::Missing);
    }
}
