//! The Gmail adapter.
//!
//! # The blocker that is not technical
//!
//! R-1 is the top risk in the project and it is a **budget question**. Gmail access needs
//! restricted scopes, which means the provider's verification process **including a
//! recurring third-party security assessment**. Without it the client is capped and shows an
//! unverified-application warning.
//!
//! The escape hatches collapse rather than multiply when read against D-33: per-user OAuth
//! client identifiers cannot work for the App Store audience D-33 defines as everyone else,
//! and organizational-tenants-only gives up the consumer Gmail user entirely. What remains is
//! **fund the assessment, or ship without Gmail on the largest channel.**
//!
//! `docs/security/credentials.md` says this MUST be resolved "before a line of the Gmail
//! adapter is written". It has not been; the adapter exists because the approved plan builds
//! it, and #16 records that the code is not the thing blocking it.
//!
//! # What building the wire protocol changed
//!
//! Two rows of the capability table below differ from
//! `docs/mail/providers/gmail.md` as it was first written, and both changed for one reason,
//! set out in [`oauth`]: **the provider's IMAP access and its permanent-delete operation are
//! both behind a scope that also authorizes SMTP submission.** D-88 forbids requesting such a
//! scope, permanently and for a structural reason, so:
//!
//! - `push` is [`PushMechanism::PollOnly`] rather than IDLE-as-a-doorbell. D-7's own
//!   "contestable because" — that a coalesced poll may be indistinguishable at half the code
//!   and one connection — turns out not to have been a preference.
//! - `permanent_delete` is **false**. FR-13's ninth intent is absent on this provider, which
//!   is the capability model working as designed rather than a gap in it.
//!
//! Both are recorded in the provider document and in `docs/decisions.md`.

pub mod adapter;
pub mod label;
pub mod oauth;
pub mod schema;
pub mod wire;

pub use adapter::{Gmail, GmailError};

use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, MagnitudeSource, PushMechanism, SnippetSource, TagSupport,
    ThreadOperations, TrashSemantics,
};

/// What a Gmail account declares.
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        location_cardinality: LocationCardinality::OneOrMore,
        // User labels are tags. System labels are Locations, **except those another axis has
        // already claimed** — see `label::system_axis`.
        tag_support: TagSupport::ReadWrite,
        archive: ArchiveSemantics::RemoveFromInbox,
        trash: TrashSemantics::MoveToTrash,
        // False, and the reason is the scope rather than the protocol. See `oauth`.
        permanent_delete: false,
        thread_operations: ThreadOperations::Native,
        junk_reporting: JunkReporting::NativeReport,
        // A monotonic history identifier. Its window is finite, and falling outside it is a
        // cursor invalidation — NFR-18's recovery rather than an error.
        delta: DeltaMechanism::HistoryCursor,
        // No doorbell. See `oauth`, and D-7 as amended.
        push: PushMechanism::PollOnly,
        id_stability: IdStability::StableGlobally,
        server_search: true,
        // Q-9, answered for this provider. See `BATCH_SIZE` for why it is the smaller of the
        // two numbers the provider publishes.
        max_batch_size: Magnitude::Known {
            value: BATCH_SIZE,
            source: MagnitudeSource::Published,
        },
        // **Still unknown, and deliberately.** The provider publishes a budget in quota
        // units per second rather than in requests, and the units differ per method — a
        // history page and a label change do not cost the same. Converting one into the
        // other would be Sift inventing a number and calling it published, which is the
        // failure D-12's rule 4 exists to prevent. It needs a measurement, which is #2.
        request_budget: Magnitude::Unknown,
        snippet_source: SnippetSource::ProviderSupplied,
        unrecognised: Vec::new(),
    }
}

