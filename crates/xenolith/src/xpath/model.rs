//! The XPath 1.0 data model (§5) implemented over a [`Document`].
//!
//! XPath evaluates a seven-kind tree that differs from the DOM in three ways: each contiguous run of text and CDATA
//! sections forms one text node; each element has namespace nodes for its in-scope bindings; and document order also
//! covers attribute and namespace nodes.
//!
//! [`Model`] exposes this view as a trait, enabling evaluators to operate independently of the underlying tree
//! representation. [`DomModel`] implements this trait for a borrowed [`Document`]. Without modifying the original
//! document, this implementation handles the merging of adjacent text, the on-demand generation of namespace nodes,
//! and the calculation of document order.

#[cfg(test)]
mod test;

use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::rc::Rc;

use crate::dom::{Document, NodeId, NodeType};
use crate::name::{NameId, XML_NS_URI, XMLNS_NS_URI};

/// One of the seven node kinds defined by the XPath data model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
  /// The document node, which is the root of the XPath tree. Its children include the document element and any
  /// top-level comments or processing instructions; document type nodes are not part of the XPath data model.
  Root,
  /// An element.
  Element,
  /// An attribute node. It belongs to an element but is not its child; use the attribute axis to select it.
  Attribute,
  /// A namespace node representing one prefix binding in scope on an element, including inherited bindings.
  Namespace,
  /// A text node representing a maximal adjacent run of text and CDATA sections.
  Text,
  /// A comment.
  Comment,
  /// A processing instruction.
  ProcessingInstruction,
}

/// An expanded name consisting of an optional namespace URI and a local name.
///
/// Elements and attributes use their namespace URI and local name. A processing instruction uses its target as the
/// local name with no namespace. A namespace node uses its bound prefix as the local name; the default namespace has
/// an empty local name. Root, text, and comment nodes have no expanded name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExpandedName {
  /// The namespace URI, or `None` when the name is in no namespace.
  pub namespace: Option<String>,
  /// The local part.
  pub local: String,
}

/// The XPath 1.0 data-model interface for a tree.
///
/// A model exposes each node's kind, parent, children, attributes, namespace nodes, names, and string-value. These
/// operations provide the information needed to implement XPath axes and node tests. The model also defines a total
/// document order. The associated [`Node`](Model::Node) handle is a cheap identity value; compare nodes by calling
/// [`document_order`](Model::document_order), because their order belongs to the tree rather than to the handle.
pub(crate) trait Model {
  /// A copyable node identity. Use [`document_order`](Model::document_order) to compare tree order.
  type Node: Copy + Eq + Hash + Debug;

  /// The root of the tree the node belongs to.
  fn root(&self, node: Self::Node) -> Self::Node;

  /// The kind of a node.
  fn kind(&self, node: Self::Node) -> NodeKind;

  /// Returns the parent, or `None` for the root. An attribute or namespace node has its owning element as its parent,
  /// but is not included among that element's children.
  fn parent(&self, node: Self::Node) -> Option<Self::Node>;

  /// Returns child nodes in document order. The result may contain elements, text, comments, and processing
  /// instructions, but never attributes or namespace nodes.
  fn children(&self, node: Self::Node) -> Vec<Self::Node>;

  /// Returns an element's attribute nodes, or an empty list for other node kinds. Namespace declarations are excluded.
  fn attributes(&self, node: Self::Node) -> Vec<Self::Node>;

  /// Returns the namespace nodes in scope on an element, including the implicit `xml` binding, or an empty list for
  /// other node kinds.
  fn namespaces(&self, node: Self::Node) -> Vec<Self::Node>;

  /// Returns the node's expanded name, or `None` for node kinds without one.
  fn expanded_name(&self, node: Self::Node) -> Option<ExpandedName>;

  /// Returns the node's lexical qualified name, including its prefix, as used by XPath's `name()` function.
  fn qualified_name(&self, node: Self::Node) -> Option<String>;

  /// Returns the element associated with an ID value, as selected by XPath's `id()` function.
  fn element_by_id(&self, id: &str) -> Option<Self::Node>;

