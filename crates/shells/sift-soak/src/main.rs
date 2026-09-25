//! NFR-45's soak harness, and NFR-44's overhead benchmark.
//!
//! ```text
//! sift-soak run      [--for 72h] [--every 60s] [--load-every 10s] [--accounts 3] [--out PATH]
//!                    [--warmup 1h]
//! sift-soak report   PATH [--warmup 1h]
//! sift-soak overhead [--rounds 100] [--trials 9] [--accounts 3]
//! sift-soak workload ROUNDS [ACCOUNTS]
//! ```
//!
//! `run` and `report` exit 0 on NFR-45's pass, 1 on its failure, and 2 where there is no
//! verdict — a run shorter than 72 hours is recorded and decomposed, and is never a pass.
//! `overhead` exits 0 within NFR-44's 2%, 1 beyond it, and 2 from a build that is not release,
//! because NFR-44 is a property of the release build and a debug figure is not evidence of it.
//!
//! `--warmup` changes only the per-subsystem decomposition, so a run can be read early. The
//! verdict always discards the first hour, whatever it says.

/// D-24's tagging allocator, as a release build ships it. `sift-soak-baseline` is the same
/// program without it.
#[global_allocator]
static ALLOC: sift_alloc::Tagging<std::alloc::System> = sift_alloc::Tagging(std::alloc::System);

use core::time::Duration;
use sift_observe::soak::{self, Verdict};
use sift_soak::{Soak, describe, parse_duration};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "sift-soak run [--for 72h] [--every 60s] [--load-every 10s] [--accounts 3] \
                     [--out PATH] [--warmup 1h]\n\
                     sift-soak report PATH [--warmup 1h]\n\
                     sift-soak overhead [--rounds 100] [--trials 9] [--accounts 3]\n\
                     sift-soak workload ROUNDS [ACCOUNTS]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((verb, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(64);
    };
    let outcome = match verb.as_str() {
        "run" => run(rest),
        "report" => report(rest),
        "overhead" => overhead(rest),
        "workload" => return sift_soak::workload_main(rest),
        _ => Err(USAGE.to_owned()),
    };
    match outcome {
        Ok(code) => code,
        Err(why) => {
            eprintln!("sift-soak: {why}");
            ExitCode::from(64)
        }
    }
}

/// `--name value` pairs, and the positional arguments between them.
struct Flags<'a> {
    pairs: Vec<(&'a str, &'a str)>,
    positional: Vec<&'a str>,
}

impl<'a> Flags<'a> {
    fn parse(args: &'a [String], known: &[&str]) -> Result<Self, String> {
        let mut pairs = Vec::new();
        let mut positional = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if !known.contains(&name) {
                    return Err(format!("unknown flag --{name}\n{USAGE}"));
                }
                let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
                pairs.push((name, value.as_str()));
            } else {
                positional.push(a.as_str());
            }
        }
        Ok(Self { pairs, positional })
    }

    fn get(&self, name: &str) -> Option<&'a str> {
        self.pairs
            .iter()
            .rev()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
    }

    fn duration(&self, name: &str, default: Duration) -> Result<Duration, String> {
        self.get(name).map_or(Ok(default), parse_duration)
    }

    fn count(&self, name: &str, default: u32) -> Result<u32, String> {
        self.get(name).map_or(Ok(default), |v| {
            v.parse()
                .map_err(|_| format!("--{name}: `{v}` is not a count"))
        })
    }
}

fn verdict_code(verdict: Verdict) -> ExitCode {
    match verdict {
        Verdict::Passed { .. } => ExitCode::SUCCESS,
        Verdict::Failed { .. } => ExitCode::from(1),
        Verdict::TooShort { .. } | Verdict::NoUsableSamples => ExitCode::from(2),
    }
}

