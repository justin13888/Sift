//! Time, and why there are two kinds of it here.
//!
//! D-25 makes the monotonic clock normative for deadlines. The failure it prevents is
//! specific: **a wall-clock correction would fire every deadline armed behind the new
//! reading at once** — a machine that syncs its clock forward by an hour would, on a
//! wall-clock wheel, immediately fire an hour of scheduled work, against every account, in
//! one burst. That is NFR-38's retry storm arriving from the clock instead of from the
//! network.
//!
//! But some intervals genuinely need to align to wall-clock instants — NFR-37's aligned
//! polls exist so that several accounts' work lands on the same fire rather than
//! scattering across the hour. So:
//!
//! > **Alignment is computed against the wall clock and the resulting deadline is armed on
//! > the monotonic one**, so a correction moves the *next* alignment rather than firing
//! > every deadline behind it.

use core::time::Duration;

/// A point on the monotonic clock, as nanoseconds since an arbitrary origin.
///
/// Deliberately not `std::time::Instant`: the wheel has to be testable against a clock a
/// test controls, and `Instant` cannot be constructed at a chosen value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Monotonic(pub u64);

impl Monotonic {
    pub const ORIGIN: Self = Self(0);

    #[must_use]
    pub const fn after(self, d: Duration) -> Self {
        Self(self.0.saturating_add(d.as_nanos() as u64))
    }

    #[must_use]
    pub fn since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

/// The clock the wheel reads.
///
/// A trait so that the wheel's behaviour under a clock jump, a suspend, and a wake can be
/// tested rather than reasoned about — D-25 names all three as the edge cases that make
/// this "a real data structure with real edge cases".
pub trait Clock: Send + Sync {
    /// Monotonic now. Never goes backwards, and does not advance across system sleep on
    /// either target platform — which is exactly why a wake is an explicit event below
    /// rather than something the wheel discovers.
    fn monotonic(&self) -> Monotonic;

    /// Wall-clock now, as milliseconds since the Unix epoch. Used **only** to compute
    /// alignment, never to arm a deadline.
    fn wall_millis(&self) -> u64;
}

/// The system clock.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn monotonic(&self) -> Monotonic {
        // std::time::Instant is monotonic on both target platforms. The origin is process
        // start, which is all the wheel needs.
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
        let origin = ORIGIN.get_or_init(std::time::Instant::now);
        Monotonic(u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }

    fn wall_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// The next wall-clock instant that is a whole multiple of `period`, expressed as a
/// monotonic deadline.
///
/// This is the rule stated above, as a function. Two accounts computing an aligned poll
/// with the same period land on the same instant without coordinating, which is what makes
/// them share one wakeup instead of taking two.
#[must_use]
pub fn next_aligned(clock: &dyn Clock, period: Duration) -> Monotonic {
    let period_ms = period.as_millis().max(1) as u64;
    let wall = clock.wall_millis();
    let until_next = period_ms - (wall % period_ms);
    clock.monotonic().after(Duration::from_millis(until_next))
}

#[cfg(test)]
pub(crate) mod test_clock {
    use super::{Clock, Monotonic};
    use core::time::Duration;
    use std::sync::Mutex;

    /// A clock a test drives by hand, so a jump, a suspend and a wake are things that can
    /// be *made* to happen rather than waited for.
    #[derive(Debug, Default)]
    pub(crate) struct TestClock {
        state: Mutex<(u64, u64)>,
    }

    impl TestClock {
        pub(crate) fn new(monotonic_ns: u64, wall_ms: u64) -> Self {
            Self {
                state: Mutex::new((monotonic_ns, wall_ms)),
            }
        }

        pub(crate) fn advance(&self, d: Duration) {
            let mut s = self.state.lock().expect("not poisoned");
            s.0 += d.as_nanos() as u64;
            s.1 += d.as_millis() as u64;
        }

        /// Move the wall clock without moving the monotonic one — a time correction.
        pub(crate) fn correct_wall_clock(&self, by_ms: i64) {
            let mut s = self.state.lock().expect("not poisoned");
            s.1 = s.1.saturating_add_signed(by_ms);
        }

        /// Time passes with the monotonic clock frozen — a system suspend.
        pub(crate) fn suspend(&self, wall_ms: u64) {
            let mut s = self.state.lock().expect("not poisoned");
            s.1 += wall_ms;
        }
    }

    impl Clock for TestClock {
        fn monotonic(&self) -> Monotonic {
            Monotonic(self.state.lock().expect("not poisoned").0)
        }
        fn wall_millis(&self) -> u64 {
            self.state.lock().expect("not poisoned").1
        }
    }
}
