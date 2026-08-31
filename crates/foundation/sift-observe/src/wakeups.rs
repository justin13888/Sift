//! Wakeup accounting — the metric NFR-11 is actually measured in.
//!
//! > "Wakeups MUST be **counted and reported, not inferred** — per subsystem and per
//! > account."
//!
//! Inferring them from timer configuration would miss socket wakes, which NFR-11 counts
//! equally: a keepalive arriving is a wakeup whether or not Sift armed anything.

use sift_subsystem::Subsystem;
use std::collections::BTreeMap;

/// What woke the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    TimerFire,
    /// NFR-11 was amended to match its instrument: it now covers **timer fires and socket
    /// wakes alike**, because a keepalive arriving wakes the machine exactly as a timer does.
    SocketWake,
}

/// Counts on both axes.
#[derive(Debug, Default)]
pub struct Wakeups {
    per_subsystem: BTreeMap<&'static str, u64>,
    per_account: BTreeMap<u128, u64>,
    total: u64,
}

impl Wakeups {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one wakeup, attributed to **every account whose work it served**.
    ///
    /// D-94: not to the account that happened to set the deadline. That rule is what composes
    /// NFR-11's per-account wording into an application-wide bound — one fire spends one
    /// wakeup against every account's budget — and it is the reason the coalescing the wheel
    /// does is visible in the metric at all.
    ///
    /// The alternative, counting a coalesced fire once, was rejected because it would make
    /// the wheel's central benefit **arithmetically invisible in the only metric that
    /// measures it**.
    pub fn record(&mut self, _cause: Cause, subsystem: Subsystem, accounts_served: &[u128]) {
        self.total += 1;
        *self.per_subsystem.entry(subsystem.name()).or_insert(0) += 1;
        for account in accounts_served {
            *self.per_account.entry(*account).or_insert(0) += 1;
        }
    }

    /// Wakeups charged to one account.
    #[must_use]
    pub fn for_account(&self, account: u128) -> u64 {
        self.per_account.get(&account).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn for_subsystem(&self, subsystem: Subsystem) -> u64 {
        self.per_subsystem
            .get(subsystem.name())
            .copied()
            .unwrap_or(0)
    }

    /// Fires taken by the whole application.
    ///
    /// D-94's composition: **two per minute for the whole application, regardless of account
    /// count.** An additional account costs nothing if its work joins existing fires, and a
    /// third fire per minute fails the gate for every account simultaneously.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Whether the application is inside NFR-11's budget over `minutes`.
    #[must_use]
    pub fn within_nfr11(&self, minutes: u64) -> bool {
        self.total <= 2 * minutes.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coalesced_fire_is_charged_to_every_account_it_served() {
        // D-94. Counting it once would make the wheel's central benefit arithmetically
        // invisible in the only metric that measures it.
        let mut w = Wakeups::new();
        w.record(Cause::TimerFire, Subsystem::Scheduler, &[1, 2, 3]);
        assert_eq!(w.total(), 1, "one fire became several");
        for account in [1, 2, 3] {
            assert_eq!(
                w.for_account(account),
                1,
                "account {account} was not charged"
            );
        }
    }

    #[test]
    fn an_additional_account_joining_an_existing_fire_costs_nothing() {
        // D-94's wanted consequence, in the metric.
        let mut two = Wakeups::new();
        two.record(Cause::TimerFire, Subsystem::Scheduler, &[1, 2]);
        let mut five = Wakeups::new();
        five.record(Cause::TimerFire, Subsystem::Scheduler, &[1, 2, 3, 4, 5]);
        assert_eq!(two.total(), five.total());
    }

    #[test]
    fn a_socket_wake_counts_the_same_as_a_timer_fire() {
        // NFR-11 was amended to match its instrument: a keepalive arriving wakes the machine
        // exactly as a timer does.
        let mut w = Wakeups::new();
        w.record(Cause::SocketWake, Subsystem::Adapters, &[1]);
        w.record(Cause::TimerFire, Subsystem::Scheduler, &[1]);
        assert_eq!(w.total(), 2);
        assert_eq!(w.for_account(1), 2);
    }

    #[test]
    fn a_third_fire_per_minute_fails_the_gate() {
        let mut w = Wakeups::new();
        for _ in 0..2 {
            w.record(Cause::TimerFire, Subsystem::Scheduler, &[1]);
        }
        assert!(w.within_nfr11(1));
        w.record(Cause::TimerFire, Subsystem::Scheduler, &[1]);
        assert!(!w.within_nfr11(1));
    }

    #[test]
    fn the_gate_does_not_scale_with_account_count() {
        // "An application bound that does not scale with accounts is unusually strict" —
        // D-94's own recorded weakness, and the property being asserted.
        let mut w = Wakeups::new();
        for _ in 0..2 {
            w.record(Cause::TimerFire, Subsystem::Scheduler, &[1, 2, 3, 4, 5]);
        }
        assert!(w.within_nfr11(1), "five accounts were allowed ten fires");
    }

    #[test]
    fn both_axes_are_reported() {
        // Per subsystem and per account. The gate is insensitive to *which* account
        // regressed, and these counters are what recovers that for diagnosis.
        let mut w = Wakeups::new();
        w.record(Cause::TimerFire, Subsystem::Sync, &[7]);
        assert_eq!(w.for_subsystem(Subsystem::Sync), 1);
        assert_eq!(w.for_account(7), 1);
    }
}
