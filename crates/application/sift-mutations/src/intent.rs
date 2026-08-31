//! FR-13 — mutations are intents, never wire operations.
//!
//! `docs/mail/mutations.md` asks for this to be budgeted as a first-class subsystem rather
//! than a thin adapter method, and gives the reason plainly: **it is the only place in Sift
//! where a bug can lose a user's mail.**

use sift_foundation::identity::LocalId;
use sift_provider::capability::{Capabilities, JunkReporting, TagSupport};

/// The closed set of things a user may do to a message.
///
/// Closed on purpose. Growing it requires three things in order — a capability that gates
/// it, a declared compensation and an answer about whether it gets a timed window, and a
/// queue schema-version step — **and a scope amendment**, because widening this set is a
/// scope change and `docs/product/scope.md` has to say so first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Archive,
    DeleteToTrash,
    /// **The single exception to FR-14.** Confirmed before it is issued rather than undone
    /// after, never applied optimistically ahead of the server, and deliberately not
    /// offered in bulk over search results — a wrong query and a wrong click composing into
    /// unrecoverable loss is the failure that exclusion exists to prevent.
    PermanentlyDelete,
    MoveTo {
        folder: i64,
    },
    MarkRead,
    MarkUnread,
    Flag,
    Unflag,
    AddTag {
        name: String,
    },
    RemoveTag {
        name: String,
    },
    /// D-40. Gated on the junk-reporting capability, and **the one mutation whose effect
    /// Sift cannot verify.**
    ReportJunk,
    ReportNotJunk,
}

impl Intent {
    /// The compensating intent that reverses this one — FR-15.
    ///
    /// Reversal is executed **as the compensation, never as a queue retraction**. A
    /// retraction would be a lie about a request that may already have been sent.
    ///
    /// `None` for permanent delete, which is the only intent with no compensation and
    /// therefore the only one confirmed in advance.
    #[must_use]
    pub fn compensation(&self, restore_to: Option<i64>) -> Option<Self> {
        Some(match self {
            Self::Archive | Self::DeleteToTrash | Self::MoveTo { .. } => Self::MoveTo {
                folder: restore_to?,
            },
            Self::PermanentlyDelete => return None,
            Self::MarkRead => Self::MarkUnread,
            Self::MarkUnread => Self::MarkRead,
            Self::Flag => Self::Unflag,
            Self::Unflag => Self::Flag,
            Self::AddTag { name } => Self::RemoveTag { name: name.clone() },
            Self::RemoveTag { name } => Self::AddTag { name: name.clone() },
            // Junk and not-junk compensate each other, which is why D-40 needed no new undo
            // machinery. **Undo cannot un-train the classifier**, and MUST NOT be described
            // as promising it.
            Self::ReportJunk => Self::ReportNotJunk,
            Self::ReportNotJunk => Self::ReportJunk,
        })
    }

    /// Whether this intent removes the message from the view the user is looking at, and
    /// therefore gets FR-15's timed undo window of L-22.
    ///
    /// Mark-read, flag and tag get no window: the message stays in front of the user, so
    /// the ordinary reversibility every intent has is enough.
    #[must_use]
    pub const fn removes_from_view(&self) -> bool {
        matches!(
            self,
            Self::Archive | Self::DeleteToTrash | Self::MoveTo { .. } | Self::ReportJunk
        )
    }

    /// Whether FR-14 applies. False for exactly one intent.
    #[must_use]
    pub const fn applies_optimistically(&self) -> bool {
        !matches!(self, Self::PermanentlyDelete)
    }

    /// Whether the user must confirm before it is issued.
    #[must_use]
    pub const fn requires_confirmation(&self) -> bool {
        matches!(self, Self::PermanentlyDelete)
    }

    /// Whether an account declaring `caps` can carry this intent at all.
    ///
    /// **An intent whose capability is absent is not offered**, rather than offered and
    /// failing — FR-39's "absent, not approximated", generalised.
    #[must_use]
    pub fn permitted_by(&self, caps: &Capabilities) -> bool {
        match self {
            Self::AddTag { .. } | Self::RemoveTag { .. } => {
                matches!(caps.tag_support, TagSupport::ReadWrite)
            }
            Self::ReportJunk | Self::ReportNotJunk => {
                !matches!(caps.junk_reporting, JunkReporting::None)
            }
            Self::PermanentlyDelete => caps.permanent_delete,
            _ => true,
        }
    }

