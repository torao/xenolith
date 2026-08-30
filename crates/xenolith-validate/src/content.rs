//! Matching children against an element content model.
//!
//! A content model like `(a, b*, (c | d)+)` is a regular expression over child element names.
//! This compiles one to a Glushkov automaton — a position for every name in the model, with
//! `first`, `last` and `follow` sets — and runs a sequence of child names through it. Glushkov
//! suits XML because the automaton is deterministic exactly when the model is *deterministic*
//! in the sense XML requires (Appendix E), so building it also answers whether the model is
//! well-formed.

use xenolith_core::name::NameId;
use xenolith_parser::dtd::{ContentParticle, Occurs};

/// A compiled content model, ready to match child sequences.
#[derive(Debug)]
pub(crate) struct ContentModel {
  /// The child-element name at each position.
  symbols: Vec<NameId>,
  /// Positions a match may begin at.
  first: Vec<usize>,
  /// Positions a match may end at.
  last: Vec<usize>,
  /// For each position, the positions that may follow it.
  follow: Vec<Vec<usize>>,
  /// Whether the model accepts no children at all.
  nullable: bool,
}

/// The intermediate fragment for one sub-expression during compilation.
struct Fragment {
  nullable: bool,
  first: Vec<usize>,
  last: Vec<usize>,
}

impl ContentModel {
  /// Compiles a content particle.
  pub(crate) fn compile(particle: &ContentParticle) -> Self {
    let mut model =
      Self { symbols: Vec::new(), first: Vec::new(), last: Vec::new(), follow: Vec::new(), nullable: false };
    let root = model.build(particle);
    model.first = root.first;
    model.last = root.last;
    model.nullable = root.nullable;
    model
  }

  /// Recursively assigns positions and accumulates `follow`, returning the fragment.
  fn build(&mut self, particle: &ContentParticle) -> Fragment {
    let (fragment, occurs) = match particle {
      ContentParticle::Name(name, occurs) => {
        let position = self.symbols.len();
        self.symbols.push(*name);
        self.follow.push(Vec::new());
        (Fragment { nullable: false, first: vec![position], last: vec![position] }, *occurs)
      }
      ContentParticle::Seq(parts, occurs) => (self.sequence(parts), *occurs),
      ContentParticle::Choice(parts, occurs) => (self.choice(parts), *occurs),
    };
    self.apply(fragment, occurs)
  }

  /// `a, b, c`: concatenation.
  fn sequence(&mut self, parts: &[ContentParticle]) -> Fragment {
    let mut nullable = true;
    let mut first = Vec::new();
    let mut last: Vec<usize> = Vec::new();
    for part in parts {
      let fragment = self.build(part);
      // `first` grows while the prefix so far is nullable.
      if nullable {
        extend_unique(&mut first, &fragment.first);
      }
      // Each earlier ending position may be followed by this part's starts.
      for &l in &last {
        for &f in &fragment.first {
          push_unique(&mut self.follow[l], f);
        }
      }
      // `last` is this part's ends, plus earlier ends while this part is nullable.
      if fragment.nullable {
        extend_unique(&mut last, &fragment.last);
      } else {
        last = fragment.last.clone();
      }
      nullable = nullable && fragment.nullable;
    }
    Fragment { nullable, first, last }
  }

  /// `a | b | c`: alternation.
  fn choice(&mut self, parts: &[ContentParticle]) -> Fragment {
    let mut nullable = false;
    let mut first = Vec::new();
    let mut last = Vec::new();
    for part in parts {
      let fragment = self.build(part);
      extend_unique(&mut first, &fragment.first);
      extend_unique(&mut last, &fragment.last);
      nullable = nullable || fragment.nullable;
    }
    Fragment { nullable, first, last }
  }

  /// Applies `?`, `*` or `+` to a fragment.
  fn apply(&mut self, fragment: Fragment, occurs: Occurs) -> Fragment {
    let repeatable = matches!(occurs, Occurs::ZeroOrMore | Occurs::OneOrMore);
    if repeatable {
      // A repeat links every ending position back to every starting one.
      for &l in &fragment.last {
        for &f in &fragment.first {
          push_unique(&mut self.follow[l], f);
        }
      }
    }
    let nullable = fragment.nullable || matches!(occurs, Occurs::Optional | Occurs::ZeroOrMore);
    Fragment { nullable, first: fragment.first, last: fragment.last }
  }

