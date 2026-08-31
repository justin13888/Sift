//! D-14, D-58, D-95, D-96 — network conditions and the policy tiers.
//!
//! # A purpose-built abstraction, and why building anything is justified here
//!
//! D-14 builds rather than adopts, and the justification is narrow: **no unified crate
//! exists**, and the scope is "roughly a file, not a project". The abstraction is one
//! condition type and one watch stream, over per-platform backends — a system path monitor
//! on macOS, NetworkManager over D-Bus on Linux.
//!
//! **R-10 is the standing risk**: under Flatpak, NetworkManager access may not be granted,
//! and the portal alternative supplies **neither the metered flag nor the link class** — the
//! two facts every tier decision below actually turns on.

pub mod condition;
pub mod tier;
