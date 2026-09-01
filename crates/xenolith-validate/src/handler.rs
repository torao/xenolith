//! Running one or more [`Validator`]s as a push [`Handler`].
//!
//! A [`Validator`] checks an event stream, whatever its source. [`ValidatingHandler`] is the adapter that feeds it
//! from a parser: it implements the parser's push [`Handler`] and turns each callback into the matching validator call.
//! Run it alongside an application's own handler with
//! [`EventSource::broadcast`](xenolith_parser::sax::EventSource::broadcast), and validation runs in the same pass as
//! the application's processing, so the application is called with data already checked against the schema.
//! [`Validatable::with_validation`](crate::Validatable::with_validation) sets this up for you.
//!
//! A handler validates against one of two things. A [`new`](ValidatingHandler::new) handler checks an explicit
//! [`Validator`], one the caller built (from a [`Schema`](crate::Schema), say). A
//! [`for_document_dtd`](ValidatingHandler::for_document_dtd) handler checks the document's declared DTD, which it
//! builds when it reads the `DOCTYPE`, the counterpart of Java's `setValidating(true)`.
//!
//! Validity errors go to the [`ErrorListener`] the handler was given, not up the parse as a fatal error, since a
//! validity error is recoverable. A listener that returns [`ControlFlow::Break`](std::ops::ControlFlow::Break) stops the
//! run at the next check.
//!

use xenolith_core::chars;
use xenolith_parser::sax::{CdataEvent, CharactersEvent, DoctypeEvent, EndElementEvent, Handler, StartElementEvent};

use crate::dtd::DtdValidator;
#[cfg(feature = "xml-id")]
use crate::ids::XmlIdValidator;
use crate::{ErrorListener, Validator};

/// Drives one or more [`Validator`]s from the parser's push [`Handler`] callbacks.
///
/// It owns the validators and borrows the [`ErrorListener`] where errors go. The per-element and character checks run
/// as the events arrive; the whole-document checks run from [`end_document`](Handler::end_document), which the parser
/// calls once the document is read in full. If another handler or the listener stops a run early, it doesn't reach
/// that final callback, so the whole-document checks are skipped.
///
/// A validity error is reported to the listener, never raised as a parse error, so a handler callback never fails.
///
pub struct ValidatingHandler<'e> {
  errors: &'e mut dyn ErrorListener,
  validators: Vec<Box<dyn Validator>>,
  /// Whether to build a validator from the document's own declared DTD when the `DOCTYPE` is read.
  use_dtd: bool,
  /// Whether `xml:id` attributes are checked as IDs.
  #[cfg(feature = "xml-id")]
  xml_id: bool,
  had_dtd: bool,
  /// Whether the one-time lazy setup at the first content event has run.
  lazy_done: bool,
  stopped: bool,
}

impl<'e> ValidatingHandler<'e> {
  /// Creates a handler that reports to `errors`, with no validators yet. Add them with the builder methods.
  ///
  #[must_use]
  pub fn new(errors: &'e mut dyn ErrorListener) -> Self {
    Self {
      errors,
      validators: Vec::new(),
      use_dtd: false,
      #[cfg(feature = "xml-id")]
      xml_id: false,
      had_dtd: false,
      lazy_done: false,
      stopped: false,
    }
  }

  /// Adds an explicit validator, one the caller built, for example from a [`Schema`](crate::Schema).
  ///
  #[must_use]
  pub fn with_validator(mut self, validator: Box<dyn Validator>) -> Self {
    self.validators.push(validator);
    self
  }

  /// When `on` is true, it also checks the document's declared DTD, built when it reads the `DOCTYPE`, the counterpart
  /// to Java's `setValidating(true)`. A document with no `DOCTYPE` is not checked against a DTD; see
  /// [`had_dtd`](Self::had_dtd).
  ///
  #[must_use]
  pub fn with_document_dtd(mut self, on: bool) -> Self {
    self.use_dtd = on;
    self
  }

  /// A handler that checks only the document's own declared DTD.
  ///
  #[must_use]
  pub fn for_document_dtd(errors: &'e mut dyn ErrorListener) -> Self {
    Self::new(errors).with_document_dtd(true)
  }

  /// Also checks `xml:id` attributes as IDs. With a DTD, this falls into the DTD validator's ID space; without a DTD,
  /// it stands on its own. Off by default.
  ///
  #[cfg(feature = "xml-id")]
  #[must_use]
  pub fn with_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// Whether a validator was built from the document's declared DTD. Meaningful after the run.
  ///
  #[must_use]
  pub fn had_dtd(&self) -> bool {
    self.had_dtd
  }

  /// One-time setup at the first content event: with `xml:id` on and no DTD, add a standalone `xml:id` validator.
  ///
  fn ensure_lazy(&mut self) {
    if self.lazy_done {
      return;
    }
    self.lazy_done = true;
    #[cfg(feature = "xml-id")]
    if self.xml_id && !self.had_dtd {
      self.validators.push(Box::new(XmlIdValidator::new()));
    }
  }
}

impl std::fmt::Debug for ValidatingHandler<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ValidatingHandler")
      .field("validators", &self.validators.len())
      .field("had_dtd", &self.had_dtd)
      .field("stopped", &self.stopped)
      .finish_non_exhaustive()
  }
}

