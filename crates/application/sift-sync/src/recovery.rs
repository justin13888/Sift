//! D-84 and NFR-18 — cursor recovery.
//!
//! # A full resync is never required for correctness
//!
//! NFR-18 is absolute about this, and both halves of "automatic **and** visible" are
//! normative: the user is never asked to press a button, and is never left wondering why
//! the account is busy.
//!
//! Recovery must preserve what the server cannot reconstruct: **queued mutations, their
//! pending overlays, and cached bodies whose content addresses still match.** Note what is
//! *not* on that list — "read state not yet flushed" was removed deliberately, because
//! there is no second durable write path. Unflushed read state **is** queued mutations.
//!
//! # The sweep, and the one distinction that makes it safe
//!
//! Recovery acquires a fresh cursor, re-enumerates the folder, matches each observed message
//! to an existing local one under D-44's rules, and treats a local message the enumeration
//! did not produce as gone from that folder.
//!
//! > **An interrupted enumeration MUST NOT sweep.** The distinction is the whole safety
//! > property.
//!
//! A partial enumeration looks exactly like a complete one that found fewer messages. If a
//! partial enumeration were allowed to sweep, a network failure half way through would
//! remove every message it had not reached yet.
//!
//! # "Gone from that folder" is not "deleted"
//!
//! Recovery **removes a location**. It MUST NOT delete a message's row and MUST NOT delete
//! its blobs — the row carries the local identity a shell may be holding for a selection,
//! any queued intents against it, and the D-44 digest that will corroborate the next join.

use sift_provider::adapter::{Provenance, RemoteMessageId};
use std::collections::BTreeSet;

/// How far an enumeration got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enumeration {
    /// The server produced the whole folder.
    Complete,
    /// It stopped early — a failure, a shed, a quit. **Nothing may be swept from this.**
    Interrupted,
}

/// What recovery decided to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Messages observed that Sift already had. Their locations stand.
    pub matched: Vec<RemoteMessageId>,
    /// Messages observed that Sift did not have. Inserted as **discovered**, never
    /// delivered — see [`Provenance`].
    pub inserted: Vec<RemoteMessageId>,
    /// Local messages the enumeration did not produce. **A location is removed; the row and
    /// its blobs are not touched.** Empty unless the enumeration completed.
    pub location_removed: Vec<RemoteMessageId>,
    /// Whether the folder may return to `Live`.
    pub complete: bool,
}

impl Outcome {
    /// Everything recovery observed is discovered, including what it inserted.
    #[must_use]
    pub const fn provenance() -> Provenance {
        Provenance::Discovered
    }
}

/// Reconcile a re-enumeration against what is held locally.
///
/// `observed` is what the server produced; `held` is what Sift has in that folder.
pub fn reconcile(
    observed: &[RemoteMessageId],
    held: &[RemoteMessageId],
    enumeration: Enumeration,
) -> Outcome {
    let held_set: BTreeSet<&RemoteMessageId> = held.iter().collect();
    let observed_set: BTreeSet<&RemoteMessageId> = observed.iter().collect();

    let mut matched = Vec::new();
    let mut inserted = Vec::new();
    for id in observed {
        if held_set.contains(id) {
            matched.push(id.clone());
        } else {
            inserted.push(id.clone());
        }
    }

    // The sweep runs **only** on a completed enumeration. An interrupted one cannot tell
    // "absent" from "not reached yet", and trusting it would delete a mailbox on a bad
    // network.
    let location_removed = match enumeration {
        Enumeration::Complete => held
            .iter()
            .filter(|id| !observed_set.contains(id))
            .cloned()
            .collect(),
        Enumeration::Interrupted => Vec::new(),
    };

    Outcome {
        matched,
        inserted,
        location_removed,
        complete: matches!(enumeration, Enumeration::Complete),
    }
}

/// What NFR-18 requires survive a recovery.
///
/// Modelled as a checklist rather than prose so that it can be asserted. The fourth entry
/// is the one most easily lost, and the reason it is *not* a separate entry is stated in
/// the module docs: there is no second durable write path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preserved {
    pub queued_mutations: bool,
    pub pending_overlays: bool,
    /// Only where the content address still matches — a body whose address changed is a
    /// different body, and keeping it would be keeping the wrong bytes.
    pub matching_cached_bodies: bool,
}

