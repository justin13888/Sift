//! #24 — score D-79's ranking against a recorded relevance corpus.
//!
//! `mise run relevance-eval -- --root DIR CORPUS`
//!
//! Opens the installation container at `DIR` — quit the application first — and, for every
//! judgement in `CORPUS`, runs its query over every account and reports where the sought
//! message landed under D-79's merge and under D-55's list order, over the same result set.
//! `docs/product/relevance-corpus.md` is the protocol for recording one.
//!
//! **A figure, never a gate.** It exits zero however badly the ranking does; only a corpus it
//! cannot read or a container it cannot open is a failure. What counts as the "measured ranking
//! failure" D-5 and D-79 name is a judgement over these figures, not a threshold in this file.
//!
//! It prints the queries, because reading them beside their ranks is how a failure is
//! understood. They are the user's own, on their own terminal; nothing leaves the machine.

// Printing is what a diagnostic *is*.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use sift_app::relevance::{self, Figures};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, corpus) = match &args[..] {
        [flag, root, corpus] if flag == "--root" => (PathBuf::from(root), PathBuf::from(corpus)),
        _ => {
            eprintln!("usage: relevance-eval --root <container> <corpus>");
            return ExitCode::from(2);
        }
    };

    let text = match std::fs::read_to_string(&corpus) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {e}", corpus.display());
            return ExitCode::FAILURE;
        }
    };
    let judgements = match relevance::parse(&text) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("{}: {e}", corpus.display());
            return ExitCode::FAILURE;
        }
    };

    let mut app = sift_app::App::new();
    if let Err(e) = app.open_container(&root) {
        eprintln!("{}: {e}", root.display());
        return ExitCode::FAILURE;
    }
    let outcomes = match app.relevance_evaluate(&judgements) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("evaluation stopped: {e}");
            return ExitCode::FAILURE;
        }
    };

    let rank = |r: Option<usize>| r.map_or_else(|| "-".to_owned(), |r| r.to_string());
    println!("d79\tlist\treturned\taccount\tquery");
    for o in &outcomes {
        if o.resolved {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                rank(o.ranked),
                rank(o.listed),
                o.returned,
                o.judgement.account,
                o.judgement.query
            );
        } else {
            println!(
                "unresolved\t\t\t{}\t{}",
                o.judgement.account, o.judgement.query
            );
        }
    }

    let summary = relevance::summarize(&outcomes);
    println!();
    println!(
        "{} judgement(s), {} unresolved (the message or account is not in this container; not \
         counted below)",
        summary.judgements, summary.unresolved
    );
    let line = |name: &str, f: Figures| {
        println!(
            "{name:<6} MRR {:.3}  @1 {}  @3 {}  @10 {}  not returned {}",
            f.mean_reciprocal_rank, f.first, f.top_three, f.top_ten, f.missed
        );
    };
    line("d79", summary.ranked);
    line("list", summary.listed);
    println!(
        "   (bodies are not indexed by the sync path yet, so a body match is a snippet match; \
         provisional, and a figure rather than a gate)"
    );
    ExitCode::SUCCESS
}
