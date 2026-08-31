//! The Linux shell's testable half.
//!
//! # A second full UI, not a recompile
//!
//! D-9 concedes this plainly: narrowing to two platforms makes the *rendering* one engine
//! family — WKWebView and WebKitGTK are the Cocoa and GTK ports of WebKit, sharing WebCore,
//! the CSS engine and the HTML parser — but it does **not** make the shell one shell. D-1
//! costs roughly twice the UI work, and R-12 names this among the three commitments weakest
//! by cost against argument.
//!
//! Q-6 asked whether this shell gets built natively at all and resolved **yes**, settled
//! early by D-2 rather than deferred to the end of a phase: a web UI would hold an engine
//! resident for chrome, and the whole idle-footprint argument is that the engine is a
//! *disposable* resource.
//!
//! # What this shell may and may not reach
//!
//! D-17 lets a Rust shell link the presentation crate directly rather than going through
//! generated declarations — but the constraint travels with the privilege:
//!
//! > **The GTK shell MUST NOT reach presentation-layer API the ABI never exposes.** Anything
//! > a shell is permitted to use must be expressible across the C ABI, and **a capability
//! > existing for one shell and not the other is a defect in this boundary, not a Linux
//! > feature.**
//!
//! [`abi_surface`] is where that is checked rather than trusted.

pub mod abi_surface;
pub mod shell;
