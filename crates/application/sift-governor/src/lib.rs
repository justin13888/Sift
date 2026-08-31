//! D-93, D-20 — the memory-pressure governor and the shed tiers.
//!
//! # The preconditions, without which none of this works
//!
//! `docs/runtime/memory-pressure.md` calls the first of these "the single most load-bearing
//! line in this document", and it is a code-review rule rather than an aspiration:
//!
//! - **Every cache has an explicit byte budget and an eviction policy. No unbounded map
//!   anywhere.** Without this the governor has nothing to release.
//! - **Every cache reports its size**, so the governor acts on declared numbers rather than
//!   estimates.
//! - MIME parsing streams.
//! - **The shell owns no authoritative state**, so destroying it at L3 loses nothing that
//!   only the network could restore.
//! - Lists are virtualized with a fixed window.
//!
//! # Tier targets are compositions, not percentages
//!
//! The earlier −30% / −50% figures were arithmetically impossible: half of NFR-9's 150 MB is
//! 75 MB, which is *below* NFR-8's 90 MB window-less floor. Each tier is now the tier above
//! **minus what it releases**, and every term is a declared, reported cache size — so the
//! subtraction can be checked rather than believed.

use core::time::Duration;
use sift_foundation::limits::L19_PRESSURE_DWELL;
use sift_subsystem::Subsystem;

/// What the platform says about memory.
///
/// **Subscribed to, never polled.** Polling free memory is both a wakeup source — counted
/// against NFR-11 — and a worse signal than the one the system already computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    Normal,
    Warning,
    Critical,
}

/// The shed tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Steady state — NFR-9's 150 MB with a window open.
    L0,
    /// Mild. Drops decoded-image caches, rendered-body caches and prefetch queues, and
    /// **releases the filter engine**.
    ///
    /// "L1 is the tier Sift will spend real time in, not a corner it occasionally visits" —
    /// the reference rig is a 2020-era laptop with 8 GB. Which means the state where a user
    /// who allowed a sender's images still does not see them is **ordinary**, and FR-33 must
    /// report it as the shed it is rather than as a rule.
    L1,
    /// Warning. **Destroys the body view** — the largest single allocation in the running
    /// application — drops parsed-MIME caches, releases database memory, shrinks the search
    /// index cache.
    ///
    /// Never goes below NFR-8's floor, because a window is still live.
    L2,
    /// Critical. Destroys **every window and every window shell's view hierarchy**, leaving
    /// the application shell that owns the always-on surface.
    ///
    /// **L3 terminates nothing.** Removing the tray would leave the application unreachable,
    /// and the honest floor is toolkit residue — whatever the toolkit keeps resident once
    /// initialized, which Q-12 says is unmeasured and which is why NFR-8 is a placeholder.
    L3,
}

impl Tier {
    #[must_use]
    pub const fn for_pressure(pressure: Pressure) -> Self {
        match pressure {
            Pressure::Normal => Self::L0,
            Pressure::Warning => Self::L2,
            Pressure::Critical => Self::L3,
        }
    }

    /// Whether the filter engine is released at this tier.
    ///
    /// Once released, **an absent authority denies**: every remote fetch is refused rather
    /// than falling through to the compiled backstop, and the engine may not be reloaded on
    /// demand — a 40 MB allocation in response to a pressure signal is the shed undoing
    /// itself.
    #[must_use]
    pub const fn releases_filter_engine(self) -> bool {
        !matches!(self, Self::L0)
    }

    #[must_use]
    pub const fn destroys_body_view(self) -> bool {
        matches!(self, Self::L2 | Self::L3)
    }

    #[must_use]
    pub const fn destroys_windows(self) -> bool {
        matches!(self, Self::L3)
    }

    /// **False at every tier.** The deepest shed is in-process; if the system needs more
    /// than L3 can give, it will terminate the process, and Sift is correct across that
    /// because the store and the queue are crash-consistent. Termination costs a repaint.
    #[must_use]
    pub const fn terminates_anything(self) -> bool {
        false
    }

    /// The deadline for *issuing* the shed — NFR-13.
    ///
    /// NFR-13 and NFR-46 measure **different clocks**, and the distinction is normative:
    /// this is signal-to-issued, and NFR-46's one second is teardown-to-pages-returned.
    /// Stated as one clock they would contradict each other.
    #[must_use]
    pub const fn issue_deadline(self) -> Option<Duration> {
        match self {
            Self::L2 => Some(Duration::from_millis(500)),
            Self::L3 => Some(Duration::from_secs(1)),
            Self::L0 | Self::L1 => None,
        }
    }

