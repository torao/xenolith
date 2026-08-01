//! Prefix bindings in scope.
//!
//! Bindings are kept in one stack rather than a map per element: declarations are rare, scopes
//! are shallow, and entering or leaving an element then costs nothing but moving an index.

use xylogue_core::name::NameId;

/// One prefix binding. A `prefix` of `None` is the default namespace.
#[derive(Clone, Copy, Debug)]
struct Binding {
  prefix: Option<NameId>,
  namespace: Option<NameId>,
}

/// The prefix bindings in scope, outermost first.
#[derive(Debug)]
pub(crate) struct NamespaceScope {
  bindings: Vec<Binding>,
}

impl NamespaceScope {
  /// Creates a scope holding only the binding of `xml`, which is fixed by the specification
  /// and always in scope.
  pub(crate) fn new() -> Self {
    Self { bindings: vec![Binding { prefix: Some(NameId::XML), namespace: Some(NameId::XML_NS) }] }
  }

  /// Records where the current element's declarations begin.
  pub(crate) fn mark(&self) -> usize {
    self.bindings.len()
  }

  /// Discards the declarations made since `mark`.
  pub(crate) fn revert(&mut self, mark: usize) {
    self.bindings.truncate(mark);
  }

  /// Adds a binding. A `namespace` of `None` undeclares the prefix, which only `xmlns=""` may
  /// do in Namespaces 1.0.
  pub(crate) fn bind(&mut self, prefix: Option<NameId>, namespace: Option<NameId>) {
    self.bindings.push(Binding { prefix, namespace });
  }

  /// Resolves a prefix, innermost binding first.
  ///
  /// `None` means the prefix is not bound; for the default namespace that is the normal state
  /// and means "no namespace".
  pub(crate) fn resolve(&self, prefix: Option<NameId>) -> Option<NameId> {
    self.bindings.iter().rev().find(|b| b.prefix == prefix).and_then(|b| b.namespace)
  }
}

#[cfg(test)]
mod tests {
  use xylogue_core::name::NamePool;

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
