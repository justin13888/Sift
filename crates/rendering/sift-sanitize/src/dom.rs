//! The tree the sanitizer's policy walks, and the sink `html5ever` builds it through.
//!
//! D-26 makes the tree builder a dependency and the policy Sift's. The tree between them was
//! `markup5ever_rcdom`, a crate its upstream publishes for its own tests and calls
//! unsupported; it pinned the sanitizer to the `html5ever` line it was cut for, which is how
//! a parser panic fixed upstream stayed reachable here (NFR-19). This is the same shape —
//! reference-counted nodes, weak parent pointers, children in order — owned here, so the
//! tree builder can be taken at the version that carries its fixes.
//!
//! Three departures from that crate, each on purpose:
//!
//! - **No panics on a broken promise.** The tree-sink interface allows an implementation to
//!   panic where the builder calls it on the wrong kind of node. This one degrades instead —
//!   a placeholder name, a detached fragment, a no-op — because NFR-19 admits no crash on any
//!   input, whatever the builder's own guarantees.
//! - **No error log.** Parse errors are not collected. Nothing read them, and a list that
//!   grows by one entry per error is an allocation the sender controls.
//! - **Release does not empty survivors.** Dropping a node is iterative so a deep tree does
//!   not exhaust the stack, but it descends only into nodes it is the last owner of. A node
//!   still referenced elsewhere keeps its children.
//!
//! `selectedcontent` cloning is left at the interface's no-op default, which is what the
//! previous tree did in practice: its search for the element never matched.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::io;
use std::rc::{Rc, Weak};

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::serialize::TraversalScope::{ChildrenOnly, IncludeNode};
use html5ever::serialize::{Serialize, Serializer, TraversalScope};
use html5ever::tendril::StrTendril;
use html5ever::{Attribute, ExpandedName, LocalName, QualName, ns};

/// What a node is.
#[derive(Debug)]
pub(crate) enum NodeData {
    /// The document, or a template's contents.
    Document,
    /// A `DOCTYPE`. Its public and system identifiers are not kept: the serializer writes
    /// the name alone, and nothing else reads them.
    Doctype {
        /// The doctype's name.
        name: StrTendril,
    },
    /// Text.
    Text {
        /// The text, which the builder extends as adjacent runs arrive.
        contents: RefCell<StrTendril>,
    },
    /// A comment.
    Comment {
        /// The comment's text.
        contents: StrTendril,
    },
    /// An element.
    Element {
        /// Its qualified name.
        name: QualName,
        /// Its attributes, in source order.
        attrs: RefCell<Vec<Attribute>>,
        /// A `template`'s contents, which are not its children.
        template_contents: RefCell<Option<Handle>>,
        /// Whether a MathML `annotation-xml` is an HTML integration point.
        mathml_annotation_xml_integration_point: bool,
    },
    /// A processing instruction, which HTML parsing produces only as a bogus comment.
    ProcessingInstruction {
        /// Its target.
        target: StrTendril,
        /// Its data.
        contents: StrTendril,
    },
}

/// One node of the tree.
pub(crate) struct Node {
    /// The parent, held weakly so the tree has no cycles.
    pub(crate) parent: Cell<Option<Weak<Node>>>,
    /// The children, in order.
    pub(crate) children: RefCell<Vec<Handle>>,
    /// What the node is.
    pub(crate) data: NodeData,
}

/// A shared reference to a node.
pub(crate) type Handle = Rc<Node>;

impl Node {
    /// A parentless node with no children.
    #[must_use]
    pub(crate) fn new(data: NodeData) -> Handle {
        Rc::new(Self {
            parent: Cell::new(None),
            children: RefCell::new(Vec::new()),
            data,
        })
    }

    /// The parent, where it is set and still alive.
    fn parent_node(&self) -> Option<Handle> {
        let weak = self.parent.take();
        let parent = weak.as_ref().and_then(Weak::upgrade);
        self.parent.set(weak);
        parent
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        // Iterative, so that releasing a deeply nested tree does not recurse once per level.
        // Only a node this walk holds the last reference to is emptied: its children are
        // then released here rather than by its own `drop`. A node referenced elsewhere is
        // left whole, because it is still somebody's tree.
        let mut pending = core::mem::take(self.children.get_mut());
        if let NodeData::Element {
            template_contents, ..
        } = &mut self.data
            && let Some(contents) = template_contents.get_mut().take()
        {
            pending.push(contents);
        }
        while let Some(node) = pending.pop() {
            if Rc::strong_count(&node) == 1 {
                pending.append(&mut node.children.borrow_mut());
                if let NodeData::Element {
                    template_contents, ..
                } = &node.data
                    && let Some(contents) = template_contents.borrow_mut().take()
                {
                    pending.push(contents);
                }
            }
        }
    }
}

impl core::fmt::Debug for Node {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Node")
            .field("data", &self.data)
            .field("children", &self.children.borrow().len())
            .finish()
    }
}

