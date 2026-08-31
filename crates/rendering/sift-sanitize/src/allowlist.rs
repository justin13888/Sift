//! The allowlist. Sift's own, and Sift's to maintain forever.
//!
//! # The growth rule, which is the important part
//!
//! > **No addition to the element, attribute, or CSS-property allowlist may land without
//! > classifying whether it introduces a fetching position, and adding an NFR-40 vector for
//! > it either way.**
//!
//! The CSS half is the likeliest failure. Content blocking enumerates the fetching
//! positions CSS carries — background images, font sources, list-style images, cursors,
//! border images, masks, reflections — and that is a list a new property is added
//! *beside* rather than *into*. A property allowed without that classification is an I2
//! hole that no test looks for.
//!
//! # What must survive, and why it is not cosmetic
//!
//! NFR-50 says body-view isolation must not sever the accessibility tree — and because I3
//! and I4 strip document-level and structural elements, **the accessible name of whatever
//! survives derives from content the sanitizer preserved.** So alternative text, table
//! structure, heading level and reading order have to be on this list, and losing them is
//! an accessibility *defect* rather than a tidier document.
//!
//! That is why NFR-50 is gated in P1 rather than with the screen-reader work in P4: this
//! allowlist is authored in P1, and gating it later would let it strip every
//! accessibility-bearing attribute it liked with nothing to catch the loss.

/// Elements that may appear in a sanitized body.
///
/// Everything absent is removed. Note what is deliberately here for NFR-50 rather than for
/// appearance: the heading levels, the table structure elements, and the list elements all
/// carry reading order and structure a screen reader announces.
pub const ELEMENTS: &[&str] = &[
    // Text flow.
    "p",
    "br",
    "hr",
    "div",
    "span",
    "pre",
    "blockquote",
    // Headings — heading *level* is structure, not decoration.
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    // Inline emphasis. The semantic pair is kept alongside the presentational one because
    // a screen reader distinguishes them.
    "b",
    "strong",
    "i",
    "em",
    "u",
    "s",
    "strike",
    "small",
    "sub",
    "sup",
    "code",
    "kbd",
    "samp",
    "var",
    "abbr",
    "cite",
    "q",
    "mark",
    "del",
    "ins",
    "wbr",
    // Lists — structure again.
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    // Tables. Mail is built out of these, and `th`, `caption`, `thead` and `tbody` are what
    // make a table navigable rather than a grid of unlabelled cells.
    "table",
    "caption",
    "colgroup",
    "col",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "td",
    "th",
    // Links and images. Both carry fetching positions or navigation, handled below.
    "a",
    "img",
    "picture",
    "source",
    // Grouping.
    "figure",
    "figcaption",
    "address",
    "article",
    "aside",
    "footer",
    "header",
    "main",
    "nav",
    "section",
    "details",
    "summary",
    "time",
    "bdi",
    "bdo",
    "ruby",
    "rt",
    "rp",
    "center",
    "font",
    "big",
    "tt",
    // A stylesheet. **Its contents never survive**: the sanitizer rebuilds them declaration
    // by declaration, because a `<style>` element serializes as raw text and a stylesheet
    // containing `</style>` would close it on the way back in. Kept at all because D-27's
    // cascade needs something to resolve — selector matching, specificity, media queries and
    // inheritance have no meaning over inline attributes alone, and L-9 bounds declarations
    // "across all stylesheets and style attributes".
    "style",
];

/// Attributes allowed on any element.
pub const GLOBAL_ATTRIBUTES: &[&str] = &[
    // `dir` and `lang` are NFR-50 and NFR-28 territory: direction and language are
    // announced, and stripping them mangles right-to-left and mixed-script mail.
    "dir",
    "lang",
    "title",
    "align",
    "valign",
    // Style is allowed and then filtered property-by-property. Removing it outright would
    // make most marketing mail unreadable; keeping it unfiltered would be an I1 and I2 hole.
    "style",
    // The handle the sanitizer stamps on every kept element, so that stage 6's overrides and
    // FR-33's removals have something to point at. Allowed so that I6 holds: a second pass
    // must not remove what the first added, or `S(S(x))` would differ from `S(x)`.
    crate::sanitize::ELEMENT_HANDLE,
];

