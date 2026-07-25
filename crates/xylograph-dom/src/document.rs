//! The document: the arena that owns every node, and the operations over it.
//!
//! A [`Document`] holds the nodes in a `Vec` and a [`NamePool`] for their names. Nodes are made
//! with the `create_*` methods, joined into a tree with [`append_child`](Document::append_child)
//! and its siblings, and read back through the navigation and value accessors — or through a
//! [`NodeRef`](crate::NodeRef) for chained reads.

use xylograph_core::chars;
use xylograph_core::name::{NameId, NamePool, QName};

use crate::exception::{DomException, ExceptionCode, Result};
use crate::node::{Attribute, ElementData, NodeData, NodeId, NodeSlot, NodeType};
use crate::noderef::NodeRef;

/// An XML document: an arena of nodes with a tree over them.
///
/// # Examples
///
/// ```
/// use xylograph_dom::{Document, NodeType};
///
/// let mut doc = Document::new();
/// let root = doc.create_element("greeting")?;
/// let hello = doc.create_text_node("Hello");
/// doc.append_child(root, hello)?;
/// doc.append_child(doc.root(), root)?;
///
/// assert_eq!(doc.document_element(), Some(root));
/// assert_eq!(doc.node_type(root), NodeType::Element);
/// assert_eq!(doc.text_content(doc.root()), "Hello");
/// # Ok::<(), xylograph_dom::DomException>(())
/// ```
#[derive(Debug)]
pub struct Document {
  nodes: Vec<NodeSlot>,
  pool: NamePool,
}

impl Default for Document {
  fn default() -> Self {
    Self::new()
  }
}

impl Document {
  /// Creates an empty document — a lone document node, with no children yet.
  #[must_use]
  pub fn new() -> Self {
    Self { nodes: vec![NodeSlot::new(NodeData::Document)], pool: NamePool::new() }
  }

  /// The document node: the root of the tree, and the node every other one descends from.
  #[must_use]
  pub const fn root(&self) -> NodeId {
    NodeId(0)
  }

  /// The pool holding every name in the document, for rendering interned names.
  #[must_use]
  pub const fn pool(&self) -> &NamePool {
    &self.pool
  }

  // --- Construction -------------------------------------------------------------------------

  /// Creates an element with a qualified name and no namespace.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name.
  pub fn create_element(&mut self, qualified_name: &str) -> Result<NodeId> {
    let name = self.parse_qname(qualified_name, None)?;
    Ok(self.push(NodeData::Element(ElementData { name, attributes: Vec::new() })))
  }

  /// Creates an element in a namespace, from a namespace name and a qualified name.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name.
  pub fn create_element_ns(&mut self, namespace: Option<&str>, qualified_name: &str) -> Result<NodeId> {
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    Ok(self.push(NodeData::Element(ElementData { name, attributes: Vec::new() })))
  }

  /// Creates a text node.
  #[must_use]
  pub fn create_text_node(&mut self, data: &str) -> NodeId {
    self.push(NodeData::Text(data.to_owned()))
  }

  /// Creates a comment.
  #[must_use]
  pub fn create_comment(&mut self, data: &str) -> NodeId {
    self.push(NodeData::Comment(data.to_owned()))
  }

  /// Creates a CDATA section.
  #[must_use]
  pub fn create_cdata_section(&mut self, data: &str) -> NodeId {
    self.push(NodeData::CdataSection(data.to_owned()))
  }

  /// Creates a processing instruction.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::InvalidCharacter`] if `target` is not a legal name.
  pub fn create_processing_instruction(&mut self, target: &str, data: &str) -> Result<NodeId> {
    if !chars::is_name(target) {
      return Err(DomException::new(ExceptionCode::InvalidCharacter, format!("{target:?} is not a valid PI target")));
    }
    let target = self.pool.intern(target);
    Ok(self.push(NodeData::ProcessingInstruction { target, data: data.to_owned() }))
  }

  /// Creates a document fragment.
  #[must_use]
  pub fn create_document_fragment(&mut self) -> NodeId {
    self.push(NodeData::DocumentFragment)
  }

