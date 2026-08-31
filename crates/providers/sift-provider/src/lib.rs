//! D-12: the capability model and the adapter contract. Adapters see only this.
//!
//! D-59 puts each adapter in its own crate that cannot see the others, so **an adapter's
//! fitness is tested by whether it compiles against this crate alone**. That is what makes
//! D-12's no-special-casing rule mechanical rather than aspirational.

pub mod adapter;
pub mod capability;
pub mod oauth;
pub mod rfc5322;

pub mod transport;
