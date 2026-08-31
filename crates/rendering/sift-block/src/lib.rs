//! D-10, D-29 — the filter engine as authority, and FR-29'''s heuristics.
//!
//! # The structural advantage this design has
//!
//! Because of N-1 the body view has no network capability, so **every candidate load
//! arrives at the resource broker as a question rather than as a request already in
//! flight.** Sift owns every byte, and blocking is a decision function inside its own fetch
//! path rather than the interception of somebody else'''s.
//!
//! That is why the failure direction here is a message with missing images rather than a
//! message that quietly fetched something.

pub mod engine;
pub mod heuristic;
pub mod link;
pub mod origin;