  /// Returns the node's string-value as defined by XPath 1.0 §5.
  fn string_value(&self, node: Self::Node) -> String;

  /// Compares two nodes by their position in document order. For nodes in the model, only a node compared with itself
  /// returns `Equal`; [`make_node_set`] relies on this behavior to remove duplicate nodes.
  fn document_order(&self, a: Self::Node, b: Self::Node) -> Ordering;
}

/// Sorts `nodes` into document order and removes duplicate nodes to produce a node-set.
///
/// Sorting places occurrences of the same node next to each other because [`Model::document_order`] returns `Equal`
/// only when both handles identify the same node. The final `dedup` call removes those adjacent duplicates.
pub(crate) fn make_node_set<M: Model>(model: &M, nodes: &mut Vec<M::Node>) {
  nodes.sort_by(|a, b| model.document_order(*a, *b));
  nodes.dedup();
}

/// A node handle in the XPath data-model view of a document.
///
/// Most handles refer directly to a DOM node ([`Tree`](DomNode::Tree)). A text handle refers to the first DOM node in
/// a contiguous text/CDATA run ([`Text`](DomNode::Text)). A namespace handle is synthesized from an element and a
/// prefix binding ([`Namespace`](DomNode::Namespace)).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum DomNode {
  /// A DOM node: the document (the XPath root), an element, an attribute, a comment or a PI.
  Tree(NodeId),
  /// A text node, identified by the first DOM text or CDATA node of its run.
  Text(NodeId),
  /// A namespace node: a prefix bound on an element.
  Namespace {
    /// The element the namespace node belongs to.
    element: NodeId,
    /// The prefix it binds, or `None` for the default namespace.
    prefix: Option<NameId>,
  },
}

/// An XPath data model backed by a borrowed [`Document`].
#[derive(Debug)]
pub(crate) struct DomModel<'d> {
  doc: &'d Document,
  /// Document-order index for every node, initialized for the whole tree on the first order comparison.
  order: OnceCell<HashMap<DomNode, usize>>,
  /// The element each ID identifies, initialized for the whole tree on the first ID lookup.
  ids: OnceCell<HashMap<String, NodeId>>,
}

impl<'d> DomModel<'d> {
  /// Creates a model over `doc`. Document-order indices are computed lazily when first needed.
  pub(crate) const fn new(doc: &'d Document) -> Self {
    Self { doc, order: OnceCell::new(), ids: OnceCell::new() }
  }

  /// The element each ID in the tree identifies.
  ///
  /// The first ID lookup traverses the document once and builds a map, so subsequent lookups do not traverse the tree
  /// again. The map lives only for this model's evaluation and does not track later document changes. If multiple
  /// elements have the same ID, the map keeps the first one in document order, matching
  /// [`Document::get_element_by_id`].
  fn identified_elements(&self) -> HashMap<String, NodeId> {
    let mut ids = HashMap::new();
    for node in self.doc.descendants(self.doc.document_node()) {
      if self.doc.node_type(node) != NodeType::ELEMENT_NODE {
        continue;
      }
      for attribute in self.doc.attributes(node).iter().filter(|&attribute| self.doc.is_id_attribute(attribute)) {
        ids.entry(self.doc.node_value(attribute).unwrap_or_default().to_owned()).or_insert(node);
      }
    }
    ids
  }

  /// Returns the document node, which is the root of the XPath tree.
  fn root_node(&self) -> DomNode {
    DomNode::Tree(self.doc.document_node())
  }

  /// Converts a DOM node to its XPath node. Text and CDATA nodes map to the first node in their contiguous run.
  pub(crate) fn node(&self, id: NodeId) -> DomNode {
    if self.is_text_like(id) { DomNode::Text(self.run_start(id)) } else { DomNode::Tree(id) }
  }

