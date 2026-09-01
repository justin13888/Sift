//! D-75/D-76 crypto primitives, the page format, and D-43 content addressing.
//!
//! One of the four crates where `unsafe` is permitted — here for zeroing key material on
//! drop, which needs a volatile write so it is not elided as dead.

pub mod address;
pub mod derive;
pub mod page;
