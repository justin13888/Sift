//! FR-33 item 9 — live invariant checks against a specific message.
//!
//! `docs/build/verification.md` and `docs/runtime/observability.md` both point at the same
//! economy: **items 4, 5 and 9 of the debug view are the same code path as NFR-40's
//! differential harness. "Build it once, use it twice."** This is that code.
//!
//! # Why a substring check is not enough
//!
//! The obvious test for "did a mutation-XSS vector survive?" is to look for `onerror` or
//! `alert` in the output. It is wrong in both directions.
//!
//! It reports **false positives** on payloads the sanitizer handled correctly: a vector that
//! ends up as `title="&lt;img src=x onerror=alert(1)&gt;"` contains all of those substrings
//! and is inert — it is escaped text in an attribute, and the engine will render it as
//! characters.
//!
//! It would also miss **false negatives**, because what matters is not what the bytes say
//! but *what the engine builds from them*. So this re-parses the output with the same
//! tree builder the engine's algorithm matches and inspects the resulting DOM. A construct
//! that is not in the tree cannot execute, whatever the bytes look like.

use crate::allowlist;
use html5ever::driver::ParseOpts;
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// Something present in the re-parsed tree that an invariant forbids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub invariant: &'static str,
    pub what: String,
}

/// Parse `html` and report every invariant it violates.
///
/// Empty means the document is clean **as the engine will build it**, which is the only
/// sense in which "clean" means anything.
#[must_use]
pub fn audit(html: &str) -> Vec<Violation> {
    let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);
    let mut found = Vec::new();
    inspect(&dom.document, &mut found);
    found
}

fn inspect(node: &Handle, found: &mut Vec<Violation>) {
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let tag = name.local.to_string().to_ascii_lowercase();

        // The tree builder's own scaffolding is not sender content.
        let scaffolding = matches!(tag.as_str(), "html" | "head" | "body");

        if !scaffolding && !allowlist::element_allowed(&tag) {
            found.push(Violation {
                invariant: invariant_for_element(&tag),
                what: format!("<{tag}>"),
            });
        }

        for attr in attrs.borrow().iter() {
            let attr_name = attr.name.local.to_string().to_ascii_lowercase();
            let value = attr.value.to_string();

            if attr_name.starts_with("on") {
                found.push(Violation {
                    invariant: "I1 no script",
                    what: format!("{tag}@{attr_name}"),
                });
            }

            // I2: an external scheme surviving in a fetching position. The internal scheme
            // is the only address a fetching position may carry after sanitization.
            if allowlist::is_fetching_attribute(&attr_name) {
                let internal = format!("{}:", sift_foundation::identifiers::INTERNAL_SCHEME);
                if !value.starts_with(&internal) {
                    found.push(Violation {
                        invariant: "I2 no implicit egress",
                        what: format!("{tag}@{attr_name}={value}"),
                    });
                }
            }

            if attr_name == "href" {
                let lowered = value.trim().to_ascii_lowercase();
                for scheme in ["javascript:", "vbscript:", "data:"] {
                    if lowered.starts_with(scheme) {
                        found.push(Violation {
                            invariant: "I1 no script",
                            what: format!("{tag}@href={scheme}…"),
                        });
                    }
                }
            }
        }
    }
    for child in node.children.borrow().iter() {
        inspect(child, found);
    }
}

fn invariant_for_element(tag: &str) -> &'static str {
    match tag {
        "script" | "noscript" | "svg" | "math" | "template" => "I1 no script",
        "iframe" | "frame" | "frameset" | "object" | "embed" | "applet" | "form" | "input"
        | "button" | "select" | "option" | "textarea" => "I3 no frames, plugins, or forms",
        "base" | "meta" | "link" | "title" | "style" => "I4 no document control",
        _ => "not on the element allowlist",
    }
}
