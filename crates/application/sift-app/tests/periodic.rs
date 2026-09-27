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
fn a_fire_splits_into_a_cheap_half_and_the_work() {
    // #49: the boundary takes what is due under its lock and does the provider round trips
    // one account at a time on a worker. The cheap half must reach no provider — it is what
    // runs where a main loop can be waiting — and must already have re-armed, because the
    // work may still be running when the next timer is asked for.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    let id = app.add_replayed_account("mail").expect("added");
    app.arm_periodic();
    advance(&hand, Duration::from_secs(65));

    let due = app.begin_fire();
    assert_eq!(
        due,
        vec![sift_app::Due::Sync(id), sift_app::Due::Flush(id)],
        "the fire's work is the account's poll and its flush, in one bucket"
    );
    assert!(
        sift_app::list_messages(app.account("mail").expect("open"))
            .expect("list")
            .is_empty(),
        "taking what is due reached the provider, so it cannot run under a main loop's lock"
    );
    assert!(
        app.next_wake().is_some(),
        "the next fire was not armed before the work, so a slow provider delays it"
    );

    let mut report = sift_app::TickReport::default();
    for item in &due {
        app.perform(item, &mut report);
    }
    assert!(
        report.inserted > 0,
        "the work brought nothing in: {report:?}"
    );

    // An account removed between the two halves has nothing left to do, and is not a failure.
    app.forget_account("mail").expect("forgotten");
    let mut after = sift_app::TickReport::default();
    app.perform(&sift_app::Due::Sync(id), &mut after);
    assert!(
        after.failures.is_empty() && after.synced.is_empty(),
        "work for a removed account ran or failed: {after:?}"
    );
}

#[test]
fn a_fire_that_brings_new_mail_says_which_account_and_which_message() {
    // FR-23, and #53: the wheel is what brings mail in with nobody watching, so it is the one
    // place a notification can come from. The backfill — a page per fire — discovers and
    // announces nothing; the delta after it carries one arrival, which is new mail.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    let id = app.add_replayed_account("mail").expect("added");
    app.arm_periodic();

    let mut delta = None;
    let mut backfilled = 0;
    for _ in 0..6 {
        advance(&hand, Duration::from_secs(65));
        let report = app.tick();
        if !report.new_mail.is_empty() {
            delta = Some(report);
            break;
        }
        backfilled += report.inserted;
    }
    assert!(
        backfilled > 0,
        "nothing was discovered before the arrival, so the backfill was never tested"
    );
    let delta = delta.expect("no fire announced the delta's arrival");
    assert_eq!(delta.new_mail.len(), 1, "one account, one entry: {delta:?}");
    let new = &delta.new_mail[0];
    assert_eq!(new.account, id);
    assert_eq!(new.delivered, 1);
    let row = new.newest.as_ref().expect("the arrival has no row to open");
    // Contained rather than equal: NFR-54 isolates display text before it is stored.
    assert!(row.subject.contains("Something new"), "{row:?}");
    assert!(row.unread);

    // **The row the notification names is the row activation finds**, by identity alone —
    // which is all a notification can carry across a relaunch.
    assert_eq!(
        app.message_row(row.id).expect("read").as_ref(),
        Some(row),
        "activation would open something other than what was announced"
    );
}

