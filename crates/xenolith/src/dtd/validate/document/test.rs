use crate::event::{Dispatch, EventCursor, EventSource};
use crate::io::StreamSource;

use super::*;

/// A validator that faults every element named `bad`, standing for a schema of the caller's own.
#[derive(Debug, Default)]
struct RejectBad {
  errors: Vec<ValidityError>,
  finished: bool,
}

impl EventHandler for RejectBad {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    match event {
      EventRef::StartElement(event) if event.local == "bad" => {
        self.errors.push(ValidityError::new("element \"bad\" is not allowed", event.location.clone()));
      }
      EventRef::EndDocument => self.finished = true,
      _ => {}
    }
    Ok(())
  }
}

impl Validator for RejectBad {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}

#[test]
fn a_validator_of_the_callers_own_checks_while_parsing() {
  let mut validator = RejectBad::default();
  StreamSource::new("<a><bad/><ok/></a>".as_bytes()).with_handler(&mut validator).emit().unwrap();

  assert_eq!(validator.errors().len(), 1);
  assert!(validator.errors()[0].to_string().contains("bad"));
  assert!(validator.finished, "the end of the document reached the validator");
}

#[test]
fn validates_the_documents_own_dtd() {
  // The validator is built from the DOCTYPE, so an undeclared element is a validity error.
  let mut validating = DocumentDtd::new();
  StreamSource::new("<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>".as_bytes())
    .with_handler(&mut validating)
    .emit()
    .unwrap();

  assert!(validating.had_dtd(), "the document declared a DTD");
  assert!(validating.errors().iter().any(|e| e.to_string().contains("c")), "{:?}", validating.errors());
}

#[test]
fn a_document_with_no_doctype_has_no_dtd() {
  let mut validating = DocumentDtd::new();
  StreamSource::new("<a><b/></a>".as_bytes()).with_handler(&mut validating).emit().unwrap();

  assert!(!validating.had_dtd());
  assert!(validating.errors().is_empty());
}

#[test]
fn a_validity_error_does_not_refuse_the_document() {
  // Validity is recoverable: the run reaches the end and the errors are read back, where a well-formedness error
  // would have stopped the source.
  let mut validating = DocumentDtd::new();
  let result =
    StreamSource::new("<!DOCTYPE a [<!ELEMENT a EMPTY>]><a><b/></a>".as_bytes()).with_handler(&mut validating).emit();

  assert!(result.is_ok(), "{result:?}");
  assert!(!validating.errors().is_empty());
}

#[test]
fn runs_beside_an_application_handler_in_one_pass() {
  // The application handler collects names while the validator checks the same events.
  #[derive(Default)]
  struct Names(Vec<String>);
  impl EventHandler for Names {
    fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
      if let EventRef::StartElement(event) = event {
        self.0.push(event.local.to_owned());
      }
      Ok(())
    }
  }

  let mut names = Names::default();
  let mut validator = RejectBad::default();
  {
    let mut both = Dispatch::new().with_handler(&mut names).with_handler(&mut validator);
    StreamSource::new("<a><bad/></a>".as_bytes()).with_handler(&mut both).emit().unwrap();
  }

  // The application saw every element, in one pass.
  assert_eq!(names.0, ["a", "bad"]);
  // The validator flagged the offending one.
  assert_eq!(validator.errors().len(), 1);
}

#[test]
fn an_error_carries_where_it_happened() {
  let mut validating = DocumentDtd::new();
  StreamSource::new("<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>".as_bytes())
    .with_handler(&mut validating)
    .emit()
    .unwrap();

  let errors = validating.errors();
  let error = errors.first().expect("one error");
  assert!(!error.location().is_unknown(), "the error says where in the document it is");
}
