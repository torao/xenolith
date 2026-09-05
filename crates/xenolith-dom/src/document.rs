//! The document: the [arena](crate#arena) that owns every node and the operations over it.
//!
//! A [`Document`] owns every node and a [`NamePool`] for their names. Nodes are made with the `create_*` methods,
//! joined into a tree with [`append_child`](Document::append_child) and its siblings, and read back through the
//! navigation and value accessors, or through a [`NodeRef`](crate::NodeRef) for chained reads.
//!

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};

use xenolith_core::chars;
use xenolith_core::name::{NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};

use crate::collection::{NamedNodeMap, NodeList, Query};
use crate::exception::{DomException, ExceptionCode, Result};
use crate::node::{AttrData, ElementData, NodeData, NodeId, NodeSlot, NodeType};
use crate::noderef::NodeRef;
use crate::walk::{Visit, Walk};

/// An XML document: an [arena](crate#arena) of nodes with a tree over them.
///
/// **Be aware of memory leaks**. The arena does not free storage. Once a node is created, it retains its allocated
/// space for the document's lifetime, even if it's not attached to the document. Furthermore,
/// [`remove_child`](Self::remove_child) detaches the node but still does not free the memory. The only way to free
/// them all is to drop the document.
///
/// Every method that takes a [`NodeId`] requires this document. A handle from elsewhere is rejected with
/// [`ExceptionCode::WRONG_DOCUMENT_ERR`], where the method returns a [`Result`] and panics rather than returning a
/// value. Use [`owns`](Self::owns) to test a handle whose origin is in doubt.
///
/// # Examples
///
/// ```
/// use xenolith_dom::{Document, NodeType};
///
/// let mut doc = Document::new();
/// let root = doc.create_element("greeting")?;
/// let hello = doc.create_text_node("Hello");
/// doc.append_child(root, hello)?;
/// doc.append_child(doc.document_node(), root)?;
///
/// assert_eq!(doc.document_element(), Some(root));
/// assert_eq!(doc.node_type(root), NodeType::ELEMENT_NODE);
/// assert_eq!(doc.text_content(doc.document_node()), "Hello");
/// # Ok::<(), xenolith_dom::DomException>(())
/// ```
#[derive(Debug)]
pub struct Document {
  /// What this document's own [`NodeId`]s carry, so a handle from elsewhere is recognized.
  id: NonZeroU32,
  nodes: Vec<NodeSlot>,
  pool: NamePool,
  /// The document's own base URI (its system identifier), interned. It falls back to a node with no nearer base and
  /// is `None` unless recorded when the tree was built.
  base: Option<NameId>,
}

impl Default for Document {
  fn default() -> Self {
    Self::new()
  }
}

impl Document {
  /// Creates an empty document, a lone document node with no children yet.
  ///
  #[must_use]
  pub fn new() -> Self {
    Self { id: next_document_id(), nodes: vec![NodeSlot::new(NodeData::Document)], pool: NamePool::new(), base: None }
  }

  /// The document node: the root of the tree, and the node every other one descends from.
  ///
  /// This is the [`NodeId`] of the document itself, and it always exists. Note that this is not the root element,
  /// which [`document_element`](Self::document_element) reports and which is absent until an element is placed here.
  ///
  #[must_use]
  pub const fn document_node(&self) -> NodeId {
    NodeId::new(self.id, 0)
  }

  /// The pool holding every name in the document, for rendering interned names.
  ///
  #[must_use]
  pub const fn pool(&self) -> &NamePool {
    &self.pool
  }

  /// Whether `id` refers to a node in this document.
  ///
  /// A [`NodeId`] handle is only valid with the document that made it. Call this to test one whose origin is in doubt,
  /// rather than passing it to an accessor and having the mistake reported.
  ///
  #[must_use]
  pub fn owns(&self, id: NodeId) -> bool {
    id.document() == self.id
  }

  /// Checks that `id` belongs to a node of this document, for the methods that report an error.
  ///
  fn require_own(&self, id: NodeId) -> Result<()> {
    if self.owns(id) {
      Ok(())
    } else {
      Err(DomException::new(ExceptionCode::WRONG_DOCUMENT_ERR, "the node was made by another document"))
    }
  }

  // --- Construction -------------------------------------------------------------------------

  /// Creates an element with a qualified name and no namespace.
  ///
  /// The new node is not yet attached. As with other `create*` methods, please use [`append_child`](Self::append_child)
  /// to place it in the tree.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `qualified_name` is not a legal name.
  ///
  pub fn create_element(&mut self, qualified_name: &str) -> Result<NodeId> {
    let name = self.parse_qname(qualified_name, None)?;
    Ok(self.push(NodeData::Element(ElementData { name, attributes: Vec::new(), base: None })))
  }

  /// Creates an element in a namespace that has not yet been attached, from a namespace name and a qualified name.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `qualified_name` is not a legal name;
  /// [`ExceptionCode::NAMESPACE_ERR`] if the prefix and the namespace are inconsistent (a prefix with no namespace, or
  /// the `xml` prefix bound to anything but the XML namespace).
  ///
  pub fn create_element_ns(&mut self, namespace: Option<&str>, qualified_name: &str) -> Result<NodeId> {
    check_qname_namespace(namespace, qualified_name, false)?;
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    Ok(self.push(NodeData::Element(ElementData { name, attributes: Vec::new(), base: None })))
  }

  /// Creates a text node.
  ///
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
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `target` is not a legal name.
  ///
  pub fn create_processing_instruction(&mut self, target: &str, data: &str) -> Result<NodeId> {
    if !chars::is_name(target) {
      return Err(DomException::new(ExceptionCode::INVALID_CHARACTER_ERR, invalid_name_message(target, "PI target")));
    }
    let target = self.pool.intern(target);
    Ok(self.push(NodeData::ProcessingInstruction { target, data: data.to_owned() }))
  }

  /// Creates a document fragment.
  ///
  #[must_use]
  pub fn create_document_fragment(&mut self) -> NodeId {
    self.push(NodeData::DocumentFragment)
  }

  /// Creates the document type node.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `name` is not a legal name.
  ///
  pub fn create_document_type(
    &mut self,
    name: &str,
    public_id: Option<&str>,
    system_id: Option<&str>,
  ) -> Result<NodeId> {
    if !chars::is_name(name) {
      return Err(DomException::new(
        ExceptionCode::INVALID_CHARACTER_ERR,
        invalid_name_message(name, "document type name"),
      ));
    }
    let name = self.pool.intern(name);
    let data = NodeData::DocumentType {
      name,
      public_id: public_id.map(ToOwned::to_owned),
      system_id: system_id.map(ToOwned::to_owned),
    };
    Ok(self.push(data))
  }