  /// True if the model is deterministic (XML 1.0 Appendix E): at every point, the next name
  /// picks the position unambiguously, i.e. no reachable set of positions holds the same
  /// symbol twice.
  pub(crate) fn is_deterministic(&self) -> bool {
    if has_duplicate_symbol(&self.first, &self.symbols) {
      return false;
    }
    self.follow.iter().all(|targets| !has_duplicate_symbol(targets, &self.symbols))
  }

  /// Matches a sequence of child element names.
  ///
  /// On failure, returns the names the model would have accepted at the point it stuck — for a
  /// name that did not fit, or for the end of a sequence that stopped short.
  pub(crate) fn matches(&self, children: &[NameId]) -> Result<(), MatchFailure> {
    let mut active: Vec<usize> = self.first.clone();
    let mut allowed_at_start = true;

    for (index, &child) in children.iter().enumerate() {
      let candidates = if index == 0 { &self.first } else { &active.clone() };
      let next: Vec<usize> = candidates.iter().copied().filter(|&p| self.symbols[p] == child).collect();
      if next.is_empty() {
        return Err(MatchFailure { at: Some(child), allowed: self.symbols_of(candidates) });
      }
      // Move to everything reachable from the matched positions.
      active = next.iter().flat_map(|&p| self.follow[p].iter().copied()).collect();
      dedup(&mut active);
      allowed_at_start = false;
    }

    // Accept if the model can end here: nothing consumed and nullable, or in a `last` position.
    let accepts = if allowed_at_start {
      self.nullable
    } else {
      // `active` holds where a next child could go; acceptance is whether the last matched
      // position was terminal, which we recompute from the consumed prefix.
      self.ends_after(children)
    };
    if accepts { Ok(()) } else { Err(MatchFailure { at: None, allowed: self.continuations(children) }) }
  }

  /// Whether the sequence leaves the automaton in an accepting position.
  fn ends_after(&self, children: &[NameId]) -> bool {
    let mut active = self.first.clone();
    let mut matched: Vec<usize> = Vec::new();
    for (index, &child) in children.iter().enumerate() {
      let candidates = if index == 0 { self.first.clone() } else { active };
      matched = candidates.into_iter().filter(|&p| self.symbols[p] == child).collect();
      active = matched.iter().flat_map(|&p| self.follow[p].iter().copied()).collect();
      dedup(&mut active);
    }
    matched.iter().any(|p| self.last.contains(p))
  }

  /// The names that could legally follow the given prefix.
  fn continuations(&self, children: &[NameId]) -> Vec<NameId> {
    let mut active = self.first.clone();
    for (index, &child) in children.iter().enumerate() {
      let candidates = if index == 0 { self.first.clone() } else { active };
      let matched: Vec<usize> = candidates.into_iter().filter(|&p| self.symbols[p] == child).collect();
      active = matched.iter().flat_map(|&p| self.follow[p].iter().copied()).collect();
      dedup(&mut active);
    }
    self.symbols_of(&active)
  }

  fn symbols_of(&self, positions: &[usize]) -> Vec<NameId> {
    let mut names: Vec<NameId> = positions.iter().map(|&p| self.symbols[p]).collect();
    names.sort_unstable_by_key(|n| n.index());
    names.dedup();
    names
  }
}

/// Why a child sequence did not match a content model.
#[derive(Debug)]
pub(crate) struct MatchFailure {
  /// The child that did not fit, or `None` if the sequence ended too soon.
  pub(crate) at: Option<NameId>,
  /// The names the model would have accepted at that point.
  pub(crate) allowed: Vec<NameId>,
}

fn has_duplicate_symbol(positions: &[usize], symbols: &[NameId]) -> bool {
  let mut seen: Vec<NameId> = Vec::with_capacity(positions.len());
  for &p in positions {
    if seen.contains(&symbols[p]) {
      return true;
    }
    seen.push(symbols[p]);
  }
  false
}

fn push_unique(target: &mut Vec<usize>, value: usize) {
  if !target.contains(&value) {
    target.push(value);
  }
}

fn extend_unique(target: &mut Vec<usize>, values: &[usize]) {
  for &value in values {
    push_unique(target, value);
  }
}

fn dedup(positions: &mut Vec<usize>) {
  positions.sort_unstable();
  positions.dedup();
}

#[cfg(test)]
mod tests {
  use xenolith_core::name::NamePool;
  use xenolith_parser::dtd::{ContentParticle as P, Occurs::*};

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
  fn detects_nondeterminism() {
    let mut pool = NamePool::new();
    let (a, b) = (pool.intern("a"), pool.intern("b"));
    // (a, b) | (a, c): ambiguous on the leading `a` — Appendix E forbids it.
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
}
