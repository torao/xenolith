//! The validation mechanism consists of a [`Validator`] that checks events from any source and the [`ValidityError`]s
//! it reports.
//!
//! A [`Validator`] is an [`EventHandler`] that compares incoming events against a schema and records any violations
//! found. A [`Schema`] generates a new validator for each document. Since both rely solely on the event vocabulary,
//! the same validator can check events originating from a parser, a tree traversal, or a writer. Schema languages
//! implement these two traits; for instance, DTDs achieve this via [`dtd::validate`](crate::dtd::validate), and
//! similar implementations are possible for RELAX NG, XSD, or application-specific rules.
//!
//! This module also includes features that are independent of any specific schema language. [`ids`] validates `xml:id`
//! attributes according to specific rules. [`ValidatorSet`] aggregates multiple validators, consolidates their
//! findings into a [`Report`], and allows processing to stop after a specified number of errors.
//!
//! # What Validators do not do
//!
//! Validators only check event constraints. They do not perform entity expansion, fill in default attribute values, or
//! normalize attribute values based on declared types. These tasks are all handled by the parser in [`io`](crate::io)
//! before the events are reported. Consequently, the schema requires nothing more than the events and their names.
//!
//! # Well-formedness and validity
//!
//! Errors regarding *well-formedness* are fatal. The parser (or
//! [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator), which handles events from any source) returns
//! these as an [`Err`], halting processing. Errors regarding *validity* are recoverable. Although the document is
//! well-formed, it falls outside the schema's constraints; the validator records the error, and processing continues.
//! All such errors can be retrieved later by calling [`Validator::errors`]. This is similar to the distinction between
//! `ErrorHandler.fatalError` and `ErrorHandler.error` in Java.
//!

pub mod ids;

pub use ids::XmlIdValidator;

use std::borrow::Cow;

use crate::dtd::validate::DocumentDtd;
use crate::error::{Error, Location, Result};
use crate::event::{EventHandler, EventRef, Outcome};

/// A schema violation detected within the document.
///
/// This is a recoverable error; the validator logs the violation and continues processing, whereas *well-formedness*
/// errors are fatal and halt processing. Consequently, it is treated as a distinct type rather than a simple [`Error`],
/// though it retains [`Location`] information just like an [`Error`].
#[derive(Clone, Debug)]
pub struct ValidityError {
  message: String,
  location: Location,
}

impl ValidityError {
  /// Creates a validity error at `location` with the specified `message`.
  #[must_use]
  pub fn new(message: impl Into<String>, location: Location) -> Self {
    Self { message: message.into(), location }
  }

  /// The description indicating the nature of the violation.
  #[must_use]
  pub fn message(&self) -> &str {
    &self.message
  }

  /// The location within the document where the violation occurs.
  #[must_use]
  pub fn location(&self) -> &Location {
    &self.location
  }

  /// This error is treated as [`Error::Validity`] for callers that must report errors via [`Result`],
  /// such as a writer rejecting content prohibited by the schema.
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

/// An [`EventHandler`] that validates document events against a schema and collects any detected violations.
///
/// Like any other handler, validators can be driven by any event source, but their specific purpose is to verify
/// constraints. It does not perform tasks such as entity expansion, filling in default attribute values, or value
/// normalization; these operations are expected to have been completed by the source before the events reach the
/// validator.
///
/// Typically, processing does not stop when a validation error occurs. Instead, the validator records the error and
/// returns `Ok`, allowing processing to continue to the end of the document; ultimately, all violations are available
/// from [`errors`](Self::errors). Conversely, a validator designed to halt upon
/// the first violation would return [`Err`] from [`handle`](EventHandler::handle), just like any other handler.
///
/// Certain validation checks requiring the entire document, such as verifying that all `IDREF`s match corresponding
/// `ID`s, are performed upon the [`EndDocument`](crate::event::EventRef::EndDocument) event. If processing stops
/// before the end of the document is reached, this event is never triggered, and consequently, these checks are not
/// performed.
///
/// Schema languages implement this trait. This allows validators, whether DTD, RELAX NG, XSD, or application-specific
/// rules, to share a common interface, enabling a single implementation to validate input being parsed, a constructed
/// tree, or a document being written.
pub trait Validator: EventHandler {
  /// Validation errors detected so far (in no particular order).
  ///
  /// The implementation can choose whether the validator holds the errors as [`Cow::Borrowed`] or collects them as
  /// [`Cow::Owned`].
  fn errors(&self) -> Cow<'_, [ValidityError]>;

