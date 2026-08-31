//! Selectors, and the specificity that orders them.
//!
//! # Why specificity cannot be skipped
//!
//! D-27 rejects "a colour-bearing subset" for a reason that is easy to miss: **specificity
//! and importance are properties of the cascade as a whole.** Deciding which `color`
//! declaration wins requires knowing which *selectors* matched and how specific each was,
//! and that is not a question a colour-only pass can answer — it would have to guess, and
//! it would guess differently from the engine rendering the same document beside it.
//!
//! # The subset, and why it is a subset
//!
//! Email CSS is written to survive a decade of mail clients, so it lives in a narrow band:
//! type, class, identifier and attribute selectors, joined by descendant and child
//! combinators. That band is implemented fully. What is deliberately absent —
//! `:nth-child()`, sibling combinators, `:has()` — is absent because a selector this does
//! not understand must **fail to match** rather than match approximately, and an honest
//! non-match is a declaration not applied rather than a wrong colour.

use std::cmp::Ordering;

/// One compound selector: a tag, and any number of qualifiers on it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compound {
    /// `None` is the universal selector.
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// `[name]` or `[name="value"]`.
    pub attributes: Vec<(String, Option<String>)>,
    /// A pseudo-class this implementation does not model.
    ///
    /// Recorded rather than dropped, because a selector carrying one must **fail to match**:
    /// dropping it would widen the selector to everything it would otherwise have narrowed.
    pub unsupported: bool,
}

/// How a compound joins to the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// The first compound in a selector.
    Root,
    /// `A B`
    Descendant,
    /// `A > B`
    Child,
}

/// A full selector: compounds, right to left is how they are matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub parts: Vec<(Combinator, Compound)>,
}

/// CSS specificity: identifiers, then classes and attributes, then types.
///
/// Ordered so that `Ord` **is** the cascade's tie-break, rather than something a comparison
/// function has to remember to do in the right order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Specificity {
    pub ids: u32,
    pub classes: u32,
    pub types: u32,
}

impl Selector {
    /// Parse one selector.
    ///
    /// Returns a selector that can never match where the input uses something this does not
    /// model, rather than `None` — a stylesheet is a list, and one selector nobody
    /// understands must not discard the rules around it.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut parts = Vec::new();
        let mut combinator = Combinator::Root;
        let mut token = String::new();

        let flush = |token: &mut String, combinator: Combinator, parts: &mut Vec<_>| {
            if !token.is_empty() {
                parts.push((combinator, Compound::parse(token)));
                token.clear();
            }
        };

        for c in text.trim().chars() {
            match c {
                '>' => {
                    flush(&mut token, combinator, &mut parts);
                    combinator = Combinator::Child;
                }
                c if c.is_whitespace() => {
                    if !token.is_empty() {
                        flush(&mut token, combinator, &mut parts);
                        // A following `>` overrides this; whitespace alone is descendant.
                        combinator = Combinator::Descendant;
                    }
                }
                // Sibling combinators are not modelled. The whole selector must fail rather
                // than silently becoming a descendant selector, which would match far more.
                '+' | '~' => {
                    token.push_str(":unsupported");
                }
                c => token.push(c),
            }
        }
        flush(&mut token, combinator, &mut parts);

        if parts.is_empty() {
            parts.push((
                Combinator::Root,
                Compound {
                    unsupported: true,
                    ..Compound::default()
                },
            ));
        }
        Self { parts }
    }

    #[must_use]
    pub fn specificity(&self) -> Specificity {
        let mut s = Specificity::default();
        for (_, c) in &self.parts {
            if c.id.is_some() {
                s.ids += 1;
            }
            s.classes += u32::try_from(c.classes.len() + c.attributes.len()).unwrap_or(0);
            if c.tag.is_some() {
                s.types += 1;
            }
        }
        s
    }

    /// Whether any compound uses something this implementation does not model.
    #[must_use]
    pub fn is_understood(&self) -> bool {
        !self.parts.iter().any(|(_, c)| c.unsupported)
    }
}

impl Compound {
    fn parse(text: &str) -> Self {
        let mut c = Self::default();
        let mut chars = text.chars().peekable();
        let mut current = String::new();
        let mut kind = '\0';

        let commit = |kind: char, current: &mut String, c: &mut Self| {
            if current.is_empty() {
                return;
            }
            match kind {
                '#' => c.id = Some(current.clone()),
                '.' => c.classes.push(current.clone()),
                '[' => {
                    let inner = current.trim_end_matches(']');
                    if let Some((n, v)) = inner.split_once('=') {
                        c.attributes.push((
                            n.trim().to_ascii_lowercase(),
                            Some(v.trim().trim_matches(['"', '\'']).to_owned()),
                        ));
                    } else {
                        c.attributes.push((inner.trim().to_ascii_lowercase(), None));
                    }
                }
                ':' => c.unsupported = true,
                _ => {
                    if current != "*" {
                        c.tag = Some(current.to_ascii_lowercase());
                    }
                }
            }
            current.clear();
        };

        while let Some(ch) = chars.next() {
            match ch {
                '#' | '.' | '[' | ':' => {
                    commit(kind, &mut current, &mut c);
                    kind = ch;
                    // `::before` is a pseudo-element; one colon or two, unsupported either way.
                    if ch == ':' {
                        chars.next_if_eq(&':');
                    }
                }
                ']' => {}
                ch => current.push(ch),
            }
        }
        commit(kind, &mut current, &mut c);
        c
    }
}

/// A node, as the matcher needs to see it.
///
/// Deliberately not the DOM type: the cascade is tested against hand-built trees, and tying
/// it to one DOM would make every test build one.
pub trait Element {
    fn tag(&self) -> &str;
    fn id(&self) -> Option<&str>;
    fn classes(&self) -> &[String];
    fn attribute(&self, name: &str) -> Option<&str>;
    /// `None` at the root.
    fn parent(&self) -> Option<usize>;
}

impl Compound {
    /// Whether this compound matches an element.
    #[must_use]
    pub fn matches<E: Element>(&self, element: &E) -> bool {
        if self.unsupported {
            // An honest non-match. Matching approximately would apply declarations the
            // engine beside us will not, and the two documents would then differ.
            return false;
        }
        if let Some(tag) = &self.tag
            && !element.tag().eq_ignore_ascii_case(tag)
        {
            return false;
        }
        if let Some(id) = &self.id
            && element.id() != Some(id.as_str())
        {
            return false;
        }
        if !self
            .classes
            .iter()
            .all(|c| element.classes().iter().any(|k| k == c))
        {
            return false;
        }
        for (name, value) in &self.attributes {
            match (element.attribute(name), value) {
                (None, _) => return false,
                (Some(actual), Some(expected)) if actual != expected => return false,
                _ => {}
            }
        }
        true
    }
}

/// The order two declarations resolve in.
///
/// `!important` wins over specificity, and specificity wins over document order — which is
/// the cascade, in the only place it should be written down.
#[must_use]
pub fn cascade_order(a: (bool, Specificity, usize), b: (bool, Specificity, usize)) -> Ordering {
    let (a_important, a_spec, a_order) = a;
    let (b_important, b_spec, b_order) = b;
    a_important
        .cmp(&b_important)
        .then(a_spec.cmp(&b_spec))
        .then(a_order.cmp(&b_order))
}