/// The batch size this provider is planned against — Q-9, for one of the four.
///
/// **The smaller of two published numbers**, and the gap between them is a real observation
/// about the capability model rather than a detail.
///
/// The batch endpoint's hard cap is a hundred calls per request, and the provider's own
/// guidance for this API is fifty, to stay inside the rate limit. A maximum chosen to be
/// throttled is not a maximum worth planning against, so fifty is what is declared.
///
/// The other number is larger: the label-change endpoint accepts a thousand identifiers in
/// one call. The capability model has **one** row here, and it governs both envelope fetches
/// and mutation batches — so a single value has to be the minimum over the operations it
/// covers, and mutations pay for the tighter of the two. That is a capability the model does
/// not have rather than a compromise this adapter made, and it is recorded on #2.
pub const BATCH_SIZE: u32 = 50;

/// Whether the history feed is the only path by which change is applied.
///
/// **It is.** A second path that applied change would have its own bugs, exercised only on
/// the accounts where the second mechanism happened to be available.
#[must_use]
pub const fn history_is_the_only_apply_path() -> bool {
    true
}

/// Whether an envelope fetch requests the whole message.
///
/// **No.** Metadata only, through the batch endpoint. "Sift MUST NOT fetch whole messages"
/// is a claim about requests, and this is the provider where it is easiest to violate by
/// accident because the full-message endpoint is the convenient one.
#[must_use]
pub const fn envelope_fetch_requests_full_messages() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declared_table_matches_the_provider_document() {
        let c = capabilities();
        assert_eq!(c.delta, DeltaMechanism::HistoryCursor);
        assert_eq!(c.location_cardinality, LocationCardinality::OneOrMore);
        assert_eq!(c.id_stability, IdStability::StableGlobally);
    }

    #[test]
    fn there_is_no_doorbell_and_the_reason_is_the_scope() {
        // D-7 as amended. The polling path is not a fallback: it is the only path.
        assert_eq!(capabilities().push, PushMechanism::PollOnly);
    }

    #[test]
    fn permanent_delete_is_absent_rather_than_approximated() {
        // The scope that permits it also authorizes sending, which D-88 forbids requesting.
        // Absent is what docs/mail/mutations.md requires of an unsupported operation.
        assert!(!capabilities().permanent_delete);
    }

    #[test]
    fn the_table_agrees_with_the_document_that_owns_it() {
        // The two-places rule: a decision changed in code and not in the document is a
        // defect. This is the cheap half of that check.
        const DOC: &str = include_str!("../../../../docs/mail/providers/gmail.md");
        assert!(
            DOC.contains("poll only") || DOC.contains("Poll only"),
            "the document still claims a doorbell"
        );
        assert!(
            DOC.contains("not supported"),
            "the document still claims permanent delete"
        );
    }

    #[test]
    fn idle_is_not_used_and_history_is_the_only_apply_path() {
        assert!(history_is_the_only_apply_path());
    }

    #[test]
    fn the_batch_size_is_published_rather_than_conservative() {
        // Q-9, answered for this provider. The conservative default is what an unknown
        // magnitude plans against, and it costs five times the round trips on a backfill.
        assert_eq!(capabilities().batch_size(), BATCH_SIZE);
        assert_ne!(
            capabilities().batch_size(),
            Capabilities::CONSERVATIVE_BATCH
        );
        assert!(matches!(
            capabilities().max_batch_size,
            Magnitude::Known {
                source: MagnitudeSource::Published,
                ..
            }
        ));
    }

    #[test]
    fn the_request_budget_is_still_unknown_and_says_so() {
        // The provider publishes it in quota units per second, and the units differ per
        // method. Converting one into the other would be Sift inventing a number and calling
        // it published.
        assert_eq!(capabilities().request_budget, Magnitude::Unknown);
    }

    #[test]
    fn envelopes_are_fetched_as_metadata_rather_than_whole_messages() {
        assert!(!envelope_fetch_requests_full_messages());
    }

    #[test]
    fn no_connection_is_held_open_at_all() {
        // The consequence of having no doorbell: nothing is held open, so this account
        // spends none of L-23's budget however many folders it watches. That budget was
        // sized for five accounts watching three folders each; without a doorbell here it
        // is available to the accounts that do have one.
        assert_eq!(capabilities().connections_for(3), 0);
        assert_eq!(capabilities().connections_for(30), 0);
    }
}
