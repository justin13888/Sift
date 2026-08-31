//! D-65's fault-injection harness.
//!
//! # What it is for, and why nothing else can do it
//!
//! Three requirements are about what happens when a process **stops mid-write**, and none of
//! them can be tested by running the code normally:
//!
//! - **NFR-16** — no data loss on abrupt termination, at any point.
//! - **NFR-17** — mutations exactly-once *observable*, which means a retry after an unknown
//!   outcome must not double-apply.
//! - **FR-18** — a replayed mutation must not double-apply.
//!
//! The failure-model's durability ordering — *an intent MUST be durably enqueued before it is
//! applied optimistically* — has exactly one window in which it can be wrong, and it is
//! milliseconds wide. **This is the only thing that can catch it.**
//!
//! # Absent from release
//!
//! Behind the `fault-injection` feature, which the workspace's flag register declares off and
//! **must not be compilable into a shipped binary**. Kill points in production code are a
//! way to stop a mail client from a message.

/// A point at which the process can be told to stop.
///
/// Named rather than numbered, because a test that says "kill at point 7" stops meaning
/// anything the moment a line moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KillPoint {
    /// After the journal committed, before the store was touched.
    ///
    /// D-74's window. Leaves an intent enqueued whose optimistic effect was never applied —
    /// **invisible and self-correcting**, because the overlay is derived from the queue.
    AfterJournalBeforeStore,
    /// After the store committed, before the journal did.
    ///
    /// **The order that must never happen.** It would leave local state saying a mutation
    /// occurred with the queue not holding it — losing a mutation the user watched succeed.
    /// A run that reaches this point is a defect in the ordering rather than a case to
    /// handle.
    AfterStoreBeforeJournal,
    /// After the durable `Issued` marker, before the request left.
    ///
    /// D-85's marker costs a write on every intent forever and buys exactly this: a restart
    /// finds `Issued` and moves it to `Reconciling` rather than guessing whether it was sent.
    AfterIssuedMarkerBeforeSend,
    /// After the request left, before the answer arrived.
    AfterSendBeforeAnswer,
    /// Mid delta page.
    ///
    /// A page and its cursor advance in **one transaction**, so a crash here resumes at the
    /// last committed page rather than skipping change.
    MidDeltaPage,
    /// After a blob's bytes landed, before its reference was recorded.
    ///
    /// Produces the orphan D-77 calls a **leak invisible to eviction**, which the refcount
    /// rebuild must collect in the same pass on the same trigger.
    AfterBlobBeforeReference,
}

impl KillPoint {
    pub const ALL: &'static [Self] = &[
        Self::AfterJournalBeforeStore,
        Self::AfterStoreBeforeJournal,
        Self::AfterIssuedMarkerBeforeSend,
        Self::AfterSendBeforeAnswer,
        Self::MidDeltaPage,
        Self::AfterBlobBeforeReference,
    ];

    /// What a restart should find, stated as the property rather than as a procedure.
    #[must_use]
    pub const fn recovery(self) -> Recovery {
        match self {
            // Self-correcting: the overlay is rebuilt from the queue, so the effect that was
            // never applied simply appears.
            Self::AfterJournalBeforeStore => Recovery::SelfCorrecting,
            // Unreachable by construction. If a run ever reaches it, the ordering is wrong.
            Self::AfterStoreBeforeJournal => Recovery::MustBeUnreachable,
            // Inferring sent-versus-unsent cannot be done correctly, so the marker means the
            // question is never asked.
            Self::AfterIssuedMarkerBeforeSend | Self::AfterSendBeforeAnswer => Recovery::Reconciles,
            Self::MidDeltaPage => Recovery::ResumesAtLastCommittedPage,
            Self::AfterBlobBeforeReference => Recovery::CollectedByRefcountRebuild,
        }
    }
}

/// What a correct restart does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Nothing to do; the derived state repairs itself.
    SelfCorrecting,
    /// **A run reaching this point is a defect rather than a case.**
    MustBeUnreachable,
    /// The adapter establishes server state before retrying, rather than blindly replaying —
    /// which is what NFR-17 means by exactly-once *observable*.
    Reconciles,
    ResumesAtLastCommittedPage,
    CollectedByRefcountRebuild,
}

/// Whether a write is allowed to complete, so a torn or reordered write can be simulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Complete,
    /// The write returned success and only part of it reached the medium.
    Torn {
        bytes_written: usize,
    },
    /// The write never reached the medium at all.
    Lost,
}

