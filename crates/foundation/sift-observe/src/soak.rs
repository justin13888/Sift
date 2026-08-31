//! NFR-45 — the soak harness and its slope gate.
//!
//! # This is the only thing that will catch NFR-12
//!
//! R-2 calls no-footprint-ratchet-over-14-days **the hardest requirement in the set**: an
//! allocator, fragmentation and cache-discipline problem visible only in long soak tests. It
//! cannot be retrofitted, which is why it is P0 work rather than a phase item — and why it
//! also produces the baseline against which D-1 and D-2 are validated.
//!
//! # The slope is derived, not chosen
//!
//! D-64 requires every gate state its metric, its threshold, and **how the threshold was
//! arrived at**. This one is arithmetic rather than judgement: NFR-12 permits 5% growth over
//! 14 days, so the run passes when the **fitted slope, extrapolated over 14 days, stays
//! within 5% of that run's own post-first-hour baseline**.
//!
//! Two details carry weight. **Least-squares over the sampled series** rather than the
//! difference between endpoints, because two endpoints are two samples and a soak produces
//! thousands. And **the first hour is discarded**, because startup allocation is not a
//! ratchet and including it would fail every run.
//!
//! Expressed as a fraction of the run's own baseline rather than an absolute byte rate,
//! because NFR-8 and NFR-9 are placeholders under Q-12 and a gate keyed to a placeholder
//! measures the placeholder.

use core::time::Duration;

/// One sample of the footprint series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub elapsed: Duration,
    /// Measured as D-16 requires — `phys_footprint` or PSS, never RSS.
    pub footprint_bytes: u64,
}

/// The minimum a run must last for the gate to mean anything.
pub const MINIMUM_RUN: Duration = Duration::from_secs(72 * 60 * 60);

/// Discarded before fitting.
pub const WARMUP: Duration = Duration::from_secs(60 * 60);

/// What NFR-12 permits.
pub const PERMITTED_GROWTH: f64 = 0.05;
pub const EXTRAPOLATION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// What a run concluded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Passed {
        projected_growth: f64,
    },
    Failed {
        projected_growth: f64,
    },
    /// Not enough run to say anything.
    ///
    /// **Not a pass.** A short run that reported success would be the most dangerous
    /// possible output: it would look like evidence.
    TooShort {
        ran_for: Duration,
    },
    /// Every sample fell inside the discarded warm-up.
    NoUsableSamples,
}

impl Verdict {
    #[must_use]
    pub const fn is_pass(self) -> bool {
        matches!(self, Self::Passed { .. })
    }
}

