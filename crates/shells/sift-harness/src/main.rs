//! D-65 — the command-driven shell harness.
//!
//! `docs/build/verification.md` lists three harnesses that must exist before the subsystems
//! they test, and this is the third: **FR-24's testability claim, taken seriously.** The
//! requirement is that every action be reachable from the keyboard without a pointer, and
//! the consequence nobody states until here is that a keyboard-complete application is one
//! that can be driven **without UI automation at all**.
//!
//! > "The harness and the action registry are one thing."
//!
//! So this binary is not a mock of the application. It links the same presentation layer a
//! shell links, invokes the same actions by the same stable identifiers a shell binds keys
//! to, and reads the same registers. What it does not have is a window.
//!
//! # What it is for
//!
//! Driving the whole application in a test or by hand: create accounts, ingest messages,
//! issue intents, watch the queue and the overlay, and inspect the state a shell would
//! render. Every answer it prints is a state or a value, never a sentence Sift composed for
//! a user — D-56 keeps prose on the shell's side of the boundary, and this shell's prose is
//! deliberately terse and diagnostic rather than a rendering of the product.

mod account;
mod command;

/// The harness installs the tagging allocator, because an attribution surface that is only
/// ever exercised by its own unit tests is one nobody finds out is wrong.
///
/// D-20 chooses mimalloc for the purge control NFR-12 needs; this wraps whatever allocator
/// is beneath it, so swapping that in later changes nothing here.
#[global_allocator]
static ALLOC: sift_alloc::Tagging<std::alloc::System> = sift_alloc::Tagging(std::alloc::System);

use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut app = sift_app::App::new();

    if args.is_empty() {
        return repl(&mut app);
    }
    // A script: one command per argument, so a test can drive a whole session in one
    // invocation and assert on the transcript.
    for line in args {
        run_and_print(&mut app, &line);
    }
    Ok(())
}

fn repl(app: &mut sift_app::App) -> io::Result<()> {
    let stdin = io::stdin();
    println!("sift-harness — `help` for commands, `quit` to leave");
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim();
        if line == "quit" || line == "exit" {
            return Ok(());
        }
        if !line.is_empty() {
            run_and_print(app, line);
        }
    }
}

fn run_and_print(app: &mut sift_app::App, line: &str) {
    match command::run(app, line) {
        Ok(output) => {
            for l in output {
                println!("{l}");
            }
        }
        // A failure is reported as what failed, not as an apology. The harness is a
        // diagnostic surface and its output is read by a person looking for a cause.
        Err(e) => println!("error: {e}"),
    }
}
