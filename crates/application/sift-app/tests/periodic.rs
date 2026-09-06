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
