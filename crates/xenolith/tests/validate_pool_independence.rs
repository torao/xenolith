//! A `DtdValidator` checks a source whose names it never interned.
//!
//! The DTD is keyed by the `NameId`s of the pool it was parsed in. An event carries a name as text, so the validator
//! interns the form it was written in into its own pool and looks that up. No id crosses from one pool to another, and
//! a source that did its own interning, a walk over a tree built earlier among them, is checked the same as the parser.

use xenolith::attr::{AttributeList, AttributeRef, Attributes};
use xenolith::dtd::Dtd;
use xenolith::dtd::validate::DtdValidator;
use xenolith::error::{Location, Result};
use xenolith::event::validate::Validator;
use xenolith::event::{
  EndElementEventRef, EventCursor, EventHandler, EventRef, EventSource, StartElementEventRef, XmlSpace,
};
use xenolith::io::StreamSource;
use xenolith::name::{NameId, NamePool};

/// Captures the DTD and the pool it was parsed in, as `DocumentDtd` does at the `DOCTYPE`.
#[derive(Default)]
struct CaptureDtd {
  dtd: Option<Dtd>,
  pool: Option<NamePool>,
  root: Option<NameId>,
}

impl EventHandler for CaptureDtd {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    let EventRef::Doctype(event) = event else { return Ok(()) };
    self.root = event.name.and_then(|name| event.pool.get(name));
    self.dtd = Some(event.dtd.clone());
    self.pool = Some(event.pool.fork());
    Ok(())
  }

  fn should_continue(&self) -> bool {
    self.dtd.is_none() // the DTD is all this wants
  }
}

/// A start tag's attributes held as the parts of their names, for driving a validator without a parser.
struct Attrs(Vec<(Option<&'static str>, &'static str)>);

impl AttributeList for Attrs {
  fn len(&self) -> usize {
    self.0.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    let (prefix, local) = self.0.get(index)?;
    Some(AttributeRef {
      prefix: *prefix,
      local,
      namespace: None,
      value: "v",
      location: Location::unknown(),
      value_location: Location::unknown(),
    })
  }
}

/// Builds a validator from `xml`'s DTD, whose pool holds every name the document declared.
fn validator_for(xml: &str) -> DtdValidator {
  let mut capture = CaptureDtd::default();
  StreamSource::new(xml.as_bytes()).with_handler(&mut capture).emit().expect("well-formed");
  DtdValidator::new(
    capture.dtd.expect("a DTD"),
    capture.pool.expect("the pool it was parsed in"),
    Some(capture.root.expect("a DOCTYPE name")),
  )
}

/// Hands the validator a start element event, as a source would. A validity error is kept rather than returned, so
/// this only fails if the validator refused the document outright.
fn start(validator: &mut DtdValidator, local: &str, attributes: &Attrs) {
  let event = EventRef::StartElement(StartElementEventRef::new(
    None,
    local,
    None,
    Attributes::new(attributes),
    XmlSpace::Default,
    None,
    None,
    Location::unknown(),
  ));
  validator.handle(&event).expect("a validity error does not refuse the document");
}

/// Hands the validator an end element event.
fn end(validator: &mut DtdValidator, local: &str) {
  let event = EventRef::EndElement(EndElementEventRef::new(None, local, None, Location::unknown()));
  validator.handle(&event).expect("a validity error does not refuse the document");
}

/// Ends the document, which is what runs the whole-document checks.
fn finish(validator: &mut DtdValidator) {
  validator.handle(&EventRef::EndDocument).expect("a validity error does not refuse the document");
}

/// The validator's errors as text.
fn reported(validator: &DtdValidator) -> Vec<String> {
  validator.errors().iter().map(ToString::to_string).collect()
}

#[test]
fn a_valid_document_from_another_source_is_accepted() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY>]><a><b/></a>";
  let mut validator = validator_for(dtd);
  let empty = Attrs(Vec::new());

  start(&mut validator, "a", &empty);
  start(&mut validator, "b", &empty);
  end(&mut validator, "b");
  end(&mut validator, "a");
  finish(&mut validator);

  let reported = reported(&validator);
  assert!(reported.is_empty(), "a document that follows its DTD must pass whatever interned its names: {reported:?}");
}

#[test]
fn a_violation_from_another_source_is_still_caught() {
  // `a` is declared `(b)`, so a `c` inside it breaks the content model. Catching this proves the names really were
  // matched against the DTD rather than silently missing each other.
  let dtd = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><a><b/></a>";
  let mut validator = validator_for(dtd);
  let empty = Attrs(Vec::new());

  start(&mut validator, "a", &empty);
  start(&mut validator, "c", &empty);
  end(&mut validator, "c");
  end(&mut validator, "a");
  finish(&mut validator);

  let reported = reported(&validator);
  assert!(reported.iter().any(|e| e.contains("\"c\"")), "the offending element must be named: {reported:?}");
}

#[test]
fn an_undeclared_element_from_another_source_is_reported() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a ANY>]><a/>";
  let mut validator = validator_for(dtd);
  let empty = Attrs(Vec::new());

  start(&mut validator, "a", &empty);
  start(&mut validator, "ghost", &empty);
  end(&mut validator, "ghost");
  end(&mut validator, "a");
  finish(&mut validator);

  let reported = reported(&validator);
  assert!(
    reported.iter().any(|e| e.contains("ghost") && e.contains("not declared")),
    "a name the DTD never declared is reported, not passed over: {reported:?}"
  );
}

#[test]
fn an_attribute_is_looked_up_by_the_form_it_was_written_in() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a EMPTY><!ATTLIST a x CDATA #IMPLIED>]><a x='v'/>";
  let mut validator = validator_for(dtd);

  start(&mut validator, "a", &Attrs(vec![(None, "x")]));
  end(&mut validator, "a");
  finish(&mut validator);

  assert!(reported(&validator).is_empty(), "{:?}", reported(&validator));
}