impl Preserved {
    /// What a correct recovery preserves.
    pub const REQUIRED: Self = Self {
        queued_mutations: true,
        pending_overlays: true,
        matching_cached_bodies: true,
    };

    #[must_use]
    pub fn satisfies_nfr18(self) -> bool {
        self == Self::REQUIRED
    }
}

/// D-84's stated retreat, if the sweep proves too optimistic in the field.
///
/// Recorded as code rather than as a comment so that adopting it is a one-line change with
/// a name, rather than a rediscovery. It doubles the cost of every recovery, which is why
/// it is not the default.
#[must_use]
pub const fn sweep_requires_two_agreeing_enumerations() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<RemoteMessageId> {
        names
            .iter()
            .map(|n| RemoteMessageId((*n).to_owned()))
            .collect()
    }

    #[test]
    fn a_completed_enumeration_sweeps_what_it_did_not_see() {
        let out = reconcile(
            &ids(&["a", "b"]),
            &ids(&["a", "b", "c"]),
            Enumeration::Complete,
        );
        assert_eq!(out.matched, ids(&["a", "b"]));
        assert_eq!(out.location_removed, ids(&["c"]));
        assert!(out.complete);
    }

    #[test]
    fn an_interrupted_enumeration_sweeps_nothing() {
        // "The distinction is the whole safety property." A partial enumeration looks
        // exactly like a complete one that found fewer messages — so if this were allowed to
        // sweep, a network failure half way through would remove every message it had not
        // reached.
        let out = reconcile(
            &ids(&["a"]),
            &ids(&["a", "b", "c"]),
            Enumeration::Interrupted,
        );
        assert!(
            out.location_removed.is_empty(),
            "an interrupted enumeration removed {} messages",
            out.location_removed.len()
        );
        assert!(
            !out.complete,
            "the folder would have returned to Live on partial evidence"
        );
    }

    #[test]
    fn a_message_the_enumeration_produced_that_sift_lacks_is_inserted() {
        let out = reconcile(&ids(&["a", "new"]), &ids(&["a"]), Enumeration::Complete);
        assert_eq!(out.inserted, ids(&["new"]));
    }

    #[test]
    fn everything_recovery_observes_is_discovered_including_what_it_inserts() {
        // Without this a cursor invalidation would notify the user about every message in
        // the folder — FR-23 defines new mail as *delivered* and unread at that moment, and
        // a recovery re-reading a mailbox that was always there has delivered nothing.
        assert_eq!(Outcome::provenance(), Provenance::Discovered);
    }

    #[test]
    fn recovery_removes_a_location_rather_than_deleting_a_message() {
        // The row carries the local identity a shell may hold for a selection, any queued
        // intents against it, and the D-44 digest the next join will corroborate against.
        // None of that survives a delete, and none of it needs to be lost.
        let out = reconcile(&ids(&[]), &ids(&["gone"]), Enumeration::Complete);
        assert_eq!(out.location_removed, ids(&["gone"]));
        // The outcome carries no notion of deleting anything, which is the point: there is
        // no field here that could delete a row or a blob.
        assert!(out.inserted.is_empty() && out.matched.is_empty());
    }

    #[test]
    fn an_empty_folder_that_stays_empty_changes_nothing() {
        let out = reconcile(&[], &[], Enumeration::Complete);
        assert!(
            out.matched.is_empty() && out.inserted.is_empty() && out.location_removed.is_empty()
        );
    }

    #[test]
    fn nfr18_names_three_things_that_must_survive() {
        assert!(Preserved::REQUIRED.satisfies_nfr18());
        // Losing any one of them is a recovery that asked the server for something the
        // server never had.
        let mut lost = Preserved::REQUIRED;
        lost.queued_mutations = false;
        assert!(!lost.satisfies_nfr18());
    }

    #[test]
    fn the_retreat_is_recorded_and_not_taken() {
        // D-84's own fallback if the sweep proves optimistic: require two consecutive
        // agreeing enumerations. It doubles the cost of every recovery, which is why it is
        // named rather than adopted.
        assert!(!sweep_requires_two_agreeing_enumerations());
    }
}
