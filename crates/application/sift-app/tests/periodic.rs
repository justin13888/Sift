//! Mail that arrives without anybody asking for it.
//!
//! # The defect these were written against
//!
//! `sift-scheduler` held a complete, tested timing wheel and **nothing had an edge to it**.
//! `sift-app` did not depend on the crate, so no aligned poll was ever armed, no queue flush
//! was ever scheduled, and `App::sync` ran only where a person had just done something. A
//! client whose whole premise is that it runs all the time fetched mail exactly as often as
//! it was clicked.
//!
//! These drive the wheel with a clock a test moves by hand, because the alternative is a
//! test that waits a minute — which is the kind that gets marked ignored and then deleted.

use core::time::Duration;
use sift_app::App;
use sift_scheduler::clock::{Clock, Monotonic};
use std::sync::Mutex;

/// A clock the test advances itself.
#[derive(Debug)]
struct Hand {
    /// Monotonic nanoseconds, and wall milliseconds.
    state: Mutex<(u64, u64)>,
}

impl Hand {
    fn new() -> Self {
        Self {
            state: Mutex::new((0, 0)),
        }
    }
}

impl Clock for Hand {
    fn monotonic(&self) -> Monotonic {
        Monotonic(self.state.lock().expect("clock").0)
    }

    fn wall_millis(&self) -> u64 {
        self.state.lock().expect("clock").1
    }
}

/// Advance a clock the app is holding, through a second handle to the same state.
fn advance(hand: &std::sync::Arc<Hand>, by: Duration) {
    let mut state = hand.state.lock().expect("clock");
    state.0 = state.0.saturating_add(by.as_nanos() as u64);
    state.1 = state.1.saturating_add(by.as_millis() as u64);
}

/// A clock handle that can be given to `App` and kept by the test.
#[derive(Debug)]
struct Shared(std::sync::Arc<Hand>);

impl Clock for Shared {
    fn monotonic(&self) -> Monotonic {
        self.0.monotonic()
    }

    fn wall_millis(&self) -> u64 {
        self.0.wall_millis()
    }
}

#[test]
fn an_idle_account_polls_without_anybody_asking() {
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    app.add_replayed_account("mail").expect("added");

    // Nothing is armed until something arms it, and a resident process with nothing
    // scheduled must take no wakeups at all.
    assert_eq!(app.next_wake(), None, "a timer was armed by nobody");

    app.arm_periodic();
    let wake = app.next_wake().expect("nothing was armed");
    assert!(
        wake <= Duration::from_secs(60) + Duration::from_secs(5),
        "the first fire is {wake:?}, past the aligned interval plus its slack"
    );

    // No user gesture between here and the fire. This is the whole claim.
    advance(&hand, Duration::from_secs(65));
    let report = app.tick();
    assert!(
        report.synced.contains(&"mail".to_owned()),
        "the fire did not sync the account: {report:?}"
    );
    assert!(report.failures.is_empty(), "{report:?}");
    // **Mail, not merely a call.** `synced` records any `Ok`, and `App::sync` returns an
    // empty report immediately for a paused account — so asserting the name alone would pass
    // for a fire that did nothing at all. This is the number that says envelopes arrived.
    assert!(
        report.inserted > 0,
        "the poll reached the account and brought nothing back: {report:?}"
    );
}

#[test]
fn a_second_account_costs_no_extra_fire() {
    // D-94: NFR-11's budget is the application's, not each account's, and the defence of
    // that is the wheel — an additional account joins a fire that already exists. Two
    // accounts aligned to the same wall-clock multiple land in one bucket, so the second
    // one must not move the next deadline.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));

    app.add_replayed_account("one").expect("added");
    app.arm_periodic();
    let with_one = app.next_wake().expect("armed");

    app.add_replayed_account("two").expect("added");
    app.arm_periodic();
    let with_two = app.next_wake().expect("armed");

    assert_eq!(
        with_one, with_two,
        "the second account moved the fire rather than joining it"
    );

    advance(&hand, Duration::from_secs(65));
    let report = app.tick();
    assert_eq!(
        report.synced.len(),
        2,
        "one fire did not serve both accounts: {report:?}"
    );
}

#[test]
fn re_arming_does_not_accumulate_timers() {
    // `arm_periodic` is called after every account change, so one that added rather than
    // replaced would grow a timer per call — a per-account sleep loop wearing the wheel's
    // clothes, which is the thing D-25 exists to forbid.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    app.add_replayed_account("mail").expect("added");

    for _ in 0..5 {
        app.arm_periodic();
    }

    advance(&hand, Duration::from_secs(65));
    let report = app.tick();
    assert_eq!(
        report.synced,
        vec!["mail".to_owned()],
        "the account was synced once per arming: {report:?}"
    );
}

