//! D-69 — startup is two phases, and only the first is on NFR-1's clock.

/// A step on the critical path.
///
/// D-69 fixes the order **within an account**, and the order is not arbitrary: a key is
/// needed to open a store, an open store is needed to migrate it, and a migrated store is
/// needed to query it. Everything else is deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CriticalStep {
    /// The credential store. Its absence is a **process-scoped refusal** under D-71: every
    /// account enters *storage unavailable*, and Sift MUST NOT retry in a loop, because a
    /// futile retry is also a wakeup counted against NFR-11.
    CredentialStore,
    AccountKeys,
    OpenStores,
    /// **On the critical path deliberately.** D-32 is forward-only, so a store that needs
    /// migrating cannot be read until it has been — and a launch that runs one is therefore
    /// **excluded from NFR-1's measurement** rather than counted as a slow start.
    Migrate,
    /// One query. Enough to paint real rows and accept input, which is where NFR-1's clock
    /// stops.
    FirstQuery,
}

impl CriticalStep {
    pub const IN_ORDER: &'static [Self] = &[
        Self::CredentialStore,
        Self::AccountKeys,
        Self::OpenStores,
        Self::Migrate,
        Self::FirstQuery,
    ];

    /// Whether a launch performing this step is measured against NFR-1.
    #[must_use]
    pub const fn counts_toward_cold_start(self) -> bool {
        !matches!(self, Self::Migrate)
    }
}

/// Everything that happens **after** the list is interactive.
///
/// The set is closed. An open-ended "and anything else slow" would let the critical path
/// grow by accretion, which is exactly how a 400 ms budget becomes a 2 s one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deferred {
    /// Parsing the filter lists.
    ///
    /// This **reverses** a statement content-blocking.md makes: it put the list load on
    /// NFR-1's path. D-69 moves it off, and the surviving obligation is narrower — the
    /// engine must be ready before the first *body* renders, not before the first *list*
    /// does. Until it is, the broker refuses, which is under-permissive rather than over.
    FilterLists,
    /// Network-condition detection. NFR-30 allows two seconds, and unknown metered maps to
    /// Conservative meanwhile — so deferring it costs a conservative first sync rather than
    /// a wrong one.
    NetworkConditions,
    /// The blob refcount rebuild. **MUST NOT gate a list of envelopes**: it is a repair
    /// path, and repairing the blob store is not a reason to withhold the inbox.
    RefcountRebuild,
    /// Sync, push, and the queue flush.
    SyncAndFlush,
    /// Contact resolution, which D-41 does on demand anyway.
    ContactResolution,
}

impl Deferred {
    pub const ALL: &'static [Self] = &[
        Self::FilterLists,
        Self::NetworkConditions,
        Self::RefcountRebuild,
        Self::SyncAndFlush,
        Self::ContactResolution,
    ];

    /// Whether this may block user input. **Nothing here may.**
    #[must_use]
    pub const fn may_block_input(self) -> bool {
        false
    }
}

/// Whether accounts start in parallel.
///
/// **They do.** The critical path is per-account, and serializing five accounts' key
/// unwrapping and migration behind each other would make cold start a function of account
/// count — which NFR-1 does not permit and D-6's separate stores make unnecessary.
#[must_use]
pub const fn accounts_start_in_parallel() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_critical_path_is_ordered_by_dependency() {
        // A key is needed to open a store, an open store to migrate it, a migrated store to
        // query it. The order is not arbitrary and cannot be shuffled for speed.
        let mut sorted = CriticalStep::IN_ORDER.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, CriticalStep::IN_ORDER);
    }

    #[test]
    fn a_launch_that_migrates_is_excluded_from_the_cold_start_measurement() {
        // Otherwise the first launch after an upgrade would fail NFR-1 for a reason that is
        // not a regression, and the number would stop meaning anything.
        assert!(!CriticalStep::Migrate.counts_toward_cold_start());
        for s in CriticalStep::IN_ORDER {
            if *s != CriticalStep::Migrate {
                assert!(s.counts_toward_cold_start());
            }
        }
    }

    #[test]
    fn nothing_deferred_may_block_input() {
        // An open-ended deferred set would let the critical path grow by accretion, which is
        // how a 400 ms budget becomes a 2 s one.
        for d in Deferred::ALL {
            assert!(!d.may_block_input(), "{d:?} blocks input");
        }
        assert_eq!(Deferred::ALL.len(), 5, "the deferred set is closed");
    }

    #[test]
    fn the_refcount_rebuild_does_not_gate_the_inbox() {
        // It is a repair path, and repairing the blob store is not a reason to withhold mail.
        assert!(Deferred::ALL.contains(&Deferred::RefcountRebuild));
    }

    #[test]
    fn filter_lists_are_deferred_and_the_obligation_narrows_rather_than_vanishing() {
        // D-69 moves the list parse off NFR-1's path; the engine must still be ready before
        // the first *body* renders. Until it is, the broker refuses — under-permissive.
        assert!(Deferred::ALL.contains(&Deferred::FilterLists));
    }

    #[test]
    fn accounts_do_not_wait_for_each_other() {
        assert!(accounts_start_in_parallel());
    }
}