/// The parent of `target` and its index there, where it has one.
fn parent_and_index(target: &Handle) -> Option<(Handle, usize)> {
    let parent = target.parent_node()?;
    let index = parent
        .children
        .borrow()
        .iter()
        .position(|child| Rc::ptr_eq(child, target))?;
    Some((parent, index))
}

/// Detach `target` from its parent, where it has one.
fn detach(target: &Handle) {
    if let Some((parent, index)) = parent_and_index(target) {
        parent.children.borrow_mut().remove(index);
    }
    target.parent.set(None);
}

/// Extend `node` with `text` if it is a text node.
fn extend_text(node: &Handle, text: &str) -> bool {
    if let NodeData::Text { contents } = &node.data {
        contents.borrow_mut().push_slice(text);
        true
    } else {
        false
    }
}

fn text_node(text: StrTendril) -> Handle {
    Node::new(NodeData::Text {
        contents: RefCell::new(text),
    })
}

/// The tree being built; the result of a parse.
pub(crate) struct Dom {
    /// The document node.
    pub(crate) document: Handle,
    /// What [`TreeSink::elem_name`] answers for a node that is not an element.
    placeholder: QualName,
}

impl Default for Dom {
    fn default() -> Self {
        Self {
            document: Node::new(NodeData::Document),
            placeholder: QualName::new(None, ns!(), LocalName::from("")),
        }
    }
}

impl TreeSink for Dom {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a>
        = ExpandedName<'a>
    where
        Self: 'a;

