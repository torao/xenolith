//! What an expression is evaluated against.
//!
//! XPath 1.0 §1 calls this the evaluation context: a node, a position and a size (which
//! `position()` and `last()` report), the namespace declarations a prefix in the expression is
//! resolved against, and the variables in scope. The first three change as a path is walked —
//! every predicate is evaluated with each candidate as the context node — while the other two
//! are fixed for an evaluation.
//!
//! The two fixed halves are separate types, as they are in Java: [`Namespaces`] is
//! `javax.xml.namespace.NamespaceContext` and [`Variables`] is
//! `javax.xml.xpath.XPathVariableResolver`. Keeping them apart is not only for the resemblance —
//! a namespace binding is two strings and says nothing about what tree the expression will run
//! over, so it can be settled when the expression is compiled, while a variable holds a value
//! that may be a set of nodes and so cannot be.

use std::collections::HashMap;
use std::fmt;

use xylograph_xdm::Model;

use crate::value::Value;

/// The namespace a prefix in an expression stands for: Java's `NamespaceContext`.
///
/// A prefix written in an expression means nothing on its own — it is not looked up in the
/// document, which may bind the same namespace to a different prefix or none. An expression that
/// uses an unbound prefix is an error, which is why there is nothing to fall back to.
///
/// # Examples
///
/// ```
/// use xylograph_xpath::Namespaces;
///
/// let namespaces = Namespaces::new().with("h", "http://www.w3.org/1999/xhtml");
/// assert_eq!(namespaces.get("h"), Some("http://www.w3.org/1999/xhtml"));
/// assert_eq!(namespaces.get("nosuch"), None);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Namespaces {
  bindings: HashMap<String, String>,
}

impl Namespaces {
  /// An empty set of bindings.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Binds a prefix to a namespace.
  #[must_use]
  pub fn with(mut self, prefix: &str, namespace: &str) -> Self {
    self.bindings.insert(prefix.to_owned(), namespace.to_owned());
    self
  }

  /// The namespace a prefix is bound to, if it is bound.
  #[must_use]
  pub fn get(&self, prefix: &str) -> Option<&str> {
    self.bindings.get(prefix).map(String::as_str)
  }
}

/// The values of the variables an expression names: Java's `XPathVariableResolver`.
///
/// # Examples
///
/// ```
/// use xylograph_xpath::{Value, Variables};
///
/// let variables = Variables::<u32>::new().with("limit", Value::Number(10.0));
/// assert_eq!(variables.get(None, "limit"), Some(&Value::Number(10.0)));
/// ```
#[derive(Debug)]
pub struct Variables<N> {
  /// By expanded name: the namespace the prefix resolved to, and the local part.
  bindings: HashMap<(Option<String>, String), Value<N>>,
}

impl<N> Default for Variables<N> {
  fn default() -> Self {
    Self::new()
  }
}

impl<N> Variables<N> {
  /// No variables at all.
  #[must_use]
  pub fn new() -> Self {
    Self { bindings: HashMap::new() }
  }

  /// Binds a variable with no namespace, as `$name` refers to it.
  #[must_use]
  pub fn with(mut self, name: &str, value: Value<N>) -> Self {
    self.bindings.insert((None, name.to_owned()), value);
    self
  }

  /// Binds a variable in a namespace, as `$prefix:name` refers to it once the prefix is bound.
  #[must_use]
  pub fn with_ns(mut self, namespace: &str, name: &str, value: Value<N>) -> Self {
    self.bindings.insert((Some(namespace.to_owned()), name.to_owned()), value);
    self
  }

  /// The value of a variable, by the namespace its prefix resolved to and its local part.
  #[must_use]
  pub fn get(&self, namespace: Option<&str>, local: &str) -> Option<&Value<N>> {
    // The key owns its strings; an expression names few variables, so this is cheap enough.
    self.bindings.get(&(namespace.map(ToOwned::to_owned), local.to_owned()))
  }
}

/// The node an expression is being evaluated against, and everything that goes with it.
///
/// Cheap to copy: everything but the position and size is held by reference, so stepping to a
/// new context node costs nothing.
pub struct Context<'a, M: Model> {
  /// The tree being walked.
  pub model: &'a M,
  /// The context node.
  pub node: M::Node,
  /// The context position, counting from 1 — what `position()` reports.
  pub position: usize,
  /// The context size — what `last()` reports.
  pub size: usize,
  /// The prefixes the expression may use.
  pub namespaces: &'a Namespaces,
  /// The variables the expression may name.
  pub variables: &'a Variables<M::Node>,
}

impl<'a, M: Model> Context<'a, M> {
  /// A context over `node`, alone: position 1 of a set of 1.
  pub fn new(model: &'a M, node: M::Node, namespaces: &'a Namespaces, variables: &'a Variables<M::Node>) -> Self {
    Self { model, node, position: 1, size: 1, namespaces, variables }
  }

  /// The same context, moved to another node at a given position in a set of `size`.
  #[must_use]
  pub fn at(&self, node: M::Node, position: usize, size: usize) -> Self {
    Self { node, position, size, ..*self }
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
