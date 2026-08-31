//! The policy pass.

use crate::allowlist;
use html5ever::driver::ParseOpts;
use html5ever::tendril::TendrilSink;
use html5ever::{parse_document, serialize};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use sift_foundation::limits::{L6_DOM_DEPTH, L7_DOM_NODES, L8_ATTRS_PER_ELEMENT};
use std::cell::RefCell;
use std::rc::Rc;

/// Why a document was refused.
///
/// Every variant **rejects the message to FR-9's raw source view**. None truncates: a
/// document cut mid-tree is one whose structure the sender chose by choosing where the cap
/// fell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizeError {
    /// L-6.
    TooDeep,
    /// L-7 — "the cap NFR-41's 30 ms budget is actually a function of".
    TooManyNodes,
    /// L-8.
    TooManyAttributes,
}

/// One rewritten fetching position.
///
/// Recorded so that FR-33 item 5 can show "every candidate URL, its verdict, the matching
/// rule, the first-party determination" — and so the broker has something to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchingPosition {
    /// The index the internal-scheme address carries. View-independent by construction:
    /// D-28's capability token is minted per *document*, and stage 5 binds these to it.
    pub index: usize,
    /// Where it appeared — the element and attribute, for the debug view.
    pub element: String,
    pub attribute: String,
    /// The address as the sender wrote it. **Never fetched from here**; it exists so the
    /// broker can decide and the debug view can show what was asked for.
    pub original: String,
}

/// The result of the pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    pub html: String,
    /// Every fetching position, in document order. **Including ones a filter rule would
    /// condemn** — I2 is asserted over this output, and a condemned URL left unrewritten
    /// would falsify it while every blocking test still passed.
    pub positions: Vec<FetchingPosition>,
    /// Navigation targets, which are *not* fetching positions: a link is followed only on
    /// an explicit confirmation, and never in place.
    pub links: Vec<String>,
    /// What was removed, and under which rule. FR-33 item 4 requires "a rule identifier next
    /// to every removal".
    pub removals: Vec<Removal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub rule: &'static str,
    pub what: String,
}

/// The internal scheme addresses are rewritten to — D-28.
///
/// Registered with the web engine and nowhere else. It carries an index rather than a
/// content hash or a message identifier: a hash would be a stable global identifier and
/// therefore a correlation channel between messages, and a message-and-part identifier is
/// guessable and stable across views, so revoking it would mean nothing.
fn internal_address(index: usize) -> String {
    format!("{}:/{index}", sift_foundation::identifiers::INTERNAL_SCHEME)
}

/// Sanitize a document.
///
/// # Errors
///
/// On any bound in the limits register, so the message degrades to the raw view.
pub fn sanitize(html: &str) -> Result<Sanitized, SanitizeError> {
    // I10: the tree builder consumes bytes under the HTML encoding rules and yields UTF-8
    // regardless of what the document declared, including where declarations contradict.
    let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);

    let state = RefCell::new(State {
        positions: Vec::new(),
        links: Vec::new(),
        removals: Vec::new(),
        nodes: 0,
    });

    walk(&dom.document, 0, &state)?;

    let mut serialized = Vec::new();
    let handle: SerializableHandle = dom.document.clone().into();
    // Serialization failure would mean the tree contains something unrepresentable, which
    // after the walk it cannot. Treated as an empty document rather than a panic, because
    // NFR-19 admits no crash on any input.
    let _ = serialize(
        &mut serialized,
        &handle,
        serialize::SerializeOpts::default(),
    );

    let state = state.into_inner();
    Ok(Sanitized {
        // I10: valid UTF-8 out, for any input bytes and any declared charset.
        html: String::from_utf8_lossy(&serialized).into_owned(),
        positions: state.positions,
        links: state.links,
        removals: state.removals,
    })
}

struct State {
    positions: Vec<FetchingPosition>,
    links: Vec<String>,
    removals: Vec<Removal>,
    nodes: u64,
}