#[test]
fn a_tier_walks_all_the_way_back_to_l0_without_further_pressure_signals() {
    // **The platform's signal is edge-triggered.** It fires when the state changes, so after
    // a machine calms down there are no further signals — and `Governor::tick` releases one
    // tier per call. Without the wheel re-ticking it, L3 would descend to L2 on the single
    // `normal` event and stall there for the life of the process, with the body view and
    // every cache below it still shed and nothing to say why.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    app.add_replayed_account("mail").expect("added");
    app.arm_periodic();

    assert!(
        app.memory_pressure(sift_governor::Pressure::Critical)
            .is_some()
    );
    assert_eq!(app.tier(), sift_governor::Tier::L3);

    // One `normal` edge, and then nothing — which is all a real source delivers.
    app.memory_pressure(sift_governor::Pressure::Normal);

    // Only the wheel from here. Each fire is a minute apart, and L-19's dwell is a minute, so
    // the tier should come down a step at a time rather than all at once or not at all.
    for _ in 0..4 {
        advance(&hand, Duration::from_secs(65));
        let _ = app.tick();
    }

    assert_eq!(
        app.tier(),
        sift_governor::Tier::L0,
        "the tier stalled on the way back down, so a shed outlived the pressure that caused it"
    );
}

#[test]
fn a_still_critical_system_is_not_released_by_the_wheel() {
    // The re-tick uses the *last level the platform reported*, not an assumption of calm. A
    // system that went critical and stayed there sends no further signal, and releasing on
    // that silence would undo the shed while the pressure that caused it was still present.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    app.add_replayed_account("mail").expect("added");
    app.arm_periodic();

    app.memory_pressure(sift_governor::Pressure::Critical);
    assert_eq!(app.tier(), sift_governor::Tier::L3);

    for _ in 0..5 {
        advance(&hand, Duration::from_secs(65));
        let _ = app.tick();
    }

    assert_eq!(
        app.tier(),
        sift_governor::Tier::L3,
        "the wheel released a tier while the system was still under critical pressure"
    );
}

#[test]
fn a_container_with_no_accounts_still_walks_a_tier_back_down() {
    // The governor's clock is the wheel, and the wheel used to be armed per account — so a
    // fresh install with nothing added went to L3 under pressure and stayed there for the
    // life of the process: every window destroyed, nothing to poll, and therefore nothing to
    // bring it back. A held tier is installation work, not an account's.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));

    app.memory_pressure(sift_governor::Pressure::Critical);
    assert_eq!(app.tier(), sift_governor::Tier::L3);
    app.arm_periodic();
    assert!(
        app.next_wake().is_some(),
        "nothing was armed, so the tier has no clock and can never come down"
    );

    app.memory_pressure(sift_governor::Pressure::Normal);
    for _ in 0..4 {
        advance(&hand, Duration::from_secs(65));
        let _ = app.tick();
    }
    assert_eq!(app.tier(), sift_governor::Tier::L0);
}

#[test]
fn a_warning_tier_does_not_revoke_the_open_documents_token() {
    // Only L3 destroys the views that hold a capability token, and D-67's callback set is
    // closed at six with nothing that can destroy a body view. Revoking at L2 would leave the
    // reader on screen with a document whose every resource request answers `Revoked` and
    // whose consent buttons fail — with no way for the shell to say why. FR-33 requires the
    // reason be stated, and a dead view that says nothing is worse than an unreleased cache.
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("sync");
    let listed = sift_app::list_messages(app.account("mail").expect("open")).expect("list");
    let id = listed.first().expect("a message").0;

    let document = app.open_document(id, false).expect("opened");
    app.memory_pressure(sift_governor::Pressure::Warning);
    assert_eq!(app.tier(), sift_governor::Tier::L2);
    assert!(
        app.resources.origin_of(&document.token).is_some(),
        "L2 revoked the token of a document still on screen"
    );

    app.memory_pressure(sift_governor::Pressure::Critical);
    assert_eq!(app.tier(), sift_governor::Tier::L3);
    assert!(
        app.resources.origin_of(&document.token).is_none(),
        "L3 destroys every window, so a token that outlived one would be unrevocable"
    );
}
