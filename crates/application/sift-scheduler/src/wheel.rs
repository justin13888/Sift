//! D-25 — the timing wheel every piece of periodic work goes through.
//!
//! # The enemy is wakeups, not cycles
//!
//! Battery cost at idle is dominated by timer wakeups preventing deep sleep states, not by
//! the work each one does. That is the whole reason this component exists rather than
//! being the async runtime's timer: the runtime's timer schedules at millisecond
//! granularity with **no "this may fire late, batch it" hint** — and that hint is the
//! entire mechanism by which wakeups coalesce.
//!
//! The runtime's timer is still used, but only for short I/O timeouts, where coalescing is
//! meaningless.
//!
//! # Slots are the coalescing
//!
//! Deadlines are bucketed by [`slack`](Wheel::new), so two deadlines that fall in the same
//! bucket fire together **structurally** rather than because something noticed they were
//! close. Five accounts polling every thirty seconds share one wakeup rather than taking
//! five, and an account added to an existing bucket costs nothing.
//!
//! That is what makes D-94's arithmetic work: the observability rule attributes a coalesced
//! fire to **every** account it served, so one fire spends one wakeup against every
//! account's budget — which composes NFR-11's per-account wording into a bound of **two
//! fires per minute for the whole application, regardless of account count**. An additional
//! account costs nothing if its work joins existing fires, and a third fire per minute
//! fails the gate for every account simultaneously.
//!
//! # What D-25 records as the hard part
//!
//! "A real data structure with real edge cases — clock jumps, suspend and resume,
//! cancellation." All three are handled explicitly below rather than discovered:
//!
//! - **Clock jumps** cannot reach a deadline at all, because deadlines are monotonic and
//!   only alignment reads the wall clock.
//! - **Suspend and resume** is an explicit event: [`Wheel::on_system_wake`]. NFR-38's "no
//!   retry storm on wake" is a property of this component specifically, and the wheel
//!   **MUST handle a wake as an event rather than discovering it through expired timers**.
//! - **Cancellation** removes the entry, and an entry cancelled after its bucket became due
//!   is not delivered.

use crate::clock::Monotonic;
use core::time::Duration;
use sift_foundation::identity::AccountId;
use sift_foundation::limits::{L24_BACKOFF_CAP, L24_BACKOFF_JITTER};
use std::collections::BTreeMap;

/// Identifies an armed timer, so it can be cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimerId(u64);

/// What a fire is for. The account is carried so that observability can attribute the fire
/// to it — and so that a coalesced fire can be attributed to *every* account it served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: TimerId,
    /// `None` for work that belongs to the installation rather than to an account — a
    /// filter-list update, for instance, whose traffic FR-36 charges to the installation
    /// and not across accounts.
    pub account: Option<AccountId>,
    pub kind: Work,
}

/// The closed set of things the scheduler fires.
///
/// Enumerated rather than a boxed closure so that the runtime debug panel can say what is
/// scheduled, which FR-34 requires, and so that a fire is inspectable in a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// Poll a folder for changes, or re-establish a push watch.
    Sync,
    /// Flush the mutation queue.
    FlushMutations,
    /// Re-issue an IMAP idle watch before it expires — L-27.
    RenewWatch,
    /// Retry after a failure, under the backoff curve.
    Retry,
    /// Reattempt while a captive portal is present — L-28.
    PortalReattempt,
    /// Work belonging to the installation rather than an account.
    Maintenance,
}

/// The wheel.
///
/// Not `Sync` on its own: D-93 requires the governor be one serialized task and D-25
/// requires one wheel, so this is owned by a single task and reached through it. That is
/// deliberate — a lock here would be a lock a shed target needs.
#[derive(Debug)]
pub struct Wheel {
    /// Bucket index → entries. The index is `deadline / slack`, so membership of a bucket
    /// *is* the coalescing decision.
    buckets: BTreeMap<u64, Vec<Entry>>,
    /// Where each armed timer lives, so cancellation does not scan.
    located: BTreeMap<TimerId, u64>,
    slack: Duration,
    next_id: u64,
}

impl Wheel {
    /// `slack` is the coalescing window: deadlines within one bucket fire together.
    ///
    /// It is the "this may fire late, batch it" hint, expressed as structure. Larger slack
    /// means fewer wakeups and later work; NFR-11's budget is what decides how large.
    #[must_use]
    pub fn new(slack: Duration) -> Self {
        Self {
            buckets: BTreeMap::new(),
            located: BTreeMap::new(),
            slack: if slack.is_zero() {
                Duration::from_millis(1)
            } else {
                slack
            },
            next_id: 0,
        }
    }