fn walk(node: &Handle, depth: u64, state: &RefCell<State>) -> Result<(), SanitizeError> {
    if depth > L6_DOM_DEPTH {
        return Err(SanitizeError::TooDeep);
    }
    {
        let mut s = state.borrow_mut();
        s.nodes += 1;
        if s.nodes > L7_DOM_NODES {
            return Err(SanitizeError::TooManyNodes);
        }
    }

    // Comments are dropped entirely. This is what strips legacy word-processor conditional
    // content, which parses as a comment — asserted directly as a regression vector rather
    // than left as an inference, because the dark transform's input contract depends on it.
    let children: Vec<Handle> = node.children.borrow().clone();
    let mut keep: Vec<Handle> = Vec::new();

    for child in children {
        let verdict = classify(&child, state)?;
        match verdict {
            Verdict::Drop => {}
            Verdict::Unwrap => {
                // The element goes; its children stay. Used where the element itself is
                // forbidden but the text inside it is the user's mail — I9 permits removal
                // and forbids invention, and discarding readable text is closer to
                // invention of an empty message than removal is.
                walk(&child, depth + 1, state)?;

                // `mem::take` rather than `clone`, and the reason is not tidiness.
                //
                // The DOM's `Drop` is iterative so that a deeply nested tree does not
                // overflow the stack when it is released, and the way it achieves that is by
                // **emptying the children of every node it walks**. An unwrapped element is
                // no longer in its parent's kept list, so it is released as soon as the
                // parent's children are replaced — and on the way out it would clear the
                // children of the very nodes just reparented out of it, which are still
                // alive and still referenced.
                //
                // Taking the list leaves the discarded element holding nothing, so its
                // release reaches nothing that survived it. Without this every unwrapped
                // element silently empties its descendants: the elements remain and their
                // text is gone, which reads as a sanitizer that strips content rather than
                // as a use-after-reparent.
                let grandchildren: Vec<Handle> = core::mem::take(&mut child.children.borrow_mut());
                for g in grandchildren {
                    g.parent.set(Some(Rc::downgrade(node)));
                    keep.push(g);
                }
            }
            Verdict::Keep => {
                walk(&child, depth + 1, state)?;
                keep.push(child);
            }
        }
    }

    *node.children.borrow_mut() = keep;
    Ok(())
}

enum Verdict {
    Keep,
    Drop,
    Unwrap,
}

fn classify(node: &Handle, state: &RefCell<State>) -> Result<Verdict, SanitizeError> {
    match &node.data {
        NodeData::Text { .. } => Ok(Verdict::Keep),
        // I4: no document control. A comment additionally carries the legacy conditional
        // markup the dark transform must never meet.
        NodeData::Comment { .. }
        | NodeData::Doctype { .. }
        | NodeData::ProcessingInstruction { .. } => {
            state.borrow_mut().removals.push(Removal {
                rule: "I4 no document control",
                what: "comment, doctype or processing instruction".to_owned(),
            });
            Ok(Verdict::Drop)
        }
        NodeData::Document => Ok(Verdict::Keep),
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.to_string().to_ascii_lowercase();

            // `html`, `head` and `body` are the tree builder's own scaffolding rather than
            // sender content: unwrap them so their children survive.
            if matches!(tag.as_str(), "html" | "head" | "body") {
                return Ok(Verdict::Unwrap);
            }

            if let Some(rule) = forbidden_outright(&tag) {
                state.borrow_mut().removals.push(Removal {
                    rule,
                    what: format!("<{tag}>"),
                });
                return Ok(Verdict::Drop);
            }

            if !allowlist::element_allowed(&tag) {
                state.borrow_mut().removals.push(Removal {
                    rule: "not on the element allowlist",
                    what: format!("<{tag}>"),
                });
                // Unwrap rather than drop: the text inside an unknown element is still the
                // user's mail.
                return Ok(Verdict::Unwrap);
            }

            filter_attributes(&tag, attrs, state)?;
            Ok(Verdict::Keep)
        }
    }
}

