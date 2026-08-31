//! D-51, D-85 — the intent model, the durable queue, and the pending overlay.
//!
//! Budgeted as a first-class subsystem rather than an adapter method, because it is the
//! only place in Sift where a bug can lose a user'''s mail.

pub mod intent;
pub mod queue;
