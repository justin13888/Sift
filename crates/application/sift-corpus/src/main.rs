//! `sift-corpus` — write the scale corpus into an installation container.
//!
//! ```text
//! sift-corpus --root DIR [--seed N] [--divide N] [--accounts N]
//! ```
//!
//! `--root` is the container to fill; it must hold no accounts. On macOS the application's own
//! is `~/Library/Application Support/net.justinchung.sift`, and pointing this at it is what makes
//! the next launch NFR-1's cold start "with the store already populated from the scale corpus".
//! The account keys go to the platform credential store, as adding an account puts them.
//!
//! `--divide` keeps the proportions and shrinks the population, for a smoke run.

use sift_corpus::{Options, Progress, Shape, generate};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sift-corpus: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage: sift-corpus --root DIR [--seed N] [--divide N] [--accounts N]";

fn run(args: Vec<String>) -> Result<(), String> {
    let mut root: Option<PathBuf> = None;
    let mut options = Options::scale();
    let mut divisor = 1u64;
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))
        };
        match flag.as_str() {
            "--root" => root = Some(PathBuf::from(value()?)),
            "--seed" => options.seed = number(&flag, &value()?)?,
            "--divide" => divisor = number(&flag, &value()?)?,
            "--accounts" => {
                options.shape.accounts = u16::try_from(number(&flag, &value()?)?)
                    .map_err(|_| "--accounts is at most 65535".to_owned())?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    let root = root.ok_or_else(|| format!("--root is required\n{USAGE}"))?;
    options.shape = options.shape.divided(divisor).map_err(|e| e.to_string())?;
    let Shape {
        accounts,
        messages,
        inbox,
    } = options.shape;
    println!(
        "sift-corpus: {accounts} accounts, {messages} messages, a {inbox}-message inbox, seed {} -> {}",
        options.seed,
        root.display()
    );

    let started = Instant::now();
    let mut last_account = usize::MAX;
    let mut last_tenth = u64::MAX;
    let mut report_progress = |p: Progress| {
        let tenth = p.written * 10 / p.total.max(1);
        if p.account != last_account || tenth != last_tenth {
            println!(
                "  account {}: {}/{} ({:.0?})",
                p.account + 1,
                p.written,
                p.total,
                started.elapsed()
            );
            last_account = p.account;
            last_tenth = tenth;
        }
    };
    let report = generate(
        &root,
        &sift_credentials::store::Platform,
        &options,
        &mut report_progress,
    )
    .map_err(|e| e.to_string())?;

    for a in &report.accounts {
        println!(
            "{:<10} {:>8} messages  {:>7} in inbox  {:>2} folders  {:>12} bytes on disk",
            a.display_name,
            a.messages,
            a.inbox,
            a.folders,
            a.store_bytes + a.journal_bytes
        );
    }
    let messages = report.messages().max(1);
    println!(
        "total: {} messages, {} bytes on disk ({} per message), {} bytes of envelope index tokens \
         ({} per message), in {:.1?}",
        report.messages(),
        report.file_bytes(),
        report.file_bytes() / messages,
        report.index_token_bytes,
        report.index_token_bytes / messages,
        started.elapsed()
    );
    Ok(())
}

fn number(flag: &str, text: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{flag} takes a whole number, not `{text}`"))
}
