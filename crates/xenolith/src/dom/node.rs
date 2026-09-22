//! The node model: identifier, type, and the payload held by each type.
//!
//! All nodes reside within the [`Document`](crate::dom::Document)'s [arena](crate::dom#arena) and are identified by a
//! [`NodeId`]. Nodes internally store their type and data, which callers access via accessors on the
//! [`Document`](crate::dom::Document) or [`NodeRef`](crate::dom::NodeRef).

#[cfg(test)]
mod test;

use std::num::NonZeroU32;

use crate::name::{NameId, QName};

/// A handle pointing to a specific node within a [`Document`](crate::dom::Document).
///
/// It is a lightweight, `Copy`-able index into the [arena](crate::dom#arena) of a specific document. Its value remains
/// stable throughout the node's lifetime. Since the handle itself does not hold a reference to the document, the
/// document is also required to read the node via the handle. While this is a trade-off inherent to the arena approach,
/// it enables low-cost storage and comparison of nodes.
///
/// A handle refers only to a node within the document that created it. Because it contains information about the
/// originating document, if the system detects that the handle has been passed to a different document, it will return
/// [`WRONG_DOCUMENT_ERR`](crate::dom::ExceptionCode::WRONG_DOCUMENT_ERR), or trigger a panic in the case of read
/// accessors, rather than erroneously reading an unrelated node.
///
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct NodeId {
  /// The document to which the node belongs. This is checked on every access via this handle. By using `NonZeroU32`,
  /// Rust can represent the `None` state of an `Option<NodeId>` using a value of 0 for this field, making the size of
  /// `Option<NodeId>` the same as that of `NodeId`.
  document: NonZeroU32,
  /// The index of where that node is located within the document's arena.
  index: u32,
}

impl NodeId {
  /// A handle to the node at `index` of `document`.
  pub(crate) const fn new(document: NonZeroU32, index: u32) -> Self {
    Self { document, index }
  }

  /// The document this handle refers to a node of.
  pub(crate) const fn document(self) -> NonZeroU32 {
    self.document
  }

  /// The arena index.
  pub(crate) const fn index(self) -> usize {
    self.index as usize
  }
}

/// Node types based on the `nodeType` codes defined in the W3C DOM specification.
///
/// The DOM tree holds only these kinds. The W3C DOM specification defines three more that this implementation does not
/// build. `ENTITY_REFERENCE_NODE` (5) is not created because the parser expands entity references. Entity and notation
/// declarations stay in the DTD, not in the tree as an `ENTITY_NODE` (6) or `NOTATION_NODE` (12).
///
/// <https://www.w3.org/TR/2003/WD-DOM-Level-3-Core-20030226/DOM3-Core.html#core-ID-1950641247>
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
#[allow(non_camel_case_types)] // the W3C DOM specification's nodeType constant names: ELEMENT_NODE and the rest
pub enum NodeType {
  /// An element: `<a/>`.
  ELEMENT_NODE = 1,
  /// An attribute: `name="value"`.
  ATTRIBUTE_NODE = 2,
  /// Character data: text.
  TEXT_NODE = 3,
  /// A CDATA section: `<![CDATA[ ... ]]>`.
  CDATA_SECTION_NODE = 4,
  /// A processing instruction: `<?target data?>`.
  PROCESSING_INSTRUCTION_NODE = 7,
  /// A comment: `<!-- ... -->`.
  COMMENT_NODE = 8,
  /// The document: the root of the tree.
  DOCUMENT_NODE = 9,
  /// The document type: `<!DOCTYPE ...>`.
  DOCUMENT_TYPE_NODE = 10,
  /// A document fragment: a lightweight, parentless container.
  DOCUMENT_FRAGMENT_NODE = 11,
}

impl NodeType {
  /// The DOM `nodeType` code.
  #[must_use]
  pub const fn code(self) -> u16 {
    self as u16
  }
}

/// Attribute node data: name, value, owner element, and whether it is of type ID.
#[derive(Clone, Debug)]
pub(crate) struct AttrData {
  pub(crate) name: QName,
  pub(crate) value: String,
  /// The element this attribute belongs to, or `None` while it is detached.
  pub(crate) owner: Option<NodeId>,
  /// Whether the attribute is of type `ID`, so [`get_element_by_id`](crate::dom::Document::get_element_by_id)
  /// considers it.
  pub(crate) is_id: bool,
}

/// Element data: its name, its attribute nodes in document order, and, if recorded during tree construction, its
/// effective base URI (XML Base).
#[derive(Clone, Debug)]
pub(crate) struct ElementData {
  pub(crate) name: QName,
  pub(crate) attributes: Vec<NodeId>,
  /// The fully resolved base URI in effect at this element, interned. It is `None` for a
  /// hand-built element, or when no base is known.
  pub(crate) base: Option<NameId>,
}

/// The kind-specific payload of a node.
#[derive(Clone, Debug)]
pub(crate) enum NodeData {
  /// The document node.
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
  /// The node type of this payload.
  pub(crate) const fn node_type(&self) -> NodeType {
    match self {
      NodeData::Document => NodeType::DOCUMENT_NODE,
      NodeData::DocumentType { .. } => NodeType::DOCUMENT_TYPE_NODE,
      NodeData::DocumentFragment => NodeType::DOCUMENT_FRAGMENT_NODE,
      NodeData::Element(_) => NodeType::ELEMENT_NODE,
      NodeData::Attribute(_) => NodeType::ATTRIBUTE_NODE,
      NodeData::Text(_) => NodeType::TEXT_NODE,
      NodeData::CdataSection(_) => NodeType::CDATA_SECTION_NODE,
      NodeData::Comment(_) => NodeType::COMMENT_NODE,
      NodeData::ProcessingInstruction { .. } => NodeType::PROCESSING_INSTRUCTION_NODE,
    }
  }

  /// Whether this kind of node may contain child nodes.
  pub(crate) const fn is_container(&self) -> bool {
    matches!(self, NodeData::Document | NodeData::DocumentFragment | NodeData::Element(_))
  }

  /// Whether this kind of node may itself be a child in the tree. The document and an attribute
  /// may not.
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
