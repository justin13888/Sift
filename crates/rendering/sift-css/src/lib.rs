//! D-27 — the CSS cascade, and the dark transform built over it.
//!
//! **This is the largest single commitment in the rendering pipeline**, and D-27 says so:
//! a cascade is a style system in miniature, measured against a specification thousands of
//! pages long, running inside NFR-41's 30 ms shared with the sanitizer and the blocker.
//! R-9 doubts it fits. It should be budgeted as comparable in size to the sanitizer rather
//! than as a step in the transform pass.

pub mod cascade;
pub mod colour;
pub mod selector;
pub mod transform;
