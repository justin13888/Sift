//! D-27 — resolving the full cascade.
//!
//! # What this is for, which is not what it looks like
//!
//! It looks like a rendering component and it is not: the body view renders the document
//! itself. **Sift resolves the cascade because three of its own passes need computed
//! style**, and none of them can get it from the engine:
//!
//! 1. The dark transform needs **inherited** colours to build its colour graph. A `<td>`
//!    with no `color` of its own still has one, and inverting the document without knowing
//!    it produces text the same shade as its background.
//! 2. Procedural cosmetic filters match on computed style **by definition**.
//! 3. Enumerating CSS fetching positions correctly needs to know which declarations actually
//!    apply — a blocked-but-overridden background image is a false positive in the debug
//!    view, and a real one that was overridden the other way is a miss.
//!
//! # This is the largest single commitment in the pipeline
//!
//! D-27 says so plainly, and so does R-9: a cascade is a style system in miniature,
//! measured against a specification thousands of pages long, running inside NFR-41's 30 ms
//! **shared with the sanitizer and the blocker**. It should be budgeted as comparable in
//! size to the sanitizer rather than as a step in the transform pass.
//!
//! If NFR-41 is missed this is where the pressure lands first, and the retreat is
//! declarations-only with inheritance approximated for a fixed property list — which would
//! serve the transform poorly and procedural filters not at all.
//!
//! # The viewport, and the assumption this is built under
//!
//! Q-15 is the hole D-27 left: media queries key on viewport width, so the resolved cascade
//! is correct **only at the width it was computed at**, and resizing the reader moves the
//! sender's colours underneath Sift's overrides.
//!
//! This is built under the answer recorded in that issue — **the body view is pinned to a
//! fixed layout width** — which is also D-50's stated retreat if the non-script content-height
//! mechanism proves unavailable. One mechanism serves both, which is why that answer was
//! preferred over recomputing on a debounced resize (which puts 30 ms onto an interactive
//! drag no requirement bounds) or dropping media queries (which weakens D-27's own case).
//!
//! [`Viewport::width`] is therefore a constant of the render rather than a live value, and
//! `nothing recomputes` stays true.

use crate::selector::{Combinator, Element, Selector, Specificity, cascade_order};
use cssparser::{Parser, ParserInput, Token};
use sift_foundation::limits::L9_CSS_DECLARATIONS;
use std::collections::BTreeMap;

/// The width the cascade is resolved at. Fixed for the life of a render — see Q-15.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
}

impl Default for Viewport {
    /// The pinned width. A single value chosen once, so that two renders of one message
    /// agree and a snapshot taken for NFR-26 describes something reproducible.
    fn default() -> Self {
        Self { width: 800 }
    }
}

/// One declaration, before the cascade decides whether it applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub property: String,
    pub value: String,
    pub important: bool,
}

/// One rule: the selectors it applies to and what it declares.
#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    /// Document order, which is the cascade's last tie-break.
    pub order: usize,
}

/// Why parsing stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssError {
    /// L-9. The bound on this cascade, which is the pass most likely to miss NFR-41.
    ///
    /// Rejects rather than truncating, like every other parse limit: a stylesheet cut part
    /// way through is one whose surviving rules the sender chose.
    TooManyDeclarations,
}

/// A parsed stylesheet.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

impl Stylesheet {
    /// Parse, evaluating media queries against `viewport`.
    ///
    /// # Errors
    /// [`CssError::TooManyDeclarations`] past L-9.
    pub fn parse(css: &str, viewport: Viewport) -> Result<Self, CssError> {
        let mut input = ParserInput::new(css);
        let mut parser = Parser::new(&mut input);
        let mut state = State {
            viewport,
            rules: Vec::new(),
            declarations_seen: 0,
        };
        parse_rules(&mut parser, &mut state, 0);
        if state.declarations_seen > L9_CSS_DECLARATIONS as usize {
            // Rejects rather than truncating, like every other parse limit: a stylesheet cut
            // part way through is one whose surviving rules the sender chose.
            return Err(CssError::TooManyDeclarations);
        }
        Ok(Self { rules: state.rules })
    }
}

