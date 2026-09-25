//! NFR-44's control: the soak's workload under the system allocator and nothing else.
//!
//! It does one thing — `workload <rounds> [accounts]` — and exists only so that `sift-soak
//! overhead` has something to divide by. Everything it runs is the library `sift-soak` runs;
//! the global allocator below is the only difference, which is what makes the ratio a
//! measurement of D-24 rather than of two programs.

#[global_allocator]
static ALLOC: std::alloc::System = std::alloc::System;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((verb, rest)) if verb == "workload" => sift_soak::workload_main(rest),
        _ => {
            eprintln!("sift-soak-baseline workload <rounds> [accounts]");
            std::process::ExitCode::from(64)
        }
    }
}
