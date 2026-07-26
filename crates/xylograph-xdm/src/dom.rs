//! The XPath data model over a [`xylograph_dom`] document.
//!
//! The DOM is close to the XPath model but not the same, so this view adjusts three things as it
//! reads (never writing): a run of adjacent text and CDATA nodes reads as one text node; every
//! element gains namespace nodes for the declarations in scope; and every node — attributes and
//! namespace nodes included — has a place in one document order, computed once when the model is
//! made.

use std::cmp::Ordering;
use std::collections::HashMap;

use xylograph_core::name::{NameId, XML_NS_URI, XMLNS_NS_URI};
use xylograph_dom::{Document, NodeId, NodeType};

use crate::{ExpandedName, Model, NodeKind};

/// A node in the XPath view of a document.
///
/// Most nodes are a DOM node ([`Tree`](DomNode::Tree)); a text node names the first DOM node of
/// its run ([`Text`](DomNode::Text)); a namespace node is synthesized from an element and the
/// prefix it binds ([`Namespace`](DomNode::Namespace)).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DomNode {
  /// A DOM node: the document (the XPath root), an element, an attribute, a comment or a PI.
  Tree(NodeId),
  /// A text node, named by the first DOM text or CDATA node of its run.
  Text(NodeId),
  /// A namespace node: a prefix bound on an element.
  Namespace {
    /// The element the namespace node belongs to.
    element: NodeId,
    /// The prefix it binds, or `None` for the default namespace.
    prefix: Option<NameId>,
  },
}

/// The XPath data model over a borrowed [`Document`].
#[derive(Debug)]
pub struct DomModel<'a> {
  doc: &'a Document,
  /// Every node's position in document order, filled once at construction.
  order: HashMap<DomNode, usize>,
}

impl<'a> DomModel<'a> {
  /// Builds the model over `doc`, computing document order for the whole tree.
  #[must_use]
  pub fn new(doc: &'a Document) -> Self {
    let mut model = Self { doc, order: HashMap::new() };
    let mut order = HashMap::new();
    let mut counter = 0;
    model.number(model.root_node(), &mut order, &mut counter);
    model.order = order;
    model
  }

  /// The root node — the document.
  #[must_use]
  pub fn root_node(&self) -> DomNode {
    DomNode::Tree(self.doc.root())
  }

  /// The XPath node for a DOM node: a text or CDATA node maps to its run's text node.
  #[must_use]
  pub fn node(&self, id: NodeId) -> DomNode {
    if self.is_text_like(id) { DomNode::Text(self.run_start(id)) } else { DomNode::Tree(id) }
  }

  /// Assigns document-order indices: a node, then its namespace nodes, its attributes, and its
  /// children in turn.
  fn number(&self, node: DomNode, order: &mut HashMap<DomNode, usize>, counter: &mut usize) {
    order.insert(node, *counter);
    *counter += 1;
    for namespace in self.namespaces_of(node) {
      order.insert(namespace, *counter);
      *counter += 1;
    }
    for attribute in self.attributes_of(node) {
      order.insert(attribute, *counter);
      *counter += 1;
    }
    for child in self.children_of(node) {
      self.number(child, order, counter);
    }
  }

  fn is_text_like(&self, id: NodeId) -> bool {
    matches!(self.doc.node_type(id), NodeType::Text | NodeType::CdataSection)
  }

  /// The first DOM node of the text run that `id` belongs to.
  fn run_start(&self, id: NodeId) -> NodeId {
    let mut first = id;
    while let Some(previous) = self.doc.previous_sibling(first) {
      if self.is_text_like(previous) {
        first = previous;
      } else {
        break;
      }
    }
    first
  }

  fn children_of(&self, node: DomNode) -> Vec<DomNode> {
    let DomNode::Tree(id) = node else { return Vec::new() };
    if !matches!(self.doc.node_type(id), NodeType::Document | NodeType::Element) {
      return Vec::new();
    }
    let mut children = Vec::new();
    let mut child = self.doc.first_child(id);
    while let Some(current) = child {
      match self.doc.node_type(current) {
        NodeType::Text | NodeType::CdataSection => {
          children.push(DomNode::Text(current));
          child = self.after_run(current);
        }
        NodeType::Element | NodeType::Comment | NodeType::ProcessingInstruction => {
          children.push(DomNode::Tree(current));
          child = self.doc.next_sibling(current);
        }
        // A document type or anything else is not part of the XPath tree.
        _ => child = self.doc.next_sibling(current),
      }
    }
    children
  }