  /// Copies a node from another document into this one, returning a new detached node.
  ///
  /// This is `importNode` in the W3C DOM specification. The copy belongs to this document, with its names re-interned
  /// here, and is detached, ready to be placed in the tree. With `deep`, the node's descendants come too. An element's
  /// attributes always come with it. The base URI recorded on an element is not copied. It is a computed property, and
  /// a caller that needs to preserve it (such as XInclude) writes an `xml:base` attribute instead.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_SUPPORTED_ERR`] for a node that cannot be imported: a document, a document type, or a bare
  /// attribute.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_dom::Document;
  ///
  /// let mut source = Document::new();
  /// let s = source.create_element("a")?;
  /// let text = source.create_text_node("hi");
  /// source.append_child(s, text)?;
  ///
  /// let mut doc = Document::new();
  /// let copy = doc.import_node(&source, s, true)?;
  /// doc.append_child(doc.document_node(), copy)?;
  /// assert_eq!(doc.text_content(copy), "hi");
  /// # Ok::<(), xenolith_dom::DomException>(())
  /// ```
  pub fn import_node(&mut self, source: &Document, node: NodeId, deep: bool) -> Result<NodeId> {
    source.require_own(node)?;
    let imported = match &source.slot(node).data {
      NodeData::Element(_) => {
        let namespace = source.namespace_uri(node).map(ToOwned::to_owned);
        let element = self.create_element_ns(namespace.as_deref(), &source.node_name(node))?;
        let attributes: Vec<NodeId> = source.attributes(node).iter().collect();
        for attr in attributes {
          let name = source.node_name(attr);
          let value = source.node_value(attr).unwrap_or_default();
          match source.namespace_uri(attr) {
            Some(namespace) => self.set_attribute_ns(element, Some(namespace), &name, value)?,
            None => self.set_attribute(element, &name, value)?,
          }
        }
        if deep {
          for child in source.children(node).collect::<Vec<_>>() {
            let child = self.import_node(source, child, true)?;
            self.append_child(element, child)?;
          }
        }
        element
      }
      NodeData::Text(data) => self.create_text_node(data),
      NodeData::CdataSection(data) => self.create_cdata_section(data),
      NodeData::Comment(data) => self.create_comment(data),
      NodeData::ProcessingInstruction { .. } => {
        self.create_processing_instruction(&source.node_name(node), source.node_value(node).unwrap_or_default())?
      }
      NodeData::DocumentFragment => {
        let fragment = self.create_document_fragment();
        if deep {
          for child in source.children(node).collect::<Vec<_>>() {
            let child = self.import_node(source, child, true)?;
            self.append_child(fragment, child)?;
          }
        }
        fragment
      }
      NodeData::Document | NodeData::DocumentType { .. } | NodeData::Attribute(_) => {
        return Err(DomException::new(ExceptionCode::NOT_SUPPORTED_ERR, "this kind of node cannot be imported"));
      }
    };
    Ok(imported)
  }

