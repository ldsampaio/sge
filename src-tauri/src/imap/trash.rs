//! Trash auto-detect (Phase 10, Plan 10-02; generalized in Phase 11).
//!
//! Thin delegating wrapper over [`roles::resolve_roles`](super::roles::resolve_roles):
//! layers, name lists, and `\Noselect` handling live in exactly one place
//! (`roles.rs`) — this function only picks the Trash entry, so there is
//! never a second detector to drift (T-11-08). The 8 tests below pass
//! UNCHANGED as the parity proof.
//!
//! Always returns the RAW wire name (modified UTF-7) — the only form valid
//! for SELECT. The resolved name is cached per account in
//! `AppState::trash_cache` and re-detected on LIST refresh.

use super::roles::{self, Role};
use super::MailboxInfo;

/// Where a delete should land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashResolution {
    /// Raw wire name of the Trash folder.
    Found(String),
    /// No Trash on the server — caller prompts confirm, then `create_trash()`.
    Missing,
}

/// Resolve the Trash folder from a LIST result: the first Trash-role entry
/// from [`roles::resolve_roles`](super::roles::resolve_roles), with
/// `\Trash`-attributed folders preferred over Trash-named ones (preserves
/// the original layer-1-before-name priority — e.g. `Archive` with `\Trash`
/// beats a plain folder named `Trash`).
pub fn detect_trash(mailboxes: &[MailboxInfo]) -> TrashResolution {
    let roles = roles::resolve_roles(mailboxes);
    let is_trash = |i: usize| roles.get(i).is_some_and(|(_, r)| *r == Role::Trash);
    // Attribute-sourced Trash first (input order).
    for (i, mb) in mailboxes.iter().enumerate() {
        if is_trash(i) && roles::has_special_use(mb, "trash") {
            return TrashResolution::Found(mb.name.clone());
        }
    }
    // Then any Trash-role entry (input order).
    for (i, mb) in mailboxes.iter().enumerate() {
        if is_trash(i) {
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
