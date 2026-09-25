//! NFR-45 — the soak harness. NFR-44 — what the attribution that feeds it costs.
//!
//! # What runs
//!
//! **The assembled application**, through the same [`sift_session::Session`] a shell binds to,
//! under a synthetic load that does what a person does with a resident mail client over and
//! over: sync every account, list its messages, open and close every one of them, search, and
//! triage a message and take the triage back. Between rounds it idles on the application's own
//! wheel, so the periodic sync and flush D-25 schedules run exactly as they would in a shell.
//!
//! The accounts are D-65's recorded corpus. No network and no credentials, so a run on the
//! reference rig measures Sift rather than a provider's latency on the day — and so a run can
//! be left for three days with nothing that expires.
//!
//! # What is sampled
//!
//! On a fixed interval: D-16's footprint (`phys_footprint` or PSS, never RSS), D-24's live bytes
//! for every row of the partition, the declared caches' total, and the wheel's fires. One
//! [`sift_observe::soak::Row`] per sample, appended and flushed to a file as it is taken, so a
//! run killed on its third day leaves two days of evidence rather than none.
//!
//! # What the harness must not do to the thing it measures
//!
//! **It keeps no series in memory.** 72 hours at one sample a minute is four thousand rows, and
//! a vector of them would grow the footprint linearly for the whole run — a ratchet the harness
//! itself introduced, charged to the shell row, and large enough on its own to fail the gate.
//! The series is written out and read back from the file once the run ends.
//!
//! **It stops rather than carrying on when the load fails.** A load that silently stopped
//! doing anything is a flat line, and a flat line passes.

use core::time::Duration;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use sift_alloc::tagged;
use sift_observe::soak::{self, Report, Row, Verdict};
use sift_observe::wakeups::{Cause, Wakeups};
use sift_session::Session;
use sift_subsystem::Subsystem;

/// The search each round runs. A word the corpus contains, so the query does work rather than
/// returning early on nothing.
const QUERY: &str = "receipt";

/// The two gestures each round makes and takes back. A pair, so that the mailbox is where it
/// started after every round and nothing accumulates across three days but what Sift keeps.
const TRIAGE: [&str; 2] = ["message.mark-read", "message.mark-unread"];

/// Rows listed per account, as a shell's list does.
const LIST_LIMIT: u32 = 500;

/// The application under a synthetic load.
pub struct Load {
    session: Session,
    accounts: Vec<String>,
    wakeups: Wakeups,
    rounds: u64,
}

impl std::fmt::Debug for Load {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Load")
            .field("accounts", &self.accounts)
            .field("rounds", &self.rounds)
            .field("wakeups", &self.wakeups.total())
            .finish_non_exhaustive()
    }
}

impl Load {
    /// An application with `accounts` replayed accounts, writes enabled, and its wheel armed.
    ///
    /// # Errors
    /// An account could not be added.
    pub fn new(accounts: usize) -> Result<Self, String> {
        let mut session = Session::new(sift_app::App::new());
        let mut names = Vec::new();
        for n in 0..accounts.max(1) {
            let name = format!("soak{n}");
            let app = session.app_mut();
            tagged(Subsystem::Adapters, || app.add_replayed_account(&name))?;
            // Triage has to reach the provider, or every gesture is held in the queue for the
            // life of the run — an accumulation of the harness's making rather than Sift's.
            app.set_writes_enabled(&name, true)?;
            names.push(name);
        }
        session.app_mut().arm_periodic();
        Ok(Self {
            session,
            accounts: names,
            wakeups: Wakeups::new(),
            rounds: 0,
        })
    }