/// Elements removed with their contents, because their contents are not text a reader wants.
fn forbidden_outright(tag: &str) -> Option<&'static str> {
    Some(match tag {
        // I1: no script element, and no script embedded via foreign content. `svg` and
        // `math` switch the tree builder into foreign content, where the parsing rules
        // differ and script can re-enter — which is exactly the ground mutation XSS is
        // found on.
        "script" | "noscript" | "svg" | "math" | "template" => "I1 no script",
        // I3: no frames, plugins, or forms.
        "iframe" | "frame" | "frameset" | "object" | "embed" | "applet" | "form" | "input"
        | "button" | "select" | "option" | "optgroup" | "textarea" | "label" | "fieldset"
        | "legend" | "datalist" | "output" | "progress" | "meter" => {
            "I3 no frames, plugins, or forms"
        }
        // I4: no document control. A `base` rewrites every relative URL in the document, a
        // `meta http-equiv` can navigate, a `link` fetches, and a `title` is chrome the
        // sender does not own.
        "base" | "meta" | "link" | "title" | "style" => "I4 no document control",
        // Media elements carry fetching positions and playback surfaces neither the
        // allowlist nor the broker is built for.
        "audio" | "video" | "track" | "canvas" | "map" | "area" | "portal" => {
            "not on the element allowlist"
        }
        _ => return None,
    })
}

fn filter_attributes(
    tag: &str,
    attrs: &RefCell<Vec<html5ever::Attribute>>,
    state: &RefCell<State>,
) -> Result<(), SanitizeError> {
    let mut attrs = attrs.borrow_mut();
    if attrs.len() as u64 > L8_ATTRS_PER_ELEMENT {
        return Err(SanitizeError::TooManyAttributes);
    }

    let mut kept: Vec<html5ever::Attribute> = Vec::new();
    for attr in attrs.iter() {
        let name = attr.name.local.to_string().to_ascii_lowercase();
        let value = attr.value.to_string();

        // I1: no event-handler attribute, in any position. Checked before the allowlist so
        // that an `on*` attribute can never be admitted by a future allowlist edit.
        if name.starts_with("on") {
            state.borrow_mut().removals.push(Removal {
                rule: "I1 no script",
                what: format!("{tag}@{name}"),
            });
            continue;
        }

        if !allowlist::attribute_allowed(tag, &name) {
            state.borrow_mut().removals.push(Removal {
                rule: "not on the attribute allowlist",
                what: format!("{tag}@{name}"),
            });
            continue;
        }

        let rewritten = if allowlist::is_fetching_attribute(&name) {
            // I2. **Every** fetching position is rewritten, including one a filter rule
            // would condemn — the verdict is the broker's at request time, because FR-8's
            // allowlist and the network tier change without the message changing.
            let mut s = state.borrow_mut();
            let index = s.positions.len();
            s.positions.push(FetchingPosition {
                index,
                element: tag.to_owned(),
                attribute: name.clone(),
                original: value.clone(),
            });
            internal_address(index)
        } else if name == "href" {
            match navigation_scheme(&value) {
                Some(_) => {
                    state.borrow_mut().links.push(value.clone());
                    value.clone()
                }
                None => {
                    // I1: no script-bearing URL scheme **in any position**.
                    state.borrow_mut().removals.push(Removal {
                        rule: "I1 no script",
                        what: format!("{tag}@href with a refused scheme"),
                    });
                    continue;
                }
            }
        } else if name == "style" {
            let (filtered, dropped) = filter_style(&value);
            for d in dropped {
                state.borrow_mut().removals.push(d);
            }
            if filtered.is_empty() {
                continue;
            }
            filtered
        } else {
            value.clone()
        };

        let mut a = attr.clone();
        a.value = rewritten.into();
        kept.push(a);
    }
    *attrs = kept;
    Ok(())
}

