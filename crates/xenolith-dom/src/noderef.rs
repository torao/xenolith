//! A borrowing handle for reading a node through chained method calls.
//!
//! A [`NodeId`] plus its [`Document`] is all it takes to read a node, but passing the document
//! through every step is repetitive. [`NodeRef`] bundles the two for the length of a borrow, so a
//! walk reads as `doc.node(root).first_child()` rather than a chain of `doc.method(id)` calls. It
//! is a read-only view. Mutation goes through [`Document`] with `&mut` access.
//!

use crate::node::{NodeId, NodeType};
use crate::{Document, NamedNodeMap, NodeList};

/// A read-only view of one node, borrowing its [`Document`].
///
/// # Examples
///
/// ```
/// use xenolith_dom::Document;
///
/// let mut doc = Document::new();
/// let root = doc.create_element("a")?;
/// let child = doc.create_element("b")?;
/// doc.append_child(root, child)?;
/// doc.append_child(doc.document_node(), root)?;
///
/// let first = doc.node(root).first_child().unwrap();
/// assert_eq!(first.node_name(), "b");
/// assert_eq!(first.parent().unwrap().id(), root);
/// # Ok::<(), xenolith_dom::DomException>(())
/// ```
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
  doc: &'a Document,
  id: NodeId,
}

impl<'a> NodeRef<'a> {
  pub(crate) const fn new(doc: &'a Document, id: NodeId) -> Self {
    Self { doc, id }
  }

  /// The node's [`NodeId`].
  #[must_use]
  pub const fn id(self) -> NodeId {
    self.id
  }

  /// The document this node belongs to.
  #[must_use]
  pub const fn document(self) -> &'a Document {
    self.doc
  }

  /// The kind of node.
  #[must_use]
  pub fn node_type(self) -> NodeType {
    self.doc.node_type(self.id)
  }

  /// The parent, if any.
  #[must_use]
  pub fn parent(self) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.parent(self.id))
  }

  /// The first child, if any.
  #[must_use]
  pub fn first_child(self) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.first_child(self.id))
  }

  /// The last child, if any.
  #[must_use]
  pub fn last_child(self) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.last_child(self.id))
  }

  /// The previous sibling, if any.
  #[must_use]
  pub fn previous_sibling(self) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.previous_sibling(self.id))
  }

  /// The next sibling, if any.
  #[must_use]
  pub fn next_sibling(self) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.next_sibling(self.id))
  }

  /// The children, first to last.
  pub fn children(self) -> impl Iterator<Item = NodeRef<'a>> {
    self.doc.children(self.id).map(move |id| NodeRef::new(self.doc, id))
  }

  /// The children as a live [`NodeList`].
  #[must_use]
  pub fn child_nodes(self) -> NodeList<'a> {
    self.doc.child_nodes(self.id)
  }

  /// The attributes as a live [`NamedNodeMap`].
  #[must_use]
  pub fn attributes(self) -> NamedNodeMap<'a> {
    self.doc.attributes(self.id)
  }

  /// The DOM `nodeName`.
  #[must_use]
  pub fn node_name(self) -> String {
    self.doc.node_name(self.id)
  }

  /// The DOM `nodeValue`, for the node kinds that have one.
  #[must_use]
  pub fn node_value(self) -> Option<&'a str> {
    self.doc.node_value(self.id)
  }

  /// The local part of an element's name.
  #[must_use]
  pub fn local_name(self) -> Option<&'a str> {
    self.doc.local_name(self.id)
  }

  /// The prefix of an element's name, if any.
  #[must_use]
  pub fn prefix(self) -> Option<&'a str> {
    self.doc.prefix(self.id)
  }

  /// The namespace name of an element, if any.
  #[must_use]
  pub fn namespace_uri(self) -> Option<&'a str> {
    self.doc.namespace_uri(self.id)
  }

  /// The DOM `textContent`: the character data of this node and its descendants.
  #[must_use]
  pub fn text_content(self) -> String {
    self.doc.text_content(self.id)
  }

  /// The value of an attribute, by qualified name.
  #[must_use]
  pub fn attribute(self, qualified_name: &str) -> Option<&'a str> {
    self.doc.attribute(self.id, qualified_name)
  }

  fn wrap(self, id: Option<NodeId>) -> Option<NodeRef<'a>> {
    id.map(|id| NodeRef::new(self.doc, id))
  }
}
