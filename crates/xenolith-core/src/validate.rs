//! The validation contract: a source-independent [`Validator`] and the errors it reports.
//!
//! This is vocabulary, not an implementation. A [`Validator`] checks a stream of document events, whatever emits them,
//! and reports each departure from the schema to an [`ErrorListener`]. The schema languages that implement it, and the
//! drivers that feed it, live in higher crates. It names only core types, so a parser, a tree, or a writer can all
//! target it.
//!

use std::ops::ControlFlow;

use crate::attr::Attributes;
use crate::error::{Error, Location};
use crate::name::{NamePool, QName};

/// A validity error is a way the document departs from its schema.
///
/// It is recoverable: a validator reports it and continues, whereas a well-formedness error is fatal and stops the
/// parse. It is therefore a separate type from the fatal [`Error`], and carries a [`Location`] just as that error does.
///
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

  /// Converts to the crate-wide [`Error`] type, as a recoverable [`Error::Validity`].
  #[must_use]
  pub fn to_error(&self) -> Error {
    Error::validity(self.message.clone()).at(self.location.clone())
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

/// Receives validity errors as a validator finds them and decides whether to keep going.
///
/// Returning [`ControlFlow::Break`] stops validation early. [`ControlFlow::Continue`] lets it find every error. The
/// two provided implementations, [`CollectErrors`] and [`FailFast`], cover the common cases.
///
pub trait ErrorListener {
  /// Receives one validation error. The return value determines whether validation continues.
  ///
  fn report(&mut self, error: ValidityError) -> ControlFlow<()>;
}

/// An [`ErrorListener`] that gathers every error and never stops early.
#[derive(Debug, Default)]
pub struct CollectErrors {
  errors: Vec<ValidityError>,
}

impl CollectErrors {
  /// The errors collected so far, in the order they were found.
  ///
  #[must_use]
  pub fn errors(&self) -> &[ValidityError] {
    &self.errors
  }

  /// Takes the collected errors, leaving the listener empty.
  ///
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
///
#[derive(Debug, Default)]
pub struct FailFast {
  first: Option<ValidityError>,
}

impl FailFast {
  /// The first error, if one was reported.
  ///
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

/// A validator checks constraints over a document's events.
///
/// It only checks constraints. It does not expand entities, supply attribute defaults, or normalize values; the source
/// has already done those by the time an event arrives.
///
/// Each method corresponds to one event type, and a source calls it as that event occurs. A validator sees the start
/// and end of each element, the character data between, and finally [`finish`](Validator::finish) for the checks that
/// need the whole document (that every `IDREF` found an `ID`, say). Names arrive as [`QName`]s resolved against `pool`,
/// which a validator keeps to render them and, for the DTD, to match on the lexical form.
///
/// A schema language implements this and nothing more. The same interface serves the DTD, RELAX NG, XSD, and a
/// caller's own rules, and the same implementation checks parsed input, a built tree, or a document being written.
///
pub trait Validator {
  /// Checks the start of an element, with its attributes (defaults included).
  ///
  fn start_element(
    &mut self,
    name: QName,
    attributes: Attributes<'_>,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()>;

  /// Checks character data inside the current element. `whitespace_only` distinguishes the whitespace that element
  /// content may contain from data it may not.
  ///
  fn characters(
    &mut self,
    text: &str,
    whitespace_only: bool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()>;

  /// Checks the end of the current element.
  ///
  fn end_element(
    &mut self,
    name: QName,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()>;

  /// Checks whatever needs the whole document after the last event.
  ///
  /// It is given no name pool, so a validator that needs a name here keeps its lexical form during the run rather than
  /// resolving it now. This lets a driver run [`finish`](Validator::finish) at the end of any source, including one
  /// that has no pool to lend.
  ///
  fn finish(&mut self, errors: &mut dyn ErrorListener) -> ControlFlow<()>;
}
