//! Folder role resolution (Phase 11, Plan 11-03).
//!
//! Generalizes `trash.rs detect_trash` into a roles schema: SPECIAL-USE
//! attributes first (`\Trash`, `\Sent`, `\Drafts`), then known-name match
//! (Sidebar `SYSTEM_ORDER` ranks + Gmail-style variants), cached per
//! account. `detect_trash` delegates to [`resolve_roles`] (thin wrapper kept
//! for the `delete_message` call site) so there is never a second detector
//! to drift (T-11-08) — the 8 `trash.rs` tests pass unchanged as the parity
//! proof.
//!
//! Always resolves RAW wire names — the only form valid for SELECT. The
//! resolved roles persist to the M8 `role` column on every LIST refresh
//! (cache survives restarts, recompute keeps it honest — T-11-07).

use super::MailboxInfo;

/// Folder role: Inbox is matched first by name; Trash/Sent/Drafts resolve
/// through SPECIAL-USE attributes then known names; everything else is
/// Custom. `\Noselect` placeholders always resolve Custom (never targets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Inbox,
    Trash,
    Sent,
    Drafts,
    Custom,
}

impl Role {
    /// Canonical lowercase wire value persisted to the M8 `role` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Inbox => "inbox",
            Role::Trash => "trash",
            Role::Sent => "sent",
            Role::Drafts => "drafts",
            Role::Custom => "custom",
        }
    }
}

/// Full wire-name candidates (compared lowercased). Moved from `trash.rs`
/// (not copied): Sidebar `SYSTEM_ORDER` rank-4 list plus Gmail-style and
/// ES variants.
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
/// Moved from `trash.rs` (not copied).
const TRASH_LEAF_NAMES: &[&str] = &["trash", "deleted", "lixeira", "papelera"];

/// Full-name candidates for Sent (Sidebar `SYSTEM_ORDER` rank 2 +
/// Gmail-style variant).
const SENT_FULL_NAMES: &[&str] = &[
    "sent",
    "sent messages",
    "enviadas",
    "enviados",
    "[gmail]/sent mail",
];

/// Last-segment candidates for nested Sent folders.
const SENT_LEAF_NAMES: &[&str] = &["sent", "enviadas", "enviados"];

/// Full-name candidates for Drafts (Sidebar `SYSTEM_ORDER` rank 1 +
/// Gmail-style variant).
const DRAFTS_FULL_NAMES: &[&str] = &["drafts", "rascunhos", "[gmail]/drafts"];

/// Last-segment candidates for nested Drafts folders.
const DRAFTS_LEAF_NAMES: &[&str] = &["drafts", "rascunhos"];

/// Hierarchy placeholders can never be SELECTed — never a role target.
fn is_noselect(mb: &MailboxInfo) -> bool {
    mb.attributes.iter().any(|a| a.contains("NoSelect"))
}

/// SPECIAL-USE attribute spelled with any backslash count (`\Trash` and
/// `\\Trash` both match — mirrors the `name_attribute_to_string` spellings).
/// Shared layer primitive (also used by the `detect_trash` delegation).
pub fn has_special_use(mb: &MailboxInfo, atom: &str) -> bool {
    mb.attributes
        .iter()
        .any(|a| a.trim_matches('\\').eq_ignore_ascii_case(atom))
}

/// Last hierarchy segment of a name (`/` and `.` delimiters), lowercased.
fn leaf_of(s: &str) -> String {
    s.split(['/', '.']).next_back().unwrap_or(s).to_lowercase()
}

/// Name-only layer (full-name then leaf, wire + display forms).
fn match_names(mb: &MailboxInfo, full: &[&str], leaf: &[&str]) -> bool {
    let wire = mb.name.to_lowercase();
    let display = mb.display_name.to_lowercase();
    if full.contains(&wire.as_str()) || full.contains(&display.as_str()) {
        return true;
    }
    let wire_leaf = leaf_of(&mb.name);
    let display_leaf = leaf_of(&mb.display_name);
    leaf.contains(&wire_leaf.as_str()) || leaf.contains(&display_leaf.as_str())
}