/// The injector.
///
/// Deliberately a plain value rather than a global: two tests running concurrently must not
/// share a kill point, and a global would make the harness's own behaviour depend on test
/// ordering — which is the thing it exists to rule out.
#[derive(Debug, Default)]
pub struct Injector {
    kill_at: Option<KillPoint>,
    writes: Vec<WriteOutcome>,
    reached: Vec<KillPoint>,
}

impl Injector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn kill_at(mut self, point: KillPoint) -> Self {
        self.kill_at = Some(point);
        self
    }

    #[must_use]
    pub fn with_write_outcomes(mut self, outcomes: Vec<WriteOutcome>) -> Self {
        self.writes = outcomes;
        self
    }

    /// Reach a kill point. Returns whether the process should stop here.
    pub fn reach(&mut self, point: KillPoint) -> bool {
        self.reached.push(point);
        self.kill_at == Some(point)
    }

    /// What the *n*th write does.
    #[must_use]
    pub fn write_outcome(&self, n: usize) -> WriteOutcome {
        self.writes
            .get(n)
            .copied()
            .unwrap_or(WriteOutcome::Complete)
    }

    /// Every point reached, in order — so a test can assert the ordering itself rather than
    /// only its outcome.
    #[must_use]
    pub fn reached(&self) -> &[KillPoint] {
        &self.reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kill_point_states_what_a_restart_should_find() {
        // A kill point with no stated recovery is one a test can only assert did not crash,
        // which is the weakest possible assertion about durability.
        for p in KillPoint::ALL {
            let _ = p.recovery();
        }
        assert_eq!(KillPoint::ALL.len(), 6);
    }

    #[test]
    fn the_forbidden_ordering_has_no_recovery_because_it_must_not_happen() {
        // It would lose a mutation the user watched succeed. A run reaching it is a defect in
        // the ordering rather than a case to handle.
        assert_eq!(
            KillPoint::AfterStoreBeforeJournal.recovery(),
            Recovery::MustBeUnreachable
        );
    }

    #[test]
    fn the_permitted_ordering_is_self_correcting() {
        // The overlay is derived from the queue, so the effect that was never applied simply
        // appears when it is rebuilt.
        assert_eq!(
            KillPoint::AfterJournalBeforeStore.recovery(),
            Recovery::SelfCorrecting
        );
    }

    #[test]
    fn a_crash_around_the_request_reconciles_rather_than_replaying() {
        // What NFR-17 means by exactly-once *observable*: the adapter establishes server
        // state before retrying, because a blind replay could double-apply.
        for p in [
            KillPoint::AfterIssuedMarkerBeforeSend,
            KillPoint::AfterSendBeforeAnswer,
        ] {
            assert_eq!(p.recovery(), Recovery::Reconciles, "{p:?}");
        }
    }

    #[test]
    fn a_blob_without_its_reference_is_collected_rather_than_leaked() {
        // The orphan D-77 calls a leak invisible to eviction, because eviction walks the
        // index and never hears about a file the index does not name.
        assert_eq!(
            KillPoint::AfterBlobBeforeReference.recovery(),
            Recovery::CollectedByRefcountRebuild
        );
    }

    #[test]
    fn the_injector_stops_only_at_the_chosen_point() {
        let mut i = Injector::new().kill_at(KillPoint::MidDeltaPage);
        assert!(!i.reach(KillPoint::AfterJournalBeforeStore));
        assert!(i.reach(KillPoint::MidDeltaPage));
    }

    #[test]
    fn the_order_points_were_reached_in_is_recorded() {
        // So a test can assert the ordering itself rather than only its outcome — which is
        // the difference between checking that journal-before-store held and checking that
        // nothing was lost this time.
        let mut i = Injector::new();
        i.reach(KillPoint::AfterJournalBeforeStore);
        i.reach(KillPoint::MidDeltaPage);
        assert_eq!(
            i.reached(),
            &[KillPoint::AfterJournalBeforeStore, KillPoint::MidDeltaPage]
        );
    }

    #[test]
    fn a_write_can_be_torn_or_lost() {
        // "Interrupted and reordered writes" — a write that returned success with only part
        // of it on the medium is the case a filesystem actually produces.
        let i = Injector::new().with_write_outcomes(vec![
            WriteOutcome::Complete,
            WriteOutcome::Torn { bytes_written: 7 },
        ]);
        assert_eq!(i.write_outcome(0), WriteOutcome::Complete);
        assert_eq!(i.write_outcome(1), WriteOutcome::Torn { bytes_written: 7 });
        assert_eq!(
            i.write_outcome(9),
            WriteOutcome::Complete,
            "an unspecified write should just work"
        );
    }
}