struct State {
    viewport: Viewport,
    rules: Vec<Rule>,
    /// Counted rather than returned as an error, because the nested-block API fixes the
    /// error type and threading a custom one through it would obscure the parse. The bound
    /// is checked once, at the end, and rejects the whole stylesheet.
    declarations_seen: usize,
}

/// Walk the token stream, collecting rules.
///
/// Hand-walked rather than driven through the crate's rule-parser traits because the shape
/// wanted here is small and the error behaviour is specific: **a malformed rule is skipped
/// and the stylesheet continues**. That is what an engine does, and a stylesheet that
/// stopped at the first bad rule would render differently from the document beside it.
fn parse_rules(parser: &mut Parser<'_, '_>, state: &mut State, depth: u32) {
    // Media queries nest; nothing legitimate nests deeply, and a bound here costs nothing.
    if depth > 8 || state.declarations_seen > L9_CSS_DECLARATIONS as usize {
        return;
    }
    let mut prelude = String::new();

    while let Ok(token) = parser.next_including_whitespace().cloned() {
        match token {
            Token::CurlyBracketBlock => {
                let selector_text = prelude.trim().to_owned();
                prelude.clear();

                if let Some(query) = selector_text.strip_prefix('@') {
                    // D-27 resolves media queries rather than ignoring them, which is what
                    // makes the resolved cascade correct at *a* width — and is the whole of
                    // Q-15's problem.
                    let enter = if query.starts_with("media") {
                        media_applies(query, state.viewport)
                    } else {
                        // @supports is entered; @font-face, @keyframes and @import are not.
                        // @import in particular is a fetching position, and the sanitizer has
                        // already refused the element that could have carried one.
                        query.starts_with("supports")
                    };
                    let _ = parser.parse_nested_block(
                        |p| -> Result<(), cssparser::ParseError<'_, ()>> {
                            if enter {
                                parse_rules(p, state, depth + 1);
                            }
                            Ok(())
                        },
                    );
                    continue;
                }

                let order = state.rules.len();
                let mut declarations = Vec::new();
                let mut seen = state.declarations_seen;
                let _ =
                    parser.parse_nested_block(|p| -> Result<(), cssparser::ParseError<'_, ()>> {
                        declarations = parse_declarations(p, &mut seen);
                        Ok(())
                    });
                state.declarations_seen = seen;

                if !selector_text.is_empty() && !declarations.is_empty() {
                    state.rules.push(Rule {
                        selectors: selector_text.split(',').map(Selector::parse).collect(),
                        declarations,
                        order,
                    });
                }
            }
            Token::Semicolon => prelude.clear(),
            // A parenthesised block is where a media condition lives, and a block token has
            // to be entered explicitly — skipping it silently drops the condition and makes
            // every query apply, which is the opposite of resolving them.
            Token::ParenthesisBlock => {
                prelude.push('(');
                let mut inner = String::new();
                let _ =
                    parser.parse_nested_block(|p| -> Result<(), cssparser::ParseError<'_, ()>> {
                        while let Ok(t) = p.next_including_whitespace().cloned() {
                            append(&mut inner, &t);
                        }
                        Ok(())
                    });
                prelude.push_str(&inner);
                prelude.push(')');
            }
            ref other => append(&mut prelude, other),
        }
    }
}

fn parse_declarations(parser: &mut Parser<'_, '_>, seen: &mut usize) -> Vec<Declaration> {
    let mut out = Vec::new();
    let mut property = String::new();
    let mut value = String::new();
    let mut in_value = false;

    while let Ok(token) = parser.next_including_whitespace().cloned() {
        match token {
            Token::Colon if !in_value => in_value = true,
            Token::Semicolon => {
                *seen += 1;
                finish_declaration(&mut property, &mut value, &mut out);
                in_value = false;
            }
            // A curly block inside a declaration is malformed; skip it rather than stopping.
            Token::CurlyBracketBlock => {
                let _ =
                    parser.parse_nested_block(|_| -> Result<(), cssparser::ParseError<'_, ()>> {
                        Ok(())
                    });
            }
            ref other => {
                if in_value {
                    append(&mut value, other);
                } else {
                    append(&mut property, other);
                }
            }
        }
    }
    *seen += 1;
    finish_declaration(&mut property, &mut value, &mut out);
    out
}