  /// Assigns document-order indices to each node, its namespace nodes, attributes, and child nodes in the tree.
  ///
  /// DOM trees can have arbitrary depth. Recursive processing could lead to a stack overflow, so this process employs
  /// an iterative traversal.
  ///
  /// Computing in-scope prefixes by tracing the ancestors of every element would take time proportional to the product
  /// of the number of nodes and the tree depth; therefore, each pending node maintains the prefix bindings inherited
  /// from its parent. For a given element, `scope_of` applies only the declarations specific to that element; elements
  /// without declarations share the parent's list.
  fn numbered_nodes(&self) -> HashMap<DomNode, usize> {
    let mut order = HashMap::new();
    let mut pending = vec![(self.root_node(), Rc::new(vec![Some(NameId::XML)]))];
    while let Some((node, inherited)) = pending.pop() {
      order.insert(node, order.len());
      let scope = match node {
        DomNode::Tree(element) if self.doc.node_type(element) == NodeType::ELEMENT_NODE => {
          let scope = self.scope_of(element, &inherited);
          for &prefix in scope.iter() {
            order.insert(DomNode::Namespace { element, prefix }, order.len());
          }
          scope
        }
        _ => inherited,
      };
      for attribute in self.attributes(node) {
        order.insert(attribute, order.len());
      }
      pending.extend(self.children(node).into_iter().rev().map(|child| (child, Rc::clone(&scope))));
    }
    order
  }

  /// Returns the prefixes in scope on `element`: its parent's bindings updated with the element's declarations, in
  /// the stable order used by [`in_scope_prefixes`](Self::in_scope_prefixes).
  fn scope_of(&self, element: NodeId, inherited: &Rc<Vec<Option<NameId>>>) -> Rc<Vec<Option<NameId>>> {
    let mut declarations = self
      .doc
      .attributes(element)
      .iter()
      .filter_map(|attribute| {
        let prefix = self.declared_prefix(attribute)?;
        // An empty value undeclares the prefix, so it shadows but adds no node.
        Some((prefix, !self.doc.node_value(attribute).unwrap_or_default().is_empty()))
      })
      .peekable();
    if declarations.peek().is_none() {
      return Rc::clone(inherited);
    }
    let mut scope = inherited.as_ref().clone();
    for (prefix, bound) in declarations {
      scope.retain(|&inherited| inherited != prefix);
      if bound {
        scope.push(prefix);
      }
    }
    if !scope.contains(&Some(NameId::XML)) {
      scope.push(Some(NameId::XML));
    }
    scope.sort_by_key(|&prefix| self.prefix_key(prefix));
    Rc::new(scope)
  }

