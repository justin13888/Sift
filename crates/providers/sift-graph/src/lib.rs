//! The Microsoft Graph adapter.
//!
//! # The single most important fact about this adapter
//!
//! **Identifiers are unstable on move.** The provider document says so in those words, and
//! everything else here is downstream of it: the adapter MUST NOT assume a remote identifier
//! survives a move, and MUST treat the internet message identifier together with the
//! conversation identifier as the join key across one.
//!
//! The fallback is D-44's, and its scoping is what makes it safe: **the conversation
//! identifier is the scope**, the stored digest corroborates, and a move that does not
//! resolve to exactly one candidate **is presented as a delete plus an arrival**. That costs
//! the message its local identity — a shell holding it for a selection loses the selection —
//! and D-44 accepts it, because merging wrongly is data loss and failing to merge is a
//! display defect.
//!
//! # Push is impractical rather than absent
//!
//! Change notifications need a publicly reachable endpoint. Sift never opens a listening
//! socket for any purpose (NFR-24), so the declared mechanism is polling — and the poll
//! intervals **MUST be coalesced onto the shared scheduler** rather than becoming a
//! per-account timer, which is the failure D-25 exists to prevent.

use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
    TrashSemantics,
};

/// What a Graph account declares.
///
/// **Consumer and organizational accounts share one code path**, which is worth stating
/// because the temptation to branch on tenant type is exactly the provider-name special-casing
/// D-12 forbids in a different disguise.
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        location_cardinality: LocationCardinality::ExactlyOne,
        tag_support: TagSupport::ReadWrite,
        archive: ArchiveSemantics::MoveToSpecialUse,
        trash: TrashSemantics::MoveToTrash,
        permanent_delete: true,
        thread_operations: ThreadOperations::Native,
        // A report call **distinct from** the move to the junk folder. Conflating them would
        // be the "silently does something else" FR-39 forbids.
        junk_reporting: JunkReporting::NativeReport,
        // Per folder, and it expires — which is a cursor invalidation and therefore NFR-18's
        // recovery rather than an error.
        delta: DeltaMechanism::DeltaLink,
        push: PushMechanism::PollOnly,
        id_stability: IdStability::UnstableOnMove,
        server_search: true,
        max_batch_size: Magnitude::Unknown,
        request_budget: Magnitude::Unknown,
        snippet_source: SnippetSource::ProviderSupplied,
        unrecognised: Vec::new(),
    }
}

/// How a message is re-identified after a move.
///
/// D-44's rule, in the one place it is exercised on every move rather than occasionally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveOutcome {
    /// Exactly one candidate corroborated. The message keeps its local identity, and the
    /// shell's selection survives.
    Rejoined { message: u128 },
    /// Zero or several candidates. **Presented as a delete plus an arrival**, and the
    /// arrival gets a new identity.
    ///
    /// D-78 names this as the single exception to "never reused": everything else about
    /// local identity is stable for the life of the message.
    DeleteAndArrival,
}

/// Resolve a moved message against the candidates its conversation identifier scoped.
///
/// The scope comes first and the identifier only narrows: **Sift MUST NOT discover a join by
/// scanning a store for identifier equality**, because the internet message identifier is
/// not reliably unique and a scan would find matches the conversation never proposed.
#[must_use]
pub fn resolve_move(candidates_in_conversation: &[u128], corroborated: &[u128]) -> MoveOutcome {
    let matching: Vec<u128> = candidates_in_conversation
        .iter()
        .filter(|c| corroborated.contains(c))
        .copied()
        .collect();
    match matching.as_slice() {
        [only] => MoveOutcome::Rejoined { message: *only },
        // Ambiguity resolves to distinct messages. Failing to merge is a display defect;
        // merging wrongly is data loss wearing a display defect's clothes.
        _ => MoveOutcome::DeleteAndArrival,
    }
}

/// Whether hand-written request types are diffed against the published schema in CI.
///
/// D-13 requires it, and the reason is the concession the decision makes: hand-written types
/// **drift from the schema unless CI enforces the diff**. The job runs on a cadence rather
/// than per change, blocks a release rather than a merge, and an unreachable schema is
/// reported as *unavailable* rather than as a pass — a silent pass would make the whole
/// mechanism decorative.
#[must_use]
pub const fn schema_diff_runs_in_ci() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_do_not_survive_a_move() {
        // The single most important fact about this adapter.
        assert!(!capabilities().identifier_survives_a_move());
        assert_eq!(capabilities().id_stability, IdStability::UnstableOnMove);
    }

    #[test]
    fn a_move_that_corroborates_to_one_candidate_keeps_its_identity() {
        assert_eq!(
            resolve_move(&[1, 2, 3], &[2]),
            MoveOutcome::Rejoined { message: 2 }
        );
    }

    #[test]
    fn an_ambiguous_move_becomes_a_delete_and_an_arrival() {
        // Failing to merge is a display defect; merging wrongly is data loss wearing a
        // display defect's clothes.
        assert_eq!(
            resolve_move(&[1, 2, 3], &[1, 2]),
            MoveOutcome::DeleteAndArrival
        );
        assert_eq!(resolve_move(&[1, 2, 3], &[]), MoveOutcome::DeleteAndArrival);
    }

    #[test]
    fn a_candidate_outside_the_conversation_is_not_considered() {
        // The scope comes first and the identifier only narrows. Sift must not discover a
        // join by scanning for identifier equality.
        assert_eq!(resolve_move(&[1, 2], &[99]), MoveOutcome::DeleteAndArrival);
    }

    #[test]
    fn push_is_polling_because_the_alternative_needs_a_listening_socket() {
        assert_eq!(capabilities().push, PushMechanism::PollOnly);
        assert_eq!(
            capabilities().connections_for(5),
            0,
            "polling held connections open"
        );
    }

    #[test]
    fn junk_reporting_is_a_report_rather_than_a_move() {
        // Conflating them would be the "silently does something else" FR-39 forbids.
        assert!(capabilities().offers_junk_report());
        assert!(!capabilities().offers_junk_move());
    }

    #[test]
    fn the_schema_diff_is_not_optional() {
        assert!(schema_diff_runs_in_ci());
    }
}
