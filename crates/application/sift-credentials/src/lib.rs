//! D-88, D-36, NFR-23 — the credential broker.
//!
//! # One place, so "which code can read a token" has one answer
//!
//! NFR-23 requires credential access be brokered in a single place so that question is
//! reviewable. **The broker sits in the core; no shell code reaches it.**
//!
//! Credentials exist **only** in the OS credential store — never in a database, never in a
//! log, never in a crash dump. The reasons are mundane and that is the point: databases get
//! backed up, synced, copied for debugging, and attached to bug reports.

pub mod flow;
pub mod oauth;
pub mod refresh;
pub mod store;