    fn bucket_of(&self, deadline: Monotonic) -> u64 {
        let slack_ns = self.slack.as_nanos().max(1) as u64;
        deadline.0 / slack_ns
    }

    fn bucket_fires_at(&self, bucket: u64) -> Monotonic {
        let slack_ns = self.slack.as_nanos().max(1) as u64;
        // The end of the bucket: every deadline in it has passed by then, so nothing fires
        // early. Firing early would be a wakeup that did no work and still woke the machine.
        Monotonic(bucket.saturating_add(1).saturating_mul(slack_ns))
    }

    /// Arm a timer.
    pub fn arm(&mut self, deadline: Monotonic, account: Option<AccountId>, kind: Work) -> TimerId {
        let id = TimerId(self.next_id);
        self.next_id += 1;
        let bucket = self.bucket_of(deadline);
        self.buckets
            .entry(bucket)
            .or_default()
            .push(Entry { id, account, kind });
        self.located.insert(id, bucket);
        id
    }

    /// Cancel an armed timer. Returns whether it was still armed.
    pub fn cancel(&mut self, id: TimerId) -> bool {
        let Some(bucket) = self.located.remove(&id) else {
            return false;
        };
        if let Some(entries) = self.buckets.get_mut(&bucket) {
            entries.retain(|e| e.id != id);
            if entries.is_empty() {
                self.buckets.remove(&bucket);
            }
        }
        true
    }

    /// When the platform timer should next be armed for, or `None` if nothing is scheduled.
    ///
    /// **`None` is the state NFR-10 and NFR-11 are really about**: an application with
    /// nothing scheduled arms no timer and takes no wakeups at all.
    #[must_use]
    pub fn next_fire(&self) -> Option<Monotonic> {
        self.buckets.keys().next().map(|b| self.bucket_fires_at(*b))
    }

    /// Everything due at `now`, as one fire.
    ///
    /// Entries come out grouped rather than one at a time, because the caller is servicing
    /// **one wakeup** and the accounting has to reflect that.
    pub fn fire_due(&mut self, now: Monotonic) -> Vec<Entry> {
        let mut fired = Vec::new();
        let due: Vec<u64> = self
            .buckets
            .keys()
            .copied()
            .take_while(|b| self.bucket_fires_at(*b) <= now)
            .collect();
        for bucket in due {
            if let Some(entries) = self.buckets.remove(&bucket) {
                for e in &entries {
                    self.located.remove(&e.id);
                }
                fired.extend(entries);
            }
        }
        fired
    }

    /// Number of timers armed.
    #[must_use]
    pub fn armed(&self) -> usize {
        self.located.len()
    }

    /// Number of distinct wakeups the currently armed timers will cost.
    ///
    /// This is the number NFR-11 is measured in, and the reason it is worth reporting
    /// separately from [`armed`](Self::armed): the difference between the two is exactly
    /// what coalescing bought.
    #[must_use]
    pub fn wakeups_scheduled(&self) -> usize {
        self.buckets.len()
    }

    /// Every account a fire of `entries` served.
    ///
    /// D-94: a coalesced fire is attributed to **every** account whose work it served, not
    /// to the one that happened to set the deadline. That rule is what turns NFR-11's
    /// per-account wording into an application-wide bound, and it belongs here rather than
    /// at the reporting site because only the wheel knows what a fire contained.
    #[must_use]
    pub fn accounts_served(entries: &[Entry]) -> Vec<AccountId> {
        let mut accounts: Vec<AccountId> = entries.iter().filter_map(|e| e.account).collect();
        accounts.sort_unstable();
        accounts.dedup();
        accounts
    }

    /// The system woke from sleep.
    ///
    /// The monotonic clock does not advance across sleep on either target platform, so the
    /// wheel cannot discover this — and discovering it through expired timers is precisely
    /// what NFR-38 forbids, because every deadline that came due during a long sleep would
    /// fire at once, against every account, in one burst. That is a retry storm arriving
    /// from the clock.
    ///
    /// So a wake **collapses everything overdue into a single bucket**: one wakeup, one
    /// batch of work, with the L-23 connection budget bounding how much of it runs at once.
    /// Work that was not overdue is untouched.
    pub fn on_system_wake(&mut self, now: Monotonic) {
        let overdue: Vec<u64> = self
            .buckets
            .keys()
            .copied()
            .take_while(|b| self.bucket_fires_at(*b) <= now)
            .collect();
        if overdue.len() <= 1 {
            return;
        }
        let target = self.bucket_of(now);
        let mut merged = Vec::new();
        for bucket in overdue {
            if let Some(entries) = self.buckets.remove(&bucket) {
                merged.extend(entries);
            }
        }
        for e in &merged {
            self.located.insert(e.id, target);
        }
        self.buckets.entry(target).or_default().extend(merged);
    }
}

