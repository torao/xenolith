use super::*;

/// Interns `name` in a pool with room for it.
fn intern(pool: &mut NamePool, name: &str) -> NameId {
  pool.intern(name).expect("room in the pool")
}

#[test]
fn reserved_names_have_stable_ids() {
  let pool = NamePool::new();
  assert_eq!(pool.resolve(NameId::EMPTY), Some(""));
  assert_eq!(pool.resolve(NameId::XML), Some("xml"));
  assert_eq!(pool.resolve(NameId::XMLNS), Some("xmlns"));
  assert_eq!(pool.resolve(NameId::XML_NS), Some(XML_NS_URI));
  assert_eq!(pool.resolve(NameId::XMLNS_NS), Some(XMLNS_NS_URI));
  assert_eq!(pool.len(), RESERVED_NAMES.len());
  assert!(pool.is_empty());
}

#[test]
fn interning_is_idempotent() {
  let mut pool = NamePool::new();
  let a = intern(&mut pool, "item");
  let b = intern(&mut pool, "item");
  assert_eq!(a, b);
  assert_eq!(pool.get("item"), Some(a));
  assert_eq!(pool.get("missing"), None);
  assert_eq!(pool.len(), 6);
  assert!(!pool.is_empty());
}

#[test]
fn a_name_interned_before_the_fork_has_one_handle_in_both_pools() {
  let mut parent = NamePool::new();
  let item = intern(&mut parent, "item");
  let child = parent.fork();

  assert!(child.owns(item));
  assert_eq!(child.resolve(item), Some("item"));
  assert_eq!(child.get("item"), Some(item), "the fork gives back the handle its parent issued");

  // Which is what lets a table keyed by the parent's handles be read through the fork's, as a DTD is.
  let table: HashMap<NameId, &str> = [(item, "declared")].into_iter().collect();
  assert_eq!(table.get(&child.get("item").expect("inherited")), Some(&"declared"));
}

#[test]
fn a_name_interned_after_the_fork_belongs_to_one_pool() {
  let mut parent = NamePool::new();
  intern(&mut parent, "shared");
  let mut child = parent.fork();
  let left = intern(&mut parent, "left");
  let right = intern(&mut child, "right");

  // Both took the same index, which is the confusion the handles exist to catch.
  assert_eq!(left.index(), right.index());
  assert_ne!(left, right);
  assert!(!child.owns(left));
  assert!(!parent.owns(right));
}

#[test]
fn a_handle_from_a_pool_that_grew_apart_is_not_resolved() {
  let mut parent = NamePool::new();
  let mut child = parent.fork();
  let left = intern(&mut parent, "left");
  intern(&mut child, "right");
  assert_eq!(child.resolve(left), None, "a mistake of the caller, so no panic");
}

#[test]
#[should_panic(expected = "is not a handle of pool")]
fn resolving_a_foreign_handle_as_owned_is_a_bug_of_the_crate() {
  let mut parent = NamePool::new();
  let mut child = parent.fork();
  let left = intern(&mut parent, "left");
  intern(&mut child, "right");
  let _ = child.resolve_own(left);
}

#[test]
fn a_fork_of_a_fork_keeps_every_earlier_range() {
  let mut a = NamePool::new();
  let from_a = intern(&mut a, "a");
  let mut b = a.fork();
  let from_b = intern(&mut b, "b");
  let c = b.fork();
  let after = intern(&mut b, "after");

  assert_eq!((c.resolve(from_a), c.resolve(from_b)), (Some("a"), Some("b")));
  assert_eq!((c.get("a"), c.get("b")), (Some(from_a), Some(from_b)));
  assert!(!c.owns(after), "b interned it after c was forked");
}

#[test]
fn pools_that_share_no_history_accept_none_of_each_others_handles() {
  let mut one = NamePool::new();
  let mut two = NamePool::new();
  let x_in_one = intern(&mut one, "x");
  let x_in_two = intern(&mut two, "x");
  assert_eq!(x_in_one.index(), x_in_two.index());
  assert_ne!(x_in_one, x_in_two);
  assert!(!two.owns(x_in_one));
  assert_eq!(two.resolve(x_in_one), None);
}

#[test]
fn the_reserved_names_have_the_same_handles_in_every_pool() {
  let mut parent = NamePool::new();
  intern(&mut parent, "x");
  let fork = parent.fork();
  let unrelated = NamePool::new();
  for pool in [&parent, &fork, &unrelated] {
    assert_eq!(pool.get("xml"), Some(NameId::XML));
    assert_eq!(pool.resolve(NameId::XMLNS_NS), Some(XMLNS_NS_URI));
  }
}

#[test]
fn a_handle_displays_the_pool_that_issued_it_and_its_index() {
  let mut pool = NamePool::new();
  let item = intern(&mut pool, "item");
  let shown = item.to_string();
  assert!(shown.ends_with("#5"), "the first name after the five reserved ones: {shown}");

  // The same index in another pool reads apart, which is what tells two stray handles from each other.
  let elsewhere = intern(&mut NamePool::new(), "item");
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
fn a_pool_is_full_one_name_before_its_count_would_not_fit_a_u32() {
  // Filling a pool for real takes four billion names, so the check is tried on the counts at the edge.
  let last = u32::MAX - 2;
  assert_eq!(next_index(last as usize), Some(last));
  assert_eq!(next_index(last as usize + 1), None);
  assert_eq!(next_index(usize::MAX), None);
}

#[test]
fn expanded_name_ignores_prefix() {
  let mut pool = NamePool::new();
  let ns = intern(&mut pool, "urn:example");
  let local = intern(&mut pool, "item");
  let a = QName::new(Some(intern(&mut pool, "a")), Some(ns), local);
  let b = QName::new(Some(intern(&mut pool, "b")), Some(ns), local);
  assert_ne!(a, b);
  assert_eq!(a.expanded, b.expanded);
  assert_eq!(a.to_lexical(&pool).as_deref(), Some("a:item"));
  assert_eq!(QName::new(None, None, local).to_lexical(&pool).as_deref(), Some("item"));
  assert_eq!(DisplayQName(&b, &pool).to_string(), "b:item");
}

#[test]
fn a_name_of_another_pool_has_no_lexical_form_there_but_still_displays() {
  let mut pool = NamePool::new();
  let local = intern(&mut pool, "item");
  let other = NamePool::new();
  let name = QName::new(None, None, local);
  assert_eq!(name.to_lexical(&other), None);
  assert_eq!(DisplayQName(&name, &other).to_string(), local.to_string(), "written as its handle, without a panic");
}
