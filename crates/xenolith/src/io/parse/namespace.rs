//! The namespace bindings in scope while parsing (Namespaces in XML 1.0).
//! The namespace bindings effective during parsing (Namespaces in XML 1.0).
//!
//! `xmlns:p="uri"` binds the namespace for the prefix `p` to `uri`, while `xmlns="uri"` binds the default namespace
//! (the namespace to which unprefixed element names belong) to `uri`. `xmlns=""` undeclares the default namespace,
//! indicating that unprefixed names do not belong to any namespace. A declaration is effective for the element
//! containing it and its descendants; however, if a declaration for the same prefix appears in a descendant element,
//! that declaration takes precedence (shadowing). The prefix `xml` is bound to `http://www.w3.org/XML/1998/namespace`
//! by specification and is always in effect.
//!
//! The bindings are managed on a single stack rather than by maintaining a map for each element. Whenever a start tag
//! appears, the parser records the stack depth and pushes the bindings corresponding to each namespace declaration on
//! that tag onto the stack. At the corresponding end tag, the stack is reverted to that depth. Since prefix resolution
//! involves searching from the top of the stack, the innermost binding is found first, eliminating the need for
//! special shadowing logic. Because the number of declarations in a document is typically small, the stack remains
//! short, and search costs are lower compared to maintaining a map for each element.

#[cfg(test)]
mod test;

use crate::name::NameId;

/// A single namespace declaration.
///
/// If `prefix` is `None`, it represents the default namespace. If `namespace` is `None`, the declaration for that
/// prefix is removed; however, under "Namespaces in XML 1.0," this is permitted only for the default namespace
/// (resulting in `xmlns=""`).
#[derive(Clone, Copy, Debug)]
struct Binding {
  prefix: Option<NameId>,
  namespace: Option<NameId>,
}

/// The namespace bindings within the scope (in order from outermost to innermost).
#[derive(Debug)]
pub(crate) struct NamespaceScope {
  bindings: Vec<Binding>,
}

impl NamespaceScope {
  /// A scope that maintains only the bindings for the `xml` prefix, which are always in scope.
  pub(crate) fn new() -> Self {
    Self { bindings: vec![Binding { prefix: Some(NameId::XML), namespace: Some(NameId::XML_NS) }] }
  }

  /// The current depth of the stack. This is used to pass a value to [`revert`](Self::revert) when an element that has
  /// started comes to an end.
  pub(crate) fn mark(&self) -> usize {
    self.bindings.len()
  }

  /// Removes the bindings added after the `mark` was obtained and closes their scopes. The parser calls this method at
  /// the end tag of the element corresponding to the start tag where the `mark` was obtained.
  pub(crate) fn revert(&mut self, mark: usize) {
    self.bindings.truncate(mark);
  }

  /// Adds a binding for `prefix`, or a binding for the default namespace if `prefix` is `None`. Specifying `None` for
  /// `namespace` undeclares the namespace, effectively acting like `xmlns=""`.
  pub(crate) fn bind(&mut self, prefix: Option<NameId>, namespace: Option<NameId>) {
    self.bindings.push(Binding { prefix, namespace });
  }

  /// Retrieves the namespace bound to `prefix`, or the default namespace if `prefix` is `None`, starting from the
  /// innermost binding.
  ///
  /// Returns `None` if the prefix is unbound or undeclared. For the default namespace, this signifies "no namespace,"
  /// which is also the initial state for all documents.
  pub(crate) fn resolve(&self, prefix: Option<NameId>) -> Option<NameId> {
    self.bindings.iter().rev().find(|b| b.prefix == prefix).and_then(|b| b.namespace)
  }
}
