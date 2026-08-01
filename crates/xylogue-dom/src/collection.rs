//! Views over the tree: [`NodeList`] and [`NamedNodeMap`].
//!
//! Each holds no snapshot — only a borrow of the [`Document`] and a description of what to
//! gather — and answers `length` and `item` by reading the tree as it is at that moment. So a
//! view is always current: obtain one, and it reflects the tree right then.
//!
//! This is the arena's version of the DOM's *live* collection, with one difference the borrow
//! checker makes. A W3C `NodeList` is held while the document is mutated through another path
//! and updates under you; here the view borrows the document, and mutation needs `&mut`, so the
//! two cannot overlap. You do not watch a held list change — you re-obtain it after the change,
//! and what you get is current. The liveness is in *how* it is computed, not in holding one
//! across a mutation.

use crate::Document;
use crate::node::{NodeData, NodeId, NodeType};

/// What a [`NodeList`] gathers, evaluated afresh on each read so the list is always current.
#[derive(Clone, Debug)]
pub(crate) enum Query {
  /// The children of a node.
  Children(NodeId),
  /// The descendant elements matching a name filter, in document order.
  ByTagName { root: NodeId, name: NameFilter },
}

impl Query {
  /// A `getElementsByTagName` query: a qualified-name match, or every element for `"*"`.
  pub(crate) fn by_tag_name(root: NodeId, name: &str) -> Self {
    let name = if name == "*" { NameFilter::Any } else { NameFilter::QualifiedName(name.to_owned()) };
    Query::ByTagName { root, name }
  }

  /// A `getElementsByTagNameNS` query: a namespace and local-name match, each with a `"*"`
  /// wildcard.
  pub(crate) fn by_tag_name_ns(root: NodeId, namespace: Option<&str>, local: &str) -> Self {
    let name = NameFilter::Expanded {
      namespace: Wildcard::of(namespace),
      local: if local == "*" { None } else { Some(local.to_owned()) },
    };
    Query::ByTagName { root, name }
  }
}

/// How a [`Query`] decides whether an element matches.
#[derive(Clone, Debug)]
pub(crate) enum NameFilter {
  /// Every element.
  Any,
  /// The element's qualified name equals this.
  QualifiedName(String),
  /// The element's namespace and local name match.
  Expanded { namespace: Wildcard, local: Option<String> },
}

/// A namespace match: any namespace, or a specific one (where `None` means "no namespace").
#[derive(Clone, Debug)]
pub(crate) enum Wildcard {
  Any,
  Exact(Option<String>),
}

impl Wildcard {
  fn of(namespace: Option<&str>) -> Self {
    match namespace {
      Some("*") => Wildcard::Any,
      other => Wildcard::Exact(other.map(ToOwned::to_owned)),
    }
  }
}

impl NameFilter {
  /// Whether an element node passes this filter.
  fn matches(&self, doc: &Document, element: NodeId) -> bool {
    match self {
      NameFilter::Any => true,
      NameFilter::QualifiedName(name) => doc.node_name(element) == *name,
      NameFilter::Expanded { namespace, local } => {
        let namespace_ok = match namespace {
          Wildcard::Any => true,
          Wildcard::Exact(expected) => doc.namespace_uri(element) == expected.as_deref(),
        };
        let local_ok = local.as_ref().is_none_or(|l| doc.local_name(element) == Some(l.as_str()));
        namespace_ok && local_ok
      }
    }
  }
}

/// An ordered list of nodes, computed on demand: the arena's take on the DOM's `NodeList`.
///
/// It holds no snapshot: each read looks at the tree as it is then, so a list obtained after a
/// change reflects that change. Unlike a W3C `NodeList` it cannot be held *across* a change — it
/// borrows the document, and a change needs `&mut` — so you re-obtain it rather than watch one
/// update under you.
///
/// # Examples
///
/// ```
/// use xylogue_dom::Document;
///
/// let mut doc = Document::new();
/// let root = doc.create_element("ul")?;
/// doc.append_child(doc.root(), root)?;
/// assert_eq!(doc.get_elements_by_tag_name("li").length(), 0);
///
/// let li = doc.create_element("li")?;
/// doc.append_child(root, li)?;
/// // A list obtained now sees the new child.
/// let items = doc.get_elements_by_tag_name("li");
/// assert_eq!(items.length(), 1);
/// assert_eq!(items.item(0), Some(li));
/// # Ok::<(), xylogue_dom::DomException>(())
/// ```
#[derive(Clone, Debug)]
pub struct NodeList<'a> {
  doc: &'a Document,
  query: Query,
}

impl<'a> NodeList<'a> {
  pub(crate) fn new(doc: &'a Document, query: Query) -> Self {
    Self { doc, query }
  }

  /// The nodes of the list, in order, computed against the tree as it is now.
  pub fn iter(&self) -> impl Iterator<Item = NodeId> + 'a {
    let doc = self.doc;
    let members: Vec<NodeId> = match &self.query {
      Query::Children(parent) => doc.children(*parent).collect(),
      Query::ByTagName { root, name } => {
        let name = name.clone();
        doc
          .descendants(*root)
          .filter(|&node| doc.node_type(node) == NodeType::Element && name.matches(doc, node))
          .collect()
      }
    };
    members.into_iter()
  }

  /// The number of nodes in the list right now.
  #[must_use]
  pub fn length(&self) -> usize {
    self.iter().count()
  }

  /// The node at `index`, or `None` if the index is past the end.
  #[must_use]
  pub fn item(&self, index: usize) -> Option<NodeId> {
    self.iter().nth(index)
  }

  /// Whether the list is empty right now.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.iter().next().is_none()
  }
}

/// A map of an element's attribute nodes, computed on demand: the DOM's `NamedNodeMap`.
///
/// Ordered like the attribute list and addressable by index or by name. Computed on each read
/// and so always current, with the same borrow caveat as a [`NodeList`].
#[derive(Clone, Debug)]
pub struct NamedNodeMap<'a> {
  doc: &'a Document,
  element: NodeId,
}

impl<'a> NamedNodeMap<'a> {
  pub(crate) fn new(doc: &'a Document, element: NodeId) -> Self {
    Self { doc, element }
  }

  /// The attribute nodes, in order.
  pub fn iter(&self) -> impl Iterator<Item = NodeId> + 'a {
    let members: Vec<NodeId> = match &self.doc.slot(self.element).data {
      NodeData::Element(data) => data.attributes.clone(),
      _ => Vec::new(),
    };
    members.into_iter()
  }

  /// The number of attributes.
  #[must_use]
  pub fn length(&self) -> usize {
    self.iter().count()
  }

  /// The attribute node at `index`, in document order.
  #[must_use]
  pub fn item(&self, index: usize) -> Option<NodeId> {
    self.iter().nth(index)
  }

  /// The attribute node with a given qualified name, if present.
  #[must_use]
  pub fn get_named_item(&self, qualified_name: &str) -> Option<NodeId> {
    self.doc.get_attribute_node(self.element, qualified_name)
  }

  /// Whether the element has no attributes.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.iter().next().is_none()
  }
}
