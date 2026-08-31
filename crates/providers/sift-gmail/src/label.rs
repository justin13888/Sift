//! Labels, and the three axes D-12 makes them land on.
//!
//! Gmail has one mechanism — a label — where the capability model has three: a **location**,
//! a **tag**, and a per-message state another axis already owns. D-12 rules that "a system
//! label is a Location **only where no other axis already claims it**", and this module
//! enumerates the claimed ones rather than leaving a reader to infer them.
//!
//! FR-5's special use resolves through the provider's own identifiers, which are stable and
//! locale-independent. It is never resolved by matching a display name: those are translated
//! per account, so "a locale table is a bug" — and worse, a *user* label may be named
//! `Trash` in any language the user likes.

use sift_provider::adapter::{RemoteFolder, RemoteFolderId, SpecialUse};

/// Which axis a label lands on — D-12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// A place a message is in. Folders, in the capability model's terms.
    Location,
    /// A user-applied label, which is what [`TagSupport`](sift_provider::capability::TagSupport)
    /// describes.
    Tag,
    /// Already spent on another axis: read state, flagged state.
    ClaimedElsewhere,
    /// D-12's **explicit residual awkward case**, left as an adapter decision on purpose.
    ///
    /// The capability table has no per-tag granularity for "a tag the user may read but not
    /// create", which is what this is. Treated as a Tag at the interface, because that is
    /// what it behaves like — and recorded as a known compromise rather than a settled
    /// answer.
    UndecidedByTheModel,
}

/// One label as the provider describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub id: String,
    pub name: String,
    /// Whether the provider calls this one of its own.
    pub system: bool,
}

impl Label {
    #[must_use]
    pub fn axis(&self) -> Axis {
        if self.system {
            system_axis(&self.id)
        } else {
            Axis::Tag
        }
    }

    /// The folder this label is, where it is one.
    #[must_use]
    pub fn as_folder(&self) -> Option<RemoteFolder> {
        matches!(self.axis(), Axis::Location).then(|| RemoteFolder {
            id: RemoteFolderId(self.id.clone()),
            display_name: self.name.clone(),
            special_use: special_use(&self.id),
        })
    }
}

/// The system labels that are places. **Enumerated, not inferred.**
///
/// The tempting shortcut is to read the identifier's shape — the provider's own user labels
/// happen to share a prefix — but that is a naming convention rather than a guarantee, and a
/// convention that changed would silently turn every user's tags into folders. So the
/// closed set is written down, and anything outside it is a tag.
pub const SYSTEM_LOCATIONS: &[&str] = &[
    "INBOX",
    "SENT",
    "DRAFT",
    "TRASH",
    "SPAM",
    "CHAT",
    "CATEGORY_PERSONAL",
    "CATEGORY_SOCIAL",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_UPDATES",
    "CATEGORY_FORUMS",
];

/// Whether a label identifier names one of the provider's own places.
#[must_use]
pub fn is_system_location(id: &str) -> bool {
    SYSTEM_LOCATIONS.contains(&id.to_ascii_uppercase().as_str())
}

/// Classify a system label.
#[must_use]
pub fn system_axis(id: &str) -> Axis {
    match id.to_ascii_uppercase().as_str() {
        // Spent on the read axis.
        "UNREAD" => Axis::ClaimedElsewhere,
        // Spent on the flagged axis.
        "STARRED" => Axis::ClaimedElsewhere,
        // The residual case D-12 declines to settle.
        "IMPORTANT" => Axis::UndecidedByTheModel,
        // The inbox tabs are *places*, which is why category labels are Locations.
        other if is_system_location(other) => Axis::Location,
        // A system label this adapter has never heard of. Treated as a place, because that
        // is the axis where being wrong is visible — a stray folder in the sidebar — rather
        // than silent, which is what a stray tag would be.
        _ => Axis::Location,
    }
}

