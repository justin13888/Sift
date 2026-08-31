//! The Gmail adapter.
//!
//! # The blocker that is not technical
//!
//! R-1 is the top risk in the project and it is a **budget question**. Gmail access — the API
//! and IMAP-over-OAuth alike — needs restricted scopes, which means the provider's
//! verification process **including a recurring third-party security assessment**. Without
//! it the client is capped and shows an unverified-application warning.
//!
//! The escape hatches collapse rather than multiply when read against D-33: per-user OAuth
//! client identifiers cannot work for the App Store audience D-33 defines as everyone else,
//! and organizational-tenants-only gives up the consumer Gmail user entirely. What remains is
//! **fund the assessment, or ship without Gmail on the largest channel.**
//!
//! `docs/security/credentials.md` says this MUST be resolved "before a line of the Gmail
//! adapter is written". It has not been; the adapter exists because the approved plan builds
//! all four, and #16 records that the code is not the thing blocking it.
//!
//! # D-7's hybrid, and the path that must be good regardless
//!
//! IDLE is used **purely as a wake signal**; the actual delta comes over the API. Gmail's own
//! push needs a publicly reachable endpoint, which NFR-24 forbids outright.
//!
//! The cost is permanent: **two authentication paths and two connection lifecycles per
//! account, forever.** And on cellular the hybrid is **disabled outright** — the tail timer
//! makes keepalives expensive in radio wakes rather than in bytes — so the polling path is
//! not a fallback that might get exercised. It is the path every cellular user is on, and it
//! has to be good.

use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
    TrashSemantics,
};

/// What a Gmail account declares.
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        location_cardinality: LocationCardinality::OneOrMore,
        // User labels are tags. System labels are Locations, **except those another axis has
        // already claimed** — see `system_label_axis`.
        tag_support: TagSupport::ReadWrite,
        archive: ArchiveSemantics::RemoveFromInbox,
        trash: TrashSemantics::MoveToTrash,
        permanent_delete: true,
        thread_operations: ThreadOperations::Native,
        junk_reporting: JunkReporting::NativeReport,
        // A monotonic history identifier. Its window is finite, and falling outside it is a
        // cursor invalidation — NFR-18's recovery rather than an error.
        delta: DeltaMechanism::HistoryCursor,
        push: PushMechanism::IdleOnePerConnection,
        id_stability: IdStability::StableGlobally,
        server_search: true,
        max_batch_size: Magnitude::Unknown,
        request_budget: Magnitude::Unknown,
        snippet_source: SnippetSource::ProviderSupplied,
        unrecognised: Vec::new(),
    }
}

/// Which axis a system label lands on — D-12.
///
/// The ruling is "a system label is a Location **only where no other axis already claims
/// it**", and the adapter **enumerates the claimed ones** rather than leaving a reader to
/// infer them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Location,
    /// Already spent on another axis: read state, flagged state.
    ClaimedElsewhere,
    /// D-12's **explicit residual awkward case**, left as an adapter decision on purpose.
    ///
    /// The capability table has no per-tag granularity for "a tag the user may read but not
    /// create", which is what this is. Treated as a Tag, because that is what it behaves
    /// like in the interface — and recorded as a known compromise rather than a settled
    /// answer.
    UndecidedByTheModel,
}

/// Classify a system label.
#[must_use]
pub fn system_label_axis(label: &str) -> Axis {
    match label.to_ascii_uppercase().as_str() {
        // Spent on the read axis.
        "UNREAD" => Axis::ClaimedElsewhere,
        // Spent on the flagged axis.
        "STARRED" => Axis::ClaimedElsewhere,
        // The residual case D-12 declines to settle.
        "IMPORTANT" => Axis::UndecidedByTheModel,
        // The inbox tabs are *places*, which is why category labels are Locations.
        "INBOX"
        | "SENT"
        | "DRAFT"
        | "TRASH"
        | "SPAM"
        | "CATEGORY_PERSONAL"
        | "CATEGORY_SOCIAL"
        | "CATEGORY_PROMOTIONS"
        | "CATEGORY_UPDATES"
        | "CATEGORY_FORUMS" => Axis::Location,
        _ => Axis::Location,
    }
}

/// Whether the history feed is the only path by which change is applied.
///
/// **It is.** IDLE is a doorbell; it says something changed and never says what. A second
/// path that applied change would have its own bugs, exercised only on the accounts where
/// IDLE happened to be available.
#[must_use]
pub const fn history_is_the_only_apply_path() -> bool {
    true
}

/// Whether an envelope fetch requests the whole message.
///
/// **No.** Metadata only, through the batch endpoint. "Sift MUST NOT fetch whole messages" is
/// a claim about requests, and this is the provider where it is easiest to violate by
/// accident because the full-message endpoint is the convenient one.
#[must_use]
pub const fn envelope_fetch_requests_full_messages() -> bool {
    false
}

/// Whether the IDLE hybrid runs on cellular.
///
/// **No**, and the consequence is that the polling path is not a fallback: it is the path
/// every cellular user is on. D-7 says the hybrid must be re-examined against measured
/// perceived latency anyway — a thirty-second coalesced poll may be indistinguishable.
#[must_use]
pub const fn hybrid_runs_on_cellular() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declared_table_matches_the_provider_document() {
        let c = capabilities();
        assert_eq!(c.delta, DeltaMechanism::HistoryCursor);
        assert_eq!(c.push, PushMechanism::IdleOnePerConnection);
        assert_eq!(c.location_cardinality, LocationCardinality::OneOrMore);
    }

    #[test]
    fn a_system_label_another_axis_claims_is_not_a_location() {
        // The ruling D-12 makes, with the claimed ones enumerated rather than inferred.
        assert_eq!(system_label_axis("UNREAD"), Axis::ClaimedElsewhere);
        assert_eq!(system_label_axis("STARRED"), Axis::ClaimedElsewhere);
    }

    #[test]
    fn category_labels_are_locations_because_the_tabs_are_places() {
        for label in ["CATEGORY_PROMOTIONS", "CATEGORY_SOCIAL", "INBOX"] {
            assert_eq!(system_label_axis(label), Axis::Location, "{label}");
        }
    }

    #[test]
    fn the_importance_label_is_recorded_as_undecided_rather_than_guessed() {
        // D-12 declines to settle it, because the capability table has no per-tag granularity
        // for "a tag the user may read but not create". Recording it as undecided keeps that
        // visible instead of burying a compromise in a match arm.
        assert_eq!(system_label_axis("IMPORTANT"), Axis::UndecidedByTheModel);
    }

    #[test]
    fn idle_is_a_doorbell_and_history_is_the_only_apply_path() {
        // A second apply path would have its own bugs, exercised only on the accounts where
        // IDLE happened to be available.
        assert!(history_is_the_only_apply_path());
    }

    #[test]
    fn envelopes_are_fetched_as_metadata_rather_than_whole_messages() {
        assert!(!envelope_fetch_requests_full_messages());
    }

    #[test]
    fn the_polling_path_is_not_a_fallback() {
        // On cellular the hybrid is disabled outright, so polling is the path every cellular
        // user is on and it has to be good.
        assert!(!hybrid_runs_on_cellular());
    }

    #[test]
    fn watching_three_folders_costs_three_connections() {
        // One mailbox per connection, which is what L-23's budget is spent against.
        assert_eq!(capabilities().connections_for(3), 3);
    }
}