  /// An [`EventHandler`] for installing this validator into an event source.
  ///
  /// Converting `&mut dyn Validator` to `&mut dyn EventHandler` requires trait upcasting, a feature stabilized in Rust
  /// 1.86. However, this crate supports Rust 1.85. Therefore, each implementation currently returns `self`.
  ///
  fn as_event_handler(&mut self) -> &mut dyn EventHandler;
}

/// A schema that generates a new [`Validator`] for each document after it has been created.
///
/// While the schema itself can be shared, the validator performing the validation maintains the state of the target
/// document, requiring a separate instance for each document. [`DtdSchema`](crate::dtd::validate::DtdSchema)
/// represents a DTD held as such a schema. Since a document-specific DTD is not known until the `DOCTYPE` is read,
/// validation is performed using [`DocumentDtd`] instead.
pub trait Schema {
  /// Generates a new validator for a single document.
  fn validator(&self) -> Box<dyn Validator>;
}

/// Indicates how the execution (run) covered by the [`Report`] concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
  /// The execution reached the end of the document. All checks were performed, including those requiring the entire
  /// document.
  Completed,

  /// All active handlers terminated early. Checks requiring the entire document were not performed.
  Stopped,

  /// The execution terminated due to an error (e.g., the input was not well-formed, a handler rejected an event, or
  /// the [`ValidatorSet`] error limit was reached). The error itself originated from the event source.
  Failed,

  /// The execution was abandoned before the source could report completion.
  Abandoned,
}

/// The result of the [`ValidatorSet`] validating the document.
#[derive(Clone, Debug)]
pub struct Report {
  errors: Vec<ValidityError>,
  had_dtd: bool,
  had_validators: bool,
  ended: Option<Ended>,
}

impl Report {
  /// Returns whether the document is valid. A document is considered valid when processing is complete, it has been
  /// checked against a set of criteria (such as a custom DTD or a validator added to the set), and no validity errors
  /// were detected.
  ///
  /// Documents that have not been checked against any criteria are not considered valid. Similarly, documents for
  /// which processing did not complete are not considered valid, as a full check of the document was not performed.
  /// See [`had_dtd`](Self::had_dtd) and [`ended`](Self::ended).
  #[must_use]
  pub fn is_valid(&self) -> bool {
    self.ended == Some(Ended::Completed) && (self.had_dtd || self.had_validators) && self.errors.is_empty()
  }

  /// Whether or not the document declared the DTD subject to verification. This refers to whether the DTD within the
  /// document was used for validation.
  #[must_use]
  pub fn had_dtd(&self) -> bool {
    self.had_dtd
  }

  /// How the execution finished. Returns `None` if completion was not explicitly specified (such as when
  /// [`finish`](EventHandler::finish) was not called during manual event delivery).
  #[must_use]
  pub fn ended(&self) -> Option<Ended> {
    self.ended
  }

  /// Returns the detected validity errors ordered by system identifier, line, and column (positional order). Errors
  /// with indeterminate locations are placed at the end.
  #[must_use]
  pub fn errors(&self) -> &[ValidityError] {
    &self.errors
  }
}

