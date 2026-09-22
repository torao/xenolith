//! Validation beside application handlers: several validators combined into one, over one source, in one pass.

use xenolith::dom::DomSource;
use xenolith::dom::build::DomBuilder;
use xenolith::error::{Location, Result};
use xenolith::event::validate::{Ended, Validator, ValidatorSet, ValidityError};
use xenolith::event::{Dispatch, EventCursor, EventHandler, EventRef, EventSource};
use xenolith::io::StreamSource;

/// Reads `xml` into a tree through the parser and the builder.
fn parse_document(xml: &[u8]) -> xenolith::Result<xenolith::dom::Document> {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml).with_handler(&mut builder).emit()?;
  Ok(builder.into_document())
}

/// A schema that allows only the element names it was given, and reports each other name at once.
#[derive(Debug)]
struct AllowedElements {
  allowed: Vec<String>,
  errors: Vec<ValidityError>,
}

impl EventHandler for AllowedElements {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if let EventRef::StartElement(event) = event {
      let local = event.local;
      if !self.allowed.iter().any(|a| a == local) {
        let message = format!("element \"{local}\" is not allowed");
        self.errors.push(ValidityError::new(message, event.location.clone()));
      }
    }
    Ok(())
  }
}

impl Validator for AllowedElements {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}

fn allowing(names: &[&str]) -> Box<dyn Validator> {
  Box::new(AllowedElements { allowed: names.iter().map(|s| (*s).to_owned()).collect(), errors: Vec::new() })
}

/// A validator that reports every element it saw, but only at the end of the document.
#[derive(Debug, Default)]
struct ReportsAtTheEnd {
  seen: Vec<(String, Location)>,
  errors: Vec<ValidityError>,
}

impl EventHandler for ReportsAtTheEnd {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    match event {
      EventRef::StartElement(event) => self.seen.push((event.local.to_owned(), event.location.clone())),
      EventRef::EndDocument => {
        for (name, at) in self.seen.drain(..) {
          self.errors.push(ValidityError::new(format!("late report of \"{name}\""), at));
        }
      }
      _ => {}
    }
    Ok(())
  }
}

impl Validator for ReportsAtTheEnd {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}

/// An application handler that records element names.
#[derive(Default)]
struct Names(Vec<String>);
impl EventHandler for Names {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    let EventRef::StartElement(event) = event else { return Ok(()) };
    self.0.push(event.local.to_owned());
    Ok(())
  }
}

#[test]
fn several_validators_and_handlers_share_one_pass() {
  let mut validation = ValidatorSet::new()
    .with_validator(allowing(&["a", "b"])) // strict: rejects "bad"
    .with_validator(allowing(&["a", "bad", "b"])); // permissive: accepts all three
  let mut names = Names::default();
  {
    let mut lane = Dispatch::new().with_handler(&mut validation).with_handler(&mut names);
    StreamSource::new("<a><bad/><b/></a>".as_bytes()).with_handler(&mut lane).emit().unwrap();
  }

  // The application handler saw every element in the single pass.
  assert_eq!(names.0, ["a", "bad", "b"]);
  // Only the strict validator flagged the offending element, so exactly one error.
  let report = validation.report();
  assert_eq!(report.errors().len(), 1);
  assert!(report.errors()[0].to_string().contains("bad"));
  assert_eq!(report.ended(), Some(Ended::Completed));
}

#[test]
fn the_same_validation_checks_a_built_dom() {
  let doc = parse_document("<a><bad/></a>".as_bytes()).unwrap();
  let mut validation = ValidatorSet::new().with_validator(allowing(&["a"]));
  DomSource::new(&doc).with_handler(&mut validation).emit().unwrap();

  let report = validation.report();
  assert_eq!(report.errors().len(), 1);
  assert!(report.errors()[0].to_string().contains("bad"));
}

#[test]
fn xml_id_is_checked_when_the_validation_is_asked_to() {
  // Checking xml:id is the validation's own policy: the source is not consulted, the caller says so.
  let mut validation = ValidatorSet::new().checking_xml_id(true);
  StreamSource::new("<a xml:id='x'><b xml:id='x'/></a>".as_bytes()).with_handler(&mut validation).emit().unwrap();
  let report = validation.report();
  assert!(
    report.errors().iter().any(|e| e.to_string().contains("more than once")),
    "the duplicate xml:id should be flagged: {:?}",
    report.errors()
  );
}

