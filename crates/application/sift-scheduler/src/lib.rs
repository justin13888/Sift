//! D-25: the timing wheel. All periodic work goes through it.
//!
//! Per-account and per-folder sleep loops are prohibited, and `cargo xtask invariants`
//! enforces that as a build failure everywhere but here.

pub mod clock;
pub mod wheel;
