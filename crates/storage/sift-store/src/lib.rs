//! D-6, D-21, D-32, D-74 — the per-account store and journal.
//!
//! One of the four crates where `unsafe` is permitted: D-21 names SQLite, and the engine
//! reaches Sift through C.

pub mod account;
pub mod flags;

// D-65's first harness. Absent from release: the flag register declares it off, and it MUST
// NOT be compilable into a shipped binary.
#[cfg(any(test, feature = "fault-injection"))]
pub mod fault;
pub mod schema;