  /// Copies a node within this document, returning a new detached node. This is `cloneNode` in the W3C DOM
  /// specification.
  ///
  /// With `deep`, descendants come too; an element's attributes always do. As with [`import_node`](Self::import_node),
  /// the computed base URI is not copied.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_SUPPORTED_ERR`] for a node that cannot be cloned this way: a document, a
  /// document type, or a bare attribute.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn clone_node(&mut self, node: NodeId, deep: bool) -> Result<NodeId> {
    self.require_own(node)?;
    let cloned = self.clone_shallow(node)?;
    if deep {
      self.clone_descendants(node, cloned)?;
    }
    Ok(cloned)
  }

  /// Copies one node on its own, leaving out its descendants. An element's attributes always come with it.
  fn clone_shallow(&mut self, node: NodeId) -> Result<NodeId> {
    let cloned = match self.slot(node).data.node_type() {
      NodeType::ELEMENT_NODE => {
        let name = self.node_name(node);
        let namespace = self.namespace_uri(node).map(ToOwned::to_owned);
        let attributes: Vec<(String, Option<String>, String)> = self
          .attributes(node)
          .iter()
          .map(|attr| {
            (
              self.node_name(attr),
              self.namespace_uri(attr).map(ToOwned::to_owned),
              self.node_value(attr).unwrap_or_default().to_owned(),
            )
          })
          .collect();
        let element = self.create_element_ns(namespace.as_deref(), &name)?;
        for (name, namespace, value) in attributes {
          match namespace {
            Some(namespace) => self.set_attribute_ns(element, Some(&namespace), &name, &value)?,
            None => self.set_attribute(element, &name, &value)?,
          }
        }
        element
      }
      NodeType::TEXT_NODE => {
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_text_node(&data)
      }
      NodeType::CDATA_SECTION_NODE => {
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_cdata_section(&data)
      }
      NodeType::COMMENT_NODE => {
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_comment(&data)
      }
      NodeType::PROCESSING_INSTRUCTION_NODE => {
        let target = self.node_name(node);
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_processing_instruction(&target, &data)?
      }
      NodeType::DOCUMENT_FRAGMENT_NODE => self.create_document_fragment(),
      NodeType::DOCUMENT_NODE | NodeType::DOCUMENT_TYPE_NODE | NodeType::ATTRIBUTE_NODE => {
        return Err(DomException::new(ExceptionCode::NOT_SUPPORTED_ERR, "this kind of node cannot be cloned"));
      }
    };
    Ok(cloned)
  }

  /// Copies the descendants of `source` under `target`, which is already a copy of `source`.
  ///
  /// This is a traversal that does not use [`walk`](Self::walk). Copying changes the document as it reads it, but a
  /// [`Walk`] borrows the document for as long as it runs, so we can't hold both at once. It also carries a value down
  /// the tree, the copy each node is appended to, which a walk over nodes alone does not. For these reasons, this keeps
  /// its own stack and still does not recurse (see [traversal depth](crate#traversal)).
  ///
  fn clone_descendants(&mut self, source: NodeId, target: NodeId) -> Result<()> {
    // Each entry pairs a node to copy with the copy of its parent, which is where the new node is appended.
    let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
    self.stack_children(source, target, &mut stack);
    while let Some((node, parent)) = stack.pop() {
      let copy = self.clone_shallow(node)?;
      self.append_child(parent, copy)?;
      self.stack_children(node, copy, &mut stack);
    }
    Ok(())
  }

  /// Stacks the children of `node`, each paired with `copy`, the copy they are appended to.
  ///
  /// They go on in reverse, so the stack pops them in document order. Each parent then receives its children in that
  /// order, which is the order [`append_child`](Self::append_child) has to see them in.
  ///
  fn stack_children(&self, node: NodeId, copy: NodeId, stack: &mut Vec<(NodeId, NodeId)>) {
    let children: Vec<NodeId> = self.children(node).collect();
    stack.extend(children.into_iter().rev().map(|child| (child, copy)));
  }

  /// Interns a node and returns its handle. The node starts detached.
  ///
  fn push(&mut self, data: NodeData) -> NodeId {
    let index = u32::try_from(self.nodes.len()).expect("a document holds fewer than 4 billion nodes");
    self.nodes.push(NodeSlot::new(data));
    NodeId::new(self.id, index)
  }

  /// Splits a qualified name and interns its parts, forming a [`QName`] in `namespace`.
  ///
  fn parse_qname(&mut self, qualified_name: &str, namespace: Option<NameId>) -> Result<QName> {
    let Some((prefix, local)) = chars::split_qname(qualified_name) else {
      return Err(DomException::new(ExceptionCode::INVALID_CHARACTER_ERR, invalid_qname_message(qualified_name)));
    };
    let prefix = prefix.map(|p| self.pool.intern(p));
    let local = self.pool.intern(local);
    Ok(QName::new(prefix, namespace, local))
  }

  // --- Navigation ---------------------------------------------------------------------------

  /// A [`NodeRef`] over a node, for chained reads.
  ///
  #[must_use]
  pub fn node(&self, id: NodeId) -> NodeRef<'_> {
    NodeRef::new(self, id)
  }

  /// The kind of a node.
  ///
  #[must_use]
  pub fn node_type(&self, id: NodeId) -> NodeType {
    self.slot(id).data.node_type()
  }

  /// The parent of a node, or `None` for the document node and any detached node.
  ///
  #[must_use]
  pub fn parent(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).parent
  }

  /// The first child of a node, if it has one.
  ///
  #[must_use]
  pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).first_child
  }

  /// The last child of a node, if it has one.
  ///
  #[must_use]
  pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).last_child
  }

  /// The node before this one under the same parent, if any.
  ///
  #[must_use]
  pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).previous_sibling
  }

  /// The node after this one under the same parent, if any.
  ///
  #[must_use]
  pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
    self.slot(id).next_sibling
  }

  /// The children of a node, first to last.
  ///
  pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let mut next = self.slot(id).first_child;
    std::iter::from_fn(move || {
      let current = next?;
      next = self.slot(current).next_sibling;
      Some(current)
    })
  }

  /// The root element of the document (its single element child), if the tree has one.
  ///
  /// An element becomes this by being placed directly under [`document_node`](Self::document_node) with
  /// [`append_child`](Self::append_child). The document holds at most one, so a second root element
  /// is refused.
  ///
  #[must_use]
  pub fn document_element(&self) -> Option<NodeId> {
    self.children(self.document_node()).find(|&child| self.node_type(child) == NodeType::ELEMENT_NODE)
  }

  /// The document type node, if the document has one.
  ///
  #[must_use]
  pub fn doctype(&self) -> Option<NodeId> {
    self.children(self.document_node()).find(|&child| self.node_type(child) == NodeType::DOCUMENT_TYPE_NODE)
  }

  // --- Names and values ---------------------------------------------------------------------

  /// The DOM `nodeName`: an element's qualified name, a PI's target, or the `#name` a node kind reports (`#text`,
  /// `#comment`, `#document`, and so on).
  ///
  #[must_use]
  pub fn node_name(&self, id: NodeId) -> String {
    match &self.slot(id).data {
      NodeData::Element(element) => element.name.to_lexical(&self.pool),
      NodeData::Attribute(attr) => attr.name.to_lexical(&self.pool),
      NodeData::ProcessingInstruction { target, .. } => self.pool.resolve(*target).to_owned(),
      NodeData::DocumentType { name, .. } => self.pool.resolve(*name).to_owned(),
      NodeData::Text(_) => "#text".to_owned(),
      NodeData::CdataSection(_) => "#cdata-section".to_owned(),
      NodeData::Comment(_) => "#comment".to_owned(),
      NodeData::Document => "#document".to_owned(),
      NodeData::DocumentFragment => "#document-fragment".to_owned(),
    }
  }

  /// The DOM `nodeValue`: the value of an attribute, or the character data of a text, CDATA, comment or PI node;
  /// `None` for the kinds that have no value of their own.
  ///
  #[must_use]
  pub fn node_value(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::Attribute(attr) => Some(&attr.value),
      NodeData::Text(data) | NodeData::CdataSection(data) | NodeData::Comment(data) => Some(data),
      NodeData::ProcessingInstruction { data, .. } => Some(data),
      _ => None,
    }
  }

  /// The payload of a node, for crate code that reads a node's kind-specific data directly, such as emitting the tree
  /// as an event stream.
  ///
  pub(crate) fn node_data(&self, id: NodeId) -> &NodeData {
    &self.slot(id).data
  }

  /// The name of an element or attribute node, if this is one.
  ///
  fn name_of(&self, id: NodeId) -> Option<&QName> {
    match &self.slot(id).data {
      NodeData::Element(element) => Some(&element.name),
      NodeData::Attribute(attr) => Some(&attr.name),
      _ => None,
    }
  }

  /// The local part of an element's or attribute's name.
  ///
  #[must_use]
  pub fn local_name(&self, id: NodeId) -> Option<&str> {
    self.name_of(id).map(|name| self.pool.resolve(name.local()))
  }

  /// The namespace prefix of an element's or attribute's name, if it has one.
  ///
  #[must_use]
  pub fn prefix(&self, id: NodeId) -> Option<&str> {
    self.name_of(id).and_then(|name| name.prefix).map(|p| self.pool.resolve(p))
  }

  /// The namespace name of an element or attribute, if it is in one.
  ///
  #[must_use]
  pub fn namespace_uri(&self, id: NodeId) -> Option<&str> {
    self.name_of(id).and_then(|name| name.namespace()).map(|ns| self.pool.resolve(ns))
  }

  /// The public identifier of a document type node, if it has one.
  ///
  #[must_use]
  pub fn public_id(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::DocumentType { public_id, .. } => public_id.as_deref(),
      _ => None,
    }
  }

  /// The system identifier of a document type node, if it has one.
  ///
  #[must_use]
  pub fn system_id(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::DocumentType { system_id, .. } => system_id.as_deref(),
      _ => None,
    }
  }

  /// The DOM `textContent`: the character data of the node and all its descendants, in order.
  ///
  /// Comments and processing instructions contribute nothing, matching the W3C DOM specification.
  ///
  #[must_use]
  pub fn text_content(&self, id: NodeId) -> String {
    let mut out = String::new();
    for (visit, node) in self.walk(id) {
      if visit == Visit::Enter {
        if let NodeData::Text(data) | NodeData::CdataSection(data) = &self.slot(node).data {
          out.push_str(data);
        }
      }
    }
    out
  }

  /// The DOM `baseURI` of a node (XML Base): the base URI in effect where the node is.
  ///
  /// It is the base recorded on the nearest element at or above the node. The document builder
  /// records each element's base, resolved from `xml:base` and the document's system identifier.
  /// Failing that, it is the document's own base URI. `None` for a tree built by hand without base
  /// information, or a document parsed without a system identifier and without `xml:base`.
  ///
  /// The base of an attribute is that of its owning element.
  ///
  #[must_use]
  pub fn base_uri(&self, id: NodeId) -> Option<String> {
    let start = match &self.slot(id).data {
      NodeData::Attribute(attr) => attr.owner,
      _ => Some(id),
    };
    let mut current = start;
    while let Some(node) = current {
      if let NodeData::Element(element) = &self.slot(node).data {
        if let Some(base) = element.base {
          return Some(self.pool.resolve(base).to_owned());
        }
      }
      current = self.slot(node).parent;
    }
    self.base.map(|base| self.pool.resolve(base).to_owned())
  }

  /// Records the document's own base URI (its system identifier). Used by the builder.
  ///
  #[cfg(feature = "parse")]
  pub(crate) fn set_document_base(&mut self, base: Option<&str>) {
    self.base = base.map(|base| self.pool.intern(base));
  }

  /// Records the effective base URI of an element. Used by the builder.
  ///
  #[cfg(feature = "parse")]
  pub(crate) fn set_element_base(&mut self, element: NodeId, base: Option<&str>) {
    let base = base.map(|base| self.pool.intern(base));
    if let NodeData::Element(data) = &mut self.nodes[element.index()].data {
      data.base = base;
    }
  }

  // --- Attributes ---------------------------------------------------------------------------

  /// Creates a detached attribute node with a qualified name and no namespace.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `qualified_name` is not a legal name.
  ///
  pub fn create_attribute(&mut self, qualified_name: &str) -> Result<NodeId> {
    let name = self.parse_qname(qualified_name, None)?;
    Ok(self.push(NodeData::Attribute(AttrData { name, value: String::new(), owner: None, is_id: false })))
  }

  /// Creates a detached attribute node in a namespace.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `qualified_name` is not a legal name;
  /// [`ExceptionCode::NAMESPACE_ERR`] if the prefix and namespace are inconsistent.
  ///
  pub fn create_attribute_ns(&mut self, namespace: Option<&str>, qualified_name: &str) -> Result<NodeId> {
    check_qname_namespace(namespace, qualified_name, true)?;
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    Ok(self.push(NodeData::Attribute(AttrData { name, value: String::new(), owner: None, is_id: false })))
  }

  /// Sets an attribute by qualified name, adding it or replacing its value.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_SUPPORTED_ERR`] if the node is not an element, or
  /// [`ExceptionCode::INVALID_CHARACTER_ERR`] if `qualified_name` is not a legal name.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn set_attribute(&mut self, element: NodeId, qualified_name: &str, value: &str) -> Result<()> {
    self.require_own(element)?;
    self.require_element(element)?;
    let name = self.parse_qname(qualified_name, None)?;
    self.put_attribute(element, name, value);
    Ok(())
  }

  /// Sets an attribute in a namespace, adding it or replacing the value of the one with the same namespace and local
  /// name.
  ///
  /// # Errors
  ///
  /// As [`set_attribute`](Self::set_attribute), plus [`ExceptionCode::NAMESPACE_ERR`] if the prefix
  /// and namespace are inconsistent.
  ///
  pub fn set_attribute_ns(
    &mut self,
    element: NodeId,
    namespace: Option<&str>,
    qualified_name: &str,
    value: &str,
  ) -> Result<()> {
    self.require_own(element)?;
    self.require_element(element)?;
    check_qname_namespace(namespace, qualified_name, true)?;
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    self.put_attribute(element, name, value);
    Ok(())
  }

  /// Adds or updates the attribute of `element` with `name`, giving it `value`.
  ///
  fn put_attribute(&mut self, element: NodeId, name: QName, value: &str) {
    if let Some(attr) = self.find_attribute(element, |a| a.name.expanded == name.expanded) {
      self.attr_data_mut(attr).value = value.to_owned();
      return;
    }
    let attr =
      self.push(NodeData::Attribute(AttrData { name, value: value.to_owned(), owner: Some(element), is_id: false }));
    self.element_data_mut(element).attributes.push(attr);
  }

  /// The attribute node of `element` whose data satisfies `predicate`, if any.
  ///
  fn find_attribute(&self, element: NodeId, predicate: impl Fn(&AttrData) -> bool) -> Option<NodeId> {
    let data = self.element_data(element)?;
    data.attributes.iter().copied().find(|&attr| match &self.slot(attr).data {
      NodeData::Attribute(attr) => predicate(attr),
      _ => false,
    })
  }

  /// The attribute node of an element by qualified name, if it has one.
  ///
  #[must_use]
  pub fn get_attribute_node(&self, element: NodeId, qualified_name: &str) -> Option<NodeId> {
    let (prefix, local) = chars::split_qname(qualified_name)?;
    let prefix = match prefix {
      Some(p) => Some(self.pool.get(p)?),
      None => None,
    };
    let local = self.pool.get(local)?;
    self.find_attribute(element, |a| a.name.prefix == prefix && a.name.local() == local)
  }

  /// The attribute node of an element by namespace and local name, if it has one.
  ///
  #[must_use]
  pub fn get_attribute_node_ns(&self, element: NodeId, namespace: Option<&str>, local: &str) -> Option<NodeId> {
    let namespace = match namespace {
      Some(ns) => Some(self.pool.get(ns)?),
      None => None,
    };
    let local = self.pool.get(local)?;
    self.find_attribute(element, |a| a.name.namespace() == namespace && a.name.local() == local)
  }

  /// The element an attribute node belongs to (`ownerElement` in the W3C DOM specification), or `None` for a detached
  /// attribute or a node that is not an attribute.
  ///
  #[must_use]
  pub fn owner_element(&self, attr: NodeId) -> Option<NodeId> {
    match &self.slot(attr).data {
      NodeData::Attribute(data) => data.owner,
      _ => None,
    }
  }

  /// The value of an element's attribute, by qualified name.
  ///
  #[must_use]
  pub fn attribute(&self, element: NodeId, qualified_name: &str) -> Option<&str> {
    self.get_attribute_node(element, qualified_name).and_then(|attr| self.node_value(attr))
  }

  /// The value of an element's attribute, by namespace name and local name.
  ///
  #[must_use]
  pub fn attribute_ns(&self, element: NodeId, namespace: Option<&str>, local: &str) -> Option<&str> {
    self.get_attribute_node_ns(element, namespace, local).and_then(|attr| self.node_value(attr))
  }

  /// Whether an element has an attribute with the given qualified name.
  ///
  #[must_use]
  pub fn has_attribute(&self, element: NodeId, qualified_name: &str) -> bool {
    self.get_attribute_node(element, qualified_name).is_some()
  }

  /// The attributes of an element, as a live [`NamedNodeMap`] of attribute nodes.
  #[must_use]
  pub fn attributes(&self, element: NodeId) -> NamedNodeMap<'_> {
    NamedNodeMap::new(self, element)
  }

  /// The qualified names of an element's attributes, in document order.
  ///
  #[must_use]
  pub fn attribute_names(&self, element: NodeId) -> Vec<String> {
    match self.element_data(element) {
      Some(data) => data.attributes.iter().map(|&attr| self.node_name(attr)).collect(),
      None => Vec::new(),
    }
  }

  /// Removes an element's attribute by qualified name; a no-op if it has none such.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_SUPPORTED_ERR`] if the node is not an element.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn remove_attribute(&mut self, element: NodeId, qualified_name: &str) -> Result<()> {
    self.require_own(element)?;
    self.require_element(element)?;
    if let Some(attr) = self.get_attribute_node(element, qualified_name) {
      self.remove_attribute_node(element, attr)?;
    }
    Ok(())
  }

  /// Attaches an attribute node to an element, replacing any it already has with the same name.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_SUPPORTED_ERR`] if `element` is not an element or `attr` is not an attribute
  /// [`ExceptionCode::INUSE_ATTRIBUTE_ERR`] if `attr` already belongs to another element.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn set_attribute_node(&mut self, element: NodeId, attr: NodeId) -> Result<()> {
    self.require_own(element)?;
    self.require_own(attr)?;
    self.require_element(element)?;
    let name = match &self.slot(attr).data {
      NodeData::Attribute(data) => data.name,
      _ => return Err(DomException::new(ExceptionCode::NOT_SUPPORTED_ERR, "not an attribute node")),
    };
    match self.attr_data(attr).owner {
      Some(owner) if owner != element => {
        return Err(DomException::new(
          ExceptionCode::INUSE_ATTRIBUTE_ERR,
          "the attribute already belongs to an element",
        ));
      }
      _ => {}
    }
    if let Some(existing) = self.find_attribute(element, |a| a.name.expanded == name.expanded) {
      self.remove_attribute_node(element, existing)?;
    }
    self.attr_data_mut(attr).owner = Some(element);
    self.element_data_mut(element).attributes.push(attr);
    Ok(())
  }

  /// Detaches an attribute node from its element.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_FOUND_ERR`] if `attr` is not an attribute of `element`.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn remove_attribute_node(&mut self, element: NodeId, attr: NodeId) -> Result<NodeId> {
    self.require_own(element)?;
    self.require_own(attr)?;
    let attributes = &mut self.element_data_mut(element).attributes;
    let Some(position) = attributes.iter().position(|&a| a == attr) else {
      return Err(DomException::new(ExceptionCode::NOT_FOUND_ERR, "the attribute does not belong to the element"));
    };
    attributes.remove(position);
    self.attr_data_mut(attr).owner = None;
    Ok(attr)
  }

  /// Marks (or unmarks) an element's attribute as being of type `ID`, so
  /// [`get_element_by_id`](Self::get_element_by_id) will find the element through it.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_FOUND_ERR`] if `element` has no such attribute.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn set_id_attribute(&mut self, element: NodeId, qualified_name: &str, is_id: bool) -> Result<()> {
    self.require_own(element)?;
    let Some(attr) = self.get_attribute_node(element, qualified_name) else {
      return Err(DomException::new(ExceptionCode::NOT_FOUND_ERR, "the element has no such attribute"));
    };
    self.attr_data_mut(attr).is_id = is_id;
    Ok(())
  }

  /// The element carrying an `ID`-typed attribute equal to `id`, in document order, if any.
  ///
  /// An attribute counts only if it was marked with [`set_id_attribute`](Self::set_id_attribute). The W3C DOM
  /// specification has no way to know an attribute named `id` is an ID unless a DTD, a schema, or the caller says so.
  /// With the `parse` feature, the document builder marks DTD- and `xml:id`-typed attributes for you.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_dom::Document;
  ///
  /// let mut doc = Document::new();
  /// let e = doc.create_element("section")?;
  /// doc.set_attribute(e, "id", "intro")?;
  /// doc.append_child(doc.document_node(), e)?;
  ///
  /// // Not found until the attribute is declared to be an ID.
  /// assert_eq!(doc.get_element_by_id("intro"), None);
  /// doc.set_id_attribute(e, "id", true)?;
  /// assert_eq!(doc.get_element_by_id("intro"), Some(e));
  /// # Ok::<(), xenolith_dom::DomException>(())
  /// ```
  #[must_use]
  pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
    self.descendants(self.document_node()).find(|&node| {
      matches!(self.node_type(node), NodeType::ELEMENT_NODE)
        && self.element_data(node).is_some_and(|data| {
          data.attributes.iter().any(|&attr| match &self.slot(attr).data {
            NodeData::Attribute(attr) => attr.is_id && attr.value == id,
            _ => false,
          })
        })
    })
  }

  fn element_data(&self, id: NodeId) -> Option<&ElementData> {
    match &self.slot(id).data {
      NodeData::Element(data) => Some(data),
      _ => None,
    }
  }

  fn element_data_mut(&mut self, id: NodeId) -> &mut ElementData {
    match &mut self.nodes[id.index()].data {
      NodeData::Element(data) => data,
      _ => unreachable!("caller checked the node is an element"),
    }
  }

  fn attr_data(&self, id: NodeId) -> &AttrData {
    match &self.slot(id).data {
      NodeData::Attribute(data) => data,
      _ => unreachable!("caller checked the node is an attribute"),
    }
  }

  fn attr_data_mut(&mut self, id: NodeId) -> &mut AttrData {
    match &mut self.nodes[id.index()].data {
      NodeData::Attribute(data) => data,
      _ => unreachable!("caller checked the node is an attribute"),
    }
  }

  fn require_element(&self, id: NodeId) -> Result<()> {
    if matches!(self.slot(id).data, NodeData::Element(_)) {
      Ok(())
    } else {
      Err(DomException::new(ExceptionCode::NOT_SUPPORTED_ERR, "attributes belong to elements only"))
    }
  }

  // --- Collections --------------------------------------------------------------------------

  /// The children of a node as a live [`NodeList`].
  ///
  #[must_use]
  pub fn child_nodes(&self, id: NodeId) -> NodeList<'_> {
    NodeList::new(self, Query::Children(id))
  }

  /// The descendant elements with a given qualified name, in document order, as a live [`NodeList`]. The name `"*"`
  /// matches every element.
  ///
  #[must_use]
  pub fn get_elements_by_tag_name(&self, name: &str) -> NodeList<'_> {
    NodeList::new(self, Query::by_tag_name(self.document_node(), name))
  }

  /// The descendant elements with a given namespace and local name, in document order, as a live [`NodeList`]. Either
  /// argument may be `"*"` to match any.
  ///
  #[must_use]
  pub fn get_elements_by_tag_name_ns(&self, namespace: Option<&str>, local: &str) -> NodeList<'_> {
    NodeList::new(self, Query::by_tag_name_ns(self.document_node(), namespace, local))
  }

  /// Traverses the subtree rooted at `id` and reports each node's entering and leaving.
  ///
  /// This is the crate's standard way to traverse the tree and is preferred over recursive traversal. Refer to [`Walk`]
  /// for details on the reported events and their order, and [traversal depth](crate#traversal) for why recursion does
  /// not occur.
  ///
  /// # Panics
  ///
  /// If `id` was created by another document.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_dom::{Document, Visit};
  ///
  /// // <a><b>hi</b></a>
  /// let mut doc = Document::new();
  /// let root = doc.create_element("a")?;
  /// let child = doc.create_element("b")?;
  /// let text = doc.create_text_node("hi");
  /// doc.append_child(child, text)?;
  /// doc.append_child(root, child)?;
  /// doc.append_child(doc.document_node(), root)?;
  ///
  /// // Both sides of every node, so the shape of the subtree comes back out. A text node has no end tag in XML, but
  /// // the walk still enters and leaves it, as it does any other node.
  /// let shape: String = doc
  ///     .walk(root)
  ///     .map(|(visit, node)| match visit {
  ///       Visit::Enter => format!("<{}>", doc.node_name(node)),
  ///       Visit::Leave => format!("</{}>", doc.node_name(node)),
  ///     })
  ///     .collect();
  /// assert_eq!(shape, "<a><b><#text></#text></b></a>");
  /// # Ok::<(), xenolith_dom::DomException>(())
  /// ```
  ///
  #[must_use]
  pub fn walk(&self, id: NodeId) -> Walk<'_> {
    // Check here rather than leaving it to the first step, so a stray handle is caught at the call that made it.
    assert!(self.owns(id), "the node was made by another document, and a node id is only valid with its own");
    Walk::new(self, id)
  }

  /// The descendants of a node in document order (preorder), excluding attribute nodes.
  ///
  /// Attributes are not children, so a walk never reaches them.
  ///
  pub(crate) fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    // Entering is preorder. The start node is entered first and is not one of its own descendants, so it is dropped.
    self.walk(id).filter_map(move |(visit, node)| (visit == Visit::Enter && node != id).then_some(node))
  }

  // --- Mutation -----------------------------------------------------------------------------

  /// Appends `child` as the last child of `parent`, detaching it from any current parent first.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::HIERARCHY_REQUEST_ERR`] if `parent` cannot hold children, or if `child` is `parent` or one of
  /// its ancestors (which would make a cycle).
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId> {
    self.insert_before(parent, child, None)
  }

  /// Appends character `data` to `parent`, extending its last child when that child is a text node.
  ///
  /// Adjacent character data is thus kept as a single text node, as the data model requires, even when the parser
  /// delivers a long run of text in fragments. When the last child is not a text node (or there is none), a new text
  /// node is created and appended.
  ///
  /// # Errors
  ///
  /// As [`append_child`](Self::append_child), when a new text node must be appended.
  ///
  pub fn append_text(&mut self, parent: NodeId, data: &str) -> Result<NodeId> {
    self.require_own(parent)?;
    if let Some(last) = self.last_child(parent) {
      if let NodeData::Text(existing) = &mut self.nodes[last.index()].data {
        existing.push_str(data);
        return Ok(last);
      }
    }
    let node = self.create_text_node(data);
    self.append_child(parent, node)
  }

  /// Inserts `child` under `parent` before `reference`, or at the end when `reference` is `None`. Detaches `child`
  /// from any current parent first.
  ///
  /// A [document fragment](Self::create_document_fragment) is not itself inserted: its children are moved in, in
  /// order, and the fragment is left empty, as the W3C DOM specification defines.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::HIERARCHY_REQUEST_ERR`] if `parent` cannot hold children, if `child` cannot be
  /// a child, if the insertion would make a cycle, or if it would break the document's own rules
  /// (one root element, one doctype, no text directly under the document);
  /// [`ExceptionCode::NOT_FOUND_ERR`] if `reference` is not a child of `parent`.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) -> Result<NodeId> {
    self.check_insertion(parent, child, reference)?;

    // A fragment inserts its children in its place, not itself.
    if matches!(self.slot(child).data, NodeData::DocumentFragment) {
      for grandchild in self.children(child).collect::<Vec<_>>() {
        self.detach(grandchild);
        self.place(parent, grandchild, reference);
      }
      return Ok(child);
    }

    self.detach(child);
    self.place(parent, child, reference);
    Ok(child)
  }

  /// Replaces `old_child` with `new_child` under `parent`, returning the node removed.
  ///
  /// # Errors
  ///
  /// As [`insert_before`](Self::insert_before); [`ExceptionCode::NOT_FOUND_ERR`] if `old_child` is not a child of
  /// `parent`.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn replace_child(&mut self, parent: NodeId, new_child: NodeId, old_child: NodeId) -> Result<NodeId> {
    self.require_own(parent)?;
    self.require_own(new_child)?;
    self.require_own(old_child)?;
    if self.slot(old_child).parent != Some(parent) {
      return Err(DomException::new(ExceptionCode::NOT_FOUND_ERR, "the node to replace is not a child of the parent"));
    }
    // Insert the new child before the old one, then take the old one out. Insertion is validated first, so a bad new
    // child leaves the tree untouched. The reference is the old child.
    self.insert_before(parent, new_child, Some(old_child))?;
    self.detach(old_child);
    Ok(old_child)
  }

  /// The shared validity checks for inserting `child` under `parent` before `reference`.
  ///
  fn check_insertion(&self, parent: NodeId, child: NodeId, reference: Option<NodeId>) -> Result<()> {
    self.require_own(parent)?;
    self.require_own(child)?;
    if let Some(reference) = reference {
      self.require_own(reference)?;
    }
    if !self.slot(parent).data.is_container() {
      let name = self.node_name(parent);
      return Err(DomException::new(ExceptionCode::HIERARCHY_REQUEST_ERR, format!("\"{name}\" cannot have children")));
    }
    if !self.slot(child).data.can_be_child() {
      let name = self.node_name(child);
      return Err(DomException::new(ExceptionCode::HIERARCHY_REQUEST_ERR, format!("\"{name}\" cannot be a child")));
    }
    if child == parent || self.is_ancestor(child, parent) {
      return Err(DomException::new(
        ExceptionCode::HIERARCHY_REQUEST_ERR,
        "a node cannot be made a descendant of itself",
      ));
    }
    if let Some(reference) = reference {
      if self.slot(reference).parent != Some(parent) {
        return Err(DomException::new(ExceptionCode::NOT_FOUND_ERR, "the reference node is not a child of the parent"));
      }
    }
    if matches!(self.slot(child).data, NodeData::DocumentFragment) {
      self.check_fragment_into_document(parent, child)?;
    } else {
      self.check_child_of_document(parent, child)?;
    }
    Ok(())
  }

  /// Enforces the document node's own child rules for a single node: at most one element and one doctype, and no
  /// character data directly under it. A no-op when `parent` is not the document.
  ///
  fn check_child_of_document(&self, parent: NodeId, child: NodeId) -> Result<()> {
    if !matches!(self.slot(parent).data, NodeData::Document) {
      return Ok(());
    }
    let has_element = self.document_element().is_some_and(|e| e != child);
    let has_doctype = self.doctype().is_some_and(|d| d != child);
    document_child_error(self.slot(child).data.node_type(), has_element, has_doctype)
  }

  /// Enforces the document's child rules for every node a fragment would bring in at once, so a fragment with two
  /// elements is refused before any of it is inserted.
  ///
  fn check_fragment_into_document(&self, parent: NodeId, fragment: NodeId) -> Result<()> {
    if !matches!(self.slot(parent).data, NodeData::Document) {
      return Ok(());
    }
    let mut has_element = self.document_element().is_some();
    let mut has_doctype = self.doctype().is_some();
    for grandchild in self.children(fragment) {
      let node_type = self.slot(grandchild).data.node_type();
      document_child_error(node_type, has_element, has_doctype)?;
      has_element |= node_type == NodeType::ELEMENT_NODE;
      has_doctype |= node_type == NodeType::DOCUMENT_TYPE_NODE;
    }
    Ok(())
  }

  /// Links an already-detached node under `parent`, at `reference` or at the end.
  ///
  fn place(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
    match reference {
      Some(reference) => self.link_before(parent, child, reference),
      None => self.link_last(parent, child),
    }
  }

  /// Removes `child` from `parent`, leaving it detached.
  ///
  /// The node keeps its place in the document. Only dropping the document reclaims it.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NOT_FOUND_ERR`] if `child` is not a child of `parent`.
  /// [`ExceptionCode::WRONG_DOCUMENT_ERR`] if a node came from another document.
  ///
  pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId> {
    self.require_own(parent)?;
    self.require_own(child)?;
    if self.slot(child).parent != Some(parent) {
      return Err(DomException::new(ExceptionCode::NOT_FOUND_ERR, "the node is not a child of the parent"));
    }
    self.detach(child);
    Ok(child)
  }

  /// Whether `ancestor` is `node` or lies above it in the tree.
  ///
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
  ///
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

  /// The slot `id` refers to.
  ///
  /// # Panics
  ///
  /// If `id` was made by another document. The accessors that read through this return a value rather than a
  /// [`Result`], so they have no way to report the mistake. A node of this document is always present, because the
  /// arena never gives a place back, so no other check is needed here.
  ///
  pub(crate) fn slot(&self, id: NodeId) -> &NodeSlot {
    assert!(self.owns(id), "the node was made by another document, and a node id is only valid with its own");
    &self.nodes[id.index()]
  }
}

