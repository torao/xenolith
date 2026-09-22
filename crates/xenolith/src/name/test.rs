use super::*;

#[test]
fn reserved_names_have_stable_ids() {
  let pool = NamePool::new();
  assert_eq!(pool.resolve(NameId::EMPTY), "");
  assert_eq!(pool.resolve(NameId::XML), "xml");
  assert_eq!(pool.resolve(NameId::XMLNS), "xmlns");
  assert_eq!(pool.resolve(NameId::XML_NS), XML_NS_URI);
  assert_eq!(pool.resolve(NameId::XMLNS_NS), XMLNS_NS_URI);
  assert_eq!(pool.len(), RESERVED_NAMES.len());
  assert!(pool.is_empty());
}

#[test]
fn interning_is_idempotent() {
  let mut pool = NamePool::new();
  let a = pool.intern("item");
  let b = pool.intern("item");
  assert_eq!(a, b);
  assert_eq!(pool.get("item"), Some(a));
  assert_eq!(pool.get("missing"), None);
  assert_eq!(pool.len(), 6);
  assert!(!pool.is_empty());
}

#[test]
fn a_name_interned_before_the_fork_has_one_handle_in_both_pools() {
  let mut parent = NamePool::new();
  let item = parent.intern("item");
  let child = parent.fork();

  assert!(child.owns(item));
  assert_eq!(child.resolve(item), "item");
  assert_eq!(child.get("item"), Some(item), "the fork gives back the handle its parent issued");

  // Which is what lets a table keyed by the parent's handles be read through the fork's, as a DTD is.
  let table: HashMap<NameId, &str> = [(item, "declared")].into_iter().collect();
  assert_eq!(table.get(&child.get("item").expect("inherited")), Some(&"declared"));
}

#[test]
fn a_name_interned_after_the_fork_belongs_to_one_pool() {
  let mut parent = NamePool::new();
  parent.intern("shared");
  let mut child = parent.fork();
  let left = parent.intern("left");
  let right = child.intern("right");

  // Both took the same index, which is the confusion the handles exist to catch.
  assert_eq!(left.index(), right.index());
  assert_ne!(left, right);
  assert!(!child.owns(left));
  assert!(!parent.owns(right));
}

#[test]
#[should_panic(expected = "is not a handle of pool")]
fn a_handle_from_a_pool_that_grew_apart_is_refused() {
  let mut parent = NamePool::new();
  let mut child = parent.fork();
  let left = parent.intern("left");
  child.intern("right");
  let _ = child.resolve(left);
}

#[test]
fn a_fork_of_a_fork_keeps_every_earlier_range() {
  let mut a = NamePool::new();
  let from_a = a.intern("a");
  let mut b = a.fork();
  let from_b = b.intern("b");
  let c = b.fork();
  let after = b.intern("after");

  assert_eq!((c.resolve(from_a), c.resolve(from_b)), ("a", "b"));
  assert_eq!((c.get("a"), c.get("b")), (Some(from_a), Some(from_b)));
  assert!(!c.owns(after), "b interned it after c was forked");
}

#[test]
fn pools_that_share_no_history_accept_none_of_each_others_handles() {
  let mut one = NamePool::new();
  let mut two = NamePool::new();
  let x_in_one = one.intern("x");
  let x_in_two = two.intern("x");
  assert_eq!(x_in_one.index(), x_in_two.index());
  assert_ne!(x_in_one, x_in_two);
  assert!(!two.owns(x_in_one));
}

#[test]
fn the_reserved_names_have_the_same_handles_in_every_pool() {
  let mut parent = NamePool::new();
  parent.intern("x");
  let fork = parent.fork();
  let unrelated = NamePool::new();
  for pool in [&parent, &fork, &unrelated] {
    assert_eq!(pool.get("xml"), Some(NameId::XML));
    assert_eq!(pool.resolve(NameId::XMLNS_NS), XMLNS_NS_URI);
  }
}

#[test]
fn a_handle_displays_the_pool_that_issued_it_and_its_index() {
  let mut pool = NamePool::new();
  let item = pool.intern("item");
  let shown = item.to_string();
  assert!(shown.ends_with("#5"), "the first name after the five reserved ones: {shown}");

  // The same index in another pool reads apart, which is what tells two stray handles from each other.
  let elsewhere = NamePool::new().intern("item");
  assert_ne!(shown, elsewhere.to_string());

  assert_eq!(NameId::XML.to_string(), "1#1", "the reserved names belong to the root, pool 1");
}

#[test]
fn ncname_interning_rejects_bad_names() {
  let mut pool = NamePool::new();
  assert!(pool.intern_ncname("item").is_ok());
  let err = pool.intern_ncname("p:item").unwrap_err();
  assert!(matches!(err, Error::Name { .. }));
  assert!(pool.intern_ncname("").is_err());
}

#[test]
fn expanded_name_ignores_prefix() {
  let mut pool = NamePool::new();
  let ns = pool.intern("urn:example");
  let local = pool.intern("item");
  let a = QName::new(Some(pool.intern("a")), Some(ns), local);
  let b = QName::new(Some(pool.intern("b")), Some(ns), local);
  assert_ne!(a, b);
  assert_eq!(a.expanded, b.expanded);
  assert_eq!(a.to_lexical(&pool), "a:item");
  assert_eq!(QName::new(None, None, local).to_lexical(&pool), "item");
  assert_eq!(DisplayQName(&b, &pool).to_string(), "b:item");
}
