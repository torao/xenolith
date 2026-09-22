//! A borrowing handle for reading nodes via method chaining.
//!
//! While a [`NodeId`] and a [`Document`] are sufficient to read a node, passing the document at every step tends to be
//! verbose. [`NodeRef`] bundles these two together to represent a borrow of the node for the duration of the borrow.
//! This enables code that traverses nodes, such as `doc.node(root).first_child()`. This is a read-only view;
//! mutations are performed via the [`Document`] using `&mut` access.

use crate::dom::node::{NodeId, NodeType};
use crate::dom::{Document, NamedNodeMap, NodeList};

/// A read-only view of a node that borrows that [`Document`].
///
/// # Examples
///
/// ```
/// use xenolith::dom::Document;
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
/// # Ok::<(), xenolith::dom::DomException>(())
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

  /// Returns a live [`NodeList`] of elements that are descendants of this element and have the specified qualified
  /// name, in document order. Specifying `"*"` as the name matches all elements.
  ///
  /// Similar to the DOM's `Element.getElementsByTagName`, the element on which the method is called is *not* included
  /// in the list.
  #[must_use]
  pub fn get_elements_by_tag_name(self, name: &str) -> NodeList<'a> {
    self.doc.get_elements_by_tag_name(self.id, name)
  }

  /// Returns a live [`NodeList`] of elements that are descendants of this element and have the specified namespace and
  /// local name, in document order. Specifying `"*"` as the name matches all elements.
  #[must_use]
  pub fn get_elements_by_tag_name_ns(self, namespace: Option<&str>, local: &str) -> NodeList<'a> {
    self.doc.get_elements_by_tag_name_ns(self.id, namespace, local)
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

  /// The local part of an element's or attribute's name.
  #[must_use]
  pub fn local_name(self) -> Option<&'a str> {
    self.doc.local_name(self.id)
  }

  /// The prefix of an element's or attribute's name, if any.
  #[must_use]
  pub fn prefix(self) -> Option<&'a str> {
    self.doc.prefix(self.id)
  }

  /// The namespace name of an element or attribute, if any.
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
