//! Validating a built DOM against a schema, by emitting the tree as events into a validator.
//!
//! This is the "validate an existing tree" path: a [`Document`] is walked out with a `DomSource` into a [`Validator`],
//! so the same validator that checks parsed input checks a tree.

use xenolith::dom::DomSource;
use xenolith::dom::build::DomBuilder;
use xenolith::error::Result;
use xenolith::event::validate::{Validator, ValidityError};
use xenolith::event::{EventCursor, EventHandler, EventRef, EventSource};
use xenolith::io::StreamSource;

/// Reads `xml` into a tree through the parser and the builder.
fn parse_document(xml: &[u8]) -> xenolith::Result<xenolith::dom::Document> {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml).with_handler(&mut builder).emit()?;
  Ok(builder.into_document())
}

/// A schema that allows only the element names it was given.
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

fn validate_dom(xml: &str, allowed: &[&str]) -> Vec<String> {
  let doc = parse_document(xml.as_bytes()).unwrap();
  let mut validator =
    AllowedElements { allowed: allowed.iter().map(|s| (*s).to_owned()).collect(), errors: Vec::new() };
  DomSource::new(&doc).with_handler(&mut validator).emit().unwrap();
  validator.errors().iter().map(ToString::to_string).collect()
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
