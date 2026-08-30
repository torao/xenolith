//! The XPath 1.0 data model for xenolith.
//!
//! XPath does not work on a DOM directly. It sees a tree of seven kinds of node (the *data
//! model*, XPath 1.0 §5) that differs from the DOM in three ways: adjacent character data is one
//! text node, every element carries **namespace nodes** for the declarations in scope, and there
//! is a strict **document order** over all of them, attributes and namespace nodes included.
//!
//! [`Model`] is that view, as a trait, so the evaluator can run over any tree that presents one
//! — the DOM today, a result tree fragment or a streaming source later (see `ROADMAP.md`,
//! decision 3). [`DomModel`] is the implementation over [`xenolith_dom`]: it merges text,
//! synthesizes namespace nodes, and orders every node, without changing the DOM it borrows.
//!
//! # Examples
//!
//! ```
//! use xenolith_dom::build;
//! use xenolith_xdm::{DomModel, Model, NodeKind};
//!
//! let doc = build::parse("<doc><p>one</p><p>two</p></doc>".as_bytes())?;
//! let model = DomModel::new(&doc);
//! let root = model.root_node();
//!
//! // The document's one element child is <doc>; its string-value is all its text.
//! let element = model.children(root)[0];
//! assert_eq!(model.kind(element), NodeKind::Element);
//! assert_eq!(model.string_value(element), "onetwo");
//! # Ok::<(), xenolith_core::Error>(())
//! ```

//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XPath 1.0] — W3C Recommendation 16 November 1999. [§5] is this whole crate: the seven node
//!   kinds, their string-values and expanded names, and document order.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation 8 December 2009, for what the
//!   namespace nodes of an element are.
//!
//! [XPath 1.0]: https://www.w3.org/TR/1999/REC-xpath-19991116/
//! [§5]: https://www.w3.org/TR/1999/REC-xpath-19991116/#data-model
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/

mod dom;

pub use dom::{DocumentId, Documents, DomModel, DomNode};

use std::cmp::Ordering;
use std::fmt::Debug;
use std::hash::Hash;

/// One of the seven kinds of node in the XPath data model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
  /// The root of the tree — the document, whose children are the document element and any
  /// top-level comments and processing instructions.
  Root,
  /// An element.
  Element,
  /// An attribute (not a child of its element, reached by the attribute axis).
  Attribute,
  /// A namespace node: one per namespace declaration in scope on an element.
  Namespace,
  /// A text node: a maximal run of character data.
  Text,
  /// A comment.
  Comment,
  /// A processing instruction.
  ProcessingInstruction,
}

/// The expanded name of a node: a namespace URI (or none) and a local part.
///
/// Elements and attributes have one; a processing instruction's is its target in no namespace; a
/// namespace node's local part is the prefix it binds (empty for the default namespace); the
/// other kinds have none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedName {
  /// The namespace URI, or `None` when the name is in no namespace.
  pub namespace: Option<String>,
  /// The local part.
  pub local: String,
}

/// The XPath 1.0 data-model view of a tree.
///
/// A `Model` presents a tree as the seven node kinds, giving each its kind, its place among its
/// neighbours (the primitives the axes are built from), its expanded name and string-value, and
/// a total document order. A node is named by the associated [`Node`](Model::Node) handle, which
/// is cheap to copy and compare for identity; ordering goes through
/// [`document_order`](Model::document_order) rather than `Ord`, since it is a property of the
/// tree, not of the handle.
pub trait Model {
  /// A handle to a node. Identity only — order is [`document_order`](Model::document_order).
  ///
  /// A handle names a node without borrowing the tree, so it owns nothing with a lifetime. That
  /// is what lets a host language built on this model — XSLT's `current()` and `key()` — keep
  /// hold of nodes between calls, which a handle borrowed from the tree could not do.
  type Node: Copy + Eq + Hash + Debug + 'static;

  /// The root of the tree the node belongs to.
  fn root(&self, node: Self::Node) -> Self::Node;

  /// The kind of a node.
  fn kind(&self, node: Self::Node) -> NodeKind;

  /// The parent of a node, or `None` for the root. An attribute's and a namespace node's parent
  /// is its element, though neither is a child of it.
  fn parent(&self, node: Self::Node) -> Option<Self::Node>;

  /// The child nodes, in document order: elements, text, comments and processing instructions,
  /// but not attributes or namespace nodes.
  fn children(&self, node: Self::Node) -> Vec<Self::Node>;

  /// The attribute nodes of an element (empty for other kinds), excluding namespace declarations.
  fn attributes(&self, node: Self::Node) -> Vec<Self::Node>;

  /// The namespace nodes of an element (empty for other kinds): the declarations in scope,
  /// including the implicit `xml`.
  fn namespaces(&self, node: Self::Node) -> Vec<Self::Node>;

  /// The expanded name of a node, or `None` for the kinds that have none.
  fn expanded_name(&self, node: Self::Node) -> Option<ExpandedName>;

  /// The qualified name of a node as it is written, prefix and all — what XPath's `name()`
  /// reports.
  ///
  /// The default is the local part, which is what a tree that does not keep prefixes can say.
  fn qualified_name(&self, node: Self::Node) -> Option<String> {
    self.expanded_name(node).map(|name| name.local)
  }

  /// The element carrying a given unique ID, which is what XPath's `id()` selects.
  ///
  /// An ID is only an ID because a DTD, a schema or the caller said so, so a tree that carries
  /// no such typing has none — hence the default.
  fn element_by_id(&self, id: &str) -> Option<Self::Node> {
    let _ = id;
    None
  }

  /// The string-value of a node (XPath 1.0 §5).
  fn string_value(&self, node: Self::Node) -> String;

  /// Compares two nodes in document order.
  fn document_order(&self, a: Self::Node, b: Self::Node) -> Ordering;
}