#[test]
fn a_message_is_found_by_identity_alone_and_an_unknown_one_is_not() {
    // Activation may arrive on a relaunched process with no window and no list, carrying only
    // an identity. One that names nothing — a message since removed by the server — must come
    // back as absent rather than as an error the shell would have to explain.
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("sync");
    let listed = sift_app::list_messages(app.account("mail").expect("open")).expect("list");
    let id = listed.first().expect("a message").0;
    assert!(app.message_row(id).expect("read").is_some());
    assert_eq!(
        app.message_row(sift_foundation::identity::LocalId::from_u128(u128::MAX))
            .expect("read"),
        None,
        "a message no account holds was found"
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

    // **No `arm_periodic` by hand.** Calling it here is what an earlier version of this test
    // did, and it hid the defect exactly: the boundary never called it on the pressure path,
    // so a real installation with no accounts stayed at L3 for the life of the process while
    // this test passed. `App::memory_pressure` arms the wheel itself now.
    app.memory_pressure(sift_governor::Pressure::Critical);
    assert_eq!(app.tier(), sift_governor::Tier::L3);
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
fn the_filter_engine_returns_once_pressure_has_been_clear_with_a_window_open() {
    // D-10 and D-93: the engine returns when pressure has been clear for L-19 **and a window
    // is open** — not when a new window opens, which would leave a user who keeps one window
    // open with no authority for that window's life after a single pass through L1.
    let hand = std::sync::Arc::new(Hand::new());
    let mut app = App::with_clock(Box::new(Shared(std::sync::Arc::clone(&hand))));
    app.set_window_present(true);
    assert!(app.filter_engine_loaded());

    app.memory_pressure(sift_governor::Pressure::Warning);
    assert!(!app.filter_engine_loaded());
    app.memory_pressure(sift_governor::Pressure::Normal);
    assert!(
        !app.filter_engine_loaded(),
        "reloaded the moment pressure cleared, with no dwell"
    );

    for _ in 0..4 {
        advance(&hand, Duration::from_secs(65));
        let _ = app.tick();
    }
    assert_eq!(app.tier(), sift_governor::Tier::L0);
    assert!(
        app.filter_engine_loaded(),
        "the same window stayed open through the shed and never got its authority back"
    );
}

#[test]
fn a_warning_tier_does_not_revoke_the_open_documents_token() {
    // Only L3 destroys the views that hold a capability token, and D-67's callback set is
    // closed, and nothing in it can destroy a body view. Revoking at L2 would leave the
    // reader on screen with a document whose every resource request answers `Revoked` and
    // whose consent buttons fail — with no way for the shell to say why. FR-33 requires the
    // reason be stated, and a dead view that says nothing is worse than an unreleased cache.
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("sync");
    let listed = sift_app::list_messages(app.account("mail").expect("open")).expect("list");
    let id = listed.first().expect("a message").0;

    let document = app.open_document(id, false, false).expect("opened");
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

// ---------------------------------------------------------------------------
// D-122: the application is held across store work only.
// ---------------------------------------------------------------------------

/// A hold that lets something happen between two of a job's held steps — which is exactly
/// where a person's gesture lands once the lock is let go of for every round trip.
struct Between<F: FnMut(&mut App)> {
    app: App,
    steps: usize,
    before_step: usize,
    act: F,
}

impl<F: FnMut(&mut App)> sift_app::Locked for Between<F> {
    fn with<R>(&mut self, step: impl FnOnce(&mut App) -> R) -> Option<R> {
        self.steps += 1;
        if self.steps == self.before_step {
            (self.act)(&mut self.app);
        }
        Some(step(&mut self.app))
    }
}

#[test]
fn a_sync_lets_go_of_the_application_between_its_round_trips() {
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    let mut between = Between {
        app,
        steps: 0,
        before_step: usize::MAX,
        act: |_: &mut App| {},
    };
    let report = App::sync_held(&mut between, "mail", 1).expect("synced");
    assert!(report.inserted > 0, "{report:?}");
    // The loan; the folders; the watched list; a cursor, what is known and the write for the
    // page; and the return. One hold for all of it was the defect.
    assert!(
        between.steps >= 7,
        "the walk ran under {} holds, so a round trip was inside one",
        between.steps
    );
    let account = between.app.account("mail").expect("open");
    assert!(
        account.adapter.is_some() && !account.lent,
        "the adapter was not returned"
    );
}

#[test]
fn a_second_sync_of_a_busy_account_is_refused_rather_than_reconnected() {
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let saw = std::rc::Rc::clone(&seen);
    let mut between = Between {
        app,
        steps: 0,
        // After the loan and the folders, while the walk is between round trips.
        before_step: 3,
        act: move |app: &mut App| {
            let second = app.sync("mail", 1);
            let account = app.account("mail").expect("open");
            *saw.borrow_mut() = Some((second, account.lent, account.adapter.is_some()));
        },
    };
    App::sync_held(&mut between, "mail", 1).expect("the first sync");
    let (second, lent, reconnected) = seen.borrow_mut().take().expect("the gesture ran");
    assert!(lent, "the adapter was not lent while the walk had it");
    assert!(
        second
            .as_ref()
            .is_err_and(|why| why.contains("already talking")),
        "a second conversation with the provider was started: {second:?}"
    );
    assert!(!reconnected, "a lent adapter was reconnected around");
}

#[test]
fn an_account_removed_mid_sync_is_written_nothing_and_its_adapter_dropped() {
    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    let mut between = Between {
        app,
        steps: 0,
        before_step: 4,
        act: |app: &mut App| {
            app.forget_account("mail").expect("forgotten");
        },
    };
    let outcome = App::sync_held(&mut between, "mail", 1);
    assert!(
        outcome.as_ref().is_err_and(|why| why.contains("removed")),
        "{outcome:?}"
    );
    assert!(between.app.account("mail").is_err());
    // And an account added under the same label afterwards is a different one: nothing the
    // removed account's walk fetched lands in it.
    between
        .app
        .add_replayed_account("mail")
        .expect("added again");
    assert!(
        sift_app::list_messages(between.app.account("mail").expect("open"))
            .expect("list")
            .is_empty()
    );
}

#[test]
fn an_intent_made_while_a_flush_is_out_waits_behind_it() {
    use sift_mutations::intent::Intent;

    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("sync");
    app.set_writes_enabled("mail", true).expect("authorized");
    let message = sift_app::list_messages(app.account("mail").expect("open"))
        .expect("list")
        .first()
        .expect("a message")
        .0;
    app.account("mail")
        .expect("open")
        .queue
        .enqueue(1, message, Intent::Archive, 0);

    let mut between = Between {
        app,
        steps: 0,
        // The issue is the first held step and the answer the second: between them the batch
        // is out and the account's queue is not held.
        before_step: 2,
        act: move |app: &mut App| {
            let account = app.account("mail").expect("open");
            assert!(
                account.lent,
                "the flush's adapter was not lent while it was out"
            );
            account.queue.enqueue(2, message, Intent::MarkRead, 0);
        },
    };
    let flushed = App::flush_held(&mut between, "mail").expect("flushed");
    assert_eq!(flushed.report.issued, 1, "{flushed:?}");
    let account = between.app.account("mail").expect("open");
    assert!(!account.lent && account.adapter.is_some());
    let left: Vec<u128> = account.queue.entries().iter().map(|q| q.id).collect();
    assert!(
        left.contains(&2),
        "the intent made while the batch was out was lost or merged: {left:?}"
    );
}

#[test]
fn an_account_removed_while_its_flush_is_out_settles_nothing_and_drops_its_adapter() {
    use sift_mutations::intent::Intent;

    let mut app = App::new();
    app.add_replayed_account("mail").expect("added");
    app.sync("mail", 1).expect("sync");
    app.set_writes_enabled("mail", true).expect("authorized");
    let message = sift_app::list_messages(app.account("mail").expect("open"))
        .expect("list")
        .first()
        .expect("a message")
        .0;
    app.account("mail")
        .expect("open")
        .queue
        .enqueue(1, message, Intent::Archive, 0);

    let mut between = Between {
        app,
        steps: 0,
        // Between the issue and the answer: the batch is out, and the account goes.
        before_step: 2,
        act: |app: &mut App| {
            assert!(app.account("mail").expect("open").lent);
            app.forget_account("mail").expect("forgotten");
        },
    };
    let flushed = App::flush_held(&mut between, "mail");
    assert!(
        flushed.as_ref().is_err_and(|why| why.contains("removed")),
        "an answer was settled against a queue that went with its account: {flushed:?}"
    );
    assert!(between.app.account("mail").is_err());
    assert!(
        !between.app.any_lent(),
        "a removed account's adapter is still counted as out, so every provider call waits"
    );

    // An account added under the same label afterwards is a different one: the dropped
    // adapter was not handed to it, and nothing of the removed queue is in it.
    between
        .app
        .add_replayed_account("mail")
        .expect("added again");
    let account = between.app.account("mail").expect("open");
    assert!(!account.lent && account.adapter.is_some());
    assert!(account.queue.entries().is_empty());
}
