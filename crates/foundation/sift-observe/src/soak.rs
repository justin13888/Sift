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
use sift_subsystem::Subsystem;

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

    let points: Vec<(Duration, f64)> = samples
        .iter()
        .map(|s| (s.elapsed, s.footprint_bytes as f64))
        .collect();

    // The run's **own** post-warm-up baseline, so the gate does not depend on NFR-8's
    // placeholder.
    let Some(baseline) = baseline_after(&points, WARMUP) else {
        return Verdict::NoUsableSamples;
    };
    let Some(slope) = fitted_slope(&points, WARMUP) else {
        return Verdict::NoUsableSamples;
    };

    // Bytes per second, extrapolated across NFR-12's window and expressed as a fraction of
    // the baseline.
    let projected_growth = (slope * EXTRAPOLATION.as_secs_f64()) / baseline;

    if projected_growth <= PERMITTED_GROWTH {
        Verdict::Passed { projected_growth }
    } else {
        Verdict::Failed { projected_growth }
    }
}

/// The first positive value at or after `warmup` — the baseline a projection is a fraction of.
fn baseline_after(points: &[(Duration, f64)], warmup: Duration) -> Option<f64> {
    points
        .iter()
        .find(|(t, _)| *t >= warmup)
        .map(|(_, v)| *v)
        .filter(|v| *v > 0.0)
}

/// The least-squares slope, in units per second, over every point at or after `warmup`.
///
/// **The one fit.** The gate's verdict and each subsystem's share of it are computed by this
/// same function, so the decomposition a maintainer reads when the gate fires cannot be
/// arithmetic of a different kind from the number that fired it.
///
/// `None` where fewer than two points survive the warm-up, or all of them share one instant.
#[must_use]
pub fn fitted_slope(points: &[(Duration, f64)], warmup: Duration) -> Option<f64> {
    let usable: Vec<(f64, f64)> = points
        .iter()
        .filter(|(t, _)| *t >= warmup)
        .map(|(t, v)| (t.as_secs_f64(), *v))
        .collect();
    if usable.len() < 2 {
        return None;
    }
    let n = usable.len() as f64;
    let mean_x = usable.iter().map(|(x, _)| x).sum::<f64>() / n;
    let mean_y = usable.iter().map(|(_, y)| y).sum::<f64>() / n;
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (x, y) in &usable {
        let dx = x - mean_x;
        numerator += dx * (y - mean_y);
        denominator += dx * dx;
    }
    (denominator != 0.0).then(|| numerator / denominator)
}

/// One row of the series the soak harness writes, one per sampling interval.
///
/// **Keyed on [`Subsystem::name`], never on position.** The partition's names are stable and
/// its order is not promised, so a series is read back by the names in its own header — and a
/// series recorded under a name this build no longer has is refused rather than silently
/// folded into another row, because that is a discontinued history, not a column to skip.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub elapsed: Duration,
    /// D-16's footprint. `None` is a sample the platform refused, never a zero.
    pub footprint_bytes: Option<u64>,
    /// D-24's live bytes per subsystem, in [`Subsystem::ALL`] order. Signed, because a
    /// subsystem that frees what another allocated reads negative, and that is the number
    /// that says the tagging is wrong.
    pub attributed: [i64; Subsystem::COUNT],
    /// The sum of every declared cache's live bytes, which the residual is taken against.
    pub declared_cache_bytes: u64,
    /// Wheel fires since the run began. Cumulative, so a lost row loses no count.
    pub wakeups: u64,
    /// Synthetic-load rounds completed since the run began.
    pub rounds: u64,
}

/// The fixed columns, before one per subsystem.
const FIXED: [&str; 5] = ["elapsed_ms", "footprint", "declared", "wakeups", "rounds"];

/// The marker on a series' first line. Bumped only if a fixed column changes meaning.
pub const SERIES_VERSION: &str = "# sift-soak series v1";

impl Row {
    /// The column header, preceded by [`SERIES_VERSION`].
    #[must_use]
    pub fn header() -> String {
        let mut columns: Vec<&str> = FIXED.to_vec();
        columns.extend(Subsystem::ALL.iter().map(|s| s.name()));
        format!("{SERIES_VERSION}\n{}", columns.join("\t"))
    }

