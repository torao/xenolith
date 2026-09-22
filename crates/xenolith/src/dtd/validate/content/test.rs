use crate::dtd::{ContentParticle as P, Occurs::*};
use crate::name::NamePool;

use super::*;

fn names(pool: &mut NamePool, list: &[&str]) -> Vec<NameId> {
  list.iter().map(|n| pool.intern(n)).collect()
}

#[test]
fn matches_a_sequence() {
  let mut pool = NamePool::new();
  let (a, b, c) = (pool.intern("a"), pool.intern("b"), pool.intern("c"));
  // (a, b, c)
  let model = ContentModel::compile(&P::Seq(vec![P::Name(a, Once), P::Name(b, Once), P::Name(c, Once)], Once));
  assert!(model.matches(&names(&mut pool, &["a", "b", "c"])).is_ok());
  assert!(model.matches(&names(&mut pool, &["a", "b"])).is_err(), "too short");
  assert!(model.matches(&names(&mut pool, &["a", "c", "b"])).is_err(), "out of order");
  assert!(model.matches(&names(&mut pool, &["a", "b", "c", "a"])).is_err(), "too long");
}

#[test]
fn honours_occurrences() {
  let mut pool = NamePool::new();
  let (a, b) = (pool.intern("a"), pool.intern("b"));
  // (a?, b*)
  let model = ContentModel::compile(&P::Seq(vec![P::Name(a, Optional), P::Name(b, ZeroOrMore)], Once));
  for seq in [&[][..], &["a"], &["b"], &["a", "b"], &["b", "b", "b"], &["a", "b", "b"]] {
    assert!(model.matches(&names(&mut pool, seq)).is_ok(), "{seq:?} should match");
  }
  assert!(model.matches(&names(&mut pool, &["b", "a"])).is_err());
}

#[test]
fn matches_a_choice_with_repetition() {
  let mut pool = NamePool::new();
  let (a, b, c) = (pool.intern("a"), pool.intern("b"), pool.intern("c"));
  // (a | b)+ , c
  let model = ContentModel::compile(&P::Seq(
    vec![P::Choice(vec![P::Name(a, Once), P::Name(b, Once)], OneOrMore), P::Name(c, Once)],
    Once,
  ));
  assert!(model.matches(&names(&mut pool, &["a", "c"])).is_ok());
  assert!(model.matches(&names(&mut pool, &["a", "b", "a", "c"])).is_ok());
  assert!(model.matches(&names(&mut pool, &["c"])).is_err(), "needs one of a|b first");
}

#[test]
fn reports_what_was_allowed() {
  let mut pool = NamePool::new();
  let (a, b, c) = (pool.intern("a"), pool.intern("b"), pool.intern("c"));
  let model = ContentModel::compile(&P::Seq(vec![P::Name(a, Once), P::Name(b, Once)], Once));
  let failure = model.matches(&names(&mut pool, &["a", "c"])).unwrap_err();
  assert_eq!(failure.at, Some(c));
  assert_eq!(failure.allowed, vec![b]);
}

#[test]
fn reports_what_a_short_sequence_still_needed() {
  let mut pool = NamePool::new();
  let (a, b) = (pool.intern("a"), pool.intern("b"));
  let model = ContentModel::compile(&P::Seq(vec![P::Name(a, Once), P::Name(b, Once)], Once));

  let stopped_short = model.matches(&names(&mut pool, &["a"])).unwrap_err();
  assert_eq!(stopped_short.at, None, "nothing was rejected; the sequence ended too soon");
  assert_eq!(stopped_short.allowed, vec![b]);

  let nothing_at_all = model.matches(&[]).unwrap_err();
  assert_eq!(nothing_at_all.at, None);
  assert_eq!(nothing_at_all.allowed, vec![a]);
}

#[test]
fn detects_nondeterminism() {
  let mut pool = NamePool::new();
  let (a, b) = (pool.intern("a"), pool.intern("b"));
  // (a, b) | (a, c): ambiguous on the leading `a`, which Appendix E forbids.
  let c = pool.intern("c");
  let ambiguous = ContentModel::compile(&P::Choice(
    vec![
      P::Seq(vec![P::Name(a, Once), P::Name(b, Once)], Once),
      P::Seq(vec![P::Name(a, Once), P::Name(c, Once)], Once),
    ],
    Once,
  ));
  assert!(!ambiguous.is_deterministic());

  let fine = ContentModel::compile(&P::Seq(vec![P::Name(a, Once), P::Name(b, Once)], Once));
  assert!(fine.is_deterministic());
}