impl Handler for ValidatingHandler<'_> {
  fn doctype(&mut self, event: DoctypeEvent<'_>) {
    if self.use_dtd {
      // The schema is the document's own DTD, complete by this event.
      if let Some(root) = event.name.and_then(|name| event.pool.get(name)) {
        let validator = DtdValidator::new(event.dtd.clone(), root);
        #[cfg(feature = "xml-id")]
        let validator = validator.with_xml_id(self.xml_id);
        self.validators.push(Box::new(validator));
        self.had_dtd = true;
      }
    }
  }

  fn start_element(&mut self, event: StartElementEvent<'_>) {
    self.ensure_lazy();
    let mut stop = false;
    {
      let errors = &mut *self.errors;
      for validator in &mut self.validators {
        stop |= validator.start_element(event.name, event.attributes, event.pool, &event.location, errors).is_break();
      }
    }
    self.stopped |= stop;
  }

  fn end_element(&mut self, event: EndElementEvent<'_>) {
    self.ensure_lazy();
    let mut stop = false;
    {
      let errors = &mut *self.errors;
      for validator in &mut self.validators {
        stop |= validator.end_element(event.name, event.pool, &event.location, errors).is_break();
      }
    }
    self.stopped |= stop;
  }

  fn characters(&mut self, event: CharactersEvent<'_>) {
    self.ensure_lazy();
    let whitespace_only = event.text.chars().all(chars::is_whitespace);
    let mut stop = false;
    {
      let errors = &mut *self.errors;
      for validator in &mut self.validators {
        stop |= validator.characters(event.text, whitespace_only, &event.location, errors).is_break();
      }
    }
    self.stopped |= stop;
  }

  fn cdata(&mut self, event: CdataEvent<'_>) {
    self.ensure_lazy();
    // A CDATA section is significant character data, never ignorable whitespace.
    let mut stop = false;
    {
      let errors = &mut *self.errors;
      for validator in &mut self.validators {
        stop |= validator.characters(event.text, false, &event.location, errors).is_break();
      }
    }
    self.stopped |= stop;
  }

  fn end_document(&mut self) {
    let mut stop = false;
    {
      let errors = &mut *self.errors;
      for validator in &mut self.validators {
        stop |= validator.finish(errors).is_break();
      }
    }
    self.stopped |= stop;
  }

  fn should_continue(&self) -> bool {
    !self.stopped
  }
}

#[cfg(test)]
mod tests {
  use std::ops::ControlFlow;

  use xenolith_core::attr::Attributes;
  use xenolith_core::error::Location;
  use xenolith_core::name::{NamePool, QName};
  use xenolith_parser::Reader;
  use xenolith_parser::sax::{EventSource, Handler, StartElementEvent};

  use super::*;
  use crate::{CollectErrors, ValidityError};

  /// A validator that faults every element named `bad` and records that `finish` ran.
  #[derive(Default)]
  struct RejectBad {
    finished: bool,
  }

  impl Validator for RejectBad {
    fn start_element(
      &mut self,
      name: QName,
      _attributes: Attributes<'_>,
      pool: &NamePool,
      at: &Location,
      errors: &mut dyn ErrorListener,
    ) -> ControlFlow<()> {
      if pool.resolve(name.local()) == "bad" {
        return errors.report(ValidityError::new("element \"bad\" is not allowed", at.clone()));
      }
      ControlFlow::Continue(())
    }

    fn characters(
      &mut self,
      _text: &str,
      _ws: bool,
      _at: &Location,
      _errors: &mut dyn ErrorListener,
    ) -> ControlFlow<()> {
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
      self.finished = true;
      ControlFlow::Continue(())
    }
  }

  #[test]
  fn validates_an_explicit_validator_while_parsing() {
    let mut errors = CollectErrors::default();
    {
      let mut handler = ValidatingHandler::new(&mut errors).with_validator(Box::<RejectBad>::default());
      Reader::new("<a><bad/><ok/></a>".as_bytes()).emit(&mut handler).unwrap();
    }
    assert_eq!(errors.errors().len(), 1);
    assert!(errors.errors()[0].to_string().contains("bad"));
  }

  #[test]
  fn validates_the_documents_own_dtd() {
    // The handler builds the validator from the DOCTYPE, so an undeclared element is a validity error.
    let mut errors = CollectErrors::default();
    let had_dtd;
    {
      let mut handler = ValidatingHandler::for_document_dtd(&mut errors);
      Reader::new("<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>".as_bytes()).emit(&mut handler).unwrap();
      had_dtd = handler.had_dtd();
    }
    assert!(had_dtd, "the document declared a DTD");
    assert!(errors.errors().iter().any(|e| e.to_string().contains("c")), "{:?}", errors.errors());
  }

  #[test]
  fn a_document_with_no_doctype_has_no_dtd() {
    let mut errors = CollectErrors::default();
    let had_dtd;
    {
      let mut handler = ValidatingHandler::for_document_dtd(&mut errors);
      Reader::new("<a><b/></a>".as_bytes()).emit(&mut handler).unwrap();
      had_dtd = handler.had_dtd();
    }
    assert!(!had_dtd);
    assert!(errors.errors().is_empty());
  }

  #[test]
  fn runs_beside_an_application_handler_in_one_pass() {
    // The application handler collects names while the validator checks the same events.
    #[derive(Default)]
    struct Names(Vec<String>);
    impl Handler for Names {
      fn start_element(&mut self, event: StartElementEvent<'_>) {
        self.0.push(event.pool.resolve(event.name.local()).to_owned());
      }
    }

    let mut errors = CollectErrors::default();
    let mut names = Names::default();
    {
      let mut validating = ValidatingHandler::new(&mut errors).with_validator(Box::<RejectBad>::default());
      Reader::new("<a><bad/></a>".as_bytes())
        .broadcast()
        .with_handler(&mut names)
        .with_handler(&mut validating)
        .run()
        .unwrap();
    }
    // The application saw every element, in one pass.
    assert_eq!(names.0, ["a", "bad"]);
    // The validator flagged the offending one.
    assert_eq!(errors.errors().len(), 1);
  }
}