/// A [`Validator`] that uses multiple validators to simultaneously validate a document and report any detected issues.
///
/// If [`validating_dtd`](Self::validating_dtd) is enabled, validation is performed based on the document's own DTD,
/// and if [`checking_xml_id`](Self::checking_xml_id) is enabled, `xml:id` attributes are checked. Additionally,
/// validators added via [`with_validator`](Self::with_validator) or created via [`with_schema`](Self::with_schema) are
/// used. Events are passed sequentially to these validators, and [`errors`](Validator::errors) gathers what they
/// found. The same validator may be added multiple times. After processing concludes, calling
/// [`report`](Self::report) allows you to retrieve error information and the final processing status. You can also use
/// [`with_error_limit`](Self::with_error_limit) to halt processing once a specified number of errors has occurred.
///
/// Since this validator does not generate or drive events itself, it functions consistently regardless of the event
/// source (e.g., a source pushing events via [`emit`](crate::event::EventCursor::emit), a caller pulling events one by
/// one, or a program manually supplying events). To ensure that application handlers receive the same events for the
/// same path, register both this validator set and the handlers with [`Dispatch`](crate::event::Dispatch), specifying
/// the validator set first.
///
/// Whenever a [`StartDocument`](EventRef::StartDocument) event occurs, signaling the start of a new document, the
/// validator set discards errors and completion statuses from the previous document and creates fresh validators for
/// DTD and `xml:id` checks. Validators added by the application are retained, but they are required to reset their
/// internal state upon the `StartDocument` event (ensuring errors carried over from the previous document are not
/// recounted).
///
/// # Examples
///
/// ```
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::event::validate::ValidatorSet;
/// use xenolith::event::{Dispatch, EventCursor, EventSource};
/// use xenolith::io::StreamSource;
///
/// let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)>]><r><item>hi</item></r>";
/// let mut validators = ValidatorSet::new().validating_dtd(true);
/// let mut builder = DomBuilder::new();
/// {
///   let mut lane = Dispatch::new().with_handler(&mut validators).with_handler(&mut builder);
///   StreamSource::new(xml.as_bytes()).with_handler(&mut lane).emit()?;
/// }
/// assert!(validators.report().is_valid());
/// let doc = builder.into_document();
/// assert_eq!(doc.node_name(doc.document_element().unwrap()), "r");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
pub struct ValidatorSet {
  /// Whether the document is validated against its own DTD.
  use_dtd: bool,
  /// Whether the `xml:id` attributes are checked.
  xml_id: bool,
  /// The error count threshold at which validation stops (if configured).
  error_limit: Option<usize>,
  /// Whether validators have been created for the current document.
  prepared: bool,
  /// The validator for the document's own DTD, created for each document when `use_dtd` is set.
  document_dtd: Option<DocumentDtd>,
  /// The `xml:id` checker for documents not validated against a DTD, created for each document when `xml_id` is set.
  /// When a DTD is used, the DTD validator performs checks within its own ID space.
  standalone_ids: Option<XmlIdValidator>,
  /// The validators added by the application.
  validators: Vec<Box<dyn Validator>>,
  /// How the validation process for the current document concluded.
  ended: Option<Ended>,
}

impl std::fmt::Debug for ValidatorSet {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ValidatorSet")
      .field("use_dtd", &self.use_dtd)
      .field("xml_id", &self.xml_id)
      .field("error_limit", &self.error_limit)
      .field("validators", &self.validators.len())
      .field("ended", &self.ended)
      .finish_non_exhaustive()
  }
}

impl Default for ValidatorSet {
  fn default() -> Self {
    Self::new()
  }
}

impl ValidatorSet {
  /// Creates a set with no validators and no error limit.
  #[must_use]
  pub fn new() -> Self {
    Self {
      use_dtd: false,
      xml_id: false,
      error_limit: None,
      prepared: false,
      document_dtd: None,
      standalone_ids: None,
      validators: Vec::new(),
      ended: None,
    }
  }

  /// Configures whether to validate the document against its DTD. This corresponds to `setValidating(true)` in Java.
  /// The default value is `false`.
  #[must_use]
  pub fn validating_dtd(mut self, on: bool) -> Self {
    self.use_dtd = on;
    self
  }

  /// Configures whether to perform checks on the `xml:id` attribute. The default value is `false`. Regardless of the
  /// source configuration, this feature is disabled unless explicitly requested.
  ///
  /// The parser configured for `xml:id` normalizes the reported values, but checking those values is the
  /// responsibility of a separate validation function. Therefore, callers requiring both operations must explicitly
  /// request both. When using [`validating_dtd`](Self::validating_dtd), `xml:id` values share the same ID space as
  /// DTD `ID` attributes.
  #[must_use]
  pub fn checking_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// Adds a validator. It receives each event after the validators already added.
  #[must_use]
  pub fn with_validator(mut self, validator: Box<dyn Validator>) -> Self {
    self.validators.push(validator);
    self
  }

  /// Adds the validators while maintaining their order.
  #[must_use]
  pub fn with_validators(mut self, validators: impl IntoIterator<Item = Box<dyn Validator>>) -> Self {
    for validator in validators {
      self = self.with_validator(validator);
    }
    self
  }

  /// Adds a validator created with `schema`.
  #[must_use]
  pub fn with_schema(self, schema: &dyn Schema) -> Self {
    self.with_validator(schema.validator())
  }

