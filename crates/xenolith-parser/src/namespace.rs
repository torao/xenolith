//! Prefix-to-namespace bindings in scope during parsing.
//!
//! A prefixed declaration `xmlns:p="..."` binds the prefix `p` to the value, while a prefix-less declaration
//! `xmlns="..."` binds the default namespace (the namespace to which prefix-less names belong) to the value. `xmlns=""`
//! removes the declaration of the default namespace. The scope of a declaration extends to the element containing it
//! and its child elements; however, if a re-declaration exists nested within a child element, it shadows the outer
//! declaration. The `xml` prefix is bound to `http://www.w3.org/XML/1998/namespace` by the specification and is always
//! in scope.
//!
//! A single element can contain multiple prefix bindings. In a naive implementation, one might think of maintaining a
//! map of prefix bindings for each element, but in this parser implementation, they are kept on a single stack. When
//! the parser detects a start tag with prefix bindings, it stores the current depth of the stack and pushes the
//! bindings for each `xmlns` declaration onto the stack. Then, when it encounters the corresponding end tag, it
//! truncates the stack to the stored depth. When resolving prefixes, a linear search from the top of the stack finds
//! the innermost binding, allowing shadowing to function naturally. In fact, there are generally not many `xmlns`
//! declarations in a document and their scope is shallow, scanning a short stack is less costly than traversing
//! elements to maintain a map.
//!

use xenolith_core::name::NameId;

/// A single prefix binding.
///
/// If `prefix` is `None`, the default namespace is used, and names without a prefix belong to this namespace. If
/// `namespace` is `None`, no prefix is bound. `xmlns=""` uses this to remove the declaration of the default namespace.
///
#[derive(Clone, Copy, Debug)]
struct Binding {
  prefix: Option<NameId>,
  namespace: Option<NameId>,
}

/// The prefix bindings within the scope, listed in order from the outermost first.
///
#[derive(Debug)]
pub(crate) struct NamespaceScope {
  bindings: Vec<Binding>,
}

impl NamespaceScope {
  /// Creates a scope contains only the `xml` binding. This binding is fixed by the specification and is always present
  /// within the scope.
  ///
  pub(crate) fn new() -> Self {
    Self { bindings: vec![Binding { prefix: Some(NameId::XML), namespace: Some(NameId::XML_NS) }] }
  }

  /// The current stack depth, This is used so that the scope can be reverted with [`revert`](Self::revert) when
  /// current element ends.
  ///
  pub(crate) fn mark(&self) -> usize {
    self.bindings.len()
  }

  /// Discards prefix binding declarations made after [`mark`](Self::mark) and terminates their scope. This is called
  /// when the element in which the declarations were made is closed.
  ///
  pub(crate) fn revert(&mut self, mark: usize) {
    self.bindings.truncate(mark);
  }

  /// Adds a prefix binding. Specifying `None` for `namespace` removes the prefix declaration. In Namespace 1.0, the
  /// only way to do this is with `xmlns=""`.
  ///
  pub(crate) fn bind(&mut self, prefix: Option<NameId>, namespace: Option<NameId>) {
    self.bindings.push(Binding { prefix, namespace });
  }

  /// Resolves the `prefix`. Bindings are processed in order, starting with the innermost binding on the stack.
  ///
  /// `None` means that no binding declaration exists for that prefix. For the default namespace, this is the initial
  /// state and means "no namespace."
  ///
  pub(crate) fn resolve(&self, prefix: Option<NameId>) -> Option<NameId> {
    self.bindings.iter().rev().find(|b| b.prefix == prefix).and_then(|b| b.namespace)
  }
}

#[cfg(test)]
mod tests {
  use xenolith_core::name::NamePool;

  use super::*;

  #[test]
  fn xml_is_bound_from_the_start() {
    let scope = NamespaceScope::new();
    assert_eq!(scope.resolve(Some(NameId::XML)), Some(NameId::XML_NS));
  }

  #[test]
  fn inner_bindings_shadow_outer_ones_and_are_reverted() {
    let mut pool = NamePool::new();
    let (p, a, b) = (pool.intern("p"), pool.intern("urn:a"), pool.intern("urn:b"));
    let mut scope = NamespaceScope::new();

    scope.bind(Some(p), Some(a));
    let mark = scope.mark();
    scope.bind(Some(p), Some(b));
    assert_eq!(scope.resolve(Some(p)), Some(b));

    scope.revert(mark);
    assert_eq!(scope.resolve(Some(p)), Some(a));
  }

  #[test]
  fn the_default_namespace_can_be_undeclared() {
    let mut pool = NamePool::new();
    let a = pool.intern("urn:a");
    let mut scope = NamespaceScope::new();

    assert_eq!(scope.resolve(None), None, "no default namespace to begin with");
    scope.bind(None, Some(a));
    assert_eq!(scope.resolve(None), Some(a));
    scope.bind(None, None); // xmlns=""
    assert_eq!(scope.resolve(None), None);
  }
}