    /// The subsystems this tier releases memory from.
    #[must_use]
    pub fn sheds(self) -> Vec<Subsystem> {
        match self {
            Self::L0 => vec![],
            Self::L1 => vec![Subsystem::Broker],
            Self::L2 => vec![
                Subsystem::Broker,
                Subsystem::Bodyview,
                Subsystem::Parse,
                Subsystem::Store,
                Subsystem::Index,
            ],
            Self::L3 => vec![
                Subsystem::Broker,
                Subsystem::Bodyview,
                Subsystem::Parse,
                Subsystem::Store,
                Subsystem::Index,
                Subsystem::Shell,
                Subsystem::Presentation,
            ],
        }
    }
}

/// D-93 — the governor.
///
/// **One serialized task.** "The current tier" then has one writer, and a signal arriving
/// during a transition **supersedes rather than interleaves** — a critical signal arriving
/// mid-L2 finishes as L3 rather than as two half-applied transitions.
#[derive(Debug)]
pub struct Governor {
    tier: Tier,
    /// How long the signal has been clear. A tier is released only after L-19.
    clear_for: Duration,
}

/// What the governor decided this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub from: Tier,
    pub to: Tier,
    /// Issued to each owner and **never awaited**.
    ///
    /// NFR-13's deadline is an *issue* deadline for this reason. At L3 destroying windows is
    /// a host callback on the shell's main loop, and waiting would mean the governor blocking
    /// on a main loop it does not control — which, against D-48's synchronous cancellation,
    /// is a deadlock rather than a delay.
    pub sheds: Vec<Subsystem>,
}

impl Default for Governor {
    fn default() -> Self {
        Self::new()
    }
}