#[test]
fn errors_are_ordered_by_location_whenever_each_validator_found_them() {
  // `ReportsAtTheEnd` reports `a` and `b` last, at `EndDocument`; `AllowedElements` reports `b` as soon as it arrives.
  let mut validation =
    ValidatorSet::new().with_validator(Box::new(ReportsAtTheEnd::default())).with_validator(allowing(&["a"]));
  StreamSource::new("<a>\n<b/>\n</a>".as_bytes()).with_handler(&mut validation).emit().unwrap();

  let lines: Vec<(u32, String)> =
    validation.report().errors().iter().map(|e| (e.location().line, e.message().to_owned())).collect();
  assert_eq!(
    lines,
    [
      (1, "late report of \"a\"".to_owned()),
      // Both at line 2, in the order the validators were added, whenever each found its error.
      (2, "late report of \"b\"".to_owned()),
      (2, "element \"b\" is not allowed".to_owned()),
    ],
    "by line, and in the order of the validators at the same location"
  );
}

#[test]
fn the_run_stops_at_the_error_limit_with_the_error_that_reached_it() {
  let mut validation = ValidatorSet::new().with_validator(allowing(&["a"])).with_error_limit(2);
  let mut names = Names::default();
  let result = {
    let mut lane = Dispatch::new().with_handler(&mut validation).with_handler(&mut names);
    StreamSource::new("<a><x/><y/><z/></a>".as_bytes()).with_handler(&mut lane).emit()
  };

  let error = result.expect_err("the second error stops the run");
  assert!(error.message().contains("\"y\""), "{error}");
  assert_eq!(names.0, ["a", "x"], "the handler behind the validation never saw the element that reached the limit");
  let report = validation.report();
  assert_eq!(report.errors().len(), 2);
  assert_eq!(report.ended(), Some(Ended::Failed));
  assert!(!report.is_valid());
}

#[test]
fn a_limit_of_one_stops_at_the_first_error() {
  let mut validation = ValidatorSet::new().with_validator(allowing(&["a"])).with_error_limit(1);
  let error = StreamSource::new("<a><x/><y/></a>".as_bytes()).with_handler(&mut validation).emit().unwrap_err();
  assert!(error.message().contains("\"x\""), "{error}");
  assert_eq!(validation.report().errors().len(), 1);
}

/// A validator that records a validity error for the element `stop`, and then refuses it in the same call.
#[derive(Debug, Default)]
struct RecordsThenRefuses {
  errors: Vec<ValidityError>,
}

impl EventHandler for RecordsThenRefuses {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if let EventRef::StartElement(event) = event {
      if event.local == "stop" {
        self.errors.push(ValidityError::new("\"stop\" is recorded before the refusal", event.location.clone()));
        return Err(xenolith::Error::validity("\"stop\" is refused"));
      }
    }
    Ok(())
  }
}

impl Validator for RecordsThenRefuses {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}

#[test]
fn errors_recorded_by_a_validator_that_then_refuses_the_event_are_kept() {
  let mut validation = ValidatorSet::new().with_validator(Box::new(RecordsThenRefuses::default()));
  let error = StreamSource::new("<a><stop/></a>".as_bytes()).with_handler(&mut validation).emit().unwrap_err();

  assert!(error.message().contains("refused"), "the refusal is what the run returns: {error}");
  let report = validation.report();
  assert_eq!(report.errors().len(), 1, "{:?}", report.errors());
  assert!(report.errors()[0].message().contains("recorded before the refusal"));
  assert_eq!(report.ended(), Some(Ended::Failed));
}

#[test]
fn a_limit_of_zero_is_no_limit() {
  let mut validation = ValidatorSet::new().with_validator(allowing(&["a"])).with_error_limit(0);
  StreamSource::new("<a><x/><y/><z/></a>".as_bytes()).with_handler(&mut validation).emit().unwrap();
  assert_eq!(validation.report().errors().len(), 3);
}

#[test]
fn each_document_is_reported_on_its_own() {
  let mut validation = ValidatorSet::new().validating_dtd(true);
  StreamSource::new("<!DOCTYPE a [<!ELEMENT a EMPTY>]><a><b/></a>".as_bytes())
    .with_handler(&mut validation)
    .emit()
    .unwrap();
  assert!(!validation.report().is_valid());

  StreamSource::new("<!DOCTYPE a [<!ELEMENT a EMPTY>]><a/>".as_bytes()).with_handler(&mut validation).emit().unwrap();
  let report = validation.report();
  assert!(report.is_valid(), "the errors of the first document are not carried over: {:?}", report.errors());
}
