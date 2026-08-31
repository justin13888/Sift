//! D-70 — quitting is bounded, and it flushes nothing.

use core::time::Duration;

/// The teardown order.
///
/// Fixed, and each step is where it is for a reason: new intents stop first so nothing
/// arrives during teardown; the body view goes before the stores because **no engine process
/// may outlive its owner**; stores are check-pointed before credential material is released
/// because a check-point may need a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    StopAcceptingIntents,
    TearDownBodyView,
    CheckpointAndCloseStores,
    ReleaseCredentialMaterial,
    Exit,
}

impl Step {
    pub const IN_ORDER: &'static [Self] = &[
        Self::StopAcceptingIntents,
        Self::TearDownBodyView,
        Self::CheckpointAndCloseStores,
        Self::ReleaseCredentialMaterial,
        Self::Exit,
    ];
}

/// The bound, after which the process exits regardless.
///
/// Expiry is "a crash by another name", and NFR-16 already covers that — which is the whole
/// argument for having a bound at all rather than an unbounded graceful shutdown.
pub const TEARDOWN_BOUND: Duration = Duration::from_secs(2);

/// Whether quit drains the mutation queue.
///
/// **It does not**, and the justification is that the durability ordering already did the
/// work: an intent is enqueued durably *before* it is applied optimistically, so nothing the
/// user watched succeed can be lost by exiting now. In-flight provider calls are abandoned
/// and NFR-17's reconciliation runs at the next launch.
///
/// D-70 records the cost honestly: this makes quit and kill nearly the same operation, and a
/// reader may reasonably want the deliberate exit to be the tidy one.
#[must_use]
pub const fn drains_the_queue() -> bool {
    false
}

/// Whether quit awaits an in-flight provider call.
#[must_use]
pub const fn awaits_provider_calls() -> bool {
    false
}

/// Whether quit blocks on D-48's synchronous cancellation.
///
/// **It must not.** Cancellation is called from the main thread and rendezvous with workers;
/// waiting for it during teardown risks the deadlock D-93 also avoids. Observations are
/// discarded by **advancing their generation** instead — D-66's mechanism, reused.
#[must_use]
pub const fn blocks_on_cancellation() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_view_goes_before_the_stores() {
        // No engine process may outlive its owner.
        let view = Step::IN_ORDER
            .iter()
            .position(|s| *s == Step::TearDownBodyView);
        let stores = Step::IN_ORDER
            .iter()
            .position(|s| *s == Step::CheckpointAndCloseStores);
        assert!(view < stores);
    }

    #[test]
    fn credential_material_is_released_after_the_stores_are_checkpointed() {
        // A check-point may need a key.
        let creds = Step::IN_ORDER
            .iter()
            .position(|s| *s == Step::ReleaseCredentialMaterial);
        let stores = Step::IN_ORDER
            .iter()
            .position(|s| *s == Step::CheckpointAndCloseStores);
        assert!(stores < creds);
    }

    #[test]
    fn intents_stop_arriving_first() {
        assert_eq!(Step::IN_ORDER.first(), Some(&Step::StopAcceptingIntents));
        assert_eq!(Step::IN_ORDER.last(), Some(&Step::Exit));
    }

    #[test]
    fn quit_flushes_nothing_and_waits_for_nothing() {
        // The durability ordering already did the work: nothing the user watched succeed can
        // be lost by exiting now.
        assert!(!drains_the_queue());
        assert!(!awaits_provider_calls());
        assert!(!blocks_on_cancellation());
    }

    #[test]
    fn the_bound_is_short_enough_that_expiry_is_ordinary() {
        // Expiry is a crash by another name, and NFR-16 covers that — which is the argument
        // for a bound rather than an unbounded graceful shutdown.
        assert!(TEARDOWN_BOUND <= Duration::from_secs(5));
    }
}
