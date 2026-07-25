//! What an expression is evaluated against.
//!
//! XPath 1.0 §1 calls this the evaluation context: a node, a position and a size (which
//! `position()` and `last()` report), the variables in scope, and the namespace declarations a
//! prefix in the expression is resolved against. The first three change as a path is walked —
//! every predicate is evaluated with each candidate as the context node — while the rest are
//! fixed for the whole evaluation, so they are kept apart in an [`Environment`].

use std::collections::HashMap;
use std::fmt;

use xylograph_xdm::Model;

use crate::value::Value;

/// The variables and namespace bindings an expression is evaluated with.
///
/// A prefix used in the expression — in a name test, a variable name or a function name — means
/// nothing on its own; it is resolved here. An expression that uses a prefix with no binding is
/// an error, which is why there is no default namespace to fall back to.
///
/// # Examples
///
/// ```
/// use xylograph_xpath::{Environment, Value};
///
/// let environment = Environment::<u32>::new()
///   .with_variable("limit", Value::Number(10.0))
///   .with_namespace("h", "http://www.w3.org/1999/xhtml");
/// assert!(environment.namespace("h").is_some());
/// ```
#[derive(Debug)]
pub struct Environment<N> {
  /// Variables by expanded name: the namespace its prefix resolved to, and the local part.
  variables: HashMap<(Option<String>, String), Value<N>>,
  namespaces: HashMap<String, String>,
}

impl<N> Default for Environment<N> {
  fn default() -> Self {
    Self::new()
  }
}

impl<N> Environment<N> {
  /// An environment with no variables and no namespace bindings.
  #[must_use]
  pub fn new() -> Self {
    Self { variables: HashMap::new(), namespaces: HashMap::new() }
  }

  /// Binds a variable with no namespace, as `$name` refers to it.
  #[must_use]
  pub fn with_variable(mut self, name: &str, value: Value<N>) -> Self {
    self.variables.insert((None, name.to_owned()), value);
    self
  }

  /// Binds a variable in a namespace, as `$prefix:name` refers to it once the prefix is bound.
  #[must_use]
  pub fn with_variable_ns(mut self, namespace: &str, name: &str, value: Value<N>) -> Self {
    self.variables.insert((Some(namespace.to_owned()), name.to_owned()), value);
    self
  }

  /// Binds a prefix to a namespace, for the prefixes the expression uses.
  #[must_use]
  pub fn with_namespace(mut self, prefix: &str, namespace: &str) -> Self {
    self.namespaces.insert(prefix.to_owned(), namespace.to_owned());
    self
  }

  /// The namespace a prefix is bound to, if it is bound.
  #[must_use]
  pub fn namespace(&self, prefix: &str) -> Option<&str> {
    self.namespaces.get(prefix).map(String::as_str)
  }

  /// The value of a variable, by the namespace its prefix resolved to and its local part.
  #[must_use]
  pub fn variable(&self, namespace: Option<&str>, local: &str) -> Option<&Value<N>> {
    // The key owns its strings, so look up with an owned pair; expressions are short.
    self.variables.get(&(namespace.map(ToOwned::to_owned), local.to_owned()))
  }
}

/// The node an expression is being evaluated against, and everything that goes with it.
///
/// Cheap to copy: it holds the tree and the environment by reference, so stepping to a new
/// context node costs nothing.
pub struct Context<'a, M: Model> {
  /// The tree being walked.
  pub model: &'a M,
  /// The context node.
  pub node: M::Node,
  /// The context position, counting from 1 — what `position()` reports.
  pub position: usize,
  /// The context size — what `last()` reports.
  pub size: usize,
  /// The variables and namespace bindings.
  pub environment: &'a Environment<M::Node>,
}

impl<'a, M: Model> Context<'a, M> {
  /// A context over `node`, alone: position 1 of a set of 1.
  pub fn new(model: &'a M, node: M::Node, environment: &'a Environment<M::Node>) -> Self {
    Self { model, node, position: 1, size: 1, environment }
  }

  /// The same context, moved to another node at a given position in a set of `size`.
  #[must_use]
  pub fn at(&self, node: M::Node, position: usize, size: usize) -> Self {
    Self { model: self.model, node, position, size, environment: self.environment }
  }
}

// Written by hand: the derives would demand `M: Clone` and `M: Debug`, which a model need not be.
impl<M: Model> Clone for Context<'_, M> {
  fn clone(&self) -> Self {
    *self
  }
}

impl<M: Model> Copy for Context<'_, M> {}

impl<M: Model> fmt::Debug for Context<'_, M> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Context")
      .field("node", &self.node)
      .field("position", &self.position)
      .field("size", &self.size)
      .finish_non_exhaustive()
  }
}

/// Sorts a set of nodes into document order and drops the duplicates.
pub(crate) fn normalize<M: Model>(model: &M, nodes: &mut Vec<M::Node>) {
  nodes.sort_by(|a, b| model.document_order(*a, *b));
  nodes.dedup();
}