/// Commit one `property: value` pair, if there is one.
fn finish_declaration(property: &mut String, value: &mut String, out: &mut Vec<Declaration>) {
    let p = property.trim().to_ascii_lowercase();
    let v = value.trim();
    if !p.is_empty() && !v.is_empty() {
        let important = v.to_ascii_lowercase().ends_with("!important");
        let v = if important {
            v[..v.len() - "!important".len()]
                .trim_end()
                .trim_end_matches('!')
                .trim()
        } else {
            v
        };
        out.push(Declaration {
            property: p,
            value: v.to_owned(),
            important,
        });
    }
    property.clear();
    value.clear();
}

/// Re-serialize a token into text.
///
/// Only the shapes email CSS actually uses; anything else contributes its text so that a
/// value is never silently truncated into a *different valid value*, which is worse than
/// being unparseable.
fn append(out: &mut String, token: &Token<'_>) {
    match token {
        Token::Ident(s) => out.push_str(s),
        // The sigil is part of the token's identity, not decoration: without it an at-rule
        // prelude is indistinguishable from a type selector, and `@media` becomes a rule
        // matching an element called "media".
        Token::AtKeyword(s) => {
            out.push('@');
            out.push_str(s);
        }
        Token::Hash(s) | Token::IDHash(s) => {
            out.push('#');
            out.push_str(s);
        }
        Token::QuotedString(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        Token::Number {
            int_value, value, ..
        } => match int_value {
            Some(i) => out.push_str(&i.to_string()),
            None => out.push_str(&value.to_string()),
        },
        Token::Percentage { unit_value, .. } => {
            out.push_str(&(unit_value * 100.0).to_string());
            out.push('%');
        }
        Token::Dimension { value, unit, .. } => {
            out.push_str(&value.to_string());
            out.push_str(unit);
        }
        Token::Function(name) => {
            out.push_str(name);
            out.push('(');
        }
        Token::WhiteSpace(_) => out.push(' '),
        Token::Comma => out.push(','),
        Token::Delim(c) => out.push(*c),
        Token::Colon => out.push(':'),
        Token::ParenthesisBlock => out.push('('),
        Token::SquareBracketBlock => out.push('['),
        Token::UnquotedUrl(u) => {
            out.push_str("url(");
            out.push_str(u);
            out.push(')');
        }
        _ => {}
    }
}

/// Evaluate a media query at the pinned width.
///
/// Handles the forms email actually uses: `min-width`, `max-width`, and the `screen` /
/// `print` media types. **Anything else does not apply** — the same rule as an unsupported
/// selector, and for the same reason: applying a query nobody evaluated would apply
/// declarations the engine beside us will not.
fn media_applies(query: &str, viewport: Viewport) -> bool {
    let q = query
        .trim_start_matches("media")
        .trim()
        .to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    // `print` is resolved rather than ignored — which is what scope's deferred printing item
    // means when it says "the machinery is present".
    if q.contains("print") && !q.contains("screen") {
        return false;
    }
    let mut applies = true;
    for condition in q.split(" and ") {
        let c = condition.trim().trim_matches(['(', ')']);
        if let Some(v) = c.strip_prefix("min-width:") {
            applies &= pixels(v).is_none_or(|px| viewport.width >= px);
        } else if let Some(v) = c.strip_prefix("max-width:") {
            applies &= pixels(v).is_none_or(|px| viewport.width <= px);
        }
    }
    applies
}

fn pixels(value: &str) -> Option<u32> {
    value.trim().trim_end_matches("px").trim().parse().ok()
}

/// Properties that inherit.
///
/// The list is short because inheritance is: what matters for the dark transform is that
/// `color` inherits and `background-color` does not, which is the asymmetry that makes a
/// colour graph necessary rather than a per-element lookup.
const INHERITED: &[&str] = &[
    "color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-variant",
    "font-weight",
    "letter-spacing",
    "line-height",
    "text-align",
    "text-indent",
    "text-transform",
    "visibility",
    "white-space",
    "word-spacing",
    "direction",
    "list-style-type",
];

#[must_use]
pub fn inherits(property: &str) -> bool {
    INHERITED.contains(&property)
}

/// The computed style of one element.
pub type Computed = BTreeMap<String, String>;

/// Resolve the cascade over a tree.
///
/// `elements` is in document order and each names its parent, so inheritance is one pass
/// rather than a walk per element.
#[must_use]
pub fn resolve<E: Element>(
    sheet: &Stylesheet,
    inline: &[Option<String>],
    elements: &[E],
    viewport: Viewport,
) -> Vec<Computed> {
    let mut computed: Vec<Computed> = Vec::with_capacity(elements.len());

    for (index, element) in elements.iter().enumerate() {
        // Every declaration that applies, with what decides which of them wins.
        let mut candidates: Vec<(bool, Specificity, usize, &Declaration)> = Vec::new();

        for rule in &sheet.rules {
            let Some(specificity) = best_match(rule, element, elements, index) else {
                continue;
            };
            for d in &rule.declarations {
                candidates.push((d.important, specificity, rule.order, d));
            }
        }

        // An inline style outranks every selector — specificity beyond any selector's, and
        // last in document order.
        let inline_declarations = inline
            .get(index)
            .and_then(Option::as_ref)
            .map(|s| parse_inline(s))
            .unwrap_or_default();
        for d in &inline_declarations {
            candidates.push((
                d.important,
                Specificity {
                    ids: u32::MAX,
                    classes: 0,
                    types: 0,
                },
                usize::MAX,
                d,
            ));
        }

        candidates.sort_by(|a, b| cascade_order((a.0, a.1, a.2), (b.0, b.1, b.2)));

        let mut style = Computed::new();
        for (_, _, _, d) in candidates {
            for (property, value) in expand_shorthand(&d.property, &d.value) {
                style.insert(property, value);
            }
        }

        // Inheritance, from the parent's already-computed style. Document order guarantees
        // the parent is resolved first, which is why this is one pass.
        if let Some(parent) = element.parent() {
            if let Some(parent_style) = computed.get(parent) {
                for (property, value) in parent_style {
                    if inherits(property) && !style.contains_key(property) {
                        style.insert(property.clone(), value.clone());
                    }
                }
            }
        }

        let _ = viewport;
        computed.push(style);
    }
    computed
}

/// The highest specificity with which any of a rule's selectors matches.
fn best_match<E: Element>(
    rule: &Rule,
    element: &E,
    elements: &[E],
    index: usize,
) -> Option<Specificity> {
    rule.selectors
        .iter()
        .filter(|s| s.is_understood() && matches_selector(s, element, elements, index))
        .map(Selector::specificity)
        .max()
}

/// Match right to left, which is how a selector is evaluated and why it is cheap.
fn matches_selector<E: Element>(
    selector: &Selector,
    element: &E,
    elements: &[E],
    index: usize,
) -> bool {
    let Some((_, subject)) = selector.parts.last() else {
        return false;
    };
    if !subject.matches(element) {
        return false;
    }
    let mut current = elements[index].parent();
    for window in selector.parts.windows(2).rev() {
        let (combinator, _) = window[1];
        let (_, ancestor) = &window[0];
        match combinator {
            Combinator::Child => {
                let Some(p) = current else { return false };
                if !ancestor.matches(&elements[p]) {
                    return false;
                }
                current = elements[p].parent();
            }
            Combinator::Descendant | Combinator::Root => loop {
                let Some(p) = current else { return false };
                current = elements[p].parent();
                if ancestor.matches(&elements[p]) {
                    break;
                }
            },
        }
    }
    true
}

fn parse_inline(style: &str) -> Vec<Declaration> {
    style
        .split(';')
        .filter_map(|d| {
            let (p, v) = d.split_once(':')?;
            let v = v.trim();
            let important = v.to_ascii_lowercase().ends_with("!important");
            let v = if important {
                v[..v.len() - 10].trim_end().trim_end_matches('!').trim()
            } else {
                v
            };
            (!p.trim().is_empty() && !v.is_empty()).then(|| Declaration {
                property: p.trim().to_ascii_lowercase(),
                value: v.to_owned(),
                important,
            })
        })
        .collect()
}

/// Expand the shorthands whose longhands the colour graph reads.
///
/// D-27 lists shorthand expansion as part of what a real cascade does, and the reason it
/// cannot be skipped is narrow and concrete: `background: #fff` sets a background colour,
/// and a transform that only looked at `background-color` would invert the text and leave
/// the background alone.
#[must_use]
pub fn expand_shorthand(property: &str, value: &str) -> Vec<(String, String)> {
    let mut out = vec![(property.to_owned(), value.to_owned())];
    match property {
        "background" => {
            // The colour component, where the shorthand carries one.
            if let Some(colour) = value.split_whitespace().find(|t| looks_like_a_colour(t)) {
                out.push(("background-color".to_owned(), colour.to_owned()));
            }
        }
        "font" => {
            if let Some(family) = value
                .split(',')
                .next()
                .and_then(|f| f.split_whitespace().last())
            {
                out.push(("font-family".to_owned(), family.to_owned()));
            }
        }
        "border" => {
            if let Some(colour) = value.split_whitespace().find(|t| looks_like_a_colour(t)) {
                out.push(("border-color".to_owned(), colour.to_owned()));
            }
        }
        _ => {}
    }
    out
}

fn looks_like_a_colour(token: &str) -> bool {
    token.starts_with('#')
        || token.starts_with("rgb")
        || token.starts_with("hsl")
        || crate::colour::named(token).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat tree the matcher can walk.
    struct Node {
        tag: String,
        id: Option<String>,
        classes: Vec<String>,
        attrs: Vec<(String, String)>,
        parent: Option<usize>,
    }

    impl Element for Node {
        fn tag(&self) -> &str {
            &self.tag
        }
        fn id(&self) -> Option<&str> {
            self.id.as_deref()
        }
        fn classes(&self) -> &[String] {
            &self.classes
        }
        fn attribute(&self, name: &str) -> Option<&str> {
            self.attrs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }
        fn parent(&self) -> Option<usize> {
            self.parent
        }
    }

    fn node(tag: &str, parent: Option<usize>) -> Node {
        Node {
            tag: tag.into(),
            id: None,
            classes: vec![],
            attrs: vec![],
            parent,
        }
    }

    fn with_class(tag: &str, class: &str, parent: Option<usize>) -> Node {
        Node {
            tag: tag.into(),
            id: None,
            classes: vec![class.into()],
            attrs: vec![],
            parent,
        }
    }

    fn resolve_css(css: &str, nodes: &[Node], inline: &[Option<String>]) -> Vec<Computed> {
        let sheet = Stylesheet::parse(css, Viewport::default()).expect("within bounds");
        resolve(&sheet, inline, nodes, Viewport::default())
    }

    #[test]
    fn a_declaration_reaches_the_element_its_selector_names() {
        let nodes = [node("body", None), node("p", Some(0))];
        let c = resolve_css("p { color: red }", &nodes, &[None, None]);
        assert_eq!(c[1].get("color").map(String::as_str), Some("red"));
        assert!(!c[0].contains_key("color"));
    }

    #[test]
    fn specificity_decides_between_two_declarations() {
        // The reason a colour-only subset cannot work: which `color` wins is a property of
        // the cascade as a whole, and a pass that only looked at colours would have to guess.
        let nodes = [with_class("p", "note", None)];
        let c = resolve_css("p { color: red } .note { color: blue }", &nodes, &[None]);
        assert_eq!(
            c[0].get("color").map(String::as_str),
            Some("blue"),
            "a class lost to a type"
        );
    }

    #[test]
    fn document_order_breaks_a_specificity_tie() {
        let nodes = [node("p", None)];
        let c = resolve_css("p { color: red } p { color: green }", &nodes, &[None]);
        assert_eq!(c[0].get("color").map(String::as_str), Some("green"));
    }

    #[test]
    fn important_beats_specificity() {
        let nodes = [Node {
            tag: "p".into(),
            id: Some("x".into()),
            classes: vec![],
            attrs: vec![],
            parent: None,
        }];
        let c = resolve_css(
            "#x { color: blue } p { color: red !important }",
            &nodes,
            &[None],
        );
        assert_eq!(c[0].get("color").map(String::as_str), Some("red"));
    }

    #[test]
    fn an_inline_style_outranks_every_selector() {
        let nodes = [Node {
            tag: "p".into(),
            id: Some("x".into()),
            classes: vec![],
            attrs: vec![],
            parent: None,
        }];
        let c = resolve_css("#x { color: blue }", &nodes, &[Some("color: green".into())]);
        assert_eq!(c[0].get("color").map(String::as_str), Some("green"));
    }

    #[test]
    fn colour_inherits_and_background_does_not() {
        // The asymmetry that makes a colour graph necessary rather than a per-element
        // lookup: a `<td>` with no colour of its own still has one, and inverting without
        // knowing it produces text the same shade as its background.
        let nodes = [
            node("body", None),
            node("table", Some(0)),
            node("td", Some(1)),
        ];
        let c = resolve_css(
            "body { color: red; background-color: white }",
            &nodes,
            &[None, None, None],
        );
        assert_eq!(
            c[2].get("color").map(String::as_str),
            Some("red"),
            "colour did not inherit"
        );
        assert!(
            !c[2].contains_key("background-color"),
            "background inherited"
        );
    }

    #[test]
    fn a_descendant_selector_matches_through_intermediate_elements() {
        let nodes = [node("div", None), node("span", Some(0)), node("a", Some(1))];
        let c = resolve_css("div a { color: red }", &nodes, &[None, None, None]);
        assert_eq!(c[2].get("color").map(String::as_str), Some("red"));
    }

    #[test]
    fn a_child_selector_does_not_match_a_grandchild() {
        let nodes = [node("div", None), node("span", Some(0)), node("a", Some(1))];
        let c = resolve_css("div > a { color: red }", &nodes, &[None, None, None]);
        assert!(
            !c[2].contains_key("color"),
            "a child combinator matched a grandchild"
        );
    }

    #[test]
    fn a_selector_this_does_not_model_fails_to_match() {
        // An honest non-match. Matching approximately would apply declarations the engine
        // beside us will not, and the two documents would then differ.
        let nodes = [node("p", None), node("p", Some(0))];
        for css in [
            "p:nth-child(2) { color: red }",
            "p + p { color: red }",
            "p::before { color: red }",
        ] {
            let c = resolve_css(css, &nodes, &[None, None]);
            assert!(!c[1].contains_key("color"), "{css} matched approximately");
        }
    }

    #[test]
    fn an_unsupported_selector_does_not_discard_the_rules_around_it() {
        let nodes = [node("p", None)];
        let c = resolve_css("p:has(x) { color: red } p { color: blue }", &nodes, &[None]);
        assert_eq!(c[0].get("color").map(String::as_str), Some("blue"));
    }

    #[test]
    fn a_media_query_is_resolved_rather_than_ignored() {
        // D-27 resolves them, which is what makes the cascade correct at *a* width — and is
        // the whole of Q-15's problem. Pinned at 800.
        let nodes = [node("p", None)];
        let applies = resolve_css(
            "@media (min-width: 600px) { p { color: red } }",
            &nodes,
            &[None],
        );
        assert_eq!(applies[0].get("color").map(String::as_str), Some("red"));

        let does_not = resolve_css(
            "@media (min-width: 2000px) { p { color: red } }",
            &nodes,
            &[None],
        );
        assert!(
            !does_not[0].contains_key("color"),
            "a query for a wider viewport applied"
        );
    }

    #[test]
    fn a_max_width_query_resolves_the_other_way() {
        let nodes = [node("p", None)];
        let c = resolve_css(
            "@media (max-width: 400px) { p { color: red } }",
            &nodes,
            &[None],
        );
        assert!(!c[0].contains_key("color"));
    }

    #[test]
    fn print_rules_do_not_apply_to_the_screen() {
        // The machinery scope's deferred printing item says is "already present".
        let nodes = [node("p", None)];
        let c = resolve_css("@media print { p { color: red } }", &nodes, &[None]);
        assert!(!c[0].contains_key("color"));
    }

    #[test]
    fn a_shorthand_sets_the_longhand_the_transform_reads() {
        // `background: #fff` sets a background colour. A transform that only looked at
        // `background-color` would invert the text and leave the background alone.
        let nodes = [node("p", None)];
        let c = resolve_css(
            "p { background: #ffffff url(x) no-repeat }",
            &nodes,
            &[None],
        );
        assert_eq!(
            c[0].get("background-color").map(String::as_str),
            Some("#ffffff")
        );
    }

    #[test]
    fn a_malformed_rule_is_skipped_and_the_stylesheet_continues() {
        // What an engine does. A stylesheet that stopped at the first bad rule would render
        // differently from the document beside it.
        let nodes = [node("p", None)];
        let c = resolve_css("@ ; } p { color: red }", &nodes, &[None]);
        assert_eq!(c[0].get("color").map(String::as_str), Some("red"));
    }

    #[test]
    fn a_stylesheet_over_l9_is_rejected_rather_than_truncated() {
        let huge =
            "p { ".to_owned() + &"color: red;".repeat(L9_CSS_DECLARATIONS as usize + 10) + " }";
        assert_eq!(
            Stylesheet::parse(&huge, Viewport::default()).err(),
            Some(CssError::TooManyDeclarations)
        );
    }

    #[test]
    fn hostile_css_never_panics() {
        // The pass runs on attacker-controlled input inside a 30 ms budget. A panic here is
        // caught by D-47 and degrades the message, but it is still a defect.
        for css in [
            "",
            "{",
            "}}}}",
            "p {",
            "p { color",
            "p { color: }",
            "@media {",
            "@media (min-width:) { p { color: red } }",
            "p { color: rgb( }",
            &"@media screen {".repeat(50),
            &"p{color:red}".repeat(5_000),
            "p { content: \"unterminated",
        ] {
            let _ = Stylesheet::parse(css, Viewport::default());
        }
    }

    #[test]
    fn the_cascade_fits_inside_its_share_of_thirty_milliseconds() {
        // R-9 doubts this fits: "a style system in miniature sharing a 30 ms budget with the
        // sanitizer and the blocker", and D-27 says this is where the pressure lands first if
        // NFR-41 is missed.
        //
        // This is **not** a conformance gate — Q-10's rig does not exist, so a figure from
        // this machine describes this machine. What it is worth is a tripwire: an order of
        // magnitude regression shows up here rather than in a phase gate months later.
        let css = (0..400)
            .map(|i| {
                format!(
                    ".c{i} p > span {{ color: #11223{}; background: #fff }}",
                    i % 10
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let nodes: Vec<Node> = (0..600usize)
            .map(|i| with_class("span", &format!("c{}", i % 400), i.checked_sub(1)))
            .collect();
        let inline: Vec<Option<String>> = vec![None; nodes.len()];

        let start = std::time::Instant::now();
        let sheet = Stylesheet::parse(&css, Viewport::default()).expect("bounded");
        let computed = resolve(&sheet, &inline, &nodes, Viewport::default());
        let elapsed = start.elapsed();

        assert_eq!(computed.len(), nodes.len());
        assert!(
            elapsed < std::time::Duration::from_millis(300),
            "the cascade took {elapsed:?} on a document far smaller than L-7 permits — \
             R-9's doubt deserves a measurement on the reference rig rather than this one"
        );
    }
}
