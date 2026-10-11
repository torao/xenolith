//! The evaluation state that supplies XPath's context values and namespace and function bindings.
//!
//! XPath 1.0 §1 defines the evaluation context as the context node, its position and size, the in-scope namespace
//! bindings, and the in-scope variables. `position()` returns the position, and `last()` returns the size. A path or
//! predicate can change the context node, position, and size as evaluation proceeds. The namespace bindings come from
//! the [`XPathContext`] the expression was parsed in; the variables come from the [`XPathVariables`] passed to the
//! evaluation and the variables that context defines.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::error::{Error, Location};
use crate::xpath::model::Model;
use crate::xpath::object::Object;
use crate::xpath::{XPath, XPathContext, XPathVariables, located};

/// The evaluation context for an XPath expression.
///
/// The context stores the current node and its one-based position and size in the current node set. It also borrows the
/// model used for tree operations and the source expression used to locate evaluation errors. [`at`](Self::at) creates
/// a context for another node and position while reusing the same model and source references.
pub(crate) struct EvalState<'a, M: Model> {
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
  /// The context the expression was parsed in, which binds its prefixes and holds its application-defined functions.
  pub context: &'a XPathContext,
  /// The variable values passed to the evaluation.
  pub variables: &'a XPathVariables,
  /// The value of each variable referenced so far in the evaluation, by the address of its definition or of the value
  /// passed, so that each is evaluated or converted once.
  pub values: &'a RefCell<HashMap<*const (), Object<M::Node>>>,
}

impl<'a, M: Model> EvalState<'a, M> {
  /// Creates a context for `node` as the only node in the current node set, evaluating `xpath` with the variable
  /// values `variables`. `values` keeps the values of the variables referenced so far.
  pub(crate) fn new(
    model: &'a M,
    node: M::Node,
    xpath: &'a XPath,
    variables: &'a XPathVariables,
    values: &'a RefCell<HashMap<*const (), Object<M::Node>>>,
  ) -> Self {
    let (source, start, context) = (xpath.source.as_str(), &xpath.location, &xpath.context);
    Self { model, node, position: 1, size: 1, source, start, context, variables, values }
  }

  /// Creates a context for `node` at `position` in a node set of `size` nodes.
  pub(crate) const fn at(&self, node: M::Node, position: usize, size: usize) -> Self {
    Self { node, position, size, ..*self }
  }

  /// Locates `error` at the byte offset `at` in the expression, unless it is located already: an error raised deeper
  /// in the expression is located where it was raised.
  pub(crate) fn locate(&self, error: Error, at: usize) -> Error {
    if error.location().is_unknown() { error.at(located(self.start, self.source, at)) } else { error }
  }
}
