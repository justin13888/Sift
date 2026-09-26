//! Folders, and FR-5's special use resolved through the provider's own names.
//!
//! # Well-known names, not display names
//!
//! A German tenant's deleted-items folder is not called "Deleted Items", and a user may name
//! any folder of their own "Archive" in any language they like. So a folder's special use is
//! never read off its display name — "a locale table is a bug" — and is resolved instead
//! through the provider's **well-known folder names**, which are stable, locale-independent,
//! and accepted wherever a folder identifier is.
//!
//! The published v1.0 folder resource carries no well-known-name property (it is a preview
//! property), so the resolution is by **asking**: one batched request per well-known name,
//! each answering with the identifier of the folder that name denotes on this account. A name
//! the account does not have — an archive folder that was never created — answers 404 and
//! resolves to nothing, which is FR-5's "prompt the user once" rather than a guess.

use sift_provider::adapter::{RemoteFolder, RemoteFolderId, SpecialUse};

/// The well-known names Sift resolves, and the special use each one is.
///
/// **Enumerated, not inferred.** Every entry is a name the provider documents; a name this
/// table does not hold is an ordinary folder.
pub const WELL_KNOWN: &[(&str, SpecialUse)] = &[
    ("inbox", SpecialUse::Inbox),
    ("archive", SpecialUse::Archive),
    ("sentitems", SpecialUse::Sent),
    ("deleteditems", SpecialUse::Trash),
    ("junkemail", SpecialUse::Spam),
    ("drafts", SpecialUse::Drafts),
];

/// The well-known name archiving moves a message to — `ArchiveSemantics::MoveToSpecialUse`.
pub const ARCHIVE: &str = "archive";
/// The well-known name deleting moves a message to — `TrashSemantics::MoveToTrash`.
pub const DELETED_ITEMS: &str = "deleteditems";

/// One folder as the provider describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: String,
    pub display_name: String,
    pub parent: Option<String>,
    /// How many folders sit directly beneath this one. Non-zero means the walk descends.
    pub children: u64,
}

/// The special use a resolved folder identifier carries, if any.
///
/// `resolved` pairs each well-known name with the identifier it answered with; a name that
/// did not resolve is simply absent from it.
#[must_use]
pub fn special_use_of(id: &str, resolved: &[(SpecialUse, String)]) -> Option<SpecialUse> {
    resolved
        .iter()
        .find(|(_, resolved_id)| resolved_id == id)
        .map(|(use_, _)| *use_)
}

/// The folder as the capability model sees it.
#[must_use]
pub fn as_remote(folder: &Folder, resolved: &[(SpecialUse, String)]) -> RemoteFolder {
    RemoteFolder {
        id: RemoteFolderId(folder.id.clone()),
        display_name: folder.display_name.clone(),
        special_use: special_use_of(&folder.id, resolved),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_special_use_fr5_names_has_a_well_known_name() {
        for wanted in [
            SpecialUse::Inbox,
            SpecialUse::Archive,
            SpecialUse::Sent,
            SpecialUse::Trash,
            SpecialUse::Spam,
            SpecialUse::Drafts,
        ] {
            assert!(
                WELL_KNOWN.iter().any(|(_, u)| *u == wanted),
                "{wanted:?} has no well-known name"
            );
        }
    }

    #[test]
    fn a_folder_named_like_a_special_use_is_not_one() {
        // "a locale table is a bug" — and so is reading the English one.
        let folder = Folder {
            id: "user-folder".into(),
            display_name: "Archive".into(),
            parent: None,
            children: 0,
        };
        let resolved = vec![(SpecialUse::Archive, "the-real-archive".to_owned())];
        assert_eq!(as_remote(&folder, &resolved).special_use, None);
    }

    #[test]
    fn a_localized_folder_resolves_through_its_identifier() {
        let folder = Folder {
            id: "AQMk-deleted".into(),
            display_name: "Gelöschte Elemente".into(),
            parent: None,
            children: 0,
        };
        let resolved = vec![(SpecialUse::Trash, "AQMk-deleted".to_owned())];
        assert_eq!(
            as_remote(&folder, &resolved).special_use,
            Some(SpecialUse::Trash)
        );
    }

    #[test]
    fn the_names_the_mutations_move_to_are_in_the_table() {
        assert!(WELL_KNOWN.iter().any(|(n, _)| *n == ARCHIVE));
        assert!(WELL_KNOWN.iter().any(|(n, _)| *n == DELETED_ITEMS));
    }
}