    /// A stable name for the queue's serialised form and for the debug view.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::DeleteToTrash => "delete-to-trash",
            Self::PermanentlyDelete => "permanently-delete",
            Self::MoveTo { .. } => "move-to",
            Self::MarkRead => "mark-read",
            Self::MarkUnread => "mark-unread",
            Self::Flag => "flag",
            Self::Unflag => "unflag",
            Self::AddTag { .. } => "add-tag",
            Self::RemoveTag { .. } => "remove-tag",
            Self::ReportJunk => "report-junk",
            Self::ReportNotJunk => "report-not-junk",
        }
    }
}

/// D-85's six states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Durably enqueued, overlay applied, not yet issued.
    Pending,
    /// **A request carrying this intent has been sent.**
    ///
    /// The transition into this state is durable and happens *before the request leaves*.
    /// That costs one durable write per issue, forever, and it buys correctness for a crash
    /// window milliseconds wide — D-85 records that trade as its own weakest point. What it
    /// prevents is inferring sent-versus-unsent at startup, which cannot be done correctly.
    Issued,
    /// The outcome is unknown and the adapter is establishing server state.
    ///
    /// Restart moves every `Issued` intent here, because a request that went out and whose
    /// answer never came back is exactly this situation.
    Reconciling,
    /// Unrecognised by this build, or its gating capability has gone away.
    ///
    /// **Never executed and never discarded.** Held, shown in the queue under FR-34, and
    /// executable by a later build or a restored capability.
    Quarantined,
    /// Retried past L-17 without success. Terminal until the user acts.
    Expired,
    /// Applied, compensated, or reconciled away. The row is removed.
    Settled,
}

impl State {
    /// Whether this intent still contributes a pending overlay.
    ///
    /// An expired or quarantined intent **retires its overlay**, so the base state shows
    /// through and the user is told which intent stopped rather than seeing a local state
    /// nothing will ever make true.
    #[must_use]
    pub const fn contributes_overlay(self) -> bool {
        matches!(self, Self::Pending | Self::Issued | Self::Reconciling)
    }

    /// Whether the user has to do something about it.
    #[must_use]
    pub const fn needs_attention(self) -> bool {
        matches!(self, Self::Quarantined | Self::Expired)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Issued => "Issued",
            Self::Reconciling => "Reconciling",
            Self::Quarantined => "Quarantined",
            Self::Expired => "Expired",
            Self::Settled => "Settled",
        }
    }
}

/// A queued intent.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    /// Client-assigned, and the value to send where a provider accepts an idempotency key.
    pub id: u128,
    /// Assigned **at the gesture**, which is what makes FR-17's bulk operation one undoable
    /// unit and what a compensation acts over.
    pub undo_group: Option<u128>,
    pub message: LocalId,
    pub intent: Intent,
    pub state: State,
    pub attempts: u32,
    pub created_millis: u64,
    /// Intents against **one** message apply in the order they were issued. Always,
    /// including through batching and retry.
    pub sequence: u64,
}