/// The error, if any, of putting a node of this type directly under the document, given whether the document already
/// has an element and a doctype.
///
fn document_child_error(node_type: NodeType, has_element: bool, has_doctype: bool) -> Result<()> {
  let offending = match node_type {
    NodeType::TEXT_NODE | NodeType::CDATA_SECTION_NODE => Some("character data"),
    NodeType::ELEMENT_NODE if has_element => Some("a second root element"),
    NodeType::DOCUMENT_TYPE_NODE if has_doctype => Some("a second document type"),
    _ => None,
  };
  match offending {
    Some(what) => {
      Err(DomException::new(ExceptionCode::HIERARCHY_REQUEST_ERR, format!("a document may not contain {what}")))
    }
    None => Ok(()),
  }
}

/// Generates the identity of a new document, which its [`NodeId`]s then carry.
///
fn next_document_id() -> NonZeroU32 {
  // The count starts at 1, which leaves zero unused as the niche that keeps `Option<NodeId>` the size of a `NodeId`.
  // It wraps after 2^32 documents in one process, and identities repeat from there, so the check it supports guards
  // against a mistake rather than proving one impossible.
  static NEXT: AtomicU32 = AtomicU32::new(1);
  let raw = NEXT.fetch_add(1, Ordering::Relaxed);
  NonZeroU32::new(raw).unwrap_or(NonZeroU32::MIN)
}

