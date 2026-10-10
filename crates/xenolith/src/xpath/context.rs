//! The values XPath uses while evaluating an expression.
//!
//! XPath 1.0 §1 defines the evaluation context as the context node, its position and size, the in-scope namespace
//! bindings, and the in-scope variables. `position()` returns the position, and `last()` returns the size. A path or
//! predicate can change the context node, position, and size as evaluation proceeds. This implementation does not
//! accept namespace bindings or variables from callers; it recognizes only the predefined `xml` prefix.

use crate::error::{Error, Location};
use crate::name::XML_NS_URI;
use crate::xpath::located;
use crate::xpath::model::Model;

/// The evaluation context for an XPath expression.
///
/// The context stores the current node and its one-based position and size in the current node set. It also borrows the
/// model used for tree operations and the source expression used to locate evaluation errors. [`at`](Self::at) creates
/// a context for another node and position while reusing the same model and source references.
pub(crate) struct Context<'a, M: Model> {
  /// The model that provides access to the tree and its nodes.
  pub model: &'a M,
  /// The node against which the expression is currently evaluated.
  pub node: M::Node,
  /// The context node's one-based position in the current node set; `position()` returns this value.
  pub position: usize,
  /// The number of nodes in the current node set; `last()` returns this value.
  pub size: usize,
  /// The expression being evaluated, as it was written.
  pub source: &'a str,
  /// The source location corresponding to byte offset zero in `source`.
  pub start: &'a Location,
}

impl<'a, M: Model> Context<'a, M> {
  /// Creates a context for `node` as the only node in the current node set, evaluating `source`, which begins at
  /// `start`.
  pub(crate) const fn new(model: &'a M, node: M::Node, source: &'a str, start: &'a Location) -> Self {
    Self { model, node, position: 1, size: 1, source, start }
  }

  /// Creates a context for `node` at `position` in a node set of `size` nodes.
  pub(crate) const fn at(&self, node: M::Node, position: usize, size: usize) -> Self {
    Self { model: self.model, node, position, size, source: self.source, start: self.start }
  }

  /// Locates `error` at the byte offset `at` in the expression, unless it is located already: an error raised deeper
  /// in the expression is located where it was raised.
  pub(crate) fn locate(&self, error: Error, at: usize) -> Error {
    if error.location().is_unknown() { error.at(located(self.start, self.source, at)) } else { error }
  }
}

/// Returns the namespace URI bound to a prefix in an XPath expression, or `None` if the prefix is unbound.
///
/// Prefixes in an XPath expression are resolved against the expression's namespace bindings, not the prefixes used in
/// the document. This implementation has no caller-provided bindings, so it recognizes only `xml`, which is
/// predefined as bound to the XML namespace (Namespaces in XML §3).
pub(crate) fn namespace_of(prefix: &str) -> Option<&'static str> {
  (prefix == "xml").then_some(XML_NS_URI)
}