impl Queued {
    /// Whether L-17 has run out.
    ///
    /// D-85 expires on **elapsed time rather than attempts**, because an intent that failed
    /// twice in a week offline and one that failed two hundred times in a minute are not
    /// the same situation. Attempt count drives backoff and terminates nothing.
    #[must_use]
    pub fn is_expired(&self, now_millis: u64) -> bool {
        let age = now_millis.saturating_sub(self.created_millis);
        age >= sift_foundation::limits::L17_INTENT_EXPIRY.as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_provider::capability::{
        ArchiveSemantics, DeltaMechanism, IdStability, JunkReporting, LocationCardinality,
        Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations, TrashSemantics,
    };

    fn caps(tag_support: TagSupport, junk: JunkReporting) -> Capabilities {
        Capabilities {
            location_cardinality: LocationCardinality::ExactlyOne,
            tag_support,
            archive: ArchiveSemantics::MoveToSpecialUse,
            trash: TrashSemantics::MoveToTrash,
            permanent_delete: true,
            thread_operations: ThreadOperations::ClientFanOut,
            junk_reporting: junk,
            delta: DeltaMechanism::FullScan,
            push: PushMechanism::PollOnly,
            id_stability: IdStability::StablePerFolder,
            server_search: true,
            max_batch_size: Magnitude::Unknown,
            request_budget: Magnitude::Unknown,
            snippet_source: SnippetSource::ClientDerived,
            unrecognised: Vec::new(),
        }
    }

    /// Every intent, so that a new one cannot be added without meeting the rules below.
    fn all() -> Vec<Intent> {
        vec![
            Intent::Archive,
            Intent::DeleteToTrash,
            Intent::PermanentlyDelete,
            Intent::MoveTo { folder: 1 },
            Intent::MarkRead,
            Intent::MarkUnread,
            Intent::Flag,
            Intent::Unflag,
            Intent::AddTag { name: "x".into() },
            Intent::RemoveTag { name: "x".into() },
            Intent::ReportJunk,
            Intent::ReportNotJunk,
        ]
    }

    #[test]
    fn every_intent_but_one_has_a_compensation() {
        // FR-15: "every intent except permanent delete has a compensating intent and MUST be
        // reversible through it".
        for i in all() {
            let has = i.compensation(Some(1)).is_some();
            if matches!(i, Intent::PermanentlyDelete) {
                assert!(!has, "permanent delete claimed a compensation");
            } else {
                assert!(has, "{} has no compensation", i.name());
            }
        }
    }

    #[test]
    fn permanent_delete_is_the_single_exception_to_optimistic_application() {
        for i in all() {
            let optimistic = i.applies_optimistically();
            let confirmed = i.requires_confirmation();
            assert_ne!(optimistic, confirmed, "{} is both or neither", i.name());
            if matches!(i, Intent::PermanentlyDelete) {
                assert!(!optimistic && confirmed);
            }
        }
    }

    #[test]
    fn only_intents_that_remove_a_message_from_view_get_a_timed_window() {
        // FR-15's second half. Mark-read, flag and tag leave the message in front of the
        // user, so ordinary reversibility is enough and a countdown would be noise.
        for i in [
            Intent::Archive,
            Intent::DeleteToTrash,
            Intent::MoveTo { folder: 1 },
            Intent::ReportJunk,
        ] {
            assert!(i.removes_from_view(), "{} should get a window", i.name());
        }
        for i in [
            Intent::MarkRead,
            Intent::Flag,
            Intent::AddTag { name: "x".into() },
        ] {
            assert!(
                !i.removes_from_view(),
                "{} should not get a window",
                i.name()
            );
        }
    }

    #[test]
    fn junk_and_not_junk_compensate_each_other() {
        // Which is why D-40 needed no new undo machinery — and why undo must not be
        // described as un-training the classifier, because it cannot.
        assert_eq!(
            Intent::ReportJunk.compensation(None),
            Some(Intent::ReportNotJunk)
        );
        assert_eq!(
            Intent::ReportNotJunk.compensation(None),
            Some(Intent::ReportJunk)
        );
    }

    #[test]
    fn an_intent_whose_capability_is_absent_is_not_offered() {
        // FR-39's "absent, not approximated", generalised to every gated intent.
        let no_tags = caps(TagSupport::None, JunkReporting::NativeReport);
        assert!(!Intent::AddTag { name: "x".into() }.permitted_by(&no_tags));

        let no_junk = caps(TagSupport::ReadWrite, JunkReporting::None);
        assert!(!Intent::ReportJunk.permitted_by(&no_junk));
        assert!(!Intent::ReportNotJunk.permitted_by(&no_junk));

        let full = caps(TagSupport::ReadWrite, JunkReporting::NativeReport);
        for i in all() {
            assert!(
                i.permitted_by(&full),
                "{} refused by a fully capable account",
                i.name()
            );
        }
    }

    #[test]
    fn a_move_only_account_still_permits_the_junk_intent() {
        // The intent exists; what changes is how it is *presented* — a move labelled as a
        // move rather than a report affordance that silently does something else.
        let move_only = caps(TagSupport::ReadWrite, JunkReporting::FolderMoveOnly);
        assert!(Intent::ReportJunk.permitted_by(&move_only));
    }

    #[test]
    fn a_compensation_that_needs_a_destination_refuses_without_one() {
        // "A compensation restores local state, never a remote side effect" — and it cannot
        // restore a location it was not told.
        assert_eq!(Intent::Archive.compensation(None), None);
        assert_eq!(
            Intent::Archive.compensation(Some(3)),
            Some(Intent::MoveTo { folder: 3 })
        );
    }

    #[test]
    fn only_states_that_are_still_in_play_contribute_an_overlay() {
        assert!(State::Pending.contributes_overlay());
        assert!(State::Issued.contributes_overlay());
        assert!(State::Reconciling.contributes_overlay());
        assert!(!State::Quarantined.contributes_overlay());
        assert!(!State::Expired.contributes_overlay());
        assert!(!State::Settled.contributes_overlay());
    }

    #[test]
    fn the_intent_set_is_exactly_the_action_registers_message_actions() {
        // D-98: "these are exactly FR-13's intent set; the action set MUST NOT contain a
        // mutation that is not an intent." Twelve here, because FR-13's nine count
        // read/unread, flag/unflag and add/remove tag as one row each.
        assert_eq!(all().len(), 12);
    }
}
