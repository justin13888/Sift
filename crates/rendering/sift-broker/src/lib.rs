//! D-91 — the resource broker.
//!
//! # Where it sits, and why that is the whole design
//!
//! The broker is **in the core, below the shell boundary**. Resource loads do not cross that
//! boundary: they arrive from the engine's scheme handler and the broker answers directly.
//! No shell is in the path.
//!
//! That rests on N-1 — the body view has no network capability, only the internal scheme is
//! registered, and every other scheme is rejected at the engine's policy layer. The
//! consequence is the structural advantage the whole content-blocking design depends on:
//!
//! > **Every candidate load arrives at one place as a question, before anything is in
//! > flight.**
//!
//! Sift owns every byte. Blocking is a decision function inside its own fetch path rather
//! than the interception of somebody else's — which is why the failure direction is a
//! message with missing images rather than a message that quietly fetched something.
//!
//! # The broker is the only component that fetches remote content
//!
//! Only for resources the user has explicitly allowed. **A fetch from anywhere else in the
//! core, for a message resource, is a defect** — and it would also falsify the completeness
//! of the privacy egress table, which is the document that claims to list every outbound
//! connection Sift makes.
//!
//! FR-42 is the case where that nearly happened: an unsubscribe request would be egress
//! from outside the broker, carrying a per-recipient token. Sift shows the destination and
//! issues nothing.

pub mod broker;
pub mod token;

pub use broker::{Answer, Broker, Request};
pub use token::{Address, Token};