/// The backoff delay after `attempt` consecutive failures.
///
/// Exponential, capped at L-24, with ±25% jitter. Two properties matter more than the
/// curve:
///
/// - **The cap matches the longest aligned poll interval**, so a backed-off account
///   rejoins an existing wheel fire rather than adding one of its own.
/// - **Jitter is what stops a shared cause producing a synchronised retry.** Fifteen
///   connections dropped by one network change would otherwise all come back at the same
///   instant, which is NFR-38's storm arriving from the network rather than the clock.
///
/// `attempt` is the count of failures so far; `0` is the first retry.
#[must_use]
pub fn backoff(attempt: u32, jitter_source: u64) -> Duration {
    let base = Duration::from_secs(1);
    let uncapped = base.saturating_mul(1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX));
    let capped = uncapped.min(L24_BACKOFF_CAP);

    // Deterministic from the caller's source rather than from a global generator, so a test
    // can assert the spread and a replayed schedule reproduces.
    let spread = (jitter_source % 2001) as f64 / 1000.0 - 1.0; // −1.0 ..= 1.0
    let factor = 1.0 + spread * L24_BACKOFF_JITTER;
    capped.mul_f64(factor.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, next_aligned, test_clock::TestClock};

    const SLACK: Duration = Duration::from_secs(30);

    fn wheel() -> Wheel {
        Wheel::new(SLACK)
    }
    fn account(n: u128) -> Option<AccountId> {
        Some(AccountId::from_u128(n))
    }
    fn at(secs: u64) -> Monotonic {
        Monotonic(secs * 1_000_000_000)
    }

    #[test]
    fn nothing_scheduled_arms_no_timer() {
        // The state NFR-10's 0.1% and NFR-11's two-per-minute are really about: an idle
        // application with nothing to do takes no wakeups at all.
        assert_eq!(wheel().next_fire(), None);
        assert_eq!(wheel().wakeups_scheduled(), 0);
    }

    #[test]
    fn deadlines_in_one_slack_window_cost_one_wakeup() {
        // The whole point. Five accounts polling on roughly the same cadence share a fire.
        let mut w = wheel();
        for i in 0..5 {
            w.arm(at(10 + i), account(u128::from(i)), Work::Sync);
        }
        assert_eq!(w.armed(), 5);
        assert_eq!(w.wakeups_scheduled(), 1, "five timers took five wakeups");
    }

    #[test]
    fn an_extra_account_joining_an_existing_fire_costs_nothing() {
        // D-94's wanted consequence, stated as a test: "an additional account costs
        // nothing if its work joins existing fires".
        let mut w = wheel();
        w.arm(at(10), account(1), Work::Sync);
        let before = w.wakeups_scheduled();
        w.arm(at(11), account(2), Work::Sync);
        assert_eq!(w.wakeups_scheduled(), before);
    }

    #[test]
    fn deadlines_far_apart_do_not_coalesce() {
        // Coalescing must not be so eager that it delays work past its window.
        let mut w = wheel();
        w.arm(at(10), account(1), Work::Sync);
        w.arm(at(400), account(2), Work::Sync);
        assert_eq!(w.wakeups_scheduled(), 2);
    }

    #[test]
    fn a_fire_never_happens_before_its_deadline() {
        // Firing early is a wakeup that does no work and still woke the machine. The bucket
        // fires at its *end*, so everything in it is genuinely due.
        let mut w = wheel();
        w.arm(at(25), account(1), Work::Sync);
        let fire = w.next_fire().expect("armed");
        assert!(
            fire >= at(25),
            "the bucket fires at {fire:?}, before the deadline"
        );
        assert!(
            w.fire_due(at(25)).is_empty(),
            "fired before the bucket was due"
        );
        assert_eq!(w.fire_due(fire).len(), 1);
    }

    #[test]
    fn a_fire_delivers_everything_due_as_one_batch() {
        let mut w = wheel();
        w.arm(at(1), account(1), Work::Sync);
        w.arm(at(2), account(2), Work::FlushMutations);
        w.arm(at(500), account(3), Work::Sync);
        let fired = w.fire_due(at(60));
        assert_eq!(
            fired.len(),
            2,
            "the caller is servicing one wakeup, not two"
        );
        assert_eq!(w.armed(), 1);
    }

    #[test]
    fn a_coalesced_fire_is_attributed_to_every_account_it_served() {
        // D-94: not to the account that happened to set the deadline. This attribution rule
        // is what composes NFR-11's per-account wording into an application-wide bound —
        // one fire spends one wakeup against every account's budget.
        let mut w = wheel();
        w.arm(at(1), account(1), Work::Sync);
        w.arm(at(2), account(2), Work::Sync);
        w.arm(at(3), account(3), Work::Sync);
        let fired = w.fire_due(at(60));
        assert_eq!(
            Wheel::accounts_served(&fired),
            vec![
                AccountId::from_u128(1),
                AccountId::from_u128(2),
                AccountId::from_u128(3)
            ]
        );
    }

    #[test]
    fn installation_work_is_charged_to_no_account() {
        // FR-36: traffic belonging to no account is charged to the installation and does
        // not count toward any account's cap.
        let mut w = wheel();
        w.arm(at(1), None, Work::Maintenance);
        let fired = w.fire_due(at(60));
        assert!(Wheel::accounts_served(&fired).is_empty());
    }

    #[test]
    fn cancelling_removes_the_work_and_the_wakeup_it_would_have_cost() {
        let mut w = wheel();
        let id = w.arm(at(10), account(1), Work::Sync);
        assert!(w.cancel(id));
        assert_eq!(w.armed(), 0);
        assert_eq!(
            w.wakeups_scheduled(),
            0,
            "an empty bucket still schedules a wakeup"
        );
        assert_eq!(w.next_fire(), None);
        assert!(!w.cancel(id), "cancelling twice reported success");
    }

    #[test]
    fn a_cancelled_timer_is_not_delivered() {
        let mut w = wheel();
        let keep = w.arm(at(1), account(1), Work::Sync);
        let drop_it = w.arm(at(2), account(2), Work::Sync);
        w.cancel(drop_it);
        let fired = w.fire_due(at(60));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].id, keep);
    }

    #[test]
    fn a_wall_clock_correction_cannot_reach_a_deadline() {
        // The failure D-25 makes the monotonic clock normative to prevent: on a wall-clock
        // wheel, a machine syncing its clock forward by an hour would immediately fire an
        // hour of scheduled work, against every account, in one burst.
        let clock = TestClock::new(0, 1_000_000);
        let mut w = wheel();
        let deadline = clock.monotonic().after(Duration::from_secs(300));
        w.arm(deadline, account(1), Work::Sync);

        clock.correct_wall_clock(60 * 60 * 1000);

        assert!(
            w.fire_due(clock.monotonic()).is_empty(),
            "an hour of work fired because the wall clock moved"
        );
        assert_eq!(w.armed(), 1);
    }

    #[test]
    fn alignment_is_computed_on_the_wall_clock_and_armed_on_the_monotonic_one() {
        // The rule that lets NFR-37's aligned polls exist without exposing deadlines to
        // clock corrections. Two accounts computing the same period land on the same
        // instant without coordinating, which is what makes them share a wakeup.
        let clock = TestClock::new(0, 12_345);
        let period = Duration::from_secs(60);
        let a = next_aligned(&clock, period);
        let b = next_aligned(&clock, period);
        assert_eq!(a, b, "two accounts did not agree on the aligned instant");
        // 12_345 ms past the epoch means 47_655 ms to the next whole minute.
        assert_eq!(a, Monotonic(47_655 * 1_000_000));
    }

    #[test]
    fn waking_from_a_long_sleep_costs_one_wakeup_rather_than_one_per_missed_deadline() {
        // NFR-38's "no retry storm on wake", which D-25 says is a property of this
        // component specifically. The monotonic clock does not advance across sleep, so the
        // wheel cannot discover this — it has to be told.
        let mut w = wheel();
        for hour in 0..8 {
            for acct in 0..5u128 {
                w.arm(at(hour * 3600 + acct as u64), account(acct), Work::Sync);
            }
        }
        assert!(
            w.wakeups_scheduled() >= 8,
            "the setup did not create distinct wakeups"
        );

        let now = at(9 * 3600);
        w.on_system_wake(now);

        assert_eq!(
            w.wakeups_scheduled(),
            1,
            "the wake left a storm of separate fires"
        );
        let fired = w.fire_due(w.next_fire().expect("armed"));
        assert_eq!(fired.len(), 40, "work was lost rather than coalesced");
    }

    #[test]
    fn a_wake_leaves_work_that_was_not_overdue_alone() {
        let mut w = wheel();
        w.arm(at(1), account(1), Work::Sync);
        w.arm(at(2), account(2), Work::Sync);
        w.arm(at(100_000), account(3), Work::Sync);

        w.on_system_wake(at(3600));

        assert_eq!(w.armed(), 3, "the wake dropped or duplicated work");
        assert_eq!(
            w.wakeups_scheduled(),
            2,
            "future work was dragged into the wake fire"
        );
    }

    #[test]
    fn a_wake_with_nothing_overdue_changes_nothing() {
        let mut w = wheel();
        w.arm(at(100_000), account(1), Work::Sync);
        let before = w.wakeups_scheduled();
        w.on_system_wake(at(10));
        assert_eq!(w.wakeups_scheduled(), before);
        assert_eq!(w.armed(), 1);
    }

    #[test]
    fn a_cancelled_timer_stays_cancellable_after_a_wake_moved_it() {
        // on_system_wake rebuckets entries; if the location index were not updated,
        // cancellation would silently stop working.
        let mut w = wheel();
        let a = w.arm(at(1), account(1), Work::Sync);
        w.arm(at(3601), account(2), Work::Sync);
        w.on_system_wake(at(7200));
        assert!(
            w.cancel(a),
            "the entry could not be found after the wake moved it"
        );
        assert_eq!(w.armed(), 1);
    }

    #[test]
    fn work_becomes_due_as_the_clock_advances() {
        // The ordinary path, driven through the clock rather than by handing `fire_due` a
        // chosen instant — so that the bucket arithmetic and the clock agree about when
        // "now" is, rather than only agreeing when a test says so.
        let clock = TestClock::new(0, 0);
        let mut w = wheel();
        let deadline = clock.monotonic().after(Duration::from_secs(45));
        w.arm(deadline, account(1), Work::Sync);

        clock.advance(Duration::from_secs(30));
        assert!(
            w.fire_due(clock.monotonic()).is_empty(),
            "fired before the deadline"
        );

        clock.advance(Duration::from_secs(40));
        assert_eq!(
            w.fire_due(clock.monotonic()).len(),
            1,
            "did not fire once due"
        );
        assert_eq!(w.armed(), 0);
    }

    #[test]
    fn a_suspend_does_not_advance_the_monotonic_clock() {
        // The premise on_system_wake rests on. If this were false the wheel could discover
        // a wake on its own and the explicit event would be unnecessary.
        let clock = TestClock::new(1_000, 1_000);
        let before = clock.monotonic();
        clock.suspend(60 * 60 * 1000);
        assert_eq!(clock.monotonic(), before);
    }

    #[test]
    fn backoff_grows_and_stops_at_the_cap() {
        let no_jitter = 1000; // maps to a spread of exactly 0
        assert_eq!(backoff(0, no_jitter), Duration::from_secs(1));
        assert_eq!(backoff(3, no_jitter), Duration::from_secs(8));
        assert_eq!(backoff(60, no_jitter), L24_BACKOFF_CAP);
        assert_eq!(backoff(u32::MAX, no_jitter), L24_BACKOFF_CAP);
    }

    #[test]
    fn backoff_jitter_stays_within_the_stated_quarter() {
        let capped = L24_BACKOFF_CAP;
        let low = capped.mul_f64(1.0 - L24_BACKOFF_JITTER);
        let high = capped.mul_f64(1.0 + L24_BACKOFF_JITTER);
        for source in 0..2001 {
            let d = backoff(60, source);
            assert!(
                d >= low && d <= high,
                "backoff {d:?} escaped ±25% at source {source}"
            );
        }
    }

    #[test]
    fn jitter_actually_spreads_a_shared_cause() {
        // Fifteen connections dropped by one network change must not all return at the same
        // instant — that is NFR-38's storm arriving from the network rather than the clock.
        let delays: std::collections::BTreeSet<Duration> =
            (0..15).map(|i| backoff(60, i * 137)).collect();
        assert!(
            delays.len() > 10,
            "jitter collapsed: only {} distinct delays",
            delays.len()
        );
    }

    #[test]
    fn the_backoff_cap_lets_an_account_rejoin_an_existing_fire() {
        // "The cap matches the longest aligned poll interval, so a backed-off account
        // rejoins an existing wheel fire rather than adding one."
        let mut w = Wheel::new(Duration::from_secs(60));
        let aligned = at(15 * 60);
        w.arm(aligned, account(1), Work::Sync);
        let before = w.wakeups_scheduled();
        w.arm(
            Monotonic::ORIGIN.after(L24_BACKOFF_CAP),
            account(2),
            Work::Retry,
        );
        assert_eq!(
            w.wakeups_scheduled(),
            before,
            "the backed-off account added a wakeup"
        );
    }
}