    /// One round of the synthetic load, across every account.
    ///
    /// Each step runs under the tag of the subsystem it exercises, as D-65's harness does, so
    /// the attribution reads the way a shell's gestures would make it read.
    ///
    /// # Errors
    /// Any step failed. The run stops on it: see the crate documentation.
    pub fn round(&mut self) -> Result<(), String> {
        for name in self.accounts.clone() {
            let app = self.session.app_mut();
            tagged(Subsystem::Sync, || app.sync(&name, 20))?;
            let rows = tagged(Subsystem::Store, || {
                app.account(&name)
                    .and_then(|a| sift_app::rows::message_rows(a, LIST_LIMIT))
            })?;
            if rows.is_empty() {
                // A round over an empty mailbox exercises nothing, and would soak as a pass.
                return Err(format!("`{name}` has no messages to read after a sync"));
            }
            for row in &rows {
                let document = tagged(Subsystem::Sanitize, || app.open_document(row.id, false))?;
                tagged(Subsystem::Sanitize, || app.close_document(&document.token));
            }
            tagged(Subsystem::Index, || app.search(QUERY, None, 50))?;

            app.selection = vec![rows[0].id];
            for action in TRIAGE {
                // The first round finds the message unread and the rest find it where the last
                // round left it, so which of the pair is available alternates. D-98 makes an
                // unavailable action absent, and invoking it would be an error of this
                // harness's making.
                if self.session.why_unavailable(action).is_some() {
                    continue;
                }
                let now = now_millis();
                let session = &mut self.session;
                tagged(Subsystem::Mutations, || {
                    session.invoke(action, None, false, now)?;
                    session.app_mut().flush(&name)
                })?;
            }
            self.session.app_mut().selection.clear();
        }
        self.rounds += 1;
        Ok(())
    }

    /// How long until the application's wheel next fires, if anything is armed.
    #[must_use]
    pub fn next_wake(&self) -> Option<Duration> {
        self.session.app().next_wake()
    }

    /// Fire the wheel, and count it as NFR-11 counts it: one wakeup, charged to every account
    /// whose work it served.
    ///
    /// # Errors
    /// An account's periodic work failed.
    pub fn tick(&mut self) -> Result<(), String> {
        let app = self.session.app_mut();
        let report = tagged(Subsystem::Scheduler, || app.tick());
        let names: BTreeSet<&String> = report
            .synced
            .iter()
            .chain(&report.flushed)
            .chain(&report.paused)
            .collect();
        let served: Vec<u128> = app
            .accounts()
            .filter(|(name, _)| names.contains(name))
            .map(|(_, a)| a.id.as_u128())
            .collect();
        self.wakeups
            .record(Cause::TimerFire, Subsystem::Scheduler, &served);
        match report.failures.first() {
            Some((name, why)) => Err(format!("the wheel's work for `{name}` failed: {why}")),
            None => Ok(()),
        }
    }

    /// One row of the series, taken now.
    #[must_use]
    pub fn sample(&self, elapsed: Duration) -> Row {
        Row {
            elapsed,
            footprint_bytes: sift_observe::footprint(),
            attributed: sift_alloc::snapshot(),
            declared_cache_bytes: declared_cache_bytes(),
            wakeups: self.wakeups.total(),
            rounds: self.rounds,
        }
    }

    /// Rounds completed.
    #[must_use]
    pub const fn rounds(&self) -> u64 {
        self.rounds
    }
}

