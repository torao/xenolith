//! A borrowing handle for reading nodes via method chaining.
//!
//! A node is read through a [`NodeId`] together with its [`Document`]. [`NodeRef`] bundles these two together to
//! represent a borrow of the node for the duration of the borrow. This enables code that traverses nodes, such as
//! `doc.node(root)?.first_child()`. [`Document::node`] makes one after checking that the node is of that document, and
//! every node reached from it is too, so reading through it cannot fail that way again. This is a read-only view;
//! mutations are performed via the [`Document`] using `&mut` access.

use crate::dom::node::{NodeId, NodeType};
use crate::dom::{Document, NamedNodeMap, NodeList, Visit};

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
/// let first = doc.node(root)?.first_child().unwrap();
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

  /// The value of an attribute, by namespace and local name.
  #[must_use]
  pub fn attribute_ns(self, namespace: Option<&str>, local: &str) -> Option<&'a str> {
    self.doc.attribute_ns(self.id, namespace, local)
  }

  /// Whether the element has an attribute with this qualified name.
  #[must_use]
  pub fn has_attribute(self, qualified_name: &str) -> bool {
    self.doc.has_attribute(self.id, qualified_name)
  }

  /// The qualified names of the element's attributes, in order.
  #[must_use]
  pub fn attribute_names(self) -> Vec<String> {
    self.doc.attribute_names(self.id)
  }

  /// The attribute node of the element, by qualified name.
  #[must_use]
  pub fn get_attribute_node(self, qualified_name: &str) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.get_attribute_node(self.id, qualified_name))
  }

  /// The attribute node of the element, by namespace and local name.
  #[must_use]
  pub fn get_attribute_node_ns(self, namespace: Option<&str>, local: &str) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.get_attribute_node_ns(self.id, namespace, local))
  }

  /// The element an attribute node belongs to, if it is attached to one.
  #[must_use]
  pub fn owner_element(self) -> Option<NodeRef<'a>> {
    self.wrap(self.doc.owner_element(self.id))
  }

  /// The public identifier of a document type node, if it has one.
  #[must_use]
  pub fn public_id(self) -> Option<&'a str> {
    self.doc.public_id(self.id)
  }

  /// The system identifier of a document type node, if it has one.
  #[must_use]
  pub fn system_id(self) -> Option<&'a str> {
    self.doc.system_id(self.id)
  }

  /// The DOM `baseURI` (XML Base): the base URI in effect where the node is.
  #[must_use]
  pub fn base_uri(self) -> Option<String> {
    self.doc.base_uri(self.id)
  }

  /// Traverses the subtree rooted at this node and reports each node's entering and leaving.
  ///
  /// This is the crate's standard way to traverse the tree and is preferred over recursive traversal. Refer to
  /// [`Walk`](crate::dom::Walk) for details on the reported events and their order, and
  /// [traversal depth](crate#traversal) for why recursion does not occur. Attributes are not children, so the walk does
  /// not reach them.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::dom::{Document, Visit};
  ///
  /// // <a><b>hi</b></a>
  /// let mut doc = Document::new();
  /// let root = doc.create_element("a")?;
  /// let child = doc.create_element("b")?;
  /// let text = doc.create_text_node("hi")?;
  /// doc.append_child(child, text)?;
  /// doc.append_child(root, child)?;
  /// doc.append_child(doc.document_node(), root)?;
  ///
  /// // Both sides of every node, so the shape of the subtree comes back out. A text node has no end tag in XML, but
  /// // the walk still enters and leaves it, as it does any other node.
  /// let shape: String = doc
  ///   .node(root)?
  ///   .walk()
  ///   .map(|(visit, node)| match visit {
  ///     Visit::Enter => format!("<{}>", node.node_name()),
  ///     Visit::Leave => format!("</{}>", node.node_name()),
  ///   })
  ///   .collect();
  /// assert_eq!(shape, "<a><b><#text></#text></b></a>");
  /// # Ok::<(), xenolith::dom::DomException>(())
  /// ```
  pub fn walk(self) -> impl Iterator<Item = (Visit, NodeRef<'a>)> {
    self.doc.walk(self.id).map(move |(visit, id)| (visit, NodeRef::new(self.doc, id)))
  }

  fn wrap(self, id: Option<NodeId>) -> Option<NodeRef<'a>> {
    id.map(|id| NodeRef::new(self.doc, id))
  }
}