  /// The sibling after the text run beginning at `start`.
  fn after_run(&self, start: NodeId) -> Option<NodeId> {
    let mut next = self.doc.next_sibling(start);
    while let Some(node) = next {
      if self.is_text_like(node) {
        next = self.doc.next_sibling(node);
      } else {
        break;
      }
    }
    next
  }

  fn attributes_of(&self, node: DomNode) -> Vec<DomNode> {
    match node {
      DomNode::Tree(id) if self.doc.node_type(id) == NodeType::Element => self
        .doc
        .attributes(id)
        .iter()
        // Namespace declarations are namespace nodes, not attributes.
        .filter(|&attr| self.declared_prefix(attr).is_none())
        .map(DomNode::Tree)
        .collect(),
      _ => Vec::new(),
    }
  }

  fn namespaces_of(&self, node: DomNode) -> Vec<DomNode> {
    match node {
      DomNode::Tree(id) if self.doc.node_type(id) == NodeType::Element => {
        self.in_scope_prefixes(id).into_iter().map(|prefix| DomNode::Namespace { element: id, prefix }).collect()
      }
      _ => Vec::new(),
    }
  }

  /// The prefixes in scope on an element, in a stable order (default first, then by name), each
  /// with a non-empty binding, plus the implicit `xml`.
  fn in_scope_prefixes(&self, element: NodeId) -> Vec<Option<NameId>> {
    let mut seen: Vec<Option<NameId>> = Vec::new();
    let mut result: Vec<Option<NameId>> = Vec::new();
    let mut current = Some(element);
    while let Some(node) = current {
      if self.doc.node_type(node) == NodeType::Element {
        for attribute in self.doc.attributes(node).iter() {
          if let Some(prefix) = self.declared_prefix(attribute) {
            if !seen.contains(&prefix) {
              seen.push(prefix);
              // An empty value undeclares the prefix, so it shadows but adds no node.
              if !self.doc.node_value(attribute).unwrap_or_default().is_empty() {
                result.push(prefix);
              }
            }
          }
        }
      }
      current = self.doc.parent(node);
    }
    if !result.contains(&Some(NameId::XML)) {
      result.push(Some(NameId::XML));
    }
    result.sort_by_key(|&prefix| self.prefix_key(prefix));
    result
  }

  /// A sort key ordering the default namespace first, then prefixes by name.
  fn prefix_key(&self, prefix: Option<NameId>) -> (bool, String) {
    match prefix {
      None => (false, String::new()),
      Some(name) => (true, self.doc.pool().resolve(name).to_owned()),
    }
  }

  /// If `attribute` is a namespace declaration, the prefix it declares (`None` for the default).
  fn declared_prefix(&self, attribute: NodeId) -> Option<Option<NameId>> {
    if self.doc.namespace_uri(attribute) != Some(XMLNS_NS_URI) {
      return None;
    }
    match self.doc.prefix(attribute) {
      Some("xmlns") => Some(self.doc.local_name(attribute).and_then(|local| self.doc.pool().get(local))),
      _ => Some(None),
    }
  }

  /// The namespace URI a prefix is bound to on an element, if any.
  fn namespace_uri(&self, element: NodeId, prefix: Option<NameId>) -> Option<String> {
    if prefix == Some(NameId::XML) {
      return Some(XML_NS_URI.to_owned());
    }
    let mut current = Some(element);
    while let Some(node) = current {
      if self.doc.node_type(node) == NodeType::Element {
        for attribute in self.doc.attributes(node).iter() {
          if self.declared_prefix(attribute) == Some(prefix) {
            let value = self.doc.node_value(attribute).unwrap_or_default();
            return (!value.is_empty()).then(|| value.to_owned());
          }
        }
      }
      current = self.doc.parent(node);
    }
    None
  }
}