/// Attributes allowed on specific elements.
///
/// The accessibility-bearing ones are marked, because a future edit that "tidies" them away
/// would be an NFR-50 defect that looks like a simplification.
pub const ELEMENT_ATTRIBUTES: &[(&str, &[&str])] = &[
    // `alt` is NFR-50. An image with no alternative text is an image a screen reader cannot
    // announce, and FR-29's tracking-pixel heuristic treats *missing* alt text as one of its
    // signals — so it is load-bearing in two directions.
    (
        "img",
        &["src", "srcset", "alt", "width", "height", "loading"],
    ),
    ("a", &["href", "name", "rel"]),
    ("source", &["srcset", "src", "type", "media"]),
    // `colspan`/`rowspan`/`headers`/`scope` are table *structure*: without them a screen
    // reader cannot associate a cell with its header.
    (
        "td",
        &[
            "colspan", "rowspan", "headers", "bgcolor", "width", "height",
        ],
    ),
    (
        "th",
        &[
            "colspan", "rowspan", "headers", "scope", "abbr", "bgcolor", "width", "height",
        ],
    ),
    (
        "table",
        &[
            "border",
            "cellpadding",
            "cellspacing",
            "bgcolor",
            "width",
            "summary",
        ],
    ),
    ("col", &["span", "width"]),
    ("colgroup", &["span", "width"]),
    ("ol", &["start", "reversed", "type"]),
    ("li", &["value"]),
    ("font", &["color", "face", "size"]),
    ("time", &["datetime"]),
    ("bdo", &["dir"]),
    ("blockquote", &["cite"]),
    ("q", &["cite"]),
    ("del", &["cite", "datetime"]),
    ("ins", &["cite", "datetime"]),
];

/// Attributes that are **fetching positions**: their value causes a load.
///
/// Every one of these is rewritten to the internal scheme at sanitize time — **including
/// one a filter rule has already condemned**. Whether a rewritten address yields bytes is
/// the broker's decision at request time, because FR-8's per-sender allowlist and the
/// active network policy tier change without the message changing.
///
/// Rewriting only what will be fetched is the mistake that "falsifies I2 while every test
/// still passes".
pub const FETCHING_ATTRIBUTES: &[&str] = &["src", "srcset", "poster", "background"];

/// CSS properties allowed through.
///
/// Deliberately narrow. Everything absent is dropped rather than passed through, because a
/// property allowed without being classified is an I2 hole nothing looks for.
pub const CSS_PROPERTIES: &[&str] = &[
    "color",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "font-variant",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "text-align",
    "text-decoration",
    "text-indent",
    "text-transform",
    "vertical-align",
    "white-space",
    "direction",
    "unicode-bidi",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-color",
    "border-style",
    "border-width",
    "border-radius",
    "border-collapse",
    "border-spacing",
    "width",
    "height",
    "max-width",
    "max-height",
    "min-width",
    "min-height",
    "display",
    "visibility",
    "overflow",
    "float",
    "clear",
    "table-layout",
    "list-style-type",
    "opacity",
];

/// CSS properties that carry a **fetching position**.
///
/// Enumerated explicitly rather than left to the element allowlist, which does not see
/// them. They are not currently allowed through at all — but the list exists so that
/// allowing one later is a decision made against a name rather than an oversight, and so
/// that the transform pass has something to check itself against.
pub const CSS_FETCHING_PROPERTIES: &[&str] = &[
    "background",
    "background-image",
    "border-image",
    "border-image-source",
    "content",
    "cursor",
    "list-style",
    "list-style-image",
    "mask",
    "mask-image",
    "src",
    "-webkit-box-reflect",
];

/// URL schemes permitted in a navigation position.
///
/// `mailto:` is here because a message legitimately contains one and FR-42 requires it be
/// *shown* and reported as needing a handler rather than silently omitted. Every scheme
/// that can execute — `javascript:`, `vbscript:`, `data:` bearing markup — is absent, and
/// I1 depends on that absence.
pub const NAVIGATION_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

/// CSS values that escape containment under I5.
///
/// Fixed positioning leaves the document's flow, and viewport units are measured against
/// the *viewport* rather than the body view's box — both let a message draw outside the
/// area the reader gave it, which is the containment I5 asserts.
pub const CONTAINMENT_ESCAPES: &[&str] = &["fixed", "sticky", "vw", "vh", "vmin", "vmax"];

#[must_use]
pub fn element_allowed(name: &str) -> bool {
    ELEMENTS.contains(&name)
}

#[must_use]
pub fn attribute_allowed(element: &str, attribute: &str) -> bool {
    if GLOBAL_ATTRIBUTES.contains(&attribute) {
        return true;
    }
    ELEMENT_ATTRIBUTES
        .iter()
        .find(|(e, _)| *e == element)
        .is_some_and(|(_, attrs)| attrs.contains(&attribute))
}

#[must_use]
pub fn is_fetching_attribute(attribute: &str) -> bool {
    FETCHING_ATTRIBUTES.contains(&attribute)
}

#[must_use]
pub fn css_property_allowed(property: &str) -> bool {
    CSS_PROPERTIES.contains(&property)
}

#[must_use]
pub fn css_property_fetches(property: &str) -> bool {
    CSS_FETCHING_PROPERTIES.contains(&property)
}
