//! Reading a sanitized document back as a flat element list.
//!
//! # Why this exists at all
//!
//! D-27's cascade needs three things the sanitizer's *output string* does not carry: the
//! elements in document order, each one's parent, and the stylesheets. The cascade is
//! deliberately generic over an element trait so that it depends on no particular DOM — and
//! this is where the sanitizer supplies one.
//!
//! **It re-parses the sanitizer's own output rather than exposing the tree it built.** That
//! is the same discipline the mutation-XSS audit follows and it is not an accident: what the
//! engine will render is the serialized string, so anything reasoning about "the document"
//! must reason about what a parser makes of *that*. A view of the pre-serialization tree
//! would be a view of a document nobody displays, and the gap between the two is exactly
//! where I8's parse differentials live.

use html5ever::driver::ParseOpts;
use html5ever::tendril::TendrilSink;
use html5ever::{ns, parse_document};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// One element, flat, with its parent's index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Element {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attributes: Vec<(String, String)>,
    /// `None` at the root of the list.
    pub parent: Option<usize>,
    /// The `style` attribute, which the cascade treats as its own origin.
    pub inline_style: Option<String>,
}

impl Element {
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A document, read back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub elements: Vec<Element>,
    /// The contents of every `<style>` element, in document order and joined.
    ///
    /// Order matters: the cascade breaks a specificity tie on document order, so
    /// concatenating them in the order they appeared is what makes the tie-break correct.
    pub stylesheet: String,
    /// The text a reader would see. I9's operational definition, written against the parsed
    /// tree rather than against a regular expression over the markup.
    pub visible_text: String,
}

/// Elements whose text content is not text a reader sees.
const NOT_VISIBLE: &[&str] = &["style", "script", "template", "head", "title"];

/// Read a sanitized document back.
#[must_use]
pub fn read(html: &str) -> Document {
    let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);
    let mut document = Document::default();
    walk(&dom.document, None, &mut document, true);
    document
}

fn walk(handle: &Handle, parent: Option<usize>, out: &mut Document, visible: bool) {
    let mut here = parent;
    let mut children_visible = visible;

    if let NodeData::Element { name, attrs, .. } = &handle.data {
        let tag = name.local.to_string();
        // Only the HTML namespace. A foreign element's attributes do not mean what the
        // cascade would take them to mean, and the sanitizer's allowlist already decided
        // what may be here.
        if name.ns == ns!(html) {
            let attributes: Vec<(String, String)> = attrs
                .borrow()
                .iter()
                .map(|a| (a.name.local.to_string(), a.value.to_string()))
                .collect();
            let element = Element {
                id: attributes
                    .iter()
                    .find(|(k, _)| k == "id")
                    .map(|(_, v)| v.clone()),
                classes: attributes
                    .iter()
                    .find(|(k, _)| k == "class")
                    .map(|(_, v)| v.split_whitespace().map(str::to_owned).collect())
                    .unwrap_or_default(),
                inline_style: attributes
                    .iter()
                    .find(|(k, _)| k == "style")
                    .map(|(_, v)| v.clone()),
                attributes,
                parent,
                tag: tag.clone(),
            };
            out.elements.push(element);
            here = Some(out.elements.len() - 1);
            if NOT_VISIBLE.contains(&tag.as_str()) {
                children_visible = false;
            }
            if tag == "style" {
                for child in handle.children.borrow().iter() {
                    if let NodeData::Text { contents } = &child.data {
                        out.stylesheet.push_str(&contents.borrow());
                        out.stylesheet.push('\n');
                    }
                }
            }
        }
    }

    if let NodeData::Text { contents } = &handle.data
        && visible
    {
        out.visible_text.push_str(&contents.borrow());
    }

    for child in handle.children.borrow().iter() {
        walk(child, here, out, children_visible);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_arrive_in_document_order_with_their_parents() {
        // The cascade needs both: descendant combinators walk parents, and a specificity tie
        // breaks on document order.
        let d = read("<html><body><div id='a'><p class='x y'>hi</p></div></body></html>");
        let tags: Vec<&str> = d.elements.iter().map(|e| e.tag.as_str()).collect();
        assert_eq!(tags, ["html", "head", "body", "div", "p"]);
        let p = d.elements.last().unwrap();
        assert_eq!(p.classes, ["x".to_owned(), "y".to_owned()]);
        assert_eq!(d.elements[p.parent.unwrap()].id.as_deref(), Some("a"));
    }

    #[test]
    fn stylesheets_are_collected_in_the_order_they_appeared() {
        // Concatenating them out of order would break the cascade's own tie-break.
        let d = read("<style>a{color:red}</style><p>x</p><style>a{color:blue}</style>");
        let red = d.stylesheet.find("red").expect("no first sheet");
        let blue = d.stylesheet.find("blue").expect("no second sheet");
        assert!(red < blue);
    }

    #[test]
    fn an_inline_style_is_carried_separately_from_the_other_attributes() {
        // The cascade treats it as its own origin, which is why it is not just another
        // attribute here.
        let d = read("<p style='color:red' title='t'>x</p>");
        let p = d.elements.iter().find(|e| e.tag == "p").unwrap();
        assert_eq!(p.inline_style.as_deref(), Some("color:red"));
        assert_eq!(p.attribute("title"), Some("t"));
    }

    #[test]
    fn visible_text_is_what_a_reader_sees_and_not_what_a_stylesheet_says() {
        // I9's operational definition, written against the parsed tree. A regular expression
        // over the markup would either include this stylesheet or exclude text that happens
        // to look like markup.
        let d = read("<style>p{content:'not text'}</style><p>the text</p>");
        assert!(d.visible_text.contains("the text"));
        assert!(!d.visible_text.contains("not text"), "{:?}", d.visible_text);
    }

    #[test]
    fn a_document_the_parser_repairs_is_read_as_the_parser_repaired_it() {
        // The whole reason this re-parses rather than exposing the tree the sanitizer built:
        // what the engine renders is what a parser makes of the *string*.
        let d = read("<p>one<p>two");
        let paragraphs = d.elements.iter().filter(|e| e.tag == "p").count();
        assert_eq!(paragraphs, 2, "the parser's own repair was not observed");
    }

    #[test]
    fn an_empty_document_reads_as_the_structure_a_parser_invents_for_it() {
        let d = read("");
        assert_eq!(
            d.elements
                .iter()
                .map(|e| e.tag.as_str())
                .collect::<Vec<_>>(),
            ["html", "head", "body"]
        );
        assert!(d.stylesheet.is_empty());
    }
}
