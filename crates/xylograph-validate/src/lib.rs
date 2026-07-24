//! Validation for xylograph.
//!
//! Validation is not only the DTD. This crate holds a schema-agnostic contract — a
//! [`Validator`] driven by the parser's events, reporting through an [`ErrorListener`] — and
//! the DTD validator as its first implementation. A validator for RELAX NG or XML Schema, or a
//! caller's own rules, is any other type that implements the same trait; see `ROADMAP.md`,
//! decision 8.
//!
//! # The boundary
//!
//! What a validator does is check constraints over the event stream. It does *not* expand
//! entities, supply attribute defaults, or type-check the way parsing does — those belong to
//! [`xylograph_parser`] and are already done by the time an event arrives. That is what lets
//! a namespace-aware schema (RELAX NG, XSD) or a custom rule sit on the same interface as the
//! DTD: they all need only the events and the names.
//!
//! # Well-formed versus valid
//!
//! A well-formedness error is fatal and stops parsing; the parser raises those. A *validity*
//! error is recoverable — the document is still a tree, just not one the schema allows — so a
//! validator reports it and carries on, and the [`ErrorListener`] decides whether to keep
//! going. This mirrors Java's `setValidating(true)`.
//!
//! # Examples
//!
//! ```
//! use xylograph_validate::validate;
//!
//! // The element `b` is used but never declared: a validity error, reported, not thrown.
//! let xml = "<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>";
//! let report = validate(xml.as_bytes())?;
//! assert!(!report.is_valid());
//! assert!(report.errors().iter().any(|e| e.to_string().contains("c")));
//! # Ok::<(), xylograph_core::Error>(())
//! ```

mod content;
mod driver;
pub mod dtd;
#[cfg(feature = "xml-id")]
pub mod ids;

pub use driver::{Report, validate, validate_reader};
#[cfg(feature = "xml-id")]
pub use ids::XmlIdValidator;

use std::ops::ControlFlow;

use xylograph_core::error::{Error, ErrorKind, Location};
use xylograph_core::name::{NamePool, QName};
use xylograph_parser::AttributeRef;

/// A validity error: a way the document departs from its schema.
///
/// Unlike a well-formedness error it is recoverable — reported, then parsing continues — so it
/// is kept distinct from the fatal [`Error`] the parser raises. It carries the same location.
#[derive(Clone, Debug)]
pub struct ValidityError {
  message: String,
  location: Location,
}

impl ValidityError {
  /// Creates a validity error at a location.
  #[must_use]
  pub fn new(message: impl Into<String>, location: Location) -> Self {
    Self { message: message.into(), location }
  }

  /// The human-readable description.
  #[must_use]
  pub fn message(&self) -> &str {
    &self.message
  }

  /// Where in the document the error is.
  #[must_use]
  pub fn location(&self) -> &Location {
    &self.location
  }

  /// Converts to the crate-wide [`Error`] type, as a recoverable [`ErrorKind::Validity`].
  #[must_use]
  pub fn to_error(&self) -> Error {
    Error::recoverable(ErrorKind::Validity, self.message.clone()).at(self.location.clone())
  }
}

impl std::fmt::Display for ValidityError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if self.location.is_unknown() {
      f.write_str(&self.message)
    } else {
      write!(f, "{}: {}", self.location, self.message)
    }
  }
}

/// Receives validity errors as a validator finds them, and decides whether to keep going.
///
/// Returning [`ControlFlow::Break`] stops validation early; [`ControlFlow::Continue`] lets it
/// find every error. The two provided implementations, [`CollectErrors`] and [`FailFast`],
/// cover the common wants.
pub trait ErrorListener {
  /// Reports one validity error. The return value decides whether validation continues.
  fn report(&mut self, error: ValidityError) -> ControlFlow<()>;
}

/// An [`ErrorListener`] that gathers every error and never stops early.
#[derive(Debug, Default)]
pub struct CollectErrors {
  errors: Vec<ValidityError>,
}

impl CollectErrors {
  /// The errors collected so far, in the order they were found.
  #[must_use]
  pub fn errors(&self) -> &[ValidityError] {
    &self.errors
  }

  /// Takes the collected errors, leaving the listener empty.
  #[must_use]
  pub fn take(&mut self) -> Vec<ValidityError> {
    std::mem::take(&mut self.errors)
  }
}

impl ErrorListener for CollectErrors {
  fn report(&mut self, error: ValidityError) -> ControlFlow<()> {
    self.errors.push(error);
    ControlFlow::Continue(())
  }
}

/// An [`ErrorListener`] that stops at the first error and keeps it.
#[derive(Debug, Default)]
pub struct FailFast {
  first: Option<ValidityError>,
}

impl FailFast {
  /// The first error, if one was reported.
  #[must_use]
  pub fn first(&self) -> Option<&ValidityError> {
    self.first.as_ref()
  }
}

impl ErrorListener for FailFast {
  fn report(&mut self, error: ValidityError) -> ControlFlow<()> {
    self.first = Some(error);
    ControlFlow::Break(())
  }
}

/// A validator: constraints checked over a document's events.
///
/// The methods mirror the events a parser emits. A validator is fed the start and end of each
/// element, the character data between, and finally [`finish`](Validator::finish) for the
/// checks that can only be made once the whole document has been seen (that every `IDREF`
/// found an `ID`, say). Names arrive as [`QName`]s resolved against `pool`, which a validator
/// keeps to render them and, for the DTD, to match on the lexical form.
///
/// A schema language implements this and nothing more; the same interface serves the DTD,
/// RELAX NG, XSD, and a caller's own rules.
pub trait Validator {
  /// The start of an element, with its attributes (defaults included).
  fn start_element(
    &mut self,
    name: QName,
    attributes: &[AttributeRef<'_>],
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()>;

  /// Character data inside the current element. `whitespace_only` distinguishes the whitespace
  /// that element content may contain from data it may not.
  fn characters(
    &mut self,
    text: &str,
    whitespace_only: bool,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()>;

  /// The end of the current element.
  fn end_element(
    &mut self,
    name: QName,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()>;

  /// The end of the document: the moment for whole-document checks.
  fn finish(&mut self, pool: &NamePool, errors: &mut dyn ErrorListener) -> ControlFlow<()>;
}

/// A compiled schema, from which a fresh [`Validator`] is taken for each document.
///
/// The DTD's schema is the document's own DTD, built once the `DOCTYPE` is read; an external
/// schema (RELAX NG, XSD) is compiled ahead of time and reused.
pub trait Schema {
  /// A validator for one document.
  fn validator(&self) -> Box<dyn Validator>;
}
