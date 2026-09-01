//! Running handlers and validators over a source in one pass.
//!
//! A [`ValidatingSource`] gathers application [`Handler`]s and [`Validator`]s, then drives an [`EventSource`] once, feeding
//! every event to all of them. So an application reads a document while it is validated, with no second pass, from any
//! source that produces parser events (a [`Reader`](xenolith_parser::Reader), or a tree through the DOM crate's
//! `DomSource`).
//!
//! Start one from a source with [`Validatable::with_validation`], then add handlers, validators, and schemas:
//!
//! ```no_run
//! use xenolith_validate::Validatable;
//! # fn f(reader: xenolith_parser::Reader<&[u8]>, schema: &dyn xenolith_validate::Schema) -> xenolith_core::Result<()> {
//! let report = reader
//!     .with_validation()
//!     .with_schema(schema)   // validate against an application schema
//!     .validating_dtd()      // and the document's own declared DTD
//!     .run()?;
//! assert!(report.errors().is_empty());
//! # Ok(()) }
//! ```

use xenolith_core::error::Result;
use xenolith_parser::sax::{EventSource, Handler};

use crate::{CollectErrors, Schema, ValidatingHandler, Validator, ValidityError};

/// The outcome of validating a document.
#[derive(Debug)]
pub struct Report {
  errors: Vec<ValidityError>,
  had_dtd: bool,
}

impl Report {
  /// Assembles a report from the errors gathered and whether a DTD was validated against.
  fn new(errors: Vec<ValidityError>, had_dtd: bool) -> Self {
    Self { errors, had_dtd }
  }

  /// True if the document is valid: it had a DTD and broke none of its constraints.
  ///
  /// A document with no `DOCTYPE` has nothing to be valid against; see [`had_dtd`](Self::had_dtd).
  #[must_use]
  pub fn is_valid(&self) -> bool {
    self.had_dtd && self.errors.is_empty()
  }

  /// True if the document declared a DTD to validate against.
  #[must_use]
  pub fn had_dtd(&self) -> bool {
    self.had_dtd
  }

  /// The validity errors found, in document order.
  #[must_use]
  pub fn errors(&self) -> &[ValidityError] {
    &self.errors
  }
}

/// A one-pass run of handlers and validators over a source.
///
/// It holds the application handlers (borrowed) and the validators (owned), reports validity errors to a listener, and
/// [`run`](Self::run) returns a [`Report`]. Start it with [`Validatable::with_validation`], add handlers and
/// validators, then call [`run`](Self::run).
///
pub struct ValidatingSource<'h, S: EventSource> {
  source: S,
  handlers: Vec<&'h mut dyn Handler>,
  validators: Vec<Box<dyn Validator>>,
  use_dtd: bool,
  #[cfg(feature = "xml-id")]
  xml_id: bool,
  errors: CollectErrors,
}

impl<S: EventSource> std::fmt::Debug for ValidatingSource<'_, S> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ValidatingSource")
      .field("handlers", &self.handlers.len())
      .field("validators", &self.validators.len())
      .field("use_dtd", &self.use_dtd)
      .finish_non_exhaustive()
  }
}

impl<'h, S: EventSource> ValidatingSource<'h, S> {
  /// Starts a run over `source` with nothing attached yet.
  fn new(source: S) -> Self {
    Self {
      // xml:id checking defaults to the source's parser configuration, so a Reader configured for xml:id validates it
      // without an explicit call. A source with no parser (a tree) leaves it off. Override with `checking_xml_id`.
      #[cfg(feature = "xml-id")]
      xml_id: source.parser_config().is_some_and(|config| config.xml_id),
      source,
      handlers: Vec::new(),
      validators: Vec::new(),
      use_dtd: false,
      errors: CollectErrors::default(),
    }
  }

  /// Adds an application handler. It receives each event after the handlers already added.
  ///
  #[must_use]
  pub fn with_handler(mut self, handler: &'h mut dyn Handler) -> Self {
    self.handlers.push(handler);
    self
  }

  /// Adds application handlers, in order.
  ///
  #[must_use]
  pub fn with_handlers(mut self, handlers: impl IntoIterator<Item = &'h mut dyn Handler>) -> Self {
    self.handlers.extend(handlers);
    self
  }

  /// Adds a validator.
  ///
  #[must_use]
  pub fn with_validator(mut self, validator: Box<dyn Validator>) -> Self {
    self.validators.push(validator);
    self
  }

  /// Adds validators.
  ///
  #[must_use]
  pub fn with_validators(mut self, validators: impl IntoIterator<Item = Box<dyn Validator>>) -> Self {
    self.validators.extend(validators);
    self
  }

  /// Adds a validator built from `schema`.
  ///
  #[must_use]
  pub fn with_schema(mut self, schema: &dyn Schema) -> Self {
    self.validators.push(schema.validator());
    self
  }

  /// Also validates against the document's own declared DTD, the counterpart of Java's `setValidating(true)`.
  ///
  #[must_use]
  pub fn validating_dtd(mut self) -> Self {
    self.use_dtd = true;
    self
  }

  /// Sets whether `xml:id` attributes are checked as IDs, overriding the default taken from the source's parser
  /// configuration.
  ///
  #[cfg(feature = "xml-id")]
  #[must_use]
  pub fn checking_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// Drives the source once, feeding every event to the handlers and validators, and returns the validation report.
  ///
  /// # Errors
  ///
  /// Returns the source's error if the input is not well-formed or reading fails.
  ///
  pub fn run(self) -> Result<Report> {
    // Move every field out, so broadcasting can consume `source` without a partial move of `self` fighting the borrow
    // of `errors`.
    let ValidatingSource {
      source,
      handlers,
      validators,
      use_dtd,
      #[cfg(feature = "xml-id")]
      xml_id,
      mut errors,
    } = self;

    // Gather the validators and toggles into one validation lane.
    let mut validating = ValidatingHandler::new(&mut errors);
    for validator in validators {
      validating = validating.with_validator(validator);
    }
    if use_dtd {
      validating = validating.with_document_dtd(true);
    }
    #[cfg(feature = "xml-id")]
    if xml_id {
      validating = validating.with_xml_id(true);
    }

    // Broadcast every event to the validation lane first, then the application handlers, so each application handler is
    // called only after the event has been checked against the schema. The handlers are added one at a time so each
    // borrow shortens to the run's lifetime, letting the borrowed `validating` sit beside them.
    let mut broadcast = source.broadcast().with_handler(&mut validating);
    for handler in handlers {
      broadcast = broadcast.with_handler(handler);
    }
    broadcast.run()?;

    // `validating`'s last use ends its borrow of `errors`, so the report can take the errors back.
    let had_dtd = validating.had_dtd();
    Ok(Report::new(errors.take(), had_dtd))
  }
}

/// The entry point that starts a [`ValidatingSource`] from any [`EventSource`].
///
/// Bring it into scope to call [`with_validation`](Self::with_validation) on a source, for example, a
/// [`Reader`](xenolith_parser::Reader) or the DOM crate's `DomSource`. It adds no behavior of its own; the one method
/// moves the source into a [`ValidatingSource`], where the builder methods live.
///
pub trait Validatable: EventSource + Sized {
  /// Starts a validation run over this source. Add handlers, validators, and schemas to the returned
  /// [`ValidatingSource`], then [`run`](ValidatingSource::run) it.
  fn with_validation<'h>(self) -> ValidatingSource<'h, Self> {
    ValidatingSource::new(self)
  }
}

impl<T: EventSource> Validatable for T {}