    fn finish(self) -> Self {
        self
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.document.clone()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        match &target.data {
            NodeData::Element { name, .. } => name.expanded(),
            _ => self.placeholder.expanded(),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        Node::new(NodeData::Element {
            name,
            attrs: RefCell::new(attrs),
            template_contents: RefCell::new(flags.template.then(|| Node::new(NodeData::Document))),
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        Node::new(NodeData::Comment { contents: text })
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        Node::new(NodeData::ProcessingInstruction {
            target,
            contents: data,
        })
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        let child = match child {
            NodeOrText::AppendText(text) => {
                if let Some(last) = parent.children.borrow().last()
                    && extend_text(last, &text)
                {
                    return;
                }
                text_node(text)
            }
            NodeOrText::AppendNode(node) => {
                detach(&node);
                node
            }
        };
        child.parent.set(Some(Rc::downgrade(parent)));
        parent.children.borrow_mut().push(child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if element.parent_node().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        self.append(
            &self.document,
            NodeOrText::AppendNode(Node::new(NodeData::Doctype { name })),
        );
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        if let NodeData::Element {
            template_contents, ..
        } = &target.data
            && let Some(contents) = template_contents.borrow().as_ref()
        {
            return contents.clone();
        }
        // The builder promises a template. Where that fails, what it inserts goes into a
        // fragment nothing holds, rather than into the document or a panic.
        Node::new(NodeData::Document)
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        Rc::ptr_eq(x, y)
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        let Some((parent, index)) = parent_and_index(sibling) else {
            // The builder promises a sibling in the tree. Where it is not, the node is
            // dropped rather than placed somewhere the builder did not ask for.
            return;
        };
        let child = match child {
            NodeOrText::AppendText(text) => {
                if index > 0 && extend_text(&parent.children.borrow()[index - 1], &text) {
                    return;
                }
                text_node(text)
            }
            NodeOrText::AppendNode(node) => {
                detach(&node);
                node
            }
        };
        // Detaching may have removed an earlier sibling from this same parent, so the
        // sibling's index is read again rather than reused.
        let index = parent
            .children
            .borrow()
            .iter()
            .position(|c| Rc::ptr_eq(c, sibling))
            .unwrap_or(index);
        child.parent.set(Some(Rc::downgrade(&parent)));
        parent.children.borrow_mut().insert(index, child);
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        let NodeData::Element {
            attrs: existing, ..
        } = &target.data
        else {
            return;
        };
        let mut existing = existing.borrow_mut();
        let names: HashSet<QualName> = existing.iter().map(|a| a.name.clone()).collect();
        existing.extend(attrs.into_iter().filter(|a| !names.contains(&a.name)));
    }

    fn remove_from_parent(&self, target: &Handle) {
        detach(target);
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if Rc::ptr_eq(node, new_parent) {
            return;
        }
        let moved = core::mem::take(&mut *node.children.borrow_mut());
        for child in &moved {
            child.parent.set(Some(Rc::downgrade(new_parent)));
        }
        new_parent.children.borrow_mut().extend(moved);
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        matches!(
            target.data,
            NodeData::Element {
                mathml_annotation_xml_integration_point: true,
                ..
            }
        )
    }
}

/// A node the serializer can write.
pub(crate) struct SerializableHandle(Handle);

impl From<Handle> for SerializableHandle {
    fn from(handle: Handle) -> Self {
        Self(handle)
    }
}

enum Step {
    Open(Handle),
    Close(QualName),
}

impl Serialize for SerializableHandle {
    fn serialize<S: Serializer>(
        &self,
        serializer: &mut S,
        scope: TraversalScope,
    ) -> io::Result<()> {
        // An explicit stack rather than recursion, for the same reason as `Drop`.
        let mut stack: Vec<Step> = match scope {
            IncludeNode => vec![Step::Open(self.0.clone())],
            ChildrenOnly(_) => self
                .0
                .children
                .borrow()
                .iter()
                .rev()
                .map(|c| Step::Open(c.clone()))
                .collect(),
        };
        while let Some(step) = stack.pop() {
            let node = match step {
                Step::Close(name) => {
                    serializer.end_elem(name)?;
                    continue;
                }
                Step::Open(node) => node,
            };
            match &node.data {
                NodeData::Element { name, attrs, .. } => {
                    serializer.start_elem(
                        name.clone(),
                        attrs.borrow().iter().map(|a| (&a.name, &a.value[..])),
                    )?;
                    stack.push(Step::Close(name.clone()));
                }
                NodeData::Document => {}
                NodeData::Doctype { name, .. } => serializer.write_doctype(name)?,
                NodeData::Text { contents } => serializer.write_text(&contents.borrow())?,
                NodeData::Comment { contents } => serializer.write_comment(contents)?,
                NodeData::ProcessingInstruction { target, contents } => {
                    serializer.write_processing_instruction(target, contents)?;
                }
            }
            if matches!(node.data, NodeData::Element { .. } | NodeData::Document) {
                stack.extend(
                    node.children
                        .borrow()
                        .iter()
                        .rev()
                        .map(|c| Step::Open(c.clone())),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html5ever::driver::ParseOpts;
    use html5ever::parse_document;
    use html5ever::tendril::TendrilSink;

    fn parse(html: &str) -> Dom {
        parse_document(Dom::default(), ParseOpts::default()).one(html)
    }

    fn serialized(dom: &Dom) -> String {
        let mut out = Vec::new();
        let handle: SerializableHandle = dom.document.clone().into();
        html5ever::serialize(
            &mut out,
            &handle,
            html5ever::serialize::SerializeOpts::default(),
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn builds_and_writes_back_a_document() {
        let dom = parse("<!doctype html><p>a<b>b</p>c");
        assert_eq!(
            serialized(&dom),
            "<!DOCTYPE html><html><head></head><body><p>a<b>b</b></p><b>c</b></body></html>"
        );
    }

    #[test]
    fn foster_parenting_and_adoption_agency_place_nodes_as_the_spec_does() {
        // Foster parenting goes through `append_before_sibling` and `append_based_on_parent_node`;
        // misnested formatting through `reparent_children` and `remove_from_parent`.
        let dom = parse("<table>x<tr><td>y</td></tr></table><a>1<p>2</a>3</p>");
        assert_eq!(
            serialized(&dom),
            "<html><head></head><body>x<table><tbody><tr><td>y</td></tr></tbody></table>\
             <a>1</a><p><a>2</a>3</p></body></html>"
        );
    }

    #[test]
    fn template_contents_are_held_apart_from_children() {
        let dom = parse("<template><p>t</p></template>");
        let html = dom.document.children.borrow()[0].clone();
        let head = html.children.borrow()[0].clone();
        let template = head.children.borrow()[0].clone();
        assert!(template.children.borrow().is_empty());
        let NodeData::Element {
            template_contents, ..
        } = &template.data
        else {
            panic!("not an element");
        };
        assert_eq!(
            template_contents
                .borrow()
                .as_ref()
                .map(|c| c.children.borrow().len()),
            Some(1)
        );
    }

    #[test]
    fn a_meta_content_ending_in_a_bare_charset_token_does_not_panic() {
        // The input the fuzz target minimized for the 0.39 tree builder (NFR-19).
        let dom = parse(r#"<meta http-equiv="Content-Type"content="charset">"#);
        assert!(serialized(&dom).contains("<meta"));
    }

    #[test]
    fn releasing_a_parent_leaves_a_still_referenced_child_whole() {
        let dom = parse("<div><p><i>kept</i></p></div>");
        let body = dom.document.children.borrow()[0].children.borrow()[1].clone();
        let div = body.children.borrow()[0].clone();
        let p = div.children.borrow()[0].clone();
        drop(dom);
        drop(body);
        drop(div);
        assert_eq!(p.children.borrow().len(), 1);
        assert_eq!(p.children.borrow()[0].children.borrow().len(), 1);
    }

    #[test]
    fn a_deep_tree_is_released_and_written_without_recursion() {
        // Deep enough that a recursive walk would exhaust a test thread's stack, shallow
        // enough that the tree builder's scope checks, quadratic in depth, stay quick.
        let depth = 20_000;
        let mut html = "<div>".repeat(depth);
        html.push('x');
        let dom = parse(&html);
        assert!(serialized(&dom).ends_with("</html>"));
        drop(dom);
    }
}
