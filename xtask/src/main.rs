//! Build-time enforcement of what `docs/build/workspace.md` states as normative.
//!
//! `docs/build/README.md` declares that tree the one place the project's
//! "no implementation detail" rule stops: the crates, what may depend on what, and what
//! a gate passing means are decisions rather than descriptions. This is where those
//! decisions are checked.

mod arch;
mod deps;
mod header;
mod invariants;
mod layers;
mod meta;

use std::process::ExitCode;

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("arch") => arch::run(),
        Some("deps") => deps::run(std::env::args().any(|a| a == "--bless")),
        Some("header") => {
            if std::env::args().any(|a| a == "--bless") {
                header::bless()
            } else {
                header::check()
            }
        }
        Some("invariants") => invariants::run(),
        Some("coverage") => invariants::coverage(),
        Some("all") | None => arch::run()
            .and_then(|()| invariants::run())
            .and_then(|()| deps::run(false))
            .and_then(|()| header::check()),
        Some(other) => Err(format!(
            "unknown task `{other}`\n\n\
             arch        the crate graph of D-59: one-way edges, the ABI a leaf, unsafe in four places\n\
             invariants  the prohibitions docs/ states as code-review rules\n\
             deps        the third dependency gate: new edges reviewed rather than absorbed\n\
             header      D-60's generated C header, and the drift check over it\n\
             coverage    which crates each invariant rule currently covers\n\
             all         all of the above (default)"
        )),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("\n{report}\n");
            ExitCode::FAILURE
        }
    }
}