impl Model for DomModel<'_> {
  type Node = DomNode;

  fn root(&self, _node: DomNode) -> DomNode {
    self.root_node()
  }

  fn kind(&self, node: DomNode) -> NodeKind {
    match node {
      DomNode::Text(_) => NodeKind::Text,
      DomNode::Namespace { .. } => NodeKind::Namespace,
      DomNode::Tree(id) => match self.doc.node_type(id) {
        NodeType::Document => NodeKind::Root,
        NodeType::Element => NodeKind::Element,
        NodeType::Attribute => NodeKind::Attribute,
        NodeType::Comment => NodeKind::Comment,
        NodeType::ProcessingInstruction => NodeKind::ProcessingInstruction,
        NodeType::Text | NodeType::CdataSection => NodeKind::Text,
        // A document type or fragment is not an XPath node; nothing reaches here in a tree.
        NodeType::DocumentType | NodeType::DocumentFragment => NodeKind::Root,
      },
    }
  }

  fn parent(&self, node: DomNode) -> Option<DomNode> {
    match node {
      DomNode::Tree(id) if self.doc.node_type(id) == NodeType::Attribute => {
        self.doc.owner_element(id).map(DomNode::Tree)
      }
      DomNode::Tree(id) => self.doc.parent(id).map(DomNode::Tree),
      DomNode::Text(first) => self.doc.parent(first).map(DomNode::Tree),
      DomNode::Namespace { element, .. } => Some(DomNode::Tree(element)),
    }
  }

  fn children(&self, node: DomNode) -> Vec<DomNode> {
    self.children_of(node)
  }

  fn attributes(&self, node: DomNode) -> Vec<DomNode> {
    self.attributes_of(node)
  }

  fn namespaces(&self, node: DomNode) -> Vec<DomNode> {
    self.namespaces_of(node)
  }

  fn expanded_name(&self, node: DomNode) -> Option<ExpandedName> {
    match node {
      DomNode::Tree(id) => match self.doc.node_type(id) {
        NodeType::Element | NodeType::Attribute => Some(ExpandedName {
          namespace: self.doc.namespace_uri(id).map(ToOwned::to_owned),
          local: self.doc.local_name(id).unwrap_or_default().to_owned(),
        }),
        NodeType::ProcessingInstruction => Some(ExpandedName { namespace: None, local: self.doc.node_name(id) }),
        _ => None,
      },
      DomNode::Namespace { prefix, .. } => Some(ExpandedName {
        namespace: None,
        local: prefix.map(|name| self.doc.pool().resolve(name).to_owned()).unwrap_or_default(),
      }),
      DomNode::Text(_) => None,
    }
  }

  fn qualified_name(&self, node: DomNode) -> Option<String> {
    match node {
      // The DOM's node name is already the lexical form, prefix included.
      DomNode::Tree(id) => match self.doc.node_type(id) {
        NodeType::Element | NodeType::Attribute | NodeType::ProcessingInstruction => Some(self.doc.node_name(id)),
        _ => None,
      },
      // A namespace node's name is the prefix it binds, and nothing for the default namespace.
      DomNode::Namespace { prefix, .. } => {
        Some(prefix.map_or_else(String::new, |name| self.doc.pool().resolve(name).to_owned()))
      }
      DomNode::Text(_) => None,
    }
  }

  fn element_by_id(&self, id: &str) -> Option<DomNode> {
    self.doc.get_element_by_id(id).map(DomNode::Tree)
  }

  fn string_value(&self, node: DomNode) -> String {
    match node {
      DomNode::Tree(id) => match self.doc.node_type(id) {
        NodeType::Document | NodeType::Element => self.doc.text_content(id),
        NodeType::Attribute | NodeType::Comment | NodeType::ProcessingInstruction => {
          self.doc.node_value(id).unwrap_or_default().to_owned()
        }
        _ => String::new(),
      },
      DomNode::Text(first) => {
        let mut value = String::new();
        let mut current = Some(first);
        while let Some(node) = current {
          if self.is_text_like(node) {
            value.push_str(self.doc.node_value(node).unwrap_or_default());
            current = self.doc.next_sibling(node);
          } else {
            break;
          }
        }
        value
      }
      DomNode::Namespace { element, prefix } => self.namespace_uri(element, prefix).unwrap_or_default(),
    }
  }

  fn document_order(&self, a: DomNode, b: DomNode) -> Ordering {
    let index = |node: &DomNode| self.order.get(node).copied().unwrap_or(usize::MAX);
    index(&a).cmp(&index(&b))
  }
}