  /// Creates the document type node.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::InvalidCharacter`] if `name` is not a legal name.
  pub fn create_document_type(
    &mut self,
    name: &str,
    public_id: Option<&str>,
    system_id: Option<&str>,
  ) -> Result<NodeId> {
    if !chars::is_name(name) {
      return Err(DomException::new(ExceptionCode::InvalidCharacter, format!("{name:?} is not a valid name")));
    }
    let name = self.pool.intern(name);
    let data = NodeData::DocumentType {
      name,
      public_id: public_id.map(ToOwned::to_owned),
      system_id: system_id.map(ToOwned::to_owned),
    };
    Ok(self.push(data))
  }

  /// Interns a node and returns its handle. The node starts detached.
  fn push(&mut self, data: NodeData) -> NodeId {
    let id = NodeId(u32::try_from(self.nodes.len()).expect("a document holds fewer than 4 billion nodes"));
    self.nodes.push(NodeSlot::new(data));
    id
  }

  /// Splits a qualified name and interns its parts, forming a [`QName`] in `namespace`.
  fn parse_qname(&mut self, qualified_name: &str, namespace: Option<NameId>) -> Result<QName> {
    let Some((prefix, local)) = chars::split_qname(qualified_name) else {
      return Err(DomException::new(
        ExceptionCode::InvalidCharacter,
        format!("{qualified_name:?} is not a valid qualified name"),
      ));
    };
    let prefix = prefix.map(|p| self.pool.intern(p));
    let local = self.pool.intern(local);
    Ok(QName::new(prefix, namespace, local))
  }

  // --- Navigation ---------------------------------------------------------------------------

