//! The JMAP adapter.
//!
//! **The best-behaved of the four, and therefore the first implemented.** The roadmap builds
//! P1 against it deliberately: shape the abstraction against the cleanest protocol first,
//! then stretch it. An abstraction shaped against the worst protocol first would encode that
//! protocol's compromises as though they were the model.
//!
//! It is also the only provider where **push is both good and practical**: a long-polled
//! event stream needing no publicly reachable endpoint. The other three either need one — and
//! so cannot have it — or hold a connection per mailbox.
//!
//! R-1's note deserves repeating beside this: P1 builds against JMAP, so **the first working
//! client serves the smallest provider population** while the largest sits behind a
//! verification process nobody has started.

use sift_provider::capability::{
    ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
    LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
    TrashSemantics,
};

/// What a JMAP account declares.
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        // Mailbox membership is natively many-to-many, which is why archive is a removal
        // rather than a move.
        location_cardinality: LocationCardinality::OneOrMore,
        tag_support: TagSupport::ReadWrite,
        archive: ArchiveSemantics::RemoveFromInbox,
        trash: TrashSemantics::MoveToTrash,
        permanent_delete: true,
        thread_operations: ThreadOperations::Native,
        junk_reporting: JunkReporting::NativeReport,
        delta: DeltaMechanism::ChangesQuery,
        push: PushMechanism::EventStream,
        id_stability: IdStability::StableGlobally,
        server_search: true,
        // Q-9. Declared unknown with a source rather than left absent, which is what lets
        // the planner batch conservatively instead of refusing to batch.
        max_batch_size: Magnitude::Unknown,
        request_budget: Magnitude::Unknown,
        snippet_source: SnippetSource::ProviderSupplied,
        unrecognised: Vec::new(),
    }
}

/// Whether several operations batch into a single request.
///
/// They do, which is the property that makes FR-17's bulk operations cheap here and
/// expensive elsewhere — and the reason a conservative batch size costs this provider more
/// than it costs the others.
#[must_use]
pub const fn batches_method_calls() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declared_table_matches_the_provider_document() {
        let c = capabilities();
        assert_eq!(c.location_cardinality, LocationCardinality::OneOrMore);
        assert_eq!(c.archive, ArchiveSemantics::RemoveFromInbox);
        assert_eq!(c.delta, DeltaMechanism::ChangesQuery);
        assert_eq!(c.push, PushMechanism::EventStream);
        assert_eq!(c.id_stability, IdStability::StableGlobally);
    }

    #[test]
    fn push_costs_one_connection_however_many_folders_are_watched() {
        // The only provider where push is both good and practical: an event stream needing
        // no publicly reachable endpoint.
        assert_eq!(capabilities().connections_for(20), 1);
    }

    #[test]
    fn identifiers_survive_a_move_so_no_corroborated_join_is_needed() {
        assert!(capabilities().identifier_survives_a_move());
    }

    #[test]
    fn q9_is_declared_unknown_rather_than_left_absent() {
        // Absent would mean unsupported, and the planner would refuse to batch at all.
        assert_eq!(capabilities().max_batch_size, Magnitude::Unknown);
        assert_eq!(
            capabilities().batch_size(),
            Capabilities::CONSERVATIVE_BATCH
        );
    }

    #[test]
    fn method_calls_batch() {
        assert!(batches_method_calls());
    }
}
