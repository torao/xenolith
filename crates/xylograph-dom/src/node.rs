//! The node model: identity, kinds, and the payload each kind carries.
//!
//! Every node lives in a [`Document`](crate::Document)'s arena and is named by a [`NodeId`]. The
//! kind and its data are held internally in [`NodeData`]; a caller reads them through the
//! accessors on [`Document`](crate::Document) or a [`NodeRef`](crate::NodeRef), never this
//! structure directly.

use xylograph_core::name::{NameId, QName};

/// A handle to one node within a [`Document`](crate::Document).
///
/// It is an index into that document's arena — small, `Copy`, and stable for the life of the
/// node. It carries no reference to the document, so reading through it needs the document too;
/// that is the arena trade-off, and what keeps a node cheap to hold and to compare.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
  /// The arena index.
  pub(crate) const fn index(self) -> usize {
    self.0 as usize
  }
}

/// What kind of node this is, with the DOM `nodeType` code it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum NodeType {
  /// An element: `<a/>`.
  Element = 1,
  /// An attribute: `name="value"`.
  Attribute = 2,
  /// Character data: text.
  Text = 3,
  /// A CDATA section: `<![CDATA[ ... ]]>`.
  CdataSection = 4,
  /// A processing instruction: `<?target data?>`.
  ProcessingInstruction = 7,
  /// A comment: `<!-- ... -->`.
  Comment = 8,
  /// The document: the root of the tree.
  Document = 9,
  /// The document type: `<!DOCTYPE ...>`.
  DocumentType = 10,
  /// A document fragment: a lightweight, parentless container.
  DocumentFragment = 11,
}

impl NodeType {
  /// The DOM `nodeType` code.
  #[must_use]
  pub const fn code(self) -> u16 {
    self as u16
  }
}

/// An attribute node's data: its name, value, owning element, and whether it is of type ID.
#[derive(Clone, Debug)]
pub(crate) struct AttrData {
  pub(crate) name: QName,
  pub(crate) value: String,
  /// The element this attribute belongs to, or `None` while it is detached.
  pub(crate) owner: Option<NodeId>,
  /// Whether the attribute is of type `ID`, so [`get_element_by_id`](crate::Document::get_element_by_id)
  /// considers it.
  pub(crate) is_id: bool,
}

/// An element's own data: its name, its attribute nodes in document order, and its effective
/// base URI (XML Base) if one was recorded when the tree was built.
#[derive(Clone, Debug)]
pub(crate) struct ElementData {
  pub(crate) name: QName,
  pub(crate) attributes: Vec<NodeId>,
  /// The fully resolved base URI in effect at this element, interned; `None` for a
  /// hand-built element or when no base is known.
  pub(crate) base: Option<NameId>,
}

/// The kind-specific payload of a node.
#[derive(Clone, Debug)]
pub(crate) enum NodeData {
  /// The document root.
  Document,
  /// A `<!DOCTYPE>`, with its declared root name and external identifiers.
  DocumentType { name: NameId, public_id: Option<String>, system_id: Option<String> },
  /// A document fragment.
  DocumentFragment,
  /// An element.
  Element(ElementData),
  /// An attribute. It is a node, but not a child: an element reaches it through its attribute
  /// list, not `first_child`.
  Attribute(AttrData),
  /// Character data.
  Text(String),
  /// A CDATA section's character data.
  CdataSection(String),
  /// A comment's text.
  Comment(String),
  /// A processing instruction.
  ProcessingInstruction { target: NameId, data: String },
}

impl NodeData {
  /// The node type this payload is.
  pub(crate) const fn node_type(&self) -> NodeType {
    match self {
      NodeData::Document => NodeType::Document,
      NodeData::DocumentType { .. } => NodeType::DocumentType,
      NodeData::DocumentFragment => NodeType::DocumentFragment,
      NodeData::Element(_) => NodeType::Element,
      NodeData::Attribute(_) => NodeType::Attribute,
      NodeData::Text(_) => NodeType::Text,
      NodeData::CdataSection(_) => NodeType::CdataSection,
      NodeData::Comment(_) => NodeType::Comment,
      NodeData::ProcessingInstruction { .. } => NodeType::ProcessingInstruction,
    }
  }

  /// Whether this kind of node may contain child nodes.
  pub(crate) const fn is_container(&self) -> bool {
    matches!(self, NodeData::Document | NodeData::DocumentFragment | NodeData::Element(_))
  }

  /// Whether this kind of node may itself be a child in the tree. Attributes and the document
  /// are the ones that may not.
  pub(crate) const fn can_be_child(&self) -> bool {
    !matches!(self, NodeData::Document | NodeData::Attribute(_))
  }
}

/// A node's place in the tree, together with its payload.
#[derive(Clone, Debug)]
pub(crate) struct NodeSlot {
  pub(crate) parent: Option<NodeId>,
  pub(crate) first_child: Option<NodeId>,
  pub(crate) last_child: Option<NodeId>,
  pub(crate) previous_sibling: Option<NodeId>,
  pub(crate) next_sibling: Option<NodeId>,
  pub(crate) data: NodeData,
}

impl NodeSlot {
  pub(crate) const fn new(data: NodeData) -> Self {
    Self { parent: None, first_child: None, last_child: None, previous_sibling: None, next_sibling: None, data }
  }
}