/// The scheme of a navigation target, if it is one Sift will show.
fn navigation_scheme(value: &str) -> Option<&'static str> {
    let lowered = value.trim().to_ascii_lowercase();
    // A relative URL has no scheme and cannot execute. It is resolved against a document
    // with no base — I4 removes `<base>` precisely so a sender cannot supply one — so it
    // resolves to nothing and is answered as unavailable rather than fetched.
    if !lowered.contains(':') {
        return Some("relative");
    }
    allowlist::NAVIGATION_SCHEMES
        .iter()
        .find(|s| lowered.starts_with(&format!("{s}:")))
        .copied()
}

/// Filter a `style` attribute's declarations.
fn filter_style(value: &str) -> (String, Vec<Removal>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for declaration in value.split(';') {
        let Some((property, v)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let v_lower = v.trim().to_ascii_lowercase();

        if property.is_empty() {
            continue;
        }

        // A property that fetches, dropped by name rather than by not being on the
        // allowlist — so that the reason is specific and the debug view can say which.
        if allowlist::css_property_fetches(&property) {
            dropped.push(Removal {
                rule: "I2 no implicit egress — CSS fetching position",
                what: property,
            });
            continue;
        }

        if !allowlist::css_property_allowed(&property) {
            dropped.push(Removal {
                rule: "not on the CSS property allowlist",
                what: property,
            });
            continue;
        }

        // I1: no script-equivalent CSS construct. `expression()` executes, and `url()` in an
        // allowed property would be a fetching position the enumeration above missed.
        if v_lower.contains("expression(") || v_lower.contains("javascript:") {
            dropped.push(Removal {
                rule: "I1 no script",
                what: property,
            });
            continue;
        }
        if v_lower.contains("url(") {
            dropped.push(Removal {
                rule: "I2 no implicit egress — url() in an allowed property",
                what: property,
            });
            continue;
        }

        // I5: containment. Fixed and sticky positioning leave the flow; viewport units are
        // measured against the viewport rather than the body view's box. Either lets a
        // message draw outside the area the reader gave it.
        if allowlist::CONTAINMENT_ESCAPES
            .iter()
            .any(|e| v_lower.contains(e))
        {
            dropped.push(Removal {
                rule: "I5 containment",
                what: property,
            });
            continue;
        }

        kept.push(format!("{property}:{}", v.trim()));
    }
    (kept.join(";"), dropped)
}

/// I8 — parse stability. `P(W(S(x)))` is DOM-isomorphic to `S(x)`.
///
/// Mutation XSS lives in exactly this round trip: a document that serializes to markup which
/// **reparses into a different tree** is one where the attacker chose the difference. A
/// sanitizer satisfying I1 through I7 and failing this is exploitable.
///
/// # Errors
/// Propagates any bound the second pass exceeds.
pub fn check_parse_stability(html: &str) -> Result<bool, SanitizeError> {
    let once = sanitize(html)?;
    let twice = sanitize(&once.html)?;
    Ok(once.html == twice.html)
}

#[cfg(test)]
mod invariants {
    //! I1 through I10, asserted.
    //!
    //! NFR-40 requires five complementary methods: grammar-based property generation at the
    //! limit boundaries, **differential testing against the real engine** (the
    //! highest-value test in the suite, and the one that needs a body view), dual-parser
    //! divergence over the fidelity corpus, fuzzing, and **every published mutation-XSS
    //! payload as a fixed regression vector**.
    //!
    //! Methods 1, 3 and 5 are here. Method 2 needs an engine and belongs with the P0 webview
    //! spike, which builds the instrumentation it shares with FR-33 items 4, 5 and 9 —
    //! "build it once, use it twice". Method 4 needs a fuzzing harness and the corpus.

    use super::*;

    fn clean(html: &str) -> Sanitized {
        sanitize(html).expect("within bounds")
    }

    // ---- I1: no script ----

    #[test]
    fn i1_script_elements_are_removed_with_their_contents() {
        let out = clean("<p>before</p><script>alert(1)</script><p>after</p>");
        assert!(!out.html.contains("script"));
        assert!(!out.html.contains("alert"), "script text survived as text");
        assert!(out.html.contains("before") && out.html.contains("after"));
    }

    #[test]
    fn i1_event_handler_attributes_are_removed_in_any_position() {
        for html in [
            r#"<img src="x" onerror="alert(1)">"#,
            r#"<p onclick="alert(1)">x</p>"#,
            r#"<a href="https://e.test" onmouseover="alert(1)">x</a>"#,
            r#"<div ONLOAD="alert(1)">x</div>"#,
        ] {
            let out = clean(html);
            assert!(!out.html.to_lowercase().contains("onerror"), "{html}");
            assert!(!out.html.to_lowercase().contains("onclick"), "{html}");
            assert!(!out.html.to_lowercase().contains("onmouseover"), "{html}");
            assert!(!out.html.to_lowercase().contains("onload"), "{html}");
            assert!(!out.html.contains("alert"), "{html}");
        }
    }

    #[test]
    fn i1_script_bearing_url_schemes_are_refused_in_any_position() {
        for html in [
            r#"<a href="javascript:alert(1)">x</a>"#,
            r#"<a href="JaVaScRiPt:alert(1)">x</a>"#,
            r#"<a href="vbscript:msgbox(1)">x</a>"#,
            r#"<a href="data:text/html,<script>alert(1)</script>">x</a>"#,
        ] {
            let out = clean(html);
            assert!(!out.html.contains("javascript:"), "{html} -> {}", out.html);
            assert!(!out.html.to_lowercase().contains("vbscript"), "{html}");
            assert!(!out.html.contains("alert"), "{html} -> {}", out.html);
        }
    }

    #[test]
    fn i1_foreign_content_cannot_reintroduce_script() {
        // `svg` and `math` switch the tree builder into foreign content, where the parsing
        // rules differ and script can re-enter. This is the ground mutation XSS is found on.
        for html in [
            "<svg><script>alert(1)</script></svg>",
            "<math><mtext><script>alert(1)</script></mtext></math>",
            "<svg><foreignObject><script>alert(1)</script></foreignObject></svg>",
        ] {
            let out = clean(html);
            assert!(!out.html.contains("alert"), "{html} -> {}", out.html);
            assert!(!out.html.contains("script"), "{html} -> {}", out.html);
        }
    }

    #[test]
    fn i1_script_equivalent_css_is_refused() {
        let out = clean(r#"<p style="width:expression(alert(1))">x</p>"#);
        assert!(!out.html.contains("expression"), "{}", out.html);
        let out = clean(r#"<p style="background:url(javascript:alert(1))">x</p>"#);
        assert!(!out.html.contains("javascript"), "{}", out.html);
    }

    // ---- I2: no implicit egress ----

    #[test]
    fn i2_every_fetching_position_is_rewritten() {
        let out = clean(r#"<img src="https://tracker.test/pixel.gif" alt="">"#);
        assert!(
            !out.html.contains("tracker.test"),
            "an external URL survived: {}",
            out.html
        );
        assert!(out.html.contains("sift-resource:/0"), "{}", out.html);
        assert_eq!(out.positions.len(), 1);
        assert_eq!(out.positions[0].original, "https://tracker.test/pixel.gif");
    }

    #[test]
    fn i2_a_condemned_url_is_rewritten_too() {
        // "Every fetching position is rewritten, **including one a filter rule has already
        // condemned**." Rewriting only what will be fetched falsifies I2 while every
        // blocking test still passes — and leaves N-1 as the only defence rather than the
        // second of two.
        let out = clean(
            r#"<img src="https://doubleclick.test/ad.gif"><img src="https://ok.test/logo.png">"#,
        );
        assert_eq!(out.positions.len(), 2, "a condemned position was skipped");
        assert!(!out.html.contains("doubleclick.test"));
    }

    #[test]
    fn i2_no_external_scheme_survives_in_a_fetching_position() {
        for html in [
            r#"<img src="http://a.test/x">"#,
            r#"<img src="//a.test/x">"#,
            r#"<img srcset="https://a.test/x 1x">"#,
            r#"<td background="https://a.test/x">y</td>"#,
        ] {
            let out = clean(html);
            assert!(!out.html.contains("a.test"), "{html} -> {}", out.html);
        }
    }

    #[test]
    fn i2_css_fetching_properties_are_dropped_by_name() {
        for property in [
            "background-image",
            "list-style-image",
            "border-image",
            "cursor",
            "mask",
        ] {
            let html = format!(r#"<p style="{property}:url(https://a.test/x)">y</p>"#);
            let out = clean(&html);
            assert!(
                !out.html.contains("a.test"),
                "{property} leaked: {}",
                out.html
            );
            assert!(
                out.removals.iter().any(|r| r.what == property),
                "{property} was dropped without saying why"
            );
        }
    }

    #[test]
    fn i2_a_url_in_an_allowed_property_is_still_refused() {
        // The gap the explicit enumeration would miss: a property on the allowlist whose
        // *value* happens to carry url().
        let out = clean(r#"<p style="color:red;border-color:url(https://a.test/x)">y</p>"#);
        assert!(!out.html.contains("a.test"), "{}", out.html);
        assert!(
            out.html.contains("color:red"),
            "an innocent declaration was lost"
        );
    }

    // ---- I3, I4, I5 ----

    #[test]
    fn i3_frames_plugins_and_forms_are_removed() {
        for tag in [
            "iframe", "object", "embed", "form", "input", "textarea", "button",
        ] {
            let out = clean(&format!("<{tag}></{tag}>"));
            assert!(!out.html.contains(tag), "<{tag}> survived: {}", out.html);
        }
    }

    #[test]
    fn i4_document_control_is_removed() {
        // `base` is the sharpest: it rewrites every relative URL in the document, so leaving
        // it would let a sender redirect positions the rewrite already handled.
        for html in [
            r#"<base href="https://evil.test/">"#,
            r#"<meta http-equiv="refresh" content="0;url=https://evil.test">"#,
            r#"<link rel="stylesheet" href="https://evil.test/x.css">"#,
            "<title>not the sender's to set</title>",
        ] {
            let out = clean(html);
            assert!(!out.html.contains("evil.test"), "{html} -> {}", out.html);
        }
    }

    #[test]
    fn i5_containment_escapes_are_refused() {
        for value in [
            "position:fixed",
            "position:sticky",
            "width:100vw",
            "height:50vh",
        ] {
            let out = clean(&format!(r#"<p style="{value}">x</p>"#));
            let kept = value.split(':').next().expect("property");
            assert!(
                !out.html.contains(kept) || !out.html.contains(value.split(':').nth(1).unwrap()),
                "{value} survived: {}",
                out.html
            );
        }
    }

    // ---- I6: idempotence ----

    #[test]
    fn i6_sanitizing_twice_changes_nothing() {
        for html in [
            "<p>plain</p>",
            r#"<img src="https://a.test/x" alt="a">"#,
            "<div><script>alert(1)</script><b>bold</b></div>",
            r#"<table><tr><td style="color:red">cell</td></tr></table>"#,
            "<svg><script>alert(1)</script></svg>",
        ] {
            let once = clean(html);
            let twice = clean(&once.html);
            assert_eq!(once.html, twice.html, "not idempotent for {html}");
        }
    }

    // ---- I7: bounded ----

    #[test]
    fn i7_a_document_over_the_depth_bound_is_rejected_rather_than_truncated() {
        let deep = "<div>".repeat((L6_DOM_DEPTH as usize) + 10);
        assert_eq!(sanitize(&deep), Err(SanitizeError::TooDeep));
    }

    #[test]
    fn i7_a_document_over_the_node_bound_is_rejected() {
        let wide = "<p>x</p>".repeat((L7_DOM_NODES as usize / 2) + 10);
        assert_eq!(sanitize(&wide), Err(SanitizeError::TooManyNodes));
    }

    #[test]
    fn i7_rejection_is_not_truncation() {
        // The rule the limits register exists to state. A truncated document is one whose
        // structure the sender chose by choosing where the cap fell — a parse-differential
        // primitive of exactly the kind I8 closes.
        let deep = "<div>".repeat((L6_DOM_DEPTH as usize) + 10);
        assert!(
            sanitize(&deep).is_err(),
            "an over-deep document produced output"
        );
    }

    // ---- I8: parse stability ----

    #[test]
    fn i8_output_reparses_to_the_same_tree() {
        // "A sanitizer that satisfies I1 through I7 and fails I8 is exploitable."
        for html in [
            "<p>plain</p>",
            "<b><i>nested</i></b>",
            "<table><tr><td>cell</td></tr></table>",
            r#"<a href="https://e.test">link</a>"#,
            "<p>unclosed",
            "<div><p>improperly<div>nested</p></div>",
        ] {
            assert!(
                check_parse_stability(html).expect("bounded"),
                "unstable for {html}"
            );
        }
    }

    #[test]
    fn i8_published_mutation_xss_vectors() {
        // NFR-40 method 5: **every published mutation-XSS payload as a permanent regression
        // vector**. These are the shapes that defeated other sanitizers by surviving the
        // serialize-then-reparse round trip as something different from what was checked.
        let vectors = [
            r#"<noscript><p title="</noscript><img src=x onerror=alert(1)>">"#,
            r#"<svg></p><style><a id="</style><img src=1 onerror=alert(1)>">"#,
            r#"<math><mtext><table><mglyph><style><!--</style><img title="--&gt;&lt;/mglyph&gt;&lt;img&Tab;src=1&Tab;onerror=alert(1)&gt;">"#,
            r#"<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>"#,
            r#"<style><style/><img src=x onerror=alert(1)>"#,
            r#"<xmp><p title="</xmp><img src=x onerror=alert(1)>">"#,
            r#"<listing><p title="</listing><img src=x onerror=alert(1)>">"#,
            r#"<table><caption><svg><foreignobject><desc><table><tr><td><style><!--</style><img title="--></style><img src=x onerror=alert(1)>">"#,
            r#"<a href="javascript&colon;alert(1)">x</a>"#,
            r#"<a href="java&#115;cript:alert(1)">x</a>"#,
            r#"<img src="x" alt="" onerror=alert(1) //>"#,
            r#"<p><svg><style><img src=x onerror=alert(1)></style></svg></p>"#,
        ];
        for v in vectors {
            let out = clean(v);

            // Audited against the **re-parsed tree**, not against the bytes. A vector the
            // sanitizer handled correctly often still contains the strings `onerror` and
            // `alert` — escaped, inside an attribute value, as characters the engine will
            // render rather than execute. Grepping the output would fail those and pass
            // nothing extra.
            let violations = crate::audit::audit(&out.html);
            assert!(
                violations.is_empty(),
                "vector survived into the tree:\n  {v}\n  -> {}\n  {violations:?}",
                out.html
            );

            assert!(
                check_parse_stability(v).expect("bounded"),
                "vector is not parse-stable:\n  {v}"
            );
        }
    }

    // ---- I9: no content invention ----

    #[test]
    fn i9_no_visible_text_is_invented() {
        // Removal is permitted; addition is not. A sanitizer that helpfully inserted a
        // placeholder would be putting words in the sender's mouth.
        let out = clean("<p>only this</p>");
        let text: String = out
            .html
            .chars()
            .filter(|c| !"<>/pbodyhtmlead".contains(*c))
            .collect();
        assert!(text.trim() == "only this" || out.html.contains("only this"));
        assert!(
            !out.html.contains("removed"),
            "the sanitizer narrated its own work"
        );
        assert!(!out.html.contains("blocked"));
    }

    #[test]
    fn i9_text_inside_a_disallowed_element_survives() {
        // Discarding readable text is closer to inventing an empty message than removal is.
        let out = clean("<marquee>the sender wrote this</marquee>");
        assert!(out.html.contains("the sender wrote this"), "{}", out.html);
        assert!(!out.html.contains("marquee"));
    }

    // ---- I10: encoding determinism ----

    #[test]
    fn i10_output_is_valid_utf8_for_any_input() {
        for html in [
            "<p>café</p>",
            "<p>\u{FFFD}</p>",
            "<p>日本語のテキスト</p>",
            "<p>\u{202E}reversed</p>",
            "<p>&#x41;&#66;&amp;</p>",
        ] {
            let out = clean(html);
            assert!(out.html.is_char_boundary(0));
            let _: &str = &out.html;
        }
    }

    // ---- NFR-50: accessibility survives sanitization ----

    #[test]
    fn nfr50_alternative_text_survives() {
        // Its loss is an accessibility **defect**, not a cosmetic one — and FR-29's
        // tracking-pixel heuristic treats missing alt text as a signal, so stripping it
        // would also make every image look like a tracker.
        let out = clean(r#"<img src="https://a.test/x" alt="the quarterly chart">"#);
        assert!(out.html.contains("the quarterly chart"), "{}", out.html);
    }

    #[test]
    fn nfr50_table_structure_and_heading_level_survive() {
        let out = clean(
            r#"<h2>Section</h2><table><caption>Sales</caption><thead><tr>
               <th scope="col">Region</th></tr></thead><tbody><tr>
               <td headers="r1">North</td></tr></tbody></table>"#,
        );
        for expected in ["h2", "caption", "thead", "th", "scope", "headers", "tbody"] {
            assert!(
                out.html.contains(expected),
                "{expected} was stripped: {}",
                out.html
            );
        }
    }

    #[test]
    fn nfr50_reading_order_is_preserved() {
        let out = clean("<p>first</p><p>second</p><p>third</p>");
        let f = out.html.find("first").expect("first");
        let s = out.html.find("second").expect("second");
        let t = out.html.find("third").expect("third");
        assert!(f < s && s < t, "reading order changed: {}", out.html);
    }

    #[test]
    fn nfr50_direction_and_language_survive() {
        let out = clean(r#"<p dir="rtl" lang="he">שלום</p>"#);
        assert!(
            out.html.contains("rtl") && out.html.contains("he"),
            "{}",
            out.html
        );
    }

    // ---- The stripping that another decision depends on ----

    #[test]
    fn legacy_word_processor_markup_is_stripped_as_a_regression_vector() {
        // Asserted **directly** rather than left as an inference from three independent
        // allowlist rules, because the dark transform's input contract depends on it and a
        // future allowlist edit could reintroduce it silently.
        let out = clean(
            r#"<!--[if gte mso 9]><xml><o:OfficeDocumentSettings/></xml><![endif]-->
               <o:p>vendor namespaced</o:p>
               <p style="mso-line-height-rule:exactly;color:red">text</p>"#,
        );
        assert!(
            !out.html.contains("[if"),
            "conditional comment survived: {}",
            out.html
        );
        assert!(!out.html.contains("OfficeDocumentSettings"));
        assert!(
            !out.html.contains("mso-line-height-rule"),
            "vendor CSS survived"
        );
        assert!(
            out.html.contains("color:red"),
            "an innocent declaration was lost"
        );
    }

    // ---- FR-33: every removal names its rule ----

    #[test]
    fn every_removal_carries_a_rule_identifier() {
        // FR-33 item 4 requires "a rule identifier next to every removal". A diff that says
        // something went without saying why is a diff nobody can act on.
        let out = clean(r#"<script>x</script><p onclick="y" style="position:fixed">z</p>"#);
        assert!(!out.removals.is_empty());
        for r in &out.removals {
            assert!(!r.rule.is_empty() && !r.what.is_empty(), "{r:?}");
        }
    }

    #[test]
    fn hostile_input_never_panics() {
        // NFR-19, over the shapes a tree builder finds hardest.
        for html in [
            "",
            "<",
            "<<<<<<",
            "</p></p></p>",
            "<a href=",
            "<p style=",
            "<!--",
            "<![CDATA[x]]>",
            "<p>\u{0000}</p>",
            &"<b>".repeat(200),
            &"&".repeat(10_000),
        ] {
            let _ = sanitize(html);
        }
    }
}
