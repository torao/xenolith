use crate::name::NamePool;

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