impl Drop for Load {
    /// The scratch root the application made. A soak runs for days and the benchmark starts
    /// dozens of processes; neither should leave its mailboxes behind in the temporary
    /// directory.
    fn drop(&mut self) {
        if let Some(root) = self.session.app().root.clone() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// The sum of every declared cache's live bytes.
///
/// **Zero today, and that is a gap rather than a measurement.** memory-pressure.md requires
/// every cache to report itself, and no cache in the assembled application yet produces a
/// [`sift_observe::cache::Report`] — so the residual this run records is the whole footprint.
/// When one does, it is summed here, and the residual starts meaning what the specification
/// says it means.
#[must_use]
pub const fn declared_cache_bytes() -> u64 {
    0
}

/// How a soak is run.
#[derive(Debug, Clone)]
pub struct Soak {
    /// How long. NFR-45's verdict needs [`soak::MINIMUM_RUN`]; anything shorter is recorded
    /// and decomposed but can never pass.
    pub duration: Duration,
    /// The sampling interval.
    pub every: Duration,
    /// The interval between synthetic-load rounds.
    pub load_every: Duration,
    pub accounts: usize,
    /// Where the series is written. Created, or truncated.
    pub out: PathBuf,
}

/// Run a soak, writing the series as it goes, and return what the file holds once it ends.
///
/// # Errors
/// The series cannot be written or read back, or the load failed.
pub fn run(config: &Soak, mut progress: impl FnMut(&Row)) -> Result<Vec<Row>, String> {
    let mut load = Load::new(config.accounts)?;
    let mut file = std::fs::File::create(&config.out)
        .map_err(|e| format!("cannot create {}: {e}", config.out.display()))?;
    let write = |file: &mut std::fs::File, line: &str| {
        writeln!(file, "{line}")
            .and_then(|()| file.flush())
            .map_err(|e| format!("cannot write {}: {e}", config.out.display()))
    };
    write(&mut file, &Row::header())?;

    let start = Instant::now();
    let mut next_sample = Duration::ZERO;
    let mut next_round = Duration::ZERO;
    loop {
        if start.elapsed() >= next_round && start.elapsed() < config.duration {
            load.round()?;
            next_round = advance(next_round, config.load_every, start.elapsed());
        }
        if load.next_wake() == Some(Duration::ZERO) {
            load.tick()?;
        }
        let elapsed = start.elapsed();
        let finished = elapsed >= config.duration;
        if elapsed >= next_sample || finished {
            let row = load.sample(elapsed);
            write(&mut file, &row.to_line())?;
            progress(&row);
            next_sample = advance(next_sample, config.every, elapsed);
        }
        if finished {
            break;
        }

        // One timer for the whole process: the earliest of the next sample, the next round,
        // the wheel's next fire, and the end of the run. This is the platform timer a shell
        // arms for the wheel, not a per-account loop, and it is the only wait in the harness.
        let elapsed = start.elapsed();
        let mut until = next_sample.min(next_round).min(config.duration);
        if let Some(wake) = load.next_wake() {
            until = until.min(elapsed + wake);
        }
        std::thread::sleep(until.saturating_sub(elapsed)); // sift-allow: no-sleep-loops — the one process-wide timer a shell arms for the wheel's next fire (D-25)
    }
    drop(file);
    drop(load);

    let text = std::fs::read_to_string(&config.out)
        .map_err(|e| format!("cannot read back {}: {e}", config.out.display()))?;
    soak::parse(&text)
}

/// The next deadline on a fixed cadence, skipping any the process slept through rather than
/// firing them back to back — a machine that slept for an hour does not owe sixty samples.
fn advance(deadline: Duration, every: Duration, now: Duration) -> Duration {
    let every = every.max(Duration::from_millis(1));
    let mut next = deadline + every;
    while next <= now {
        next += every;
    }
    next
}

/// Read a series from a file.
///
/// # Errors
/// The file cannot be read or is not a series.
pub fn read(path: &Path) -> Result<Vec<Row>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    soak::parse(&text)
}

/// A report as lines a person reads.
#[must_use]
pub fn describe(report: &Report) -> Vec<String> {
    let mut out = vec![match report.verdict {
        Verdict::Passed { projected_growth } => format!(
            "PASS  NFR-45: projected growth {:.2}% over 14 days, within NFR-12's {:.0}%",
            projected_growth * 100.0,
            soak::PERMITTED_GROWTH * 100.0
        ),
        Verdict::Failed { projected_growth } => format!(
            "FAIL  NFR-45: projected growth {:.2}% over 14 days, beyond NFR-12's {:.0}%",
            projected_growth * 100.0,
            soak::PERMITTED_GROWTH * 100.0
        ),
        Verdict::TooShort { ran_for } => format!(
            "NO VERDICT  the run lasted {}, and NFR-45 needs {} — not a pass",
            hours(ran_for),
            hours(soak::MINIMUM_RUN)
        ),
        Verdict::NoUsableSamples => {
            "NO VERDICT  no footprint samples survived the warm-up — not a pass".to_owned()
        }
    }];
    match report.baseline_bytes {
        Some(b) => out.push(format!(
            "baseline footprint {b:.0} bytes (after the warm-up)"
        )),
        None => out.push("no baseline: nothing was sampled after the warm-up".to_owned()),
    }
    if report.missing_samples > 0 {
        out.push(format!(
            "{} sample(s) had no footprint and were left out of the fit",
            report.missing_samples
        ));
    }
    out.push(format!(
        "{:<14} {:>16} {:>10}",
        "series", "bytes / 14 days", "of baseline"
    ));
    for share in report
        .subsystems
        .iter()
        .chain(&report.unattributed)
        .chain(&report.residual)
    {
        out.push(format!(
            "{:<14} {:>16.0} {:>9.3}%",
            share.name,
            share.projected_bytes,
            share.projected_growth * 100.0
        ));
    }
    if let Some(rate) = report.wakeups_per_minute {
        out.push(format!("wheel fires {rate:.2} per minute"));
    }
    out
}

fn hours(d: Duration) -> String {
    format!("{:.2} h", d.as_secs_f64() / 3600.0)
}

/// NFR-44's budget: attribution overhead at or under 2% in release, counters-only.
pub const NFR44_BUDGET: f64 = 0.02;

/// Rounds each benchmark process runs before it starts timing, so the first sync's inserts and
/// the first render's cold caches are not charged to either allocator.
const WARM_ROUNDS: u32 = 5;

/// Run `rounds` rounds of the synthetic load after [`WARM_ROUNDS`], and time them.
///
/// Under whichever global allocator the calling binary installed, which is the whole point.
///
/// # Errors
/// The load failed.
pub fn workload(rounds: u32, accounts: usize) -> Result<Duration, String> {
    let mut load = Load::new(accounts)?;
    for _ in 0..WARM_ROUNDS {
        load.round()?;
    }
    let start = Instant::now();
    for _ in 0..rounds {
        load.round()?;
    }
    Ok(start.elapsed())
}

/// The entry point both binaries share for `workload <rounds> [accounts]`: print the timed
/// span in nanoseconds and nothing else, because the parent reads it.
#[must_use]
pub fn workload_main(args: &[String]) -> std::process::ExitCode {
    let parse = |i: usize, default: u32| -> Result<u32, String> {
        args.get(i).map_or(Ok(default), |s| {
            s.parse().map_err(|_| format!("`{s}` is not a count"))
        })
    };
    let result = parse(0, 100)
        .and_then(|rounds| Ok((rounds, parse(1, 3)?)))
        .and_then(|(rounds, accounts)| workload(rounds, accounts as usize));
    match result {
        Ok(span) => {
            println!("{}", span.as_nanos());
            std::process::ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("workload: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// What NFR-44's benchmark measured.
#[derive(Debug, Clone)]
pub struct Overhead {
    /// Each trial's timed span under the tagging allocator.
    pub tagged: Vec<Duration>,
    /// Each trial's timed span under the system allocator alone.
    pub baseline: Vec<Duration>,
}

impl Overhead {
    /// The median tagged span over the median baseline span, less one.
    ///
    /// Medians, because a trial that shared the machine with something else is an outlier in
    /// one direction and a mean would let it decide the verdict.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        median(&self.tagged).as_secs_f64() / median(&self.baseline).as_secs_f64() - 1.0
    }

    #[must_use]
    pub fn within_budget(&self) -> bool {
        self.fraction() <= NFR44_BUDGET
    }
}

fn median(spans: &[Duration]) -> Duration {
    let mut sorted = spans.to_vec();
    sorted.sort();
    sorted
        .get(sorted.len() / 2)
        .copied()
        .unwrap_or(Duration::ZERO)
}

/// NFR-44: the same workload under each allocator, in separate processes, alternated.
///
/// Separate processes because a global allocator is chosen per binary; alternated, and the
/// order swapped every trial, so a machine that warms or throttles over the run charges both
/// sides equally rather than whichever ran second.
///
/// # Errors
/// A benchmark process could not be started, failed, or printed something that is not a span.
pub fn overhead(
    tagged_exe: &Path,
    baseline_exe: &Path,
    rounds: u32,
    trials: u32,
    accounts: usize,
) -> Result<Overhead, String> {
    let once = |exe: &Path| -> Result<Duration, String> {
        let output = Command::new(exe)
            .args(["workload", &rounds.to_string(), &accounts.to_string()])
            .output()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        if !output.status.success() {
            return Err(format!(
                "{} failed: {}",
                exe.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.trim()
            .parse::<u64>()
            .map(Duration::from_nanos)
            .map_err(|_| format!("{} printed `{}`, not a span", exe.display(), text.trim()))
    };
    let mut result = Overhead {
        tagged: Vec::new(),
        baseline: Vec::new(),
    };
    for trial in 0..trials.max(1) {
        if trial % 2 == 0 {
            result.baseline.push(once(baseline_exe)?);
            result.tagged.push(once(tagged_exe)?);
        } else {
            result.tagged.push(once(tagged_exe)?);
            result.baseline.push(once(baseline_exe)?);
        }
    }
    Ok(result)
}

/// Parse `72h`, `30m`, `90s` or a bare number of seconds.
///
/// # Errors
/// The text is not one of those.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let (number, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => text.split_at(i),
        None => (text, "s"),
    };
    let n: u64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a duration like 72h, 30m or 90s"))?;
    let seconds = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("`{text}` is not a duration like 72h, 30m or 90s")),
    };
    Ok(Duration::from_secs(seconds))
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_the_way_a_person_writes_them() {
        assert_eq!(
            parse_duration("72h").unwrap(),
            Duration::from_secs(72 * 3600)
        );
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
        assert!(parse_duration("3d").is_err());
        assert!(parse_duration("h").is_err());
    }

    #[test]
    fn a_slept_through_deadline_is_skipped_rather_than_owed() {
        // A machine that slept an hour does not take sixty samples back to back on waking.
        let every = Duration::from_secs(60);
        let next = advance(Duration::ZERO, every, Duration::from_secs(3601));
        assert_eq!(next, Duration::from_secs(3660));
        assert_eq!(advance(Duration::ZERO, every, Duration::ZERO), every);
    }

    #[test]
    fn the_overhead_is_read_from_medians() {
        let ms = Duration::from_millis;
        let o = Overhead {
            tagged: vec![ms(102), ms(101), ms(500)],
            baseline: vec![ms(100), ms(100), ms(10)],
        };
        assert!((o.fraction() - 0.02).abs() < 1e-9, "{}", o.fraction());
        assert!(o.within_budget());
    }

    #[test]
    fn a_round_does_every_step_and_leaves_the_mailbox_where_it_found_it() {
        let mut load = Load::new(2).expect("replayed accounts");
        load.round().expect("first round");
        load.round().expect("second round");
        assert_eq!(load.rounds(), 2);
        for name in load.accounts.clone() {
            let account = load.session.app_mut().account(&name).unwrap();
            assert_eq!(
                account.queue.len(),
                0,
                "triage accumulated in `{name}`'s queue"
            );
        }
        assert_eq!(
            load.session.app().resources.live_documents(),
            0,
            "a document was opened and never closed"
        );
    }

    #[test]
    fn a_sample_carries_every_row_of_the_partition_and_a_footprint() {
        let load = Load::new(1).expect("a replayed account");
        let row = load.sample(Duration::from_secs(1));
        assert_eq!(row.attributed.len(), Subsystem::COUNT);
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(row.footprint_bytes.is_some_and(|b| b > 0));
        }
    }

    #[test]
    fn a_short_soak_writes_a_series_it_can_read_back_and_never_passes() {
        let out = std::env::temp_dir().join(format!("sift-soak-test-{}.tsv", std::process::id()));
        let config = Soak {
            duration: Duration::from_millis(600),
            every: Duration::from_millis(200),
            load_every: Duration::from_millis(100),
            accounts: 1,
            out: out.clone(),
        };
        let mut seen = 0;
        let rows = run(&config, |_| seen += 1).expect("the run");
        let _ = std::fs::remove_file(&out);
        assert_eq!(rows.len(), seen, "the file and the progress disagree");
        assert!(
            rows.len() >= 3,
            "{} samples in 600 ms at 200 ms",
            rows.len()
        );
        assert!(rows.last().unwrap().rounds >= 2, "the load did not run");
        let report = soak::report(&rows, Duration::ZERO);
        assert!(!report.verdict.is_pass(), "{:?}", report.verdict);
    }
}