/// Fit the series and judge it.
#[must_use]
pub fn evaluate(samples: &[Sample]) -> Verdict {
    let Some(last) = samples.last() else {
        return Verdict::TooShort {
            ran_for: Duration::ZERO,
        };
    };
    if last.elapsed < MINIMUM_RUN {
        return Verdict::TooShort {
            ran_for: last.elapsed,
        };
    }

    let usable: Vec<&Sample> = samples.iter().filter(|s| s.elapsed >= WARMUP).collect();
    if usable.len() < 2 {
        return Verdict::NoUsableSamples;
    }

    // The run's **own** post-warm-up baseline, so the gate does not depend on NFR-8's
    // placeholder.
    let baseline = usable[0].footprint_bytes as f64;
    if baseline <= 0.0 {
        return Verdict::NoUsableSamples;
    }

    let n = usable.len() as f64;
    let mean_x: f64 = usable.iter().map(|s| s.elapsed.as_secs_f64()).sum::<f64>() / n;
    let mean_y: f64 = usable.iter().map(|s| s.footprint_bytes as f64).sum::<f64>() / n;

    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for s in &usable {
        let dx = s.elapsed.as_secs_f64() - mean_x;
        numerator += dx * (s.footprint_bytes as f64 - mean_y);
        denominator += dx * dx;
    }
    if denominator == 0.0 {
        return Verdict::NoUsableSamples;
    }

    // Bytes per second, extrapolated across NFR-12's window and expressed as a fraction of
    // the baseline.
    let slope = numerator / denominator;
    let projected_growth = (slope * EXTRAPOLATION.as_secs_f64()) / baseline;

    if projected_growth <= PERMITTED_GROWTH {
        Verdict::Passed { projected_growth }
    } else {
        Verdict::Failed { projected_growth }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(hours: u64, bytes_at: impl Fn(u64) -> u64) -> Vec<Sample> {
        (0..=hours)
            .map(|h| Sample {
                elapsed: Duration::from_secs(h * 3600),
                footprint_bytes: bytes_at(h),
            })
            .collect()
    }

    #[test]
    fn a_flat_run_passes() {
        let v = evaluate(&series(80, |_| 100_000_000));
        assert!(v.is_pass(), "{v:?}");
    }

    #[test]
    fn a_ratcheting_run_fails() {
        // The failure NFR-12 exists to catch: a small, steady climb that nothing notices in
        // an hour and that doubles the footprint in a fortnight.
        let v = evaluate(&series(80, |h| 100_000_000 + h * 2_000_000));
        assert!(!v.is_pass(), "{v:?}");
        if let Verdict::Failed { projected_growth } = v {
            assert!(projected_growth > PERMITTED_GROWTH);
        }
    }

    #[test]
    fn a_run_that_grows_within_five_percent_over_a_fortnight_passes() {
        // 5% of 100 MB over 14 days is about 15 KB an hour.
        let v = evaluate(&series(80, |h| 100_000_000 + h * 14_000));
        assert!(v.is_pass(), "{v:?}");
    }

    #[test]
    fn a_short_run_is_not_a_pass() {
        // The most dangerous possible output would be a short run reporting success: it
        // would look like evidence.
        let v = evaluate(&series(10, |_| 100_000_000));
        assert!(matches!(v, Verdict::TooShort { .. }));
        assert!(!v.is_pass());
    }

    #[test]
    fn an_empty_run_is_not_a_pass() {
        assert!(!evaluate(&[]).is_pass());
    }

    #[test]
    fn startup_allocation_is_discarded_rather_than_fitted() {
        // Including the first hour would fail every run, because startup allocation is not a
        // ratchet.
        let mut samples = vec![
            Sample {
                elapsed: Duration::ZERO,
                footprint_bytes: 20_000_000,
            },
            Sample {
                elapsed: Duration::from_secs(1800),
                footprint_bytes: 90_000_000,
            },
        ];
        samples.extend(
            series(80, |_| 100_000_000)
                .into_iter()
                .filter(|s| s.elapsed >= WARMUP),
        );
        let v = evaluate(&samples);
        assert!(v.is_pass(), "the warm-up was fitted: {v:?}");
    }

    #[test]
    fn the_gate_is_relative_to_the_runs_own_baseline() {
        // Expressed as a fraction rather than an absolute byte rate, because NFR-8 and NFR-9
        // are placeholders under Q-12 and a gate keyed to a placeholder measures the
        // placeholder.
        let small = evaluate(&series(80, |h| 100_000_000 + h * 30_000_000));
        let large = evaluate(&series(80, |h| 1_000_000_000 + h * 300_000_000));
        assert_eq!(
            small.is_pass(),
            large.is_pass(),
            "the verdict depended on absolute size"
        );
    }

    #[test]
    fn a_run_whose_samples_are_all_warm_up_says_so() {
        let samples = [
            Sample {
                elapsed: Duration::from_secs(0),
                footprint_bytes: 1,
            },
            Sample {
                elapsed: MINIMUM_RUN,
                footprint_bytes: 1,
            },
        ];
        // The last sample is past the minimum but only one survives the warm-up filter.
        assert_eq!(
            evaluate(&samples[..1]),
            Verdict::TooShort {
                ran_for: Duration::ZERO
            }
        );
    }

    #[test]
    fn a_declining_run_passes() {
        // A footprint that shrinks is not a ratchet, and a gate that failed it would be
        // measuring change rather than growth.
        let v = evaluate(&series(80, |h| 200_000_000 - h * 100_000));
        assert!(v.is_pass(), "{v:?}");
    }
}
