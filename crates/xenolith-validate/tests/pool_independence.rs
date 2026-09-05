//! A `DtdValidator` checks a source that interns its names somewhere else.
//!
//! The DTD is keyed by the `NameId`s of the pool it was parsed in. A source that did its own interning, a walk over a
//! tree built earlier among them, hands over names from a different pool, and the same lexical name has a different id
//! there. The validator owns the DTD's pool and interns what arrives into it, so the two never get compared by id
//! across pools.

use std::ops::ControlFlow;

use xenolith_core::attr::{AttributeList, AttributeRef, Attributes};
use xenolith_core::error::Location;
use xenolith_core::name::{NameId, NamePool, QName};
use xenolith_core::validate::{CollectErrors, Validator};
use xenolith_parser::Reader;
use xenolith_parser::dtd::Dtd;
use xenolith_parser::sax::{DoctypeEvent, EventSource, Handler};
use xenolith_validate::dtd::DtdValidator;

/// Captures the DTD and the pool it was parsed in, as `ValidatingHandler` does at the `DOCTYPE`.
#[derive(Default)]
struct CaptureDtd {
  dtd: Option<Dtd>,
  pool: Option<NamePool>,
  root: Option<NameId>,
}

impl Handler for CaptureDtd {
  fn doctype(&mut self, event: DoctypeEvent<'_>) {
    self.root = event.name.and_then(|name| event.pool.get(name));
    self.dtd = Some(event.dtd.clone());
    self.pool = Some(event.pool.clone());
  }
  fn should_continue(&self) -> bool {
    self.dtd.is_none() // the DTD is all this wants
  }
}

/// An attribute list built by hand, for driving a validator without a parser.
struct Attrs(Vec<(QName, String)>);

impl AttributeList for Attrs {
  fn len(&self) -> usize {
    self.0.len()
  }
  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    let (name, value) = self.0.get(index)?;
    Some(AttributeRef { name: *name, value, declares_namespace: false })
  }
}

/// Builds a validator from `xml`'s DTD, and a pool of its own with nothing in common with the parser's.
fn validator_and_foreign_pool(xml: &str) -> (DtdValidator, NamePool) {
  let mut capture = CaptureDtd::default();
  Reader::new(xml.as_bytes()).emit(&mut capture).expect("well-formed");
  let validator = DtdValidator::new(
    capture.dtd.expect("a DTD"),
    capture.pool.expect("the pool it was parsed in"),
    Some(capture.root.expect("a DOCTYPE name")),
  );

  // A pool of its own. Interning in a different order gives the same names different ids, which is the whole point.
  let mut foreign = NamePool::new();
  for name in ["z", "y", "x", "w", "v", "u"] {
    foreign.intern(name);
  }
  (validator, foreign)
}

fn element(pool: &mut NamePool, name: &str) -> QName {
  QName::new(None, None, pool.intern(name))
}

#[test]
fn a_valid_document_from_another_pool_is_accepted() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY>]><a><b/></a>";
  let (mut validator, mut pool) = validator_and_foreign_pool(dtd);

  let a = element(&mut pool, "a");
  let b = element(&mut pool, "b");
  assert_ne!(a.local(), b.local());

  let mut errors = CollectErrors::default();
  let at = Location::unknown();
  let empty = Attrs(Vec::new());
  let none = Attributes::new(&empty);

  assert_eq!(validator.start_element(a, none, &pool, &at, &mut errors), ControlFlow::Continue(()));
  assert_eq!(validator.start_element(b, none, &pool, &at, &mut errors), ControlFlow::Continue(()));
  let _ = validator.end_element(b, &pool, &at, &mut errors);
  let _ = validator.end_element(a, &pool, &at, &mut errors);
  let _ = validator.finish(&mut errors);

  let reported: Vec<String> = errors.errors().iter().map(ToString::to_string).collect();
  assert!(reported.is_empty(), "a document that follows its DTD must pass whatever pool it arrives in: {reported:?}");
}

#[test]
fn a_violation_from_another_pool_is_still_caught() {
  // `a` is declared `(b)`, so a `c` inside it breaks the content model. Catching this proves the names really were
  // matched against the DTD rather than silently missing each other across pools.
  let dtd = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><a><b/></a>";
  let (mut validator, mut pool) = validator_and_foreign_pool(dtd);

  let a = element(&mut pool, "a");
  let c = element(&mut pool, "c");

  let mut errors = CollectErrors::default();
  let at = Location::unknown();
  let empty = Attrs(Vec::new());
  let none = Attributes::new(&empty);

  let _ = validator.start_element(a, none, &pool, &at, &mut errors);
  let _ = validator.start_element(c, none, &pool, &at, &mut errors);
  let _ = validator.end_element(c, &pool, &at, &mut errors);
  let _ = validator.end_element(a, &pool, &at, &mut errors);
  let _ = validator.finish(&mut errors);

  let reported: Vec<String> = errors.errors().iter().map(ToString::to_string).collect();
  assert!(reported.iter().any(|e| e.contains("\"c\"")), "the offending element must be named: {reported:?}");
}

#[test]
fn an_undeclared_element_from_another_pool_is_reported() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a ANY>]><a/>";
  let (mut validator, mut pool) = validator_and_foreign_pool(dtd);

  let a = element(&mut pool, "a");
  let ghost = element(&mut pool, "ghost");

  let mut errors = CollectErrors::default();
  let at = Location::unknown();
  let empty = Attrs(Vec::new());
  let none = Attributes::new(&empty);

  let _ = validator.start_element(a, none, &pool, &at, &mut errors);
  let _ = validator.start_element(ghost, none, &pool, &at, &mut errors);
  let _ = validator.end_element(ghost, &pool, &at, &mut errors);
  let _ = validator.end_element(a, &pool, &at, &mut errors);
  let _ = validator.finish(&mut errors);

  let reported: Vec<String> = errors.errors().iter().map(ToString::to_string).collect();
  assert!(
    reported.iter().any(|e| e.contains("ghost") && e.contains("not declared")),
    "a name the DTD never declared is reported, not passed over: {reported:?}"
  );
}