impl Governor {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tier: Tier::L0,
            clear_for: Duration::ZERO,
        }
    }

    #[must_use]
    pub const fn tier(&self) -> Tier {
        self.tier
    }

    /// Advance by one tick.
    ///
    /// Deepening is immediate; **releasing waits for L-19 of clear signal and steps one tier
    /// at a time.** Without the hysteresis a system oscillating around the threshold
    /// reparses the 40 MB filter engine on every crossing, which costs more than it saves.
    ///
    /// D-93 records the weakest point: **one dwell serves every tier and every cache**, and
    /// a body view and a filter engine are not alike — so the number is wrong for at least
    /// one of them.
    pub fn tick(&mut self, pressure: Pressure, elapsed: Duration) -> Option<Transition> {
        let demanded = Tier::for_pressure(pressure);
        let from = self.tier;

        if demanded > self.tier {
            self.tier = demanded;
            self.clear_for = Duration::ZERO;
            return Some(Transition {
                from,
                to: self.tier,
                sheds: self.tier.sheds(),
            });
        }

        if pressure == Pressure::Normal && self.tier > Tier::L0 {
            self.clear_for += elapsed;
            if self.clear_for >= L19_PRESSURE_DWELL {
                self.tier = match self.tier {
                    Tier::L3 => Tier::L2,
                    Tier::L2 => Tier::L1,
                    Tier::L1 | Tier::L0 => Tier::L0,
                };
                self.clear_for = Duration::ZERO;
                return Some(Transition {
                    from,
                    to: self.tier,
                    sheds: vec![],
                });
            }
        } else {
            self.clear_for = Duration::ZERO;
        }
        None
    }

    /// Whether the filter engine may be reloaded.
    ///
    /// D-93 is precise here and it is easy to get wrong: the engine returns when pressure has
    /// been clear for L-19 **and a window is open** — *not* when a new window opens. That
    /// distinction is the whole of what the hysteresis buys, and it is what separates this
    /// from the reload-on-demand the shed exists to prevent.
    #[must_use]
    pub const fn filter_engine_may_return(&self, a_window_is_open: bool) -> bool {
        a_window_is_open && matches!(self.tier, Tier::L0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_secs(10);

    #[test]
    fn pressure_deepens_immediately() {
        let mut g = Governor::new();
        let t = g.tick(Pressure::Critical, TICK).expect("transition");
        assert_eq!(t.to, Tier::L3);
        assert_eq!(g.tier(), Tier::L3);
    }

    #[test]
    fn a_deeper_signal_supersedes_rather_than_interleaves() {
        // One serialized task means "the current tier" has one writer. A critical signal
        // arriving mid-L2 finishes as L3, not as two half-applied transitions.
        let mut g = Governor::new();
        g.tick(Pressure::Warning, TICK);
        let t = g.tick(Pressure::Critical, TICK).expect("transition");
        assert_eq!(t.from, Tier::L2);
        assert_eq!(t.to, Tier::L3);
    }

    #[test]
    fn a_tier_is_not_released_until_the_signal_has_been_clear_for_l19() {
        // Without the hysteresis a system oscillating around the threshold reparses the
        // 40 MB filter engine on every crossing.
        let mut g = Governor::new();
        g.tick(Pressure::Critical, TICK);
        assert!(g.tick(Pressure::Normal, Duration::from_secs(30)).is_none());
        assert_eq!(g.tier(), Tier::L3, "released before the dwell elapsed");
        assert!(g.tick(Pressure::Normal, Duration::from_secs(31)).is_some());
    }

    #[test]
    fn tiers_are_released_one_step_at_a_time() {
        let mut g = Governor::new();
        g.tick(Pressure::Critical, TICK);
        assert_eq!(
            g.tick(Pressure::Normal, L19_PRESSURE_DWELL).expect("t").to,
            Tier::L2
        );
        assert_eq!(
            g.tick(Pressure::Normal, L19_PRESSURE_DWELL).expect("t").to,
            Tier::L1
        );
        assert_eq!(
            g.tick(Pressure::Normal, L19_PRESSURE_DWELL).expect("t").to,
            Tier::L0
        );
        assert!(g.tick(Pressure::Normal, L19_PRESSURE_DWELL).is_none());
    }

    #[test]
    fn pressure_returning_resets_the_dwell() {
        let mut g = Governor::new();
        g.tick(Pressure::Critical, TICK);
        g.tick(Pressure::Normal, Duration::from_secs(50));
        g.tick(Pressure::Warning, TICK);
        assert!(
            g.tick(Pressure::Normal, Duration::from_secs(20)).is_none(),
            "the dwell carried over across a pressure event"
        );
    }

    #[test]
    fn the_filter_engine_returns_only_when_clear_and_a_window_is_open() {
        // "Not when a *new* window opens" — that distinction is what the hysteresis buys, and
        // what separates this from the reload-on-demand the shed exists to prevent.
        let mut g = Governor::new();
        g.tick(Pressure::Warning, TICK);
        assert!(
            !g.filter_engine_may_return(true),
            "reloaded while still shed"
        );
        for _ in 0..4 {
            g.tick(Pressure::Normal, L19_PRESSURE_DWELL);
        }
        assert_eq!(g.tier(), Tier::L0);
        assert!(
            !g.filter_engine_may_return(false),
            "reloaded with no window open"
        );
        assert!(g.filter_engine_may_return(true));
    }

    #[test]
    fn the_deepest_shed_terminates_nothing() {
        // Removing the tray would leave the application unreachable. If the system needs
        // more than L3 can give it will terminate the process, and Sift is correct across
        // that — termination costs a repaint.
        for t in [Tier::L0, Tier::L1, Tier::L2, Tier::L3] {
            assert!(!t.terminates_anything());
        }
    }

    #[test]
    fn l3_destroys_windows_but_keeps_the_application_shell() {
        assert!(Tier::L3.destroys_windows());
        assert!(!Tier::L2.destroys_windows(), "a warning destroyed windows");
        // The application shell is Shell's, alongside window shells — it is shed *from*, not
        // destroyed, which is why the tray survives.
        assert!(Tier::L3.sheds().contains(&Subsystem::Shell));
    }

    #[test]
    fn the_body_view_goes_at_the_warning_tier() {
        // The largest single allocation in the running application, and the mechanism by
        // which L2 reclaims it.
        assert!(Tier::L2.destroys_body_view());
        assert!(!Tier::L1.destroys_body_view());
    }

    #[test]
    fn the_filter_engine_goes_at_the_first_tier_that_is_not_steady_state() {
        assert!(!Tier::L0.releases_filter_engine());
        for t in [Tier::L1, Tier::L2, Tier::L3] {
            assert!(t.releases_filter_engine(), "{t:?} kept the 40 MB");
        }
    }

    #[test]
    fn issue_deadlines_are_a_different_clock_from_reclaim() {
        // NFR-13 is signal-to-issued; NFR-46's one second is teardown-to-pages-returned.
        // Stated as one clock they contradict each other.
        assert_eq!(Tier::L2.issue_deadline(), Some(Duration::from_millis(500)));
        assert_eq!(Tier::L3.issue_deadline(), Some(Duration::from_secs(1)));
        assert_eq!(Tier::L0.issue_deadline(), None);
    }

    #[test]
    fn each_tier_sheds_everything_the_one_above_it_does() {
        // Targets are compositions: each tier is the one above minus what it releases. A
        // tier that released less than a shallower one would make the subtraction meaningless.
        for (shallower, deeper) in [
            (Tier::L0, Tier::L1),
            (Tier::L1, Tier::L2),
            (Tier::L2, Tier::L3),
        ] {
            for s in shallower.sheds() {
                assert!(deeper.sheds().contains(&s), "{deeper:?} does not shed {s}");
            }
        }
    }
}