  /// A [`NodeRef`] over a node, for chained reads.
  #[must_use]
  pub fn node(&self, id: NodeId) -> NodeRef<'_> {
    NodeRef::new(self, id)
  }

  /// The kind of a node.
  #[must_use]
  pub fn node_type(&self, id: NodeId) -> NodeType {
    self.slot(id).data.node_type()
  }

  /// The parent of a node, or `None` for the document root and any detached node.
  #[must_use]
  pub fn parent(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).parent
  }

  /// The first child of a node, if it has one.
  #[must_use]
  pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).first_child
  }

  /// The last child of a node, if it has one.
  #[must_use]
  pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).last_child
  }

  /// The node before this one under the same parent, if any.
  #[must_use]
  pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).previous_sibling
  }

  /// The node after this one under the same parent, if any.
  #[must_use]
  pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).next_sibling
  }

  /// The children of a node, first to last.
  pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let mut next = self.slot(id).first_child;
    std::iter::from_fn(move || {
      let current = next?;
      next = self.slot(current).next_sibling;
      Some(current)
    })
  }

  /// The root element of the document — its single element child — if the tree has one.
  #[must_use]
  pub fn document_element(&self) -> Option<NodeId> {
    self.children(self.root()).find(|&child| self.node_type(child) == NodeType::Element)
  }

  /// The document type node, if the document has one.
  #[must_use]
  pub fn doctype(&self) -> Option<NodeId> {
    self.children(self.root()).find(|&child| self.node_type(child) == NodeType::DocumentType)
  }

  // --- Names and values ---------------------------------------------------------------------

  /// The DOM `nodeName`: an element's qualified name, a PI's target, or the `#name` a node kind
  /// reports (`#text`, `#comment`, `#document`, and so on).
  #[must_use]
  pub fn node_name(&self, id: NodeId) -> String {
    match &self.slot(id).data {
      NodeData::Element(element) => element.name.to_lexical(&self.pool),
      NodeData::ProcessingInstruction { target, .. } => self.pool.resolve(*target).to_owned(),
      NodeData::DocumentType { name, .. } => self.pool.resolve(*name).to_owned(),
      NodeData::Text(_) => "#text".to_owned(),
      NodeData::CdataSection(_) => "#cdata-section".to_owned(),
      NodeData::Comment(_) => "#comment".to_owned(),
      NodeData::Document => "#document".to_owned(),
      NodeData::DocumentFragment => "#document-fragment".to_owned(),
    }
  }

  /// The DOM `nodeValue`: the character data of a text, CDATA, comment or PI node; `None` for
  /// the kinds that have no value of their own.
  #[must_use]
  pub fn node_value(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::Text(data) | NodeData::CdataSection(data) | NodeData::Comment(data) => Some(data),
      NodeData::ProcessingInstruction { data, .. } => Some(data),
      _ => None,
    }
  }

  /// The local part of an element's name.
  #[must_use]
  pub fn local_name(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::Element(element) => Some(self.pool.resolve(element.name.local())),
      _ => None,
    }
  }

  /// The namespace prefix of an element's name, if it has one.
  #[must_use]
  pub fn prefix(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::Element(element) => element.name.prefix.map(|p| self.pool.resolve(p)),
      _ => None,
    }
  }

  /// The namespace name of an element, if it is in one.
  #[must_use]
  pub fn namespace_uri(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::Element(element) => element.name.namespace().map(|ns| self.pool.resolve(ns)),
      _ => None,
    }
  }

  /// The public identifier of a document type node, if it has one.
  #[must_use]
  pub fn public_id(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::DocumentType { public_id, .. } => public_id.as_deref(),
      _ => None,
    }
  }

  /// The system identifier of a document type node, if it has one.
  #[must_use]
  pub fn system_id(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::DocumentType { system_id, .. } => system_id.as_deref(),
      _ => None,
    }
  }

  /// The DOM `textContent`: the character data of the node and all its descendants, in order.
  ///
  /// Comments and processing instructions contribute nothing, matching the DOM.
  #[must_use]
  pub fn text_content(&self, id: NodeId) -> String {
    let mut out = String::new();
    self.append_text_content(id, &mut out);
    out
  }

  fn append_text_content(&self, id: NodeId, out: &mut String) {
    match &self.slot(id).data {
      NodeData::Text(data) | NodeData::CdataSection(data) => out.push_str(data),
      NodeData::Element(_) | NodeData::Document | NodeData::DocumentFragment => {
        for child in self.children(id) {
          self.append_text_content(child, out);
        }
      }
      _ => {}
    }
  }

  // --- Attributes ---------------------------------------------------------------------------

  /// Sets an attribute by qualified name, adding it or replacing its value.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NotSupported`] if the node is not an element, or
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name.
  pub fn set_attribute(&mut self, element: NodeId, qualified_name: &str, value: &str) -> Result<()> {
    let name = self.parse_qname(qualified_name, None)?;
    self.put_attribute(element, name, value)
  }

  /// Sets an attribute in a namespace, adding it or replacing the value of the one with the same
  /// namespace and local name.
  ///
  /// # Errors
  ///
  /// As [`set_attribute`](Self::set_attribute).
  pub fn set_attribute_ns(
    &mut self,
    element: NodeId,
    namespace: Option<&str>,
    qualified_name: &str,
    value: &str,
  ) -> Result<()> {
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    self.put_attribute(element, name, value)
  }

  fn put_attribute(&mut self, element: NodeId, name: QName, value: &str) -> Result<()> {
    let attributes = self.element_attributes_mut(element)?;
    if let Some(existing) = attributes.iter_mut().find(|a| a.name.expanded == name.expanded) {
      existing.value = value.to_owned();
    } else {
      attributes.push(Attribute { name, value: value.to_owned() });
    }
    Ok(())
  }

  /// The value of an element's attribute, by qualified name.
  #[must_use]
  pub fn attribute(&self, element: NodeId, qualified_name: &str) -> Option<&str> {
    let (prefix, local) = chars::split_qname(qualified_name)?;
    let prefix = match prefix {
      Some(p) => Some(self.pool.get(p)?),
      None => None,
    };
    let local = self.pool.get(local)?;
    let attributes = self.element_data(element)?;
    attributes.attributes.iter().find(|a| a.name.prefix == prefix && a.name.local() == local).map(|a| a.value.as_str())
  }

  /// The value of an element's attribute, by namespace name and local name.
  #[must_use]
  pub fn attribute_ns(&self, element: NodeId, namespace: Option<&str>, local: &str) -> Option<&str> {
    let namespace = match namespace {
      Some(ns) => Some(self.pool.get(ns)?),
      None => None,
    };
    let local = self.pool.get(local)?;
    let attributes = self.element_data(element)?;
    attributes
      .attributes
      .iter()
      .find(|a| a.name.namespace() == namespace && a.name.local() == local)
      .map(|a| a.value.as_str())
  }

  /// Whether an element has an attribute with the given qualified name.
  #[must_use]
  pub fn has_attribute(&self, element: NodeId, qualified_name: &str) -> bool {
    self.attribute(element, qualified_name).is_some()
  }

  /// The qualified names of an element's attributes, in document order.
  #[must_use]
  pub fn attribute_names(&self, element: NodeId) -> Vec<String> {
    match self.element_data(element) {
      Some(data) => data.attributes.iter().map(|a| a.name.to_lexical(&self.pool)).collect(),
      None => Vec::new(),
    }
  }

  /// Removes an element's attribute by qualified name; a no-op if it has none such.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NotSupported`] if the node is not an element.
  pub fn remove_attribute(&mut self, element: NodeId, qualified_name: &str) -> Result<()> {
    let Some((prefix, local)) = chars::split_qname(qualified_name) else { return Ok(()) };
    let prefix = prefix.and_then(|p| self.pool.get(p));
    let Some(local) = self.pool.get(local) else { return Ok(()) };
    let attributes = self.element_attributes_mut(element)?;
    attributes.retain(|a| !(a.name.prefix == prefix && a.name.local() == local));
    Ok(())
  }

  fn element_data(&self, id: NodeId) -> Option<&ElementData> {
    match &self.slot(id).data {
      NodeData::Element(data) => Some(data),
      _ => None,
    }
  }

  fn element_attributes_mut(&mut self, id: NodeId) -> Result<&mut Vec<Attribute>> {
    match &mut self.nodes[id.index()].data {
      NodeData::Element(data) => Ok(&mut data.attributes),
      _ => Err(DomException::new(ExceptionCode::NotSupported, "attributes belong to elements only")),
    }
  }

  // --- Mutation -----------------------------------------------------------------------------

  /// Appends `child` as the last child of `parent`, detaching it from any current parent first.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::HierarchyRequest`] if `parent` cannot hold children, or if `child` is
  /// `parent` or one of its ancestors (which would make a cycle).
  pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId> {
    self.insert_before(parent, child, None)
  }

  /// Inserts `child` under `parent` before `reference`, or at the end when `reference` is
  /// `None`. Detaches `child` from any current parent first.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::HierarchyRequest`] if `parent` cannot hold children, or the insertion
  /// would make a cycle; [`ExceptionCode::NotFound`] if `reference` is not a child of `parent`.
  pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) -> Result<NodeId> {
    if !self.slot(parent).data.is_container() {
      let name = self.node_name(parent);
      return Err(DomException::new(ExceptionCode::HierarchyRequest, format!("\"{name}\" cannot have children")));
    }
    if child == parent || self.is_ancestor(child, parent) {
      return Err(DomException::new(ExceptionCode::HierarchyRequest, "a node cannot be made a descendant of itself"));
    }
    if let Some(reference) = reference {
      if self.slot(reference).parent != Some(parent) {
        return Err(DomException::new(ExceptionCode::NotFound, "the reference node is not a child of the parent"));
      }
    }

    self.detach(child);
    match reference {
      Some(reference) => self.link_before(parent, child, reference),
      None => self.link_last(parent, child),
    }
    Ok(child)
  }

  /// Removes `child` from `parent`, leaving it detached.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NotFound`] if `child` is not a child of `parent`.
  pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId> {
    if self.slot(child).parent != Some(parent) {
      return Err(DomException::new(ExceptionCode::NotFound, "the node is not a child of the parent"));
    }
    self.detach(child);
    Ok(child)
  }

  /// Whether `ancestor` is `node` or lies above it in the tree.
  fn is_ancestor(&self, ancestor: NodeId, node: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
      if id == ancestor {
        return true;
      }
      current = self.slot(id).parent;
    }
    false
  }

  /// Unlinks a node from its parent and siblings, if it has any.
  fn detach(&mut self, id: NodeId) {
    let (parent, previous, next) = {
      let slot = self.slot(id);
      (slot.parent, slot.previous_sibling, slot.next_sibling)
    };
    match previous {
      Some(previous) => self.nodes[previous.index()].next_sibling = next,
      None => {
        if let Some(parent) = parent {
          self.nodes[parent.index()].first_child = next;
        }
      }
    }
    match next {
      Some(next) => self.nodes[next.index()].previous_sibling = previous,
      None => {
        if let Some(parent) = parent {
          self.nodes[parent.index()].last_child = previous;
        }
      }
    }
    let slot = &mut self.nodes[id.index()];
    slot.parent = None;
    slot.previous_sibling = None;
    slot.next_sibling = None;
  }

  /// Links a detached node as the last child of `parent`.
  fn link_last(&mut self, parent: NodeId, child: NodeId) {
    let previous = self.nodes[parent.index()].last_child;
    self.nodes[child.index()].parent = Some(parent);
    self.nodes[child.index()].previous_sibling = previous;
    self.nodes[child.index()].next_sibling = None;
    match previous {
      Some(previous) => self.nodes[previous.index()].next_sibling = Some(child),
      None => self.nodes[parent.index()].first_child = Some(child),
    }
    self.nodes[parent.index()].last_child = Some(child);
  }

  /// Links a detached node before `reference` under `parent`.
  fn link_before(&mut self, parent: NodeId, child: NodeId, reference: NodeId) {
    let previous = self.nodes[reference.index()].previous_sibling;
    self.nodes[child.index()].parent = Some(parent);
    self.nodes[child.index()].previous_sibling = previous;
    self.nodes[child.index()].next_sibling = Some(reference);
    self.nodes[reference.index()].previous_sibling = Some(child);
    match previous {
      Some(previous) => self.nodes[previous.index()].next_sibling = Some(child),
      None => self.nodes[parent.index()].first_child = Some(child),
    }
  }

  fn slot(&self, id: NodeId) -> &NodeSlot {
    &self.nodes[id.index()]
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Builds `<r a="1"><b/>text<c/></r>` under the document root, returning (r, b, c).
  fn sample() -> (Document, NodeId, NodeId, NodeId) {
    let mut doc = Document::new();
    let r = doc.create_element("r").unwrap();
    doc.set_attribute(r, "a", "1").unwrap();
    let b = doc.create_element("b").unwrap();
    let text = doc.create_text_node("text");
    let c = doc.create_element("c").unwrap();
    doc.append_child(r, b).unwrap();
    doc.append_child(r, text).unwrap();
    doc.append_child(r, c).unwrap();
    doc.append_child(doc.root(), r).unwrap();
    (doc, r, b, c)
  }

  #[test]
  fn navigates_the_tree() {
    let (doc, r, b, c) = sample();
    assert_eq!(doc.document_element(), Some(r));
    assert_eq!(doc.parent(r), Some(doc.root()));
    assert_eq!(doc.first_child(r), Some(b));
    assert_eq!(doc.last_child(r), Some(c));
    assert_eq!(doc.next_sibling(b).and_then(|t| doc.next_sibling(t)), Some(c));
    assert_eq!(doc.previous_sibling(c).and_then(|t| doc.previous_sibling(t)), Some(b));
    let children: Vec<_> = doc.children(r).collect();
    assert_eq!(children.len(), 3);
  }

  #[test]
  fn reports_names_types_and_values() {
    let (doc, r, b, _) = sample();
    assert_eq!(doc.node_type(r), NodeType::Element);
    assert_eq!(doc.node_name(r), "r");
    assert_eq!(doc.local_name(b), Some("b"));
    let text = doc.next_sibling(b).unwrap();
    assert_eq!(doc.node_type(text), NodeType::Text);
    assert_eq!(doc.node_name(text), "#text");
    assert_eq!(doc.node_value(text), Some("text"));
    assert_eq!(doc.text_content(r), "text");
  }

  #[test]
  fn namespaced_names_split_into_parts() {
    let mut doc = Document::new();
    let e = doc.create_element_ns(Some("urn:x"), "p:a").unwrap();
    assert_eq!(doc.node_name(e), "p:a");
    assert_eq!(doc.local_name(e), Some("a"));
    assert_eq!(doc.prefix(e), Some("p"));
    assert_eq!(doc.namespace_uri(e), Some("urn:x"));
  }

  #[test]
  fn reads_and_removes_attributes() {
    let (mut doc, r, _, _) = sample();
    assert_eq!(doc.attribute(r, "a"), Some("1"));
    assert!(doc.has_attribute(r, "a"));
    doc.set_attribute(r, "a", "2").unwrap();
    assert_eq!(doc.attribute(r, "a"), Some("2"), "setting an existing attribute replaces it");
    doc.set_attribute(r, "b", "y").unwrap();
    assert_eq!(doc.attribute_names(r), ["a", "b"]);
    doc.remove_attribute(r, "a").unwrap();
    assert!(!doc.has_attribute(r, "a"));
  }

  #[test]
  fn namespaced_attributes_are_found_by_namespace() {
    let mut doc = Document::new();
    let e = doc.create_element("e").unwrap();
    doc.set_attribute_ns(e, Some("urn:x"), "p:k", "v").unwrap();
    assert_eq!(doc.attribute_ns(e, Some("urn:x"), "k"), Some("v"));
  }

  #[test]
  fn insert_before_places_a_node_among_its_siblings() {
    let (mut doc, r, b, c) = sample();
    let x = doc.create_element("x").unwrap();
    doc.insert_before(r, x, Some(c)).unwrap();
    assert_eq!(doc.previous_sibling(c), Some(x));
    assert_eq!(doc.first_child(r), Some(b));
  }

  #[test]
  fn moving_a_node_detaches_it_from_its_old_parent() {
    let (mut doc, r, b, c) = sample();
    // Move b under c; it must leave r's child list, so r now starts with the text node.
    doc.append_child(c, b).unwrap();
    assert_eq!(doc.parent(b), Some(c));
    assert!(!doc.children(r).any(|n| n == b));
    assert_eq!(doc.node_type(doc.first_child(r).unwrap()), NodeType::Text);
  }

  #[test]
  fn remove_child_detaches() {
    let (mut doc, r, b, _) = sample();
    doc.remove_child(r, b).unwrap();
    assert_eq!(doc.parent(b), None);
    assert!(!doc.children(r).any(|n| n == b));
  }

  #[test]
  fn a_cycle_is_refused() {
    let (mut doc, r, b, _) = sample();
    let error = doc.append_child(b, r).unwrap_err();
    assert_eq!(error.code(), ExceptionCode::HierarchyRequest);
  }

  #[test]
  fn a_leaf_cannot_take_children() {
    let mut doc = Document::new();
    let text = doc.create_text_node("t");
    let child = doc.create_element("c").unwrap();
    let error = doc.append_child(text, child).unwrap_err();
    assert_eq!(error.code(), ExceptionCode::HierarchyRequest);
  }

  #[test]
  fn insert_before_a_non_child_is_not_found() {
    let (mut doc, r, _, _) = sample();
    let stray = doc.create_element("stray").unwrap();
    let x = doc.create_element("x").unwrap();
    let error = doc.insert_before(r, x, Some(stray)).unwrap_err();
    assert_eq!(error.code(), ExceptionCode::NotFound);
  }

  #[test]
  fn an_invalid_name_is_rejected() {
    let mut doc = Document::new();
    assert_eq!(doc.create_element("1bad").unwrap_err().code(), ExceptionCode::InvalidCharacter);
  }
}