    /// One tab-separated line, in [`Row::header`]'s column order.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut fields = vec![
            self.elapsed.as_millis().to_string(),
            self.footprint_bytes
                .map_or_else(|| "-".to_owned(), |b| b.to_string()),
            self.declared_cache_bytes.to_string(),
            self.wakeups.to_string(),
            self.rounds.to_string(),
        ];
        fields.extend(self.attributed.iter().map(i64::to_string));
        fields.join("\t")
    }

    /// Footprint minus the sum of declared caches — what a maintainer chases when the gate
    /// fires. `None` where the footprint sample is missing.
    #[must_use]
    pub fn residual(&self) -> Option<i64> {
        let footprint = i64::try_from(self.footprint_bytes?).ok()?;
        Some(footprint - i64::try_from(self.declared_cache_bytes).unwrap_or(i64::MAX))
    }

    /// Footprint minus everything the tagging allocator accounts for: thread stacks, code
    /// pages, and anything allocated beneath the global allocator rather than through it.
    #[must_use]
    pub fn unattributed(&self) -> Option<i64> {
        let footprint = i64::try_from(self.footprint_bytes?).ok()?;
        Some(footprint - self.attributed.iter().sum::<i64>())
    }
}

/// Read a series back.
///
/// # Errors
/// The text is not a series, a line has the wrong number of fields or a field that is not a
/// number, or the header names a subsystem this build's partition does not have.
pub fn parse(text: &str) -> Result<Vec<Row>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    if lines.next() != Some(SERIES_VERSION) {
        return Err(format!(
            "not a soak series: the first line is not `{SERIES_VERSION}`"
        ));
    }
    let header: Vec<&str> = lines
        .next()
        .ok_or("the series has no column header")?
        .split('\t')
        .collect();
    if header.get(..FIXED.len()) != Some(&FIXED[..]) {
        return Err(format!("the fixed columns are not {FIXED:?}"));
    }
    let mut columns = Vec::new();
    for name in &header[FIXED.len()..] {
        let subsystem = Subsystem::from_name(name).ok_or_else(|| {
            format!(
                "the series was recorded with a subsystem `{name}` this partition does not \
                 have — a renamed or merged row discontinues its history"
            )
        })?;
        columns.push(subsystem.index());
    }

    let data: Vec<&str> = lines.collect();
    let mut rows = Vec::new();
    for (n, line) in data.iter().enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != header.len() {
            // A run killed mid-write leaves a torn last line, and a series being read while
            // its run is still going may end in one. Anywhere else a torn line is damage.
            if n + 1 == data.len() {
                break;
            }
            return Err(format!(
                "row {} has {} fields, not {}",
                n + 1,
                fields.len(),
                header.len()
            ));
        }
        let number = |i: usize| -> Result<u64, String> {
            fields[i]
                .parse()
                .map_err(|_| format!("row {}: `{}` is not a count", n + 1, fields[i]))
        };
        let mut attributed = [0i64; Subsystem::COUNT];
        for (k, index) in columns.iter().enumerate() {
            let field = fields[FIXED.len() + k];
            attributed[*index] = field
                .parse()
                .map_err(|_| format!("row {}: `{field}` is not a byte count", n + 1))?;
        }
        rows.push(Row {
            elapsed: Duration::from_millis(number(0)?),
            footprint_bytes: if fields[1] == "-" {
                None
            } else {
                Some(number(1)?)
            },
            declared_cache_bytes: number(2)?,
            wakeups: number(3)?,
            rounds: number(4)?,
            attributed,
        });
    }
    Ok(rows)
}

/// One series' share of the projected growth.
#[derive(Debug, Clone, PartialEq)]
pub struct Share {
    pub name: &'static str,
    /// Bytes the fitted slope adds over [`EXTRAPOLATION`].
    pub projected_bytes: f64,
    /// The same, as a fraction of the run's footprint baseline — **the gate's own unit**, so
    /// the subsystem shares and the unattributed share sum to the footprint's projection.
    pub projected_growth: f64,
}

/// What a finished or running soak concluded, and where the growth came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// NFR-45's verdict, from [`evaluate`] with its fixed warm-up. Never loosened by the
    /// decomposition's warm-up below.
    pub verdict: Verdict,
    /// The footprint baseline the shares are fractions of, where there is one.
    pub baseline_bytes: Option<f64>,
    /// One per subsystem, in [`Subsystem::ALL`] order.
    pub subsystems: Vec<Share>,
    /// Footprint minus everything attributed.
    pub unattributed: Option<Share>,
    /// Footprint minus declared caches.
    pub residual: Option<Share>,
    /// Wheel fires per minute across the post-warm-up span, where it has one.
    pub wakeups_per_minute: Option<f64>,
    /// Samples whose footprint the platform refused.
    pub missing_samples: usize,
}

