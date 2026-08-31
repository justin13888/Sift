//! D-18, D-56 — view models, selection, formatting, merge and diff.
//!
//! Platform-neutral **from the first commit, while only one shell exists**. It exposes no
//! type, lifecycle or callback shape that mirrors a specific toolkit'''s widget model, and
//! holds no state that only a live shell could reconstruct.

pub mod action;
