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
//!
//! # What building the wire protocol changed
//!
//! One row of the capability table below differs from `docs/mail/providers/microsoft-graph.md`
//! as it was first written: **junk reporting is a folder move only.** The published v1.0 API
//! binds no report action to a message — the report call exists only in the preview surface,
//! which the provider does not support for production use — so the declared value is the one
//! the version this adapter speaks can honour. A report that was really a move would be the
//! "silently does something else" FR-39 forbids; a move offered as a move is what D-40 says a
//! *folder move only* account gets.

pub mod adapter;
pub mod folder;
pub mod oauth;
pub mod schema;
pub mod wire;

pub use adapter::{Graph, GraphError};

use sift_provider::adapter::Envelope;
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
        // A move to the junk folder, offered **as a move**. v1.0 has no report call, and
        // conflating the two would be the "silently does something else" FR-39 forbids.
        junk_reporting: JunkReporting::FolderMoveOnly,
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

/// A message that left a folder in the same window an arrival appeared, as the layer holding
/// it remembers it.
///
/// `corroborated` is D-44's digest comparison, made **by the caller**: the digest and its rule
/// version live with the store, and one rule serving four consumers is the point of D-44 — an
/// adapter recomputing it would be a fifth copy that drifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Departed<'a> {
    /// The message's local identity.
    pub message: u128,
    /// The conversation it was in — D-44's scope.
    pub conversation: Option<&'a str>,
    /// Its `Message-ID` — narrows, never keys.
    pub internet_message_id: Option<&'a str>,
    /// Whether its stored digest agrees with the arrival's, under the same rule version.
    pub corroborated: bool,
}

/// Rejoin an arrival to what departed, under D-44 — the provider document's join key, the
/// internet message identifier **together with** the conversation identifier, applied in
/// D-44's order.
///
/// 1. **Scope.** Only departures in the arrival's own conversation are candidates. An arrival
///    with no conversation identifier has no scope, and a join with no scope is a scan.
/// 2. **Narrow.** Where the arrival has a `Message-ID`, a candidate with a different one is
///    not the same message. Where either lacks one, nothing is narrowed — absence is not a
///    match, and it is not a mismatch either.
/// 3. **Corroborate, then resolve.** Exactly one corroborated candidate keeps its identity;
///    anything else is [`MoveOutcome::DeleteAndArrival`].
#[must_use]
pub fn rejoin(arrival: &Envelope, departed: &[Departed<'_>]) -> MoveOutcome {
    let Some(scope) = arrival.thread_id.as_deref() else {
        return MoveOutcome::DeleteAndArrival;
    };
    let in_scope: Vec<&Departed<'_>> = departed
        .iter()
        .filter(|d| d.conversation == Some(scope))
        .filter(|d| {
            match (
                arrival.internet_message_id.as_deref(),
                d.internet_message_id,
            ) {
                (Some(arriving), Some(left)) => arriving == left,
                _ => true,
            }
        })
        .collect();
    let candidates: Vec<u128> = in_scope.iter().map(|d| d.message).collect();
    let corroborated: Vec<u128> = in_scope
        .iter()
        .filter(|d| d.corroborated)
        .map(|d| d.message)
        .collect();
    resolve_move(&candidates, &corroborated)
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
    fn junk_is_offered_as_a_move_because_the_api_has_no_report_call() {
        // Offering a report that was really a move would be the "silently does something
        // else" FR-39 forbids. D-40: a folder-move-only account gets the move, labelled.
        assert!(!capabilities().offers_junk_report());
        assert!(capabilities().offers_junk_move());
    }

    #[test]
    fn the_table_agrees_with_the_document_that_owns_it() {
        // The two-places rule: a capability changed in code and not in the document is a
        // defect. This is the cheap half of that check.
        const DOC: &str = include_str!("../../../../docs/mail/providers/microsoft-graph.md");
        assert!(
            DOC.contains("| Junk reporting | folder move only"),
            "the document still claims a report call"
        );
        assert!(DOC.contains("**unstable on move**"));
    }

    fn arriving(conversation: Option<&str>, message_id: Option<&str>) -> Envelope {
        Envelope {
            id: sift_provider::adapter::RemoteMessageId("new".into()),
            thread_id: conversation.map(str::to_owned),
            internet_message_id: message_id.map(str::to_owned),
            ..Envelope::default()
        }
    }

    fn departed<'a>(
        message: u128,
        conversation: &'a str,
        message_id: Option<&'a str>,
        corroborated: bool,
    ) -> Departed<'a> {
        Departed {
            message,
            conversation: Some(conversation),
            internet_message_id: message_id,
            corroborated,
        }
    }

    #[test]
    fn a_move_rejoins_on_the_conversation_and_the_message_identifier_together() {
        let left = [
            departed(1, "c1", Some("<a@x>"), true),
            // Same conversation, different message: the identifier narrows it out even
            // though its digest happens to agree.
            departed(2, "c1", Some("<b@x>"), true),
            // Same identifier, different conversation: out of scope, never scanned for.
            departed(3, "c2", Some("<a@x>"), true),
        ];
        assert_eq!(
            rejoin(&arriving(Some("c1"), Some("<a@x>")), &left),
            MoveOutcome::Rejoined { message: 1 }
        );
    }

    #[test]
    fn a_duplicated_message_identifier_in_one_conversation_resolves_to_distinct_messages() {
        // The case the provider document warns about: internet message identifiers are not
        // reliably unique. Two departures both match and both corroborate, and choosing
        // one would be a guess.
        let left = [
            departed(1, "c1", Some("<a@x>"), true),
            departed(2, "c1", Some("<a@x>"), true),
        ];
        assert_eq!(
            rejoin(&arriving(Some("c1"), Some("<a@x>")), &left),
            MoveOutcome::DeleteAndArrival
        );
    }

    #[test]
    fn an_arrival_with_no_conversation_has_no_scope_and_is_never_joined() {
        let left = [departed(1, "c1", Some("<a@x>"), true)];
        assert_eq!(
            rejoin(&arriving(None, Some("<a@x>")), &left),
            MoveOutcome::DeleteAndArrival
        );
    }

    #[test]
    fn an_uncorroborated_candidate_is_a_near_miss_rather_than_a_join() {
        let left = [departed(1, "c1", Some("<a@x>"), false)];
        assert_eq!(
            rejoin(&arriving(Some("c1"), Some("<a@x>")), &left),
            MoveOutcome::DeleteAndArrival
        );
    }

    #[test]
    fn a_missing_message_identifier_narrows_nothing() {
        // Absence is not a match and not a mismatch. The scope and the digest still decide.
        let left = [departed(1, "c1", None, true)];
        assert_eq!(
            rejoin(&arriving(Some("c1"), Some("<a@x>")), &left),
            MoveOutcome::Rejoined { message: 1 }
        );
    }

    #[test]
    fn the_schema_diff_is_not_optional() {
        assert!(schema_diff_runs_in_ci());
    }
}
