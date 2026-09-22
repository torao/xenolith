//! [`NodeList`] and [`NamedNodeMap`] act as views of the tree.
//!
//! These collections do not store a snapshot of the state; instead, they hold a reference (borrow) to the [`Document`]
//! and the definition of what they are meant to collect. When `length` or `item` is accessed, they read the current
//! state of the tree to return the result, ensuring the view always reflects the latest state.
//!
//! This is an implementation of the *live* collections defined in the W3C DOM specification, adapted for the
//! [arena](crate::dom#arena). However, there are differences arising from Rust's borrow checker. In the W3C DOM, a
//! [`NodeList`] persists even while the document is modified via other channels, meaning its contents can change
//! during iteration. In contrast, because the Rust view borrows the document, modification operations require `&mut`,
//! ensuring that simultaneous modifications via other channels cannot occur. In other words, the borrow checker
//! guarantees that the held list will not change unexpectedly.

use crate::dom::Document;
use crate::dom::node::{NodeData, NodeId, NodeType};

/// What the [`NodeList`] collects. Because it is re-evaluated each time it is read, the list's view is always kept up
/// to date.
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

/// An ordered list of nodes computed on demand. This is an [arena](crate::dom#arena) feature corresponding to the W3C
/// `NodeList`.
///
/// It holds no snapshot: each read looks at the tree as it is then, so a list obtained after a change includes that
/// change. Unlike a W3C `NodeList`, it cannot be held *across* a change. It borrows the document, and a change needs
/// `&mut`, so you re-obtain it rather than watch one update under you.
///
/// A W3C-compliant `NodeList` is a "live collection" that reflects changes made to the original document during its
/// use. However, because Rust's borrow checker guarantees that the original document remains read-only (i.e.,
/// non-`&mut`) for the duration of the list's scope, the document cannot be modified while the list is in use. If
/// modifications are required, you must exit the list's scope and re-acquire the list after the update.
///
/// # Examples
///
/// ```
/// use xenolith::dom::Document;
///
/// let mut doc = Document::new();
/// let root = doc.create_element("ul")?;
/// doc.append_child(doc.document_node(), root)?;
/// assert_eq!(doc.get_elements_by_tag_name(doc.document_node(), "li").length(), 0);
///
/// let li = doc.create_element("li")?;
/// doc.append_child(root, li)?;
/// // A list obtained now sees the new child.
/// let items = doc.get_elements_by_tag_name(doc.document_node(), "li");
/// assert_eq!(items.length(), 1);
/// assert_eq!(items.item(0), Some(li));
/// # Ok::<(), xenolith::dom::DomException>(())
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

  /// The nodes of the list (in order of appearance), calculated based on the current tree.
  pub fn iter(&self) -> impl Iterator<Item = NodeId> + 'a {
    let doc = self.doc;
    let members: Vec<NodeId> = match &self.query {
      Query::Children(parent) => doc.children(*parent).collect(),
      Query::ByTagName { root, name } => {
        let name = name.clone();
        doc
          .descendants(*root)
          .filter(|&node| doc.node_type(node) == NodeType::ELEMENT_NODE && name.matches(doc, node))
          .collect()
      }
    };
    members.into_iter()
  }

  /// The number of nodes in the current list.
  #[must_use]
  pub fn length(&self) -> usize {
    self.iter().count()
  }

  /// The node at `index`. Returns `None` if the index is out of bounds.
  #[must_use]
  pub fn item(&self, index: usize) -> Option<NodeId> {
    self.iter().nth(index)
  }

  /// Whether the list is currently empty.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.iter().next().is_none()
  }
}

/// A map of an element's attribute nodes. It conforms to the W3C `NamedNodeMap` and is computed on demand.
///
/// Like an attribute list, the order is preserved, and access is possible via index or name. Since it is computed upon
/// each read, it always reflects the current state; however, there are caveats regarding "borrowing" similar to those
/// for [`NodeList`].
#[derive(Clone, Debug)]
pub struct NamedNodeMap<'a> {
  doc: &'a Document,
  element: NodeId,
}

impl<'a> NamedNodeMap<'a> {
  pub(crate) fn new(doc: &'a Document, element: NodeId) -> Self {
    Self { doc, element }
  }

  /// Attribute nodes (in order of appearance).
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

  /// The attribute node at the specified index, in document order.
  #[must_use]
  pub fn item(&self, index: usize) -> Option<NodeId> {
    self.iter().nth(index)
  }

  /// The attribute node with the specified qualified name (if it exists).
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
