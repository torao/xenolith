//! Validating a built DOM against a schema, by emitting the tree as events into a validator.
//!
//! This is the "validate an existing tree" path: a [`Document`] is emitted with a `DomSource` into a
//! [`ValidatingHandler`], so the same [`Validator`] that checks parsed input checks a tree.

use std::ops::ControlFlow;

use xenolith_core::attr::Attributes;
use xenolith_core::error::Location;
use xenolith_core::name::{NamePool, QName};
use xenolith_dom::{DomSource, build};
use xenolith_parser::sax::EventSource;
use xenolith_validate::{CollectErrors, ErrorListener, ValidatingHandler, Validator, ValidityError};

/// A schema that allows only the element names it was given.
struct AllowedElements {
  allowed: Vec<String>,
}

impl Validator for AllowedElements {
  fn start_element(
    &mut self,
    name: QName,
    _attributes: Attributes<'_>,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    let local = pool.resolve(name.local());
    if self.allowed.iter().any(|a| a == local) {
      ControlFlow::Continue(())
    } else {
      errors.report(ValidityError::new(format!("element \"{local}\" is not allowed"), at.clone()))
    }
  }

  fn characters(&mut self, _text: &str, _ws: bool, _at: &Location, _errors: &mut dyn ErrorListener) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }

  fn end_element(
    &mut self,
    _name: QName,
    _pool: &NamePool,
    _at: &Location,
    _errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }

  fn finish(&mut self, _errors: &mut dyn ErrorListener) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }
}

fn validate_dom(xml: &str, allowed: &[&str]) -> Vec<String> {
  let doc = build::parse(xml.as_bytes()).unwrap();
  let validator = AllowedElements { allowed: allowed.iter().map(|s| (*s).to_owned()).collect() };
  let mut errors = CollectErrors::default();
  {
    let mut handler = ValidatingHandler::new(&mut errors).with_validator(Box::new(validator));
    DomSource::new(&doc).emit(&mut handler).unwrap();
  }
  errors.errors().iter().map(ToString::to_string).collect()
}

#[test]
fn a_conforming_tree_reports_nothing() {
  let errors = validate_dom("<a><b/><b/></a>", &["a", "b"]);
  assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_tree_with_a_disallowed_element_is_reported() {
  let errors = validate_dom("<a><b/><c/></a>", &["a", "b"]);
  assert_eq!(errors.len(), 1);
  assert!(errors[0].contains('c'), "{errors:?}");
}