/// The first character of `name` that an XML name may not contain, paired with whether it fell at the start (where the
/// rules are stricter). `None` if `name` is empty or is already a valid name.
///
/// A caller renders the returned character with `{:?}`, so a non-printing one shows as its Unicode escape (for
/// example, `'\u{7}'`) rather than as an invisible byte in the message.
///
fn first_invalid_name_char(name: &str) -> Option<(char, bool)> {
  let mut iter = name.chars();
  match iter.next() {
    None => None,
    Some(first) if !chars::is_name_start_char(first) => Some((first, true)),
    Some(_) => iter.find(|&c| !chars::is_name_char(c)).map(|c| (c, false)),
  }
}

/// A message for a plain XML name that failed validation, giving the offending character. `subject` says what the
/// string was meant to be, for example, `"PI target"`.
///
fn invalid_name_message(name: &str, subject: &str) -> String {
  match first_invalid_name_char(name) {
    Some((c, true)) => format!("{c:?} is not allowed at the start of the {subject} {name:?}"),
    Some((c, false)) => format!("{c:?} is not allowed in the {subject} {name:?}"),
    None => format!("the {subject} is empty"),
  }
}

/// A message for a qualified name that is not a valid `QName`, giving the structural fault or the offending character.
///
fn invalid_qname_message(qualified_name: &str) -> String {
  let parts: Vec<&str> = qualified_name.split(':').collect();
  match parts.as_slice() {
    [_] => {}
    [prefix, local] => {
      if prefix.is_empty() {
        return format!("the qualified name {qualified_name:?} has an empty prefix before the colon");
      }
      if local.is_empty() {
        return format!("the qualified name {qualified_name:?} has an empty local part after the colon");
      }
    }
    _ => return format!("the qualified name {qualified_name:?} has more than one colon"),
  }
  for &part in &parts {
    if let Some((c, at_start)) = first_invalid_name_char(part) {
      let position = if at_start { "at the start of" } else { "in" };
      return format!("{c:?} is not allowed {position} the qualified name {qualified_name:?}");
    }
  }
  format!("{qualified_name:?} is not a valid qualified name")
}