/// FR-5's semantic, resolved through the provider's own identifier.
///
/// **There is no archive folder, and its absence is not an omission.** Archiving here means
/// removing the inbox label, which is what `ArchiveSemantics::RemoveFromInbox` declares —
/// so a caller looking for an archive folder correctly finds none, and plans the archive
/// against the capability instead.
#[must_use]
pub fn special_use(id: &str) -> Option<SpecialUse> {
    Some(match id.to_ascii_uppercase().as_str() {
        "INBOX" => SpecialUse::Inbox,
        "SENT" => SpecialUse::Sent,
        "TRASH" => SpecialUse::Trash,
        "SPAM" => SpecialUse::Spam,
        "DRAFT" => SpecialUse::Drafts,
        _ => return None,
    })
}

/// The label that carries a message's read state — its **presence** means unread.
pub const UNREAD: &str = "UNREAD";
/// The label that carries a message's flagged state.
pub const STARRED: &str = "STARRED";
pub const INBOX: &str = "INBOX";
pub const TRASH: &str = "TRASH";
pub const SPAM: &str = "SPAM";

#[cfg(test)]
mod tests {
    use super::*;

    fn system(id: &str) -> Label {
        Label {
            id: id.to_owned(),
            name: id.to_owned(),
            system: true,
        }
    }

    #[test]
    fn a_system_label_another_axis_claims_is_not_a_location() {
        assert_eq!(system("UNREAD").axis(), Axis::ClaimedElsewhere);
        assert_eq!(system("STARRED").axis(), Axis::ClaimedElsewhere);
        assert!(system("UNREAD").as_folder().is_none());
    }

    #[test]
    fn category_labels_are_locations_because_the_tabs_are_places() {
        for id in ["CATEGORY_PROMOTIONS", "CATEGORY_SOCIAL", "INBOX"] {
            assert_eq!(system(id).axis(), Axis::Location, "{id}");
        }
    }

    #[test]
    fn the_importance_label_is_recorded_as_undecided_rather_than_guessed() {
        // D-12 declines to settle it. Recording it keeps the compromise visible instead of
        // burying it in a match arm.
        assert_eq!(system("IMPORTANT").axis(), Axis::UndecidedByTheModel);
    }

    #[test]
    fn a_user_label_is_a_tag_whatever_it_is_called() {
        // The reason special use never resolves on a name: a user may name a label `TRASH`.
        let user = Label {
            id: "Label_12".into(),
            name: "TRASH".into(),
            system: false,
        };
        assert_eq!(user.axis(), Axis::Tag);
        assert!(user.as_folder().is_none());
    }

    #[test]
    fn special_use_resolves_on_the_identifier_and_never_on_the_name() {
        let folder = Label {
            id: "TRASH".into(),
            // A display name in another language, which is the ordinary case.
            name: "Corbeille".into(),
            system: true,
        }
        .as_folder()
        .unwrap();
        assert_eq!(folder.special_use, Some(SpecialUse::Trash));
        assert_eq!(folder.display_name, "Corbeille");
    }

    #[test]
    fn there_is_no_archive_folder_and_that_is_the_capability_talking() {
        // Archiving is removing the inbox label. A caller finds no archive folder and plans
        // against ArchiveSemantics::RemoveFromInbox instead.
        assert_eq!(special_use("ARCHIVE"), None);
        assert_eq!(
            crate::capabilities().archive,
            sift_provider::capability::ArchiveSemantics::RemoveFromInbox
        );
    }

    #[test]
    fn every_location_label_that_names_a_semantic_resolves_to_one() {
        for (id, expect) in [
            ("INBOX", SpecialUse::Inbox),
            ("SENT", SpecialUse::Sent),
            ("TRASH", SpecialUse::Trash),
            ("SPAM", SpecialUse::Spam),
            ("DRAFT", SpecialUse::Drafts),
        ] {
            assert_eq!(special_use(id), Some(expect), "{id}");
        }
    }
}
