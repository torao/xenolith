//! The one-pass validation run: several application handlers and several validators over one source.

use std::ops::ControlFlow;

use xenolith_core::attr::Attributes;
use xenolith_core::error::Location;
use xenolith_core::name::{NamePool, QName};
use xenolith_dom::{DomSource, build};
use xenolith_parser::Reader;
use xenolith_parser::sax::{Handler, StartElementEvent};
use xenolith_validate::{ErrorListener, Validatable, Validator, ValidityError};

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

fn allowing(names: &[&str]) -> Box<dyn Validator> {
  Box::new(AllowedElements { allowed: names.iter().map(|s| (*s).to_owned()).collect() })
}

/// An application handler that records element names.
#[derive(Default)]
struct Names(Vec<String>);
impl Handler for Names {
  fn start_element(&mut self, event: StartElementEvent<'_>) {
    self.0.push(event.pool.resolve(event.name.local()).to_owned());
  }
}

/// A second application handler that counts elements.
#[derive(Default)]
struct Count(usize);
impl Handler for Count {
  fn start_element(&mut self, _event: StartElementEvent<'_>) {
    self.0 += 1;
  }
}

#[test]
fn several_handlers_and_validators_over_a_reader_in_one_pass() {
  let mut names = Names::default();
  let mut count = Count::default();
  let report = Reader::new("<a><bad/><b/></a>".as_bytes())
    .with_validation()
    .with_handler(&mut names)
    .with_handler(&mut count)
    .with_validator(allowing(&["a", "b"])) // strict: rejects "bad"
    .with_validator(allowing(&["a", "bad", "b"])) // permissive: accepts all three
    .run()
    .unwrap();

  // Both application handlers saw every element in the single pass.
  assert_eq!(names.0, ["a", "bad", "b"]);
  assert_eq!(count.0, 3);
  // Only the strict validator flagged the offending element, so exactly one error.
  assert_eq!(report.errors().len(), 1);
  assert!(report.errors()[0].to_string().contains("bad"));
}

#[test]
fn the_same_pipeline_validates_a_built_dom() {
  let doc = build::parse("<a><bad/></a>".as_bytes()).unwrap();
  let mut names = Names::default();
  let report =
    DomSource::new(&doc).with_validation().with_handler(&mut names).with_validator(allowing(&["a"])).run().unwrap();

  assert_eq!(names.0, ["a", "bad"]);
  assert_eq!(report.errors().len(), 1);
  assert!(report.errors()[0].to_string().contains("bad"));
}

#[cfg(feature = "xml-id")]
#[test]
fn xml_id_checking_defaults_to_the_readers_parser_config() {
  // A default Reader enables xml:id (its ParserConfig), so with_validation() checks it with no explicit call.
  let report = Reader::new("<a xml:id='x'><b xml:id='x'/></a>".as_bytes()).with_validation().run().unwrap();
  assert!(
    report.errors().iter().any(|e| e.to_string().contains("more than once")),
    "the duplicate xml:id should be flagged: {:?}",
    report.errors()
  );
}
