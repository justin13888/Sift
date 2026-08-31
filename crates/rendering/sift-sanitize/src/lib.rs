//! D-26 — a spec-conformant tree builder with Sift's own policy over it, and invariants
//! I1 through I10.
//!
//! # Why not an existing sanitizer crate
//!
//! D-26 rejected them for a specific reason rather than on taste: they make user-submitted
//! markup safe to embed in a page, and they have **no notion of rewriting every fetching
//! position to an internal scheme (I2)** and **no CSS pipeline at all**. Those two are most
//! of what this has to do.
//!
//! What is *not* rejected is the tree builder. I8 sets the bar — the parser must implement
//! the same algorithm the rendering engine does — and NFR-40's dual-parser divergence test
//! needs the same property. So the builder is `html5ever` and the policy is Sift's.
//!
//! # I8 is the one that will bite
//!
//! > "Mutation XSS lives in the serialize-then-reparse round trip. **A sanitizer that
//! > satisfies I1 through I7 and fails I8 is exploitable.** Prioritize it accordingly."
//!
//! [`check_parse_stability`] is that check, and it runs over the fidelity corpus in CI as
//! NFR-40 method 3.
//!
//! # Which invariants have a backstop, and which do not
//!
//! | Invariant | Backstop |
//! |---|---|
//! | I1 no script | JavaScript disabled at the engine level (NFR-20, D-50) |
//! | I2 no implicit egress | the body view has no network capability (N-1) |
//! | I5 containment | the body renders in its own document and data store (NFR-25) |
//!
//! **I6 through I10 have none**, and of those I8 carries the most risk. That is where
//! review attention belongs.

pub mod allowlist;
pub mod audit;
pub mod document;
pub mod sanitize;

pub use sanitize::{SanitizeError, Sanitized, sanitize};

/// Whether a document's own stylesheets declare a dark mode — FR-32's input.
///
/// Here rather than beside the transform because it must be read off the **sanitized**
/// document: what the sender wrote is not necessarily what survives, and honouring a
/// declaration the reader will never see would leave the message untransformed for a reason
/// nothing in it explains.
#[must_use]
pub fn transform_input_declares_dark_mode(sanitized_html: &str) -> bool {
    transform_input_declares_dark_mode_in(&document::read(sanitized_html).stylesheet)
}

/// The same question, asked of a stylesheet already read back.
#[must_use]
pub fn transform_input_declares_dark_mode_in(stylesheet: &str) -> bool {
    let compact = stylesheet.to_ascii_lowercase().replace(' ', "");
    compact.contains("prefers-color-scheme:dark") || compact.contains("color-scheme:dark")
}