  /// Sets an upper limit for halting processing due to validity errors.
  ///
  /// When the number of validity errors reaches this `limit` while processing an event, the `limit`-th error (in the
  /// order found within [`Report::errors`]) is returned as an [`Err`] (specifically, [`Error::Validity`]). Processing
  /// stops at that point, and no subsequent events are sent to downstream handlers. Since multiple errors can occur
  /// within a single event, [`report`](Self::report) retains all errors detected up to that moment (at least `limit`
  /// of them). The default value of 0 signifies *no limit*, meaning processing continues to the end of the document
  /// regardless of how many validity errors are found.
  #[must_use]
  pub fn with_error_limit(mut self, limit: usize) -> Self {
    self.error_limit = (limit > 0).then_some(limit);
    self
  }

  /// The results indicating what was found in the current document and how the scan concluded.
  #[must_use]
  pub fn report(&self) -> Report {
    Report {
      errors: self.sorted_errors(),
      had_dtd: self.document_dtd.as_ref().is_some_and(DocumentDtd::had_dtd),
      had_validators: !self.validators.is_empty(),
      ended: self.ended,
    }
  }

  /// Document processing begins. New DTD and `xml:id` validators are created, and any information detected previously
  /// is discarded.
  fn start_document(&mut self) {
    self.document_dtd = self.use_dtd.then(|| DocumentDtd::new().checking_xml_id(self.xml_id));
    self.standalone_ids = (!self.use_dtd && self.xml_id).then(XmlIdValidator::new);
    self.ended = None;
    self.prepared = true;
  }

  /// The iterator of validators to which events are passed. The order is: DTD validator, `xml:id` check, and any added
  /// validators.
  fn members(&mut self) -> impl Iterator<Item = &mut (dyn Validator + 'static)> + '_ {
    let dtd = self.document_dtd.as_mut().map(|validator| validator as &mut dyn Validator);
    let ids = self.standalone_ids.as_mut().map(|validator| validator as &mut dyn Validator);
    dtd.into_iter().chain(ids).chain(self.validators.iter_mut().map(AsMut::as_mut))
  }

  /// Errors for each validator (in the order of the validators).
  fn gathered_errors(&self) -> Vec<ValidityError> {
    let mut errors = Vec::new();
    if let Some(validator) = &self.document_dtd {
      errors.extend_from_slice(&validator.errors());
    }
    if let Some(validator) = &self.standalone_ids {
      errors.extend_from_slice(&validator.errors());
    }
    for validator in &self.validators {
      errors.extend_from_slice(&validator.errors());
    }
    errors
  }

  /// Returns the errors from each validator, sorted by their occurrence position. Since a stable sort is used, the
  /// order of validators is preserved for errors occurring at the same position.
  fn sorted_errors(&self) -> Vec<ValidityError> {
    let mut errors = self.gathered_errors();
    errors.sort_by(|a, b| position(a.location()).cmp(&position(b.location())));
    errors
  }
}

impl EventHandler for ValidatorSet {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    // Events fed without a `StartDocument` still need the validators of a document.
    if matches!(event, EventRef::StartDocument) || !self.prepared {
      self.start_document();
    }
    for validator in self.members() {
      validator.handle(event)?;
    }
    if let Some(limit) = self.error_limit {
      // Counted without gathering, since a validator that holds its errors lends them at no cost.
      let count: usize = self.members().map(|validator| validator.errors().len()).sum();
      if count >= limit {
        return Err(self.sorted_errors()[limit - 1].to_error());
      }
    }
    Ok(())
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    self.ended = Some(match outcome {
      Outcome::Completed => Ended::Completed,
      Outcome::Stopped => Ended::Stopped,
      Outcome::Failed(_) => Ended::Failed,
      Outcome::Abandoned => Ended::Abandoned,
    });
    for validator in self.members() {
      validator.finish(outcome);
    }
  }
}

impl Validator for ValidatorSet {
  fn errors(&self) -> Cow<'_, [ValidityError]> {
    Cow::Owned(self.gathered_errors())
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}

/// Sort key for ordering errors by location: sorts with known locations first, followed by system identifier, line,
/// and column.
fn position(at: &Location) -> (bool, Option<&str>, u32, u32) {
  (at.is_unknown(), at.system_id.as_deref(), at.line, at.column)
}
