//! The document: the arena that owns every node, and the operations over it.
//!
//! A [`Document`] holds the nodes in a `Vec` and a [`NamePool`] for their names. Nodes are made
//! with the `create_*` methods, joined into a tree with [`append_child`](Document::append_child)
//! and its siblings, and read back through the navigation and value accessors — or through a
//! [`NodeRef`](crate::NodeRef) for chained reads.

use xenolith_core::chars;
use xenolith_core::name::{NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};

use crate::collection::{NamedNodeMap, NodeList, Query};
use crate::exception::{DomException, ExceptionCode, Result};
use crate::node::{AttrData, ElementData, NodeData, NodeId, NodeSlot, NodeType};
use crate::noderef::NodeRef;

/// An XML document: an arena of nodes with a tree over them.
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
/// doc.append_child(doc.root(), root)?;
///
/// assert_eq!(doc.document_element(), Some(root));
/// assert_eq!(doc.node_type(root), NodeType::Element);
/// assert_eq!(doc.text_content(doc.root()), "Hello");
/// # Ok::<(), xenolith_dom::DomException>(())
/// ```
#[derive(Debug)]
pub struct Document {
  nodes: Vec<NodeSlot>,
  pool: NamePool,
  /// The document's own base URI (its system identifier), interned; the fallback for a node
  /// with no nearer base. `None` unless recorded when the tree was built.
  base: Option<NameId>,
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
    Self { nodes: vec![NodeSlot::new(NodeData::Document)], pool: NamePool::new(), base: None }
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
    Ok(self.push(NodeData::Element(ElementData { name, attributes: Vec::new(), base: None })))
  }

  /// Creates an element in a namespace, from a namespace name and a qualified name.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name;
  /// [`ExceptionCode::Namespace`] if the prefix and the namespace are inconsistent (a prefix
  /// with no namespace, or the `xml` prefix bound to anything but the XML namespace).
  pub fn create_element_ns(&mut self, namespace: Option<&str>, qualified_name: &str) -> Result<NodeId> {
    check_qname_namespace(namespace, qualified_name, false)?;
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    Ok(self.push(NodeData::Element(ElementData { name, attributes: Vec::new(), base: None })))
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

  /// Copies a node from another document into this one, returning a new detached node.
  ///
  /// This is the DOM's `importNode`: the copy belongs to this document — its names are re-interned
  /// here — and is detached, ready to be placed in the tree. With `deep`, the node's descendants
  /// come too. An element's attributes always come with it. The base URI recorded on an element
  /// is not copied; it is a computed property, and a caller that needs to preserve it (such as
  /// XInclude) writes an `xml:base` attribute instead.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NotSupported`] for a node that cannot be imported — a document, a document
  /// type, or a bare attribute.
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
  /// doc.append_child(doc.root(), copy)?;
  /// assert_eq!(doc.text_content(copy), "hi");
  /// # Ok::<(), xenolith_dom::DomException>(())
  /// ```
  pub fn import_node(&mut self, source: &Document, node: NodeId, deep: bool) -> Result<NodeId> {
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
        return Err(DomException::new(ExceptionCode::NotSupported, "this kind of node cannot be imported"));
      }
    };
    Ok(imported)
  }

  /// Copies a node within this document, returning a new detached node — the DOM's `cloneNode`.
  ///
  /// With `deep`, descendants come too; an element's attributes always do. As with
  /// [`import_node`](Self::import_node), the computed base URI is not copied.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::NotSupported`] for a node that cannot be cloned this way — a document, a
  /// document type, or a bare attribute.
  pub fn clone_node(&mut self, node: NodeId, deep: bool) -> Result<NodeId> {
    let cloned = match self.slot(node).data.node_type() {
      NodeType::Element => {
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
        if deep {
          for child in self.children(node).collect::<Vec<_>>() {
            let child = self.clone_node(child, true)?;
            self.append_child(element, child)?;
          }
        }
        element
      }
      NodeType::Text => {
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_text_node(&data)
      }
      NodeType::CdataSection => {
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_cdata_section(&data)
      }
      NodeType::Comment => {
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_comment(&data)
      }
      NodeType::ProcessingInstruction => {
        let target = self.node_name(node);
        let data = self.node_value(node).unwrap_or_default().to_owned();
        self.create_processing_instruction(&target, &data)?
      }
      NodeType::DocumentFragment => {
        let fragment = self.create_document_fragment();
        if deep {
          for child in self.children(node).collect::<Vec<_>>() {
            let child = self.clone_node(child, true)?;
            self.append_child(fragment, child)?;
          }
        }
        fragment
      }
      NodeType::Document | NodeType::DocumentType | NodeType::Attribute => {
        return Err(DomException::new(ExceptionCode::NotSupported, "this kind of node cannot be cloned"));
      }
    };
    Ok(cloned)
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

  /// The DOM `nodeValue`: the value of an attribute, or the character data of a text, CDATA,
  /// comment or PI node; `None` for the kinds that have no value of their own.
  #[must_use]
  pub fn node_value(&self, id: NodeId) -> Option<&str> {
    match &self.slot(id).data {
      NodeData::Attribute(attr) => Some(&attr.value),
      NodeData::Text(data) | NodeData::CdataSection(data) | NodeData::Comment(data) => Some(data),
      NodeData::ProcessingInstruction { data, .. } => Some(data),
      _ => None,
    }
  }

  /// The name of an element or attribute node, if this is one.
  fn name_of(&self, id: NodeId) -> Option<&QName> {
    match &self.slot(id).data {
      NodeData::Element(element) => Some(&element.name),
      NodeData::Attribute(attr) => Some(&attr.name),
      _ => None,
    }
  }

  /// The local part of an element's or attribute's name.
  #[must_use]
  pub fn local_name(&self, id: NodeId) -> Option<&str> {
    self.name_of(id).map(|name| self.pool.resolve(name.local()))
  }

  /// The namespace prefix of an element's or attribute's name, if it has one.
  #[must_use]
  pub fn prefix(&self, id: NodeId) -> Option<&str> {
    self.name_of(id).and_then(|name| name.prefix).map(|p| self.pool.resolve(p))
  }

  /// The namespace name of an element or attribute, if it is in one.
  #[must_use]
  pub fn namespace_uri(&self, id: NodeId) -> Option<&str> {
    self.name_of(id).and_then(|name| name.namespace()).map(|ns| self.pool.resolve(ns))
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

  /// The DOM `baseURI` of a node (XML Base): the base URI in effect where the node is.
  ///
  /// It is the base recorded on the nearest element at or above the node — the document builder
  /// records each element's, resolved from `xml:base` and the document's system identifier — or,
  /// failing that, the document's own base URI. `None` for a tree built by hand without base
  /// information, or a document parsed without a system identifier and without `xml:base`.
  ///
  /// The base of an attribute is that of its owning element.
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
  #[cfg(feature = "parse")]
  pub(crate) fn set_document_base(&mut self, base: Option<&str>) {
    self.base = base.map(|base| self.pool.intern(base));
  }

  /// Records the effective base URI of an element. Used by the builder.
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
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name.
  pub fn create_attribute(&mut self, qualified_name: &str) -> Result<NodeId> {
    let name = self.parse_qname(qualified_name, None)?;
    Ok(self.push(NodeData::Attribute(AttrData { name, value: String::new(), owner: None, is_id: false })))
  }

  /// Creates a detached attribute node in a namespace.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name;
  /// [`ExceptionCode::Namespace`] if the prefix and namespace are inconsistent.
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
  /// [`ExceptionCode::NotSupported`] if the node is not an element, or
  /// [`ExceptionCode::InvalidCharacter`] if `qualified_name` is not a legal name.
  pub fn set_attribute(&mut self, element: NodeId, qualified_name: &str, value: &str) -> Result<()> {
    self.require_element(element)?;
    let name = self.parse_qname(qualified_name, None)?;
    self.put_attribute(element, name, value);
    Ok(())
  }

  /// Sets an attribute in a namespace, adding it or replacing the value of the one with the same
  /// namespace and local name.
  ///
  /// # Errors
  ///
  /// As [`set_attribute`](Self::set_attribute), plus [`ExceptionCode::Namespace`] if the prefix
  /// and namespace are inconsistent.
  pub fn set_attribute_ns(
    &mut self,
    element: NodeId,
    namespace: Option<&str>,
    qualified_name: &str,
    value: &str,
  ) -> Result<()> {
    self.require_element(element)?;
    check_qname_namespace(namespace, qualified_name, true)?;
    let namespace = namespace.map(|ns| self.pool.intern(ns));
    let name = self.parse_qname(qualified_name, namespace)?;
    self.put_attribute(element, name, value);
    Ok(())
  }

  /// Adds or updates the attribute of `element` with `name`, giving it `value`.
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
  fn find_attribute(&self, element: NodeId, predicate: impl Fn(&AttrData) -> bool) -> Option<NodeId> {
    let data = self.element_data(element)?;
    data.attributes.iter().copied().find(|&attr| match &self.slot(attr).data {
      NodeData::Attribute(attr) => predicate(attr),
      _ => false,
    })
  }

  /// The attribute node of an element by qualified name, if it has one.
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
  #[must_use]
  pub fn get_attribute_node_ns(&self, element: NodeId, namespace: Option<&str>, local: &str) -> Option<NodeId> {
    let namespace = match namespace {
      Some(ns) => Some(self.pool.get(ns)?),
      None => None,
    };
    let local = self.pool.get(local)?;
    self.find_attribute(element, |a| a.name.namespace() == namespace && a.name.local() == local)
  }

  /// The element an attribute node belongs to (the DOM's `ownerElement`), or `None` for a
  /// detached attribute or a node that is not an attribute.
  #[must_use]
  pub fn owner_element(&self, attr: NodeId) -> Option<NodeId> {
    match &self.slot(attr).data {
      NodeData::Attribute(data) => data.owner,
      _ => None,
    }
  }

  /// The value of an element's attribute, by qualified name.
  #[must_use]
  pub fn attribute(&self, element: NodeId, qualified_name: &str) -> Option<&str> {
    self.get_attribute_node(element, qualified_name).and_then(|attr| self.node_value(attr))
  }

  /// The value of an element's attribute, by namespace name and local name.
  #[must_use]
  pub fn attribute_ns(&self, element: NodeId, namespace: Option<&str>, local: &str) -> Option<&str> {
    self.get_attribute_node_ns(element, namespace, local).and_then(|attr| self.node_value(attr))
  }

  /// Whether an element has an attribute with the given qualified name.
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
  /// [`ExceptionCode::NotSupported`] if the node is not an element.
  pub fn remove_attribute(&mut self, element: NodeId, qualified_name: &str) -> Result<()> {
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
  /// [`ExceptionCode::NotSupported`] if `element` is not an element or `attr` is not an
  /// attribute; [`ExceptionCode::InuseAttribute`] if `attr` already belongs to another element.
  pub fn set_attribute_node(&mut self, element: NodeId, attr: NodeId) -> Result<()> {
    self.require_element(element)?;
    let name = match &self.slot(attr).data {
      NodeData::Attribute(data) => data.name,
      _ => return Err(DomException::new(ExceptionCode::NotSupported, "not an attribute node")),
    };
    match self.attr_data(attr).owner {
      Some(owner) if owner != element => {
        return Err(DomException::new(ExceptionCode::InuseAttribute, "the attribute already belongs to an element"));
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
  /// [`ExceptionCode::NotFound`] if `attr` is not an attribute of `element`.
  pub fn remove_attribute_node(&mut self, element: NodeId, attr: NodeId) -> Result<NodeId> {
    let attributes = &mut self.element_data_mut(element).attributes;
    let Some(position) = attributes.iter().position(|&a| a == attr) else {
      return Err(DomException::new(ExceptionCode::NotFound, "the attribute does not belong to the element"));
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
  /// [`ExceptionCode::NotFound`] if `element` has no such attribute.
  pub fn set_id_attribute(&mut self, element: NodeId, qualified_name: &str, is_id: bool) -> Result<()> {
    let Some(attr) = self.get_attribute_node(element, qualified_name) else {
      return Err(DomException::new(ExceptionCode::NotFound, "the element has no such attribute"));
    };
    self.attr_data_mut(attr).is_id = is_id;
    Ok(())
  }

  /// The element carrying an `ID`-typed attribute equal to `id`, in document order, if any.
  ///
  /// An attribute counts only if it was marked with [`set_id_attribute`](Self::set_id_attribute):
  /// the DOM has no way to know an attribute named `id` is an ID unless a DTD, a schema, or the
  /// caller says so. The [document builder](crate::build) marks DTD- and `xml:id`-typed
  /// attributes for you.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_dom::Document;
  ///
  /// let mut doc = Document::new();
  /// let e = doc.create_element("section")?;
  /// doc.set_attribute(e, "id", "intro")?;
  /// doc.append_child(doc.root(), e)?;
  ///
  /// // Not found until the attribute is declared to be an ID.
  /// assert_eq!(doc.get_element_by_id("intro"), None);
  /// doc.set_id_attribute(e, "id", true)?;
  /// assert_eq!(doc.get_element_by_id("intro"), Some(e));
  /// # Ok::<(), xenolith_dom::DomException>(())
  /// ```
  #[must_use]
  pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
    self.descendants(self.root()).find(|&node| {
      matches!(self.node_type(node), NodeType::Element)
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
      Err(DomException::new(ExceptionCode::NotSupported, "attributes belong to elements only"))
    }
  }

  // --- Collections --------------------------------------------------------------------------

  /// The children of a node as a live [`NodeList`].
  #[must_use]
  pub fn child_nodes(&self, id: NodeId) -> NodeList<'_> {
    NodeList::new(self, Query::Children(id))
  }

  /// The descendant elements with a given qualified name, in document order, as a live
  /// [`NodeList`]. The name `"*"` matches every element.
  #[must_use]
  pub fn get_elements_by_tag_name(&self, name: &str) -> NodeList<'_> {
    NodeList::new(self, Query::by_tag_name(self.root(), name))
  }

  /// The descendant elements with a given namespace and local name, in document order, as a
  /// live [`NodeList`]. Either argument may be `"*"` to match any.
  #[must_use]
  pub fn get_elements_by_tag_name_ns(&self, namespace: Option<&str>, local: &str) -> NodeList<'_> {
    NodeList::new(self, Query::by_tag_name_ns(self.root(), namespace, local))
  }

  /// The descendants of a node in document order (preorder), excluding attribute nodes.
  pub(crate) fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let mut stack: Vec<NodeId> = self.children(id).collect();
    stack.reverse();
    std::iter::from_fn(move || {
      let node = stack.pop()?;
      let mut children: Vec<NodeId> = self.children(node).collect();
      children.reverse();
      stack.extend(children);
      Some(node)
    })
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

  /// Appends character `data` to `parent`, extending its last child when that child is a text node.
  ///
  /// Adjacent character data is thus kept as a single text node, as the data model requires, even when
  /// the parser delivers a long run of text in fragments. When the last child is not a text node (or
  /// there is none), a new text node is created and appended.
  ///
  /// # Errors
  ///
  /// As [`append_child`](Self::append_child), when a new text node must be appended.
  pub fn append_text(&mut self, parent: NodeId, data: &str) -> Result<NodeId> {
    if let Some(last) = self.last_child(parent) {
      if let NodeData::Text(existing) = &mut self.nodes[last.index()].data {
        existing.push_str(data);
        return Ok(last);
      }
    }
    let node = self.create_text_node(data);
    self.append_child(parent, node)
  }

  /// Inserts `child` under `parent` before `reference`, or at the end when `reference` is
  /// `None`. Detaches `child` from any current parent first.
  ///
  /// A [document fragment](Self::create_document_fragment) is not itself inserted: its children
  /// are moved in, in order, and the fragment is left empty — the DOM's behaviour.
  ///
  /// # Errors
  ///
  /// [`ExceptionCode::HierarchyRequest`] if `parent` cannot hold children, if `child` cannot be
  /// a child, if the insertion would make a cycle, or if it would break the document's own rules
  /// (one root element, one doctype, no text directly under the document);
  /// [`ExceptionCode::NotFound`] if `reference` is not a child of `parent`.
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
  /// As [`insert_before`](Self::insert_before); [`ExceptionCode::NotFound`] if `old_child` is
  /// not a child of `parent`.
  pub fn replace_child(&mut self, parent: NodeId, new_child: NodeId, old_child: NodeId) -> Result<NodeId> {
    if self.slot(old_child).parent != Some(parent) {
      return Err(DomException::new(ExceptionCode::NotFound, "the node to replace is not a child of the parent"));
    }
    // Insert the new child before the old one, then take the old one out. Insertion is validated
    // first (so a bad new child leaves the tree untouched); the reference is the old child.
    self.insert_before(parent, new_child, Some(old_child))?;
    self.detach(old_child);
    Ok(old_child)
  }

  /// The shared validity checks for inserting `child` under `parent` before `reference`.
  fn check_insertion(&self, parent: NodeId, child: NodeId, reference: Option<NodeId>) -> Result<()> {
    if !self.slot(parent).data.is_container() {
      let name = self.node_name(parent);
      return Err(DomException::new(ExceptionCode::HierarchyRequest, format!("\"{name}\" cannot have children")));
    }
    if !self.slot(child).data.can_be_child() {
      let name = self.node_name(child);
      return Err(DomException::new(ExceptionCode::HierarchyRequest, format!("\"{name}\" cannot be a child")));
    }
    if child == parent || self.is_ancestor(child, parent) {
      return Err(DomException::new(ExceptionCode::HierarchyRequest, "a node cannot be made a descendant of itself"));
    }
    if let Some(reference) = reference {
      if self.slot(reference).parent != Some(parent) {
        return Err(DomException::new(ExceptionCode::NotFound, "the reference node is not a child of the parent"));
      }
    }
    if matches!(self.slot(child).data, NodeData::DocumentFragment) {
      self.check_fragment_into_document(parent, child)?;
    } else {
      self.check_child_of_document(parent, child)?;
    }
    Ok(())
  }

  /// Enforces the document node's own child rules for a single node: at most one element and one
  /// doctype, and no character data directly under it. A no-op when `parent` is not the document.
  fn check_child_of_document(&self, parent: NodeId, child: NodeId) -> Result<()> {
    if !matches!(self.slot(parent).data, NodeData::Document) {
      return Ok(());
    }
    let has_element = self.document_element().is_some_and(|e| e != child);
    let has_doctype = self.doctype().is_some_and(|d| d != child);
    document_child_error(self.slot(child).data.node_type(), has_element, has_doctype)
  }

  /// Enforces the document's child rules for every node a fragment would bring in at once, so a
  /// fragment with two elements is refused before any of it is inserted.
  fn check_fragment_into_document(&self, parent: NodeId, fragment: NodeId) -> Result<()> {
    if !matches!(self.slot(parent).data, NodeData::Document) {
      return Ok(());
    }
    let mut has_element = self.document_element().is_some();
    let mut has_doctype = self.doctype().is_some();
    for grandchild in self.children(fragment) {
      let node_type = self.slot(grandchild).data.node_type();
      document_child_error(node_type, has_element, has_doctype)?;
      has_element |= node_type == NodeType::Element;
      has_doctype |= node_type == NodeType::DocumentType;
    }
    Ok(())
  }

  /// Links an already-detached node under `parent`, at `reference` or at the end.
  fn place(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
    match reference {
      Some(reference) => self.link_before(parent, child, reference),
      None => self.link_last(parent, child),
    }
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

  pub(crate) fn slot(&self, id: NodeId) -> &NodeSlot {
    &self.nodes[id.index()]
  }
}

/// The error, if any, of putting a node of this type directly under the document, given whether
/// the document already has an element and a doctype.
fn document_child_error(node_type: NodeType, has_element: bool, has_doctype: bool) -> Result<()> {
  let offending = match node_type {
    NodeType::Text | NodeType::CdataSection => Some("character data"),
    NodeType::Element if has_element => Some("a second root element"),
    NodeType::DocumentType if has_doctype => Some("a second document type"),
    _ => None,
  };
  match offending {
    Some(what) => Err(DomException::new(ExceptionCode::HierarchyRequest, format!("a document may not contain {what}"))),
    None => Ok(()),
  }
}

/// Checks a qualified name against a namespace for the `*NS` constructors (Namespaces in XML,
/// as the DOM applies it). `is_attribute` turns on the extra `xmlns` rules that apply to
/// attribute names.
fn check_qname_namespace(namespace: Option<&str>, qualified_name: &str, is_attribute: bool) -> Result<()> {
  let Some((prefix, _)) = chars::split_qname(qualified_name) else {
    return Err(DomException::new(
      ExceptionCode::InvalidCharacter,
      format!("{qualified_name:?} is not a valid qualified name"),
    ));
  };
  let namespace_error = |message: &str| Err(DomException::new(ExceptionCode::Namespace, message.to_owned()));

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

  #[test]
  fn attributes_are_nodes() {
    let (doc, r, _, _) = sample();
    let attr = doc.get_attribute_node(r, "a").unwrap();
    assert_eq!(doc.node_type(attr), NodeType::Attribute);
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
    assert_eq!(doc.node_type(attr), NodeType::Attribute);
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
    assert_eq!(doc.set_attribute_node(b, attr).unwrap_err().code(), ExceptionCode::InuseAttribute);
  }

  #[test]
  fn an_attribute_is_not_a_child() {
    let mut doc = Document::new();
    let e = doc.create_element("e").unwrap();
    let attr = doc.create_attribute("k").unwrap();
    assert_eq!(doc.append_child(e, attr).unwrap_err().code(), ExceptionCode::HierarchyRequest);
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
    doc.append_child(doc.root(), root).unwrap();
    assert_eq!(doc.get_elements_by_tag_name_ns(Some("urn:x"), "a").length(), 1);
    assert_eq!(doc.get_elements_by_tag_name_ns(Some("*"), "a").length(), 2);
    assert_eq!(doc.get_elements_by_tag_name_ns(Some("urn:y"), "*").length(), 1);
  }

  #[test]
  fn get_element_by_id_finds_marked_ids() {
    let mut doc = Document::new();
    let root = doc.create_element("root").unwrap();
    doc.set_attribute(root, "id", "top").unwrap();
    doc.append_child(doc.root(), root).unwrap();
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
    doc.append_child(doc.root(), root).unwrap();
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
    doc.append_child(doc.root(), first).unwrap();
    assert_eq!(doc.append_child(doc.root(), second).unwrap_err().code(), ExceptionCode::HierarchyRequest);
  }

  #[test]
  fn a_document_refuses_bare_text() {
    let mut doc = Document::new();
    let text = doc.create_text_node("x");
    assert_eq!(doc.append_child(doc.root(), text).unwrap_err().code(), ExceptionCode::HierarchyRequest);
  }

  #[test]
  fn a_fragment_of_two_elements_is_refused_by_the_document_whole() {
    let mut doc = Document::new();
    let fragment = doc.create_document_fragment();
    let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
    doc.append_child(fragment, a).unwrap();
    doc.append_child(fragment, b).unwrap();
    // Two root elements at once: refused before anything is inserted.
    assert_eq!(doc.append_child(doc.root(), fragment).unwrap_err().code(), ExceptionCode::HierarchyRequest);
    assert!(doc.document_element().is_none(), "the tree is untouched by the failed insert");
  }

  #[test]
  fn namespace_rules_are_enforced() {
    let mut doc = Document::new();
    // A prefix with no namespace.
    assert_eq!(doc.create_element_ns(None, "p:a").unwrap_err().code(), ExceptionCode::Namespace);
    // The xml prefix bound to the wrong namespace.
    assert_eq!(doc.create_element_ns(Some("urn:x"), "xml:a").unwrap_err().code(), ExceptionCode::Namespace);
    // An xmlns attribute in the wrong namespace.
    let e = doc.create_element("e").unwrap();
    assert_eq!(doc.set_attribute_ns(e, Some("urn:x"), "xmlns:p", "v").unwrap_err().code(), ExceptionCode::Namespace);
  }

  #[cfg(feature = "parse")]
  #[test]
  fn base_uri_walks_to_the_nearest_recorded_base() {
    let mut doc = Document::new();
    doc.set_document_base(Some("file:///doc.xml"));
    let a = doc.create_element("a").unwrap();
    doc.append_child(doc.root(), a).unwrap();
    let b = doc.create_element("b").unwrap();
    doc.append_child(a, b).unwrap();
    doc.set_element_base(b, Some("file:///sub/"));
    let text = doc.create_text_node("x");
    doc.append_child(b, text).unwrap();

    assert_eq!(doc.base_uri(a).as_deref(), Some("file:///doc.xml"), "falls back to the document base");
    assert_eq!(doc.base_uri(b).as_deref(), Some("file:///sub/"), "uses its own recorded base");
    assert_eq!(doc.base_uri(text).as_deref(), Some("file:///sub/"), "a text node inherits the nearest element's base");
    assert_eq!(Document::new().base_uri(NodeId(0)), None, "no base information means no base URI");
  }
}