  fn is_text_like(&self, id: NodeId) -> bool {
    matches!(self.doc.node_type(id), NodeType::TEXT_NODE | NodeType::CDATA_SECTION_NODE)
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

  /// The prefixes in scope on an element, in a stable order (default first, then by name), each with a non-empty
  /// binding, plus the implicit `xml`.
  fn in_scope_prefixes(&self, element: NodeId) -> Vec<Option<NameId>> {
    let mut seen: Vec<Option<NameId>> = Vec::new();
    let mut result: Vec<Option<NameId>> = Vec::new();
    let mut current = Some(element);
    while let Some(node) = current {
      if self.doc.node_type(node) == NodeType::ELEMENT_NODE {
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
      if self.doc.node_type(node) == NodeType::ELEMENT_NODE {
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

  /// Where a node sits in document order; a node outside the tree sorts after every node in it.
  fn position(&self, node: DomNode) -> usize {
    self.order.get_or_init(|| self.numbered_nodes()).get(&node).copied().unwrap_or(usize::MAX)
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
        NodeType::DOCUMENT_NODE => NodeKind::Root,
        NodeType::ELEMENT_NODE => NodeKind::Element,
        NodeType::ATTRIBUTE_NODE => NodeKind::Attribute,
        NodeType::COMMENT_NODE => NodeKind::Comment,
        NodeType::PROCESSING_INSTRUCTION_NODE => NodeKind::ProcessingInstruction,
        NodeType::TEXT_NODE | NodeType::CDATA_SECTION_NODE => NodeKind::Text,
        // A document type or fragment is not an XPath node; nothing reaches here in a tree.
        NodeType::DOCUMENT_TYPE_NODE | NodeType::DOCUMENT_FRAGMENT_NODE => NodeKind::Root,
      },
    }
  }

  fn parent(&self, node: DomNode) -> Option<DomNode> {
    match node {
      DomNode::Tree(id) if id == self.doc.document_node() => None,
      DomNode::Tree(id) if self.doc.node_type(id) == NodeType::ATTRIBUTE_NODE => {
        self.doc.owner_element(id).map(DomNode::Tree)
      }
      DomNode::Tree(id) | DomNode::Text(id) => self.doc.parent(id).map(DomNode::Tree),
      DomNode::Namespace { element, .. } => Some(DomNode::Tree(element)),
    }
  }

  fn children(&self, node: DomNode) -> Vec<DomNode> {
    let DomNode::Tree(id) = node else { return Vec::new() };
    if !matches!(
      self.doc.node_type(id),
      NodeType::DOCUMENT_NODE | NodeType::ELEMENT_NODE | NodeType::DOCUMENT_FRAGMENT_NODE
    ) {
      return Vec::new();
    }
    let mut children = Vec::new();
    let mut child = self.doc.first_child(id);
    while let Some(current) = child {
      match self.doc.node_type(current) {
        NodeType::TEXT_NODE | NodeType::CDATA_SECTION_NODE => {
          children.push(DomNode::Text(current));
          child = self.after_run(current);
        }
        NodeType::ELEMENT_NODE | NodeType::COMMENT_NODE | NodeType::PROCESSING_INSTRUCTION_NODE => {
          children.push(DomNode::Tree(current));
          child = self.doc.next_sibling(current);
        }
        // A document type or anything else is not part of the XPath tree.
        _ => child = self.doc.next_sibling(current),
      }
    }
    children
  }

  fn attributes(&self, node: DomNode) -> Vec<DomNode> {
    match node {
      DomNode::Tree(id) if self.doc.node_type(id) == NodeType::ELEMENT_NODE => self
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

  fn namespaces(&self, node: DomNode) -> Vec<DomNode> {
    match node {
      DomNode::Tree(id) if self.doc.node_type(id) == NodeType::ELEMENT_NODE => {
        self.in_scope_prefixes(id).into_iter().map(|prefix| DomNode::Namespace { element: id, prefix }).collect()
      }
      _ => Vec::new(),
    }
  }

  fn expanded_name(&self, node: DomNode) -> Option<ExpandedName> {
    match node {
      DomNode::Tree(id) => match self.doc.node_type(id) {
        NodeType::ELEMENT_NODE | NodeType::ATTRIBUTE_NODE => Some(ExpandedName {
          namespace: self.doc.namespace_uri(id).map(ToOwned::to_owned),
          local: self.doc.local_name(id).unwrap_or_default().to_owned(),
        }),
        NodeType::PROCESSING_INSTRUCTION_NODE => Some(ExpandedName { namespace: None, local: self.doc.node_name(id) }),
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
        NodeType::ELEMENT_NODE | NodeType::ATTRIBUTE_NODE | NodeType::PROCESSING_INSTRUCTION_NODE => {
          Some(self.doc.node_name(id))
        }
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
    self.ids.get_or_init(|| self.identified_elements()).get(id).copied().map(DomNode::Tree)
  }

  fn string_value(&self, node: DomNode) -> String {
    match node {
      DomNode::Tree(id) => match self.doc.node_type(id) {
        NodeType::DOCUMENT_NODE | NodeType::ELEMENT_NODE | NodeType::DOCUMENT_FRAGMENT_NODE => {
          self.doc.text_content(id)
        }
        NodeType::ATTRIBUTE_NODE | NodeType::COMMENT_NODE | NodeType::PROCESSING_INSTRUCTION_NODE => {
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
    // Every node in the tree has a position of its own, so a tie is between nodes outside it, which are put in the
    // order of the handles themselves (the order they were created in) to keep the order total.
    self.position(a).cmp(&self.position(b)).then_with(|| a.cmp(&b))
  }
}
