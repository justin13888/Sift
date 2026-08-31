//! D-6, D-21, D-32, D-74 — the per-account store and journal.
//!
//! One of the four crates where `unsafe` is permitted: D-21 names SQLite, and the engine
//! reaches Sift through C.

pub mod account;
pub mod schema;