/// Judge a series and decompose its slope by subsystem.
///
/// `warmup` applies to the decomposition only. The verdict always discards [`WARMUP`] and
/// always requires [`MINIMUM_RUN`], so an exploratory run can be decomposed early without any
/// way for that to produce a pass.
#[must_use]
pub fn report(rows: &[Row], warmup: Duration) -> Report {
    let samples: Vec<Sample> = rows
        .iter()
        .filter_map(|r| {
            Some(Sample {
                elapsed: r.elapsed,
                footprint_bytes: r.footprint_bytes?,
            })
        })
        .collect();
    let verdict = evaluate(&samples);

    let footprint: Vec<(Duration, f64)> = samples
        .iter()
        .map(|s| (s.elapsed, s.footprint_bytes as f64))
        .collect();
    let baseline = baseline_after(&footprint, warmup);

    let share = |name: &'static str, points: &[(Duration, f64)]| -> Option<Share> {
        let slope = fitted_slope(points, warmup)?;
        let projected_bytes = slope * EXTRAPOLATION.as_secs_f64();
        Some(Share {
            name,
            projected_bytes,
            projected_growth: baseline.map_or(f64::NAN, |b| projected_bytes / b),
        })
    };

    let subsystems = Subsystem::ALL
        .iter()
        .filter_map(|s| {
            // Only the rows the footprint was fitted over: a share fitted over rows the
            // footprint skipped is a different regression, and the shares would stop summing
            // to the footprint's projection.
            let points: Vec<(Duration, f64)> = rows
                .iter()
                .filter(|r| r.footprint_bytes.is_some())
                .map(|r| (r.elapsed, r.attributed[s.index()] as f64))
                .collect();
            share(s.name(), &points)
        })
        .collect();
    let derived = |f: fn(&Row) -> Option<i64>| -> Vec<(Duration, f64)> {
        rows.iter()
            .filter_map(|r| Some((r.elapsed, f(r)? as f64)))
            .collect()
    };

    let span: Vec<&Row> = rows.iter().filter(|r| r.elapsed >= warmup).collect();
    let wakeups_per_minute = match (span.first(), span.last()) {
        (Some(a), Some(b)) if b.elapsed > a.elapsed => Some(
            b.wakeups.saturating_sub(a.wakeups) as f64 / (b.elapsed - a.elapsed).as_secs_f64()
                * 60.0,
        ),
        _ => None,
    };

    Report {
        verdict,
        baseline_bytes: baseline,
        subsystems,
        unattributed: share("unattributed", &derived(Row::unattributed)),
        residual: share("residual", &derived(Row::residual)),
        wakeups_per_minute,
        missing_samples: rows.iter().filter(|r| r.footprint_bytes.is_none()).count(),
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

    fn row(hours: u64, footprint: u64, sync: i64, index: i64) -> Row {
        let mut attributed = [0i64; Subsystem::COUNT];
        attributed[Subsystem::Sync.index()] = sync;
        attributed[Subsystem::Index.index()] = index;
        Row {
            elapsed: Duration::from_secs(hours * 3600),
            footprint_bytes: Some(footprint),
            attributed,
            declared_cache_bytes: 1_000_000,
            wakeups: hours * 120,
            rounds: hours * 10,
        }
    }

    #[test]
    fn a_series_reads_back_as_written() {
        let rows = vec![
            row(0, 50_000_000, 10, -3),
            Row {
                footprint_bytes: None,
                ..row(1, 0, 11, 4)
            },
        ];
        let mut text = Row::header();
        for r in &rows {
            text.push('\n');
            text.push_str(&r.to_line());
        }
        assert_eq!(parse(&text).unwrap(), rows);
    }

    #[test]
    fn a_series_is_read_by_name_rather_than_by_position() {
        // The partition's order is not promised; its names are. A series written by a build
        // that listed the rows differently must still land each column in its own row.
        let text = format!(
            "{SERIES_VERSION}\nelapsed_ms\tfootprint\tdeclared\twakeups\trounds\tindex\tsync\n\
             3600000\t100\t0\t0\t0\t7\t9\n"
        );
        let rows = parse(&text).unwrap();
        assert_eq!(rows[0].attributed[Subsystem::Index.index()], 7);
        assert_eq!(rows[0].attributed[Subsystem::Sync.index()], 9);
    }

    #[test]
    fn a_series_under_a_renamed_subsystem_is_refused() {
        // Folding it into another row would be a discontinued history presented as a
        // continuous one.
        let text = format!(
            "{SERIES_VERSION}\nelapsed_ms\tfootprint\tdeclared\twakeups\trounds\tpanics\n0\t1\t0\t0\t0\t0\n"
        );
        assert!(parse(&text).unwrap_err().contains("panics"));
    }

    #[test]
    fn only_a_torn_last_line_is_forgiven() {
        let mut text = Row::header();
        text.push('\n');
        text.push_str(&row(0, 1, 0, 0).to_line());
        text.push_str("\n5\t6");
        assert_eq!(
            parse(&text).unwrap().len(),
            1,
            "a torn tail stopped the read"
        );

        let mut damaged = Row::header();
        damaged.push_str("\n5\t6\n");
        damaged.push_str(&row(0, 1, 0, 0).to_line());
        assert!(parse(&damaged).is_err(), "a torn middle was read past");
    }

    #[test]
    fn the_decomposition_names_the_subsystem_that_is_growing() {
        // The point of the per-subsystem series: when the gate fires, the share that moved
        // is the one to chase. Sync grows 2 MB an hour and carries the whole slope; Index is
        // flat and carries none of it.
        let rows: Vec<Row> = (0..=80)
            .map(|h| {
                row(
                    h,
                    100_000_000 + h * 2_000_000,
                    5_000_000 + i64::try_from(h).unwrap() * 2_000_000,
                    3_000_000,
                )
            })
            .collect();
        let r = report(&rows, WARMUP);
        assert!(!r.verdict.is_pass(), "{:?}", r.verdict);

        let sync = r.subsystems.iter().find(|s| s.name == "sync").unwrap();
        let index = r.subsystems.iter().find(|s| s.name == "index").unwrap();
        let Verdict::Failed { projected_growth } = r.verdict else {
            panic!("{:?}", r.verdict)
        };
        assert!((sync.projected_growth - projected_growth).abs() < 1e-9);
        assert!(index.projected_growth.abs() < 1e-9);
        let unattributed = r.unattributed.unwrap();
        assert!(unattributed.projected_growth.abs() < 1e-9);
        // Two wakeups a minute, as written.
        assert!((r.wakeups_per_minute.unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn the_shares_and_the_unattributed_remainder_sum_to_the_footprint() {
        // One fit, one unit. If they did not sum, the decomposition would be a different
        // arithmetic from the number that fired the gate.
        let rows: Vec<Row> = (0..=80)
            .map(|h| {
                let h_i = i64::try_from(h).unwrap();
                row(
                    h,
                    90_000_000 + h * 12_000,
                    1_000 + h_i * 7_000,
                    500 + h_i * 3_000,
                )
            })
            .collect();
        let r = report(&rows, WARMUP);
        let Verdict::Passed { projected_growth } = r.verdict else {
            panic!("{:?}", r.verdict)
        };
        let sum: f64 = r.subsystems.iter().map(|s| s.projected_growth).sum::<f64>()
            + r.unattributed.unwrap().projected_growth;
        assert!(
            (sum - projected_growth).abs() < 1e-9,
            "{sum} != {projected_growth}"
        );
    }

    #[test]
    fn a_short_warm_up_decomposes_early_but_never_passes() {
        // An exploratory run may be read after ten minutes. Nothing about that may turn into
        // a verdict: the gate keeps its own warm-up and its own minimum.
        let rows: Vec<Row> = (0..=10)
            .map(|m| Row {
                elapsed: Duration::from_secs(m * 60),
                ..row(0, 100_000_000, 0, 0)
            })
            .collect();
        let r = report(&rows, Duration::ZERO);
        assert!(matches!(r.verdict, Verdict::TooShort { .. }));
        assert_eq!(r.subsystems.len(), Subsystem::COUNT);
    }

    #[test]
    fn a_missing_footprint_is_skipped_rather_than_fitted_as_zero() {
        let mut rows: Vec<Row> = (0..=80).map(|h| row(h, 100_000_000, 0, 0)).collect();
        rows[40].footprint_bytes = None;
        let r = report(&rows, WARMUP);
        assert!(r.verdict.is_pass(), "{:?}", r.verdict);
        assert_eq!(r.missing_samples, 1);
    }

    #[test]
    fn the_shares_still_sum_to_the_footprint_when_a_sample_is_missing() {
        // A row with no footprint is skipped by the footprint fit; the shares must skip it
        // too, or they are a regression over different points. Its attributed bytes are an
        // outlier here, so fitting them would move Sync's share visibly.
        let mut rows: Vec<Row> = (0..=80)
            .map(|h| {
                let h_i = i64::try_from(h).unwrap();
                row(
                    h,
                    90_000_000 + h * 12_000,
                    1_000 + h_i * 7_000,
                    500 + h_i * 3_000,
                )
            })
            .collect();
        rows[70].footprint_bytes = None;
        rows[70].attributed[Subsystem::Sync.index()] = 50_000_000;
        let r = report(&rows, WARMUP);
        assert_eq!(r.missing_samples, 1);
        let Verdict::Passed { projected_growth } = r.verdict else {
            panic!("{:?}", r.verdict)
        };
        let sum: f64 = r.subsystems.iter().map(|s| s.projected_growth).sum::<f64>()
            + r.unattributed.unwrap().projected_growth;
        assert!(
            (sum - projected_growth).abs() < 1e-9,
            "{sum} != {projected_growth}"
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