/// Checks a qualified name against a namespace for the `*NS` constructors (Namespaces in XML, as the W3C DOM
/// specification applies it). `is_attribute` turns on the extra `xmlns` rules that apply to attribute names.
///
fn check_qname_namespace(namespace: Option<&str>, qualified_name: &str, is_attribute: bool) -> Result<()> {
  let Some((prefix, _)) = chars::split_qname(qualified_name) else {
    return Err(DomException::new(ExceptionCode::INVALID_CHARACTER_ERR, invalid_qname_message(qualified_name)));
  };
  let namespace_error = |message: &str| Err(DomException::new(ExceptionCode::NAMESPACE_ERR, message.to_owned()));

  if prefix.is_some() && namespace.is_none() {
    return namespace_error("a prefixed name must have a namespace");
  }
  if prefix == Some("xml") && namespace != Some(XML_NS_URI) {
    return namespace_error("the prefix \"xml\" may only name the XML namespace");
  }
  if is_attribute {
    let is_xmlns = qualified_name == "xmlns" || prefix == Some("xmlns");
    if is_xmlns && namespace != Some(XMLNS_NS_URI) {
      return namespace_error("\"xmlns\" may only name the XMLNS namespace");
    }
    if namespace == Some(XMLNS_NS_URI) && !is_xmlns {
      return namespace_error("the XMLNS namespace is only for \"xmlns\" attributes");
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Builds `<r a="1"><b/>text<c/></r>` under the document node, returning (r, b, c).
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
    doc.append_child(doc.document_node(), r).unwrap();
    (doc, r, b, c)
  }

  #[test]
  fn navigates_the_tree() {
    let (doc, r, b, c) = sample();
    assert_eq!(doc.document_element(), Some(r));
    assert_eq!(doc.parent(r), Some(doc.document_node()));
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
    assert_eq!(doc.node_type(r), NodeType::ELEMENT_NODE);
    assert_eq!(doc.node_name(r), "r");
    assert_eq!(doc.local_name(b), Some("b"));
    let text = doc.next_sibling(b).unwrap();
    assert_eq!(doc.node_type(text), NodeType::TEXT_NODE);
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
    assert_eq!(doc.node_type(doc.first_child(r).unwrap()), NodeType::TEXT_NODE);
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
    assert_eq!(error.code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  }

  #[test]
  fn a_leaf_cannot_take_children() {
    let mut doc = Document::new();
    let text = doc.create_text_node("t");
    let child = doc.create_element("c").unwrap();
    let error = doc.append_child(text, child).unwrap_err();
    assert_eq!(error.code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  }

  #[test]
  fn insert_before_a_non_child_is_not_found() {
    let (mut doc, r, _, _) = sample();
    let stray = doc.create_element("stray").unwrap();
    let x = doc.create_element("x").unwrap();
    let error = doc.insert_before(r, x, Some(stray)).unwrap_err();
    assert_eq!(error.code(), ExceptionCode::NOT_FOUND_ERR);
  }

  #[test]
  fn an_invalid_name_is_rejected() {
    let mut doc = Document::new();
    assert_eq!(doc.create_element("1bad").unwrap_err().code(), ExceptionCode::INVALID_CHARACTER_ERR);
  }

  #[test]
  fn a_pi_target_error_names_the_offending_character() {
    let mut doc = Document::new();
    // A non-printing character shows as its Unicode escape, not as an invisible byte.
    let bell = doc.create_processing_instruction("ab\u{7}c", "d").unwrap_err();
    assert_eq!(bell.code(), ExceptionCode::INVALID_CHARACTER_ERR);
    assert!(bell.message().contains(r"'\u{7}'"), "message was: {}", bell.message());

    // A character legal in a name but not at the start is reported as a start-position fault.
    let digit = doc.create_processing_instruction("1abc", "d").unwrap_err();
    assert!(digit.message().contains("start"), "message was: {}", digit.message());
    assert!(digit.message().contains("'1'"), "message was: {}", digit.message());
  }

  #[test]
  fn a_qualified_name_error_explains_the_fault() {
    let mut doc = Document::new();
    // A structural fault reports the structure, not a single character.
    let two_colons = doc.create_element("a:b:c").unwrap_err();
    assert_eq!(two_colons.code(), ExceptionCode::INVALID_CHARACTER_ERR);
    assert!(two_colons.message().contains("more than one colon"), "message was: {}", two_colons.message());
    // A bad character inside a part is reported.
    let space = doc.create_element("a b").unwrap_err();
    assert!(space.message().contains("' '"), "message was: {}", space.message());
    // An empty prefix or local part is called out.
    let empty_local = doc.create_element("p:").unwrap_err();
    assert!(empty_local.message().contains("empty local part"), "message was: {}", empty_local.message());
  }

  #[test]
  fn a_node_from_another_document_is_refused() {
    let mut a = Document::new();
    let mut b = Document::new();
    let in_a = a.create_element("a").unwrap();
    let in_b = b.create_element("b").unwrap();

    assert!(a.owns(in_a));
    assert!(!a.owns(in_b), "a handle carries the document that made it");

    // Every fallible method reports the mistake rather than reading or linking an unrelated node.
    assert_eq!(a.append_child(a.document_node(), in_b).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert_eq!(a.insert_before(in_a, in_b, None).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert_eq!(a.remove_child(a.document_node(), in_b).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert_eq!(a.replace_child(a.document_node(), in_b, in_a).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert_eq!(a.clone_node(in_b, false).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert_eq!(a.set_attribute(in_b, "k", "v").unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert_eq!(a.append_text(in_b, "t").unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);

    // `import_node` measures the node against the document it is read from, not the one it is copied into.
    assert_eq!(a.import_node(&b, in_a, true).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
    assert!(a.import_node(&b, in_b, true).is_ok(), "the node does belong to the source document");
  }

  #[test]
  #[should_panic(expected = "another document")]
  fn reading_through_a_node_from_another_document_panics() {
    let a = Document::new();
    let mut b = Document::new();
    let in_b = b.create_element("b").unwrap();
    // An accessor that returns a value has no way to report the mistake, so it stops the caller instead.
    let _ = a.node_type(in_b);
  }

  #[test]
  fn attributes_are_nodes() {
    let (doc, r, _, _) = sample();
    let attr = doc.get_attribute_node(r, "a").unwrap();
    assert_eq!(doc.node_type(attr), NodeType::ATTRIBUTE_NODE);
    assert_eq!(doc.node_name(attr), "a");
    assert_eq!(doc.node_value(attr), Some("1"));
    let map = doc.attributes(r);
    assert_eq!(map.length(), 1);
    assert_eq!(map.item(0), Some(attr));
    assert_eq!(map.get_named_item("a"), Some(attr));
  }

  #[test]
  fn set_and_remove_attribute_node() {
    let mut doc = Document::new();
    let e = doc.create_element("e").unwrap();
    let attr = doc.create_attribute("k").unwrap();
    doc.set_attribute_node(e, attr).unwrap();
    assert_eq!(doc.node_type(attr), NodeType::ATTRIBUTE_NODE);
    assert_eq!(doc.attributes(e).length(), 1);
    doc.remove_attribute_node(e, attr).unwrap();
    assert!(doc.attributes(e).is_empty());
  }

  #[test]
  fn an_attribute_cannot_belong_to_two_elements() {
    let mut doc = Document::new();
    let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
    let attr = doc.create_attribute("k").unwrap();
    doc.set_attribute_node(a, attr).unwrap();
    assert_eq!(doc.set_attribute_node(b, attr).unwrap_err().code(), ExceptionCode::INUSE_ATTRIBUTE_ERR);
  }

  #[test]
  fn an_attribute_is_not_a_child() {
    let mut doc = Document::new();
    let e = doc.create_element("e").unwrap();
    let attr = doc.create_attribute("k").unwrap();
    assert_eq!(doc.append_child(e, attr).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  }

  #[test]
  fn get_elements_by_tag_name_walks_in_document_order() {
    let (doc, _, _, _) = sample();
    let names: Vec<_> = doc.get_elements_by_tag_name("*").iter().map(|n| doc.node_name(n)).collect();
    assert_eq!(names, ["r", "b", "c"]);
    assert_eq!(doc.get_elements_by_tag_name("b").length(), 1);
  }

  #[test]
  fn get_elements_by_tag_name_ns_filters_on_namespace() {
    let mut doc = Document::new();
    let root = doc.create_element_ns(Some("urn:x"), "p:a").unwrap();
    let child = doc.create_element_ns(Some("urn:y"), "q:a").unwrap();
    doc.append_child(root, child).unwrap();
    doc.append_child(doc.document_node(), root).unwrap();
    assert_eq!(doc.get_elements_by_tag_name_ns(Some("urn:x"), "a").length(), 1);
    assert_eq!(doc.get_elements_by_tag_name_ns(Some("*"), "a").length(), 2);
    assert_eq!(doc.get_elements_by_tag_name_ns(Some("urn:y"), "*").length(), 1);
  }

  #[test]
  fn get_element_by_id_finds_marked_ids() {
    let mut doc = Document::new();
    let root = doc.create_element("root").unwrap();
    doc.set_attribute(root, "id", "top").unwrap();
    doc.append_child(doc.document_node(), root).unwrap();
    // Not an ID until marked as one.
    assert_eq!(doc.get_element_by_id("top"), None);
    doc.set_id_attribute(root, "id", true).unwrap();
    assert_eq!(doc.get_element_by_id("top"), Some(root));
  }

  #[test]
  fn replace_child_swaps_a_node() {
    let (mut doc, r, b, c) = sample();
    let x = doc.create_element("x").unwrap();
    let removed = doc.replace_child(r, x, b).unwrap();
    assert_eq!(removed, b);
    assert_eq!(doc.parent(b), None);
    assert_eq!(doc.first_child(r), Some(x));
    assert_eq!(doc.last_child(r), Some(c));
  }

  #[test]
  fn a_document_fragment_inserts_its_children() {
    let mut doc = Document::new();
    let root = doc.create_element("root").unwrap();
    doc.append_child(doc.document_node(), root).unwrap();
    let fragment = doc.create_document_fragment();
    let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
    doc.append_child(fragment, a).unwrap();
    doc.append_child(fragment, b).unwrap();
    doc.append_child(root, fragment).unwrap();
    // The fragment's children moved in; the fragment is left empty.
    let names: Vec<_> = doc.children(root).map(|n| doc.node_name(n)).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(doc.first_child(fragment), None);
  }

  #[test]
  fn a_document_takes_only_one_root_element() {
    let mut doc = Document::new();
    let first = doc.create_element("a").unwrap();
    let second = doc.create_element("b").unwrap();
    doc.append_child(doc.document_node(), first).unwrap();
    assert_eq!(doc.append_child(doc.document_node(), second).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  }

  #[test]
  fn a_document_refuses_bare_text() {
    let mut doc = Document::new();
    let text = doc.create_text_node("x");
    assert_eq!(doc.append_child(doc.document_node(), text).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  }

  #[test]
  fn a_fragment_of_two_elements_is_refused_by_the_document_whole() {
    let mut doc = Document::new();
    let fragment = doc.create_document_fragment();
    let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
    doc.append_child(fragment, a).unwrap();
    doc.append_child(fragment, b).unwrap();
    // Two root elements at once: refused before anything is inserted.
    assert_eq!(
      doc.append_child(doc.document_node(), fragment).unwrap_err().code(),
      ExceptionCode::HIERARCHY_REQUEST_ERR
    );
    assert!(doc.document_element().is_none(), "the tree is untouched by the failed insert");
  }

  #[test]
  fn namespace_rules_are_enforced() {
    let mut doc = Document::new();
    // A prefix with no namespace.
    assert_eq!(doc.create_element_ns(None, "p:a").unwrap_err().code(), ExceptionCode::NAMESPACE_ERR);
    // The xml prefix bound to the wrong namespace.
    assert_eq!(doc.create_element_ns(Some("urn:x"), "xml:a").unwrap_err().code(), ExceptionCode::NAMESPACE_ERR);
    // An xmlns attribute in the wrong namespace.
    let e = doc.create_element("e").unwrap();
    assert_eq!(
      doc.set_attribute_ns(e, Some("urn:x"), "xmlns:p", "v").unwrap_err().code(),
      ExceptionCode::NAMESPACE_ERR
    );
  }

  #[cfg(feature = "parse")]
  #[test]
  fn base_uri_walks_to_the_nearest_recorded_base() {
    let mut doc = Document::new();
    doc.set_document_base(Some("file:///doc.xml"));
    let a = doc.create_element("a").unwrap();
    doc.append_child(doc.document_node(), a).unwrap();
    let b = doc.create_element("b").unwrap();
    doc.append_child(a, b).unwrap();
    doc.set_element_base(b, Some("file:///sub/"));
    let text = doc.create_text_node("x");
    doc.append_child(b, text).unwrap();

    assert_eq!(doc.base_uri(a).as_deref(), Some("file:///doc.xml"), "falls back to the document base");
    assert_eq!(doc.base_uri(b).as_deref(), Some("file:///sub/"), "uses its own recorded base");
    assert_eq!(doc.base_uri(text).as_deref(), Some("file:///sub/"), "a text node inherits the nearest element's base");
    let bare = Document::new();
    assert_eq!(bare.base_uri(bare.document_node()), None, "no base information means no base URI");
  }
}