/// Resolve the role of every mailbox in LIST order.
///
/// Layer order per mailbox: INBOX name (case-insensitive, first — INBOX is
/// never Trash) → SPECIAL-USE attributes (win over names across roles:
/// a `\Sent` folder named `Trash` resolves Sent) → full-name match → leaf
/// match, first hit winning with role priority Trash > Sent > Drafts.
/// `\Noselect` rows always resolve Custom. Returns `(raw wire name, role)`
/// pairs in input order.
pub fn resolve_roles(mailboxes: &[MailboxInfo]) -> Vec<(String, Role)> {
    mailboxes
        .iter()
        .map(|mb| {
            let role = if is_noselect(mb) {
                Role::Custom
            } else if mb.name.eq_ignore_ascii_case("inbox")
                || mb.display_name.eq_ignore_ascii_case("inbox")
            {
                Role::Inbox
            } else if has_special_use(mb, "trash") {
                Role::Trash
            } else if has_special_use(mb, "sent") {
                Role::Sent
            } else if has_special_use(mb, "drafts") {
                Role::Drafts
            } else if match_names(mb, TRASH_FULL_NAMES, TRASH_LEAF_NAMES) {
                Role::Trash
            } else if match_names(mb, SENT_FULL_NAMES, SENT_LEAF_NAMES) {
                Role::Sent
            } else if match_names(mb, DRAFTS_FULL_NAMES, DRAFTS_LEAF_NAMES) {
                Role::Drafts
            } else {
                Role::Custom
            };
            (mb.name.clone(), role)
        })
        .collect()
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

    fn role_of(folders: &[MailboxInfo], name: &str) -> Role {
        resolve_roles(folders)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r)
            .expect("mailbox must resolve")
    }

    #[test]
    fn sent_attribute_wins_over_trash_name() {
        // A folder NAMED Trash carrying `\Sent` resolves Sent: attributes
        // beat names across roles (the trash.rs layer inversion is
        // intentional here — one schema, attribute-first).
        let folders = vec![mb("Trash", &["\\Sent"]), mb("INBOX", &[])];
        assert_eq!(role_of(&folders, "Trash"), Role::Sent);
    }

    #[test]
    fn drafts_attribute_wins_over_name() {
        let folders = vec![mb("Drafts", &["\\Drafts"]), mb("INBOX", &[])];
        assert_eq!(role_of(&folders, "Drafts"), Role::Drafts);
    }

    #[test]
    fn gmail_style_sent_and_drafts_match() {
        let folders = vec![
            mb("INBOX", &[]),
            mb("[Gmail]/Sent Mail", &["\\HasNoChildren"]),
            mb("[Gmail]/Drafts", &["\\HasNoChildren"]),
        ];
        assert_eq!(role_of(&folders, "[Gmail]/Sent Mail"), Role::Sent);
        assert_eq!(role_of(&folders, "[Gmail]/Drafts"), Role::Drafts);
    }

    #[test]
    fn leaf_match_for_nested_system_folders() {
        let folders = vec![
            mb("INBOX", &[]),
            mb("Archive/Enviadas", &[]),
            mb("Archive/Rascunhos", &[]),
        ];
        assert_eq!(role_of(&folders, "Archive/Enviadas"), Role::Sent);
        assert_eq!(role_of(&folders, "Archive/Rascunhos"), Role::Drafts);
    }

    #[test]
    fn noselect_never_targeted() {
        let folders = vec![
            mb("Trash", &["\\NoSelect"]),
            mb("Sent", &["\\NoSelect"]),
            mb("INBOX", &[]),
        ];
        assert_eq!(role_of(&folders, "Trash"), Role::Custom);
        assert_eq!(role_of(&folders, "Sent"), Role::Custom);
    }

    #[test]
    fn inbox_never_trash() {
        // Even a hypothetical `\Trash`-attributed INBOX stays Inbox:
        // INBOX matches first by name.
        let folders = vec![mb("INBOX", &["\\Trash"])];
        assert_eq!(role_of(&folders, "INBOX"), Role::Inbox);
    }

    #[test]
    fn trash_layers_match_detect_trash_order() {
        // Attribute beats a competing name (same mailbox, both present).
        let folders = vec![
            mb("Trash", &[]),
            mb("Archive", &["\\Trash"]),
            mb("INBOX", &[]),
        ];
        assert_eq!(role_of(&folders, "Archive"), Role::Trash);
        assert_eq!(role_of(&folders, "Trash"), Role::Trash);
    }

    #[test]
    fn role_wire_values() {
        assert_eq!(Role::Inbox.as_str(), "inbox");
        assert_eq!(Role::Trash.as_str(), "trash");
        assert_eq!(Role::Sent.as_str(), "sent");
        assert_eq!(Role::Drafts.as_str(), "drafts");
        assert_eq!(Role::Custom.as_str(), "custom");
    }
}
