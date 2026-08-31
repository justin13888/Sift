//! Foundation — limits, identifiers, the state register, and the error states.
//!
//! The bottom layer of D-59's crate graph. It may depend on nothing above it, which in
//! practice means it depends on nothing at all: everything here is a definition the rest
//! of Sift agrees on rather than a mechanism.
//!
//! What lives here is the set of things that must have exactly one statement, because
//! several independent consumers assert them and a second copy is a copy that drifts.

pub mod condition;
pub mod identity;
pub mod limits;
pub mod state;