fn print_report(rows: &[soak::Row], warmup: Duration) -> ExitCode {
    let report = soak::report(rows, warmup);
    for line in describe(&report) {
        println!("{line}");
    }
    verdict_code(report.verdict)
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let flags = Flags::parse(
        args,
        &["for", "every", "load-every", "accounts", "out", "warmup"],
    )?;
    let out = flags.get("out").map_or_else(
        || {
            let started = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            PathBuf::from(format!("sift-soak-{started}.tsv"))
        },
        PathBuf::from,
    );
    let config = Soak {
        duration: flags.duration("for", soak::MINIMUM_RUN)?,
        every: flags.duration("every", Duration::from_secs(60))?,
        load_every: flags.duration("load-every", Duration::from_secs(10))?,
        accounts: flags.count("accounts", 3)? as usize,
        out,
    };
    let warmup = flags.duration("warmup", soak::WARMUP)?;
    println!(
        "soaking for {} s, sampling every {} s, a load round every {} s, {} account(s)",
        config.duration.as_secs(),
        config.every.as_secs(),
        config.load_every.as_secs(),
        config.accounts
    );
    println!("series: {}", config.out.display());
    if config.duration < soak::MINIMUM_RUN {
        println!(
            "shorter than NFR-45's 72 hours: this run is recorded and decomposed, and cannot pass"
        );
    }
    let rows = sift_soak::run(&config, |row| {
        println!(
            "{:>9.0}s  footprint {:>12}  attributed {:>12}  rounds {:>6}  fires {:>5}",
            row.elapsed.as_secs_f64(),
            row.footprint_bytes
                .map_or_else(|| "-".to_owned(), |b| b.to_string()),
            row.attributed.iter().sum::<i64>(),
            row.rounds,
            row.wakeups
        );
    })?;
    Ok(print_report(&rows, warmup))
}

fn report(args: &[String]) -> Result<ExitCode, String> {
    let flags = Flags::parse(args, &["warmup"])?;
    let [path] = flags.positional[..] else {
        return Err(format!("report takes one series\n{USAGE}"));
    };
    let rows = sift_soak::read(std::path::Path::new(path))?;
    Ok(print_report(&rows, flags.duration("warmup", soak::WARMUP)?))
}

fn overhead(args: &[String]) -> Result<ExitCode, String> {
    let flags = Flags::parse(args, &["rounds", "trials", "accounts"])?;
    let tagged = std::env::current_exe().map_err(|e| format!("cannot find this binary: {e}"))?;
    let baseline = tagged.with_file_name(format!(
        "sift-soak-baseline{}",
        std::env::consts::EXE_SUFFIX
    ));
    if !baseline.exists() {
        return Err(format!(
            "{} is missing; build both binaries with `cargo build --release -p sift-soak`",
            baseline.display()
        ));
    }
    let rounds = flags.count("rounds", 100)?;
    let trials = flags.count("trials", 9)?;
    let accounts = flags.count("accounts", 3)? as usize;
    println!("{trials} trial(s) of {rounds} round(s) each, {accounts} account(s), alternated");

    let measured = sift_soak::overhead(&tagged, &baseline, rounds, trials, accounts)?;
    for (i, (t, b)) in measured.tagged.iter().zip(&measured.baseline).enumerate() {
        println!(
            "trial {:>2}  tagged {:>10.3} ms  baseline {:>10.3} ms  {:>+7.2}%",
            i + 1,
            t.as_secs_f64() * 1e3,
            b.as_secs_f64() * 1e3,
            (t.as_secs_f64() / b.as_secs_f64() - 1.0) * 100.0
        );
    }
    let fraction = measured.fraction();
    if cfg!(debug_assertions) {
        println!(
            "NO VERDICT  {:+.2}% from a debug build — NFR-44 is a property of release; \
             rerun with `cargo build --release -p sift-soak`",
            fraction * 100.0
        );
        return Ok(ExitCode::from(2));
    }
    if measured.within_budget() {
        println!(
            "PASS  NFR-44: attribution costs {:+.2}% (ratio of medians), within {:.0}%",
            fraction * 100.0,
            sift_soak::NFR44_BUDGET * 100.0
        );
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "FAIL  NFR-44: attribution costs {:+.2}% (ratio of medians), beyond {:.0}%",
            fraction * 100.0,
            sift_soak::NFR44_BUDGET * 100.0
        );
        Ok(ExitCode::from(1))
    }
}
