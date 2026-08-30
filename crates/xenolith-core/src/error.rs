//! Errors and source locations.
//!

use std::fmt;
use std::sync::Arc;

/// Result type used throughout xenolith.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A position in an entity.
///
/// The XML/SAX specification requires that, when an error occurs while the parser is reading the content of an external
/// entity referenced from an XML document, the parser report the position within the external entity rather than the
/// XML document from which it began reading. The position within the external entity is reported as the entity's
/// system identifier followed by its line number.
///
/// # Examples
///
/// ```
/// use xenolith_core::Location;
///
/// let at = Location { line: 3, column: 17, offset: 94, ..Location::unknown() }
///   .with_system_id("file:///docs/part.ent");
/// assert_eq!(at.to_string(), "file:///docs/part.ent:3:17");
///
/// assert!(Location::unknown().is_unknown());
/// assert_eq!(Location::unknown().to_string(), "<unknown>");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Location {
  /// System identifier (absolute URI) of the entity, if known.
  pub system_id: Option<Arc<str>>,
  /// Public identifier of the entity, if declared.
  pub public_id: Option<Arc<str>>,
  /// 1-based line number; 0 when unknown.
  pub line: u32,
  /// 1-based column number in characters (not bytes); 0 when unknown.
  pub column: u32,
  /// 0-based character offset from the start of the entity.
  pub offset: u64,
}

impl Location {
  /// A location with no information at all.
  #[must_use]
  pub const fn unknown() -> Self {
    Self { system_id: None, public_id: None, line: 0, column: 0, offset: 0 }
  }

  /// Returns a copy with `system_id` set.
  #[must_use]
  pub fn with_system_id(mut self, system_id: impl Into<Arc<str>>) -> Self {
    self.system_id = Some(system_id.into());
    self
  }

  /// True if neither a position nor an entity identifier is known.
  #[must_use]
  pub fn is_unknown(&self) -> bool {
    self.line == 0 && self.system_id.is_none()
  }

  /// Advances this location past `c`. This allows the location to be updated using the characters read from the entity.
  ///
  /// A line feed (U+000A; LF, '\n') indicates the start of a new line. This increments the line number by one so that
  /// it points to the next line, and reset [`column`](Self::column) to 1 at the beginning of the line. All other
  /// characters advance the column by 1. [`offset`](Self::offset) is incremented by 1 regardless of the character.
  ///
  /// Since the end of a line is considered to be normalized to a single U+000A (XML 1.0 §2.11), a line cannot begin
  /// with any other character.
  ///
  pub fn advance(&mut self, c: char) {
    if c == '\n' {
      self.line += 1;
      self.column = 1;
    } else {
      self.column += 1;
    }
    self.offset += 1;
  }
}

impl fmt::Display for Location {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match (&self.system_id, self.line) {
      (Some(id), 0) => write!(f, "{id}"),
      (Some(id), line) => write!(f, "{id}:{line}:{}", self.column),
      (None, 0) => f.write_str("<unknown>"),
      (None, line) => write!(f, "{line}:{}", self.column),
    }
  }
}

/// Severity of a diagnostic, corresponds to the three level of SAX's `ErrorHandler`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
  /// Does not affect the result; reported for information.
  Warning,
  /// Recoverable: processing may continue. Validity errors land here.
  Error,
  /// Unrecoverable: well-formedness errors and anything that stops processing.
  Fatal,
}

impl fmt::Display for Severity {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      Self::Warning => "warning",
      Self::Error => "error",
      Self::Fatal => "fatal error",
    })
  }
}

/// A location that can only be identified as "somewhere", shared by errors that have no fixed location.
static UNKNOWN_LOCATION: Location = Location::unknown();

/// An error produced anywhere in the xenolith pipeline.
///
/// Each variant represents a distinct failure type and conveys the information that the failure is intended to
/// indicate. Since the number of error type may increase as fixes are made and subsequent phases are implemented,
/// unintended errors should be matched using a wildcard arm to ensure backward compatibility.
///
/// # Examples
///
/// ```
/// use xenolith_core::{Error, Location, Severity};
///
/// let err = Error::well_formedness("mismatched end tag: expected </a>")
///   .at(Location { line: 12, column: 3, ..Location::unknown() }.with_system_id("file:///doc.xml"));
///
/// assert!(matches!(err, Error::WellFormedness { .. }));
/// assert_eq!(err.severity(), Severity::Fatal); // well-formedness errors stop processing
/// assert_eq!(err.location().line, 12);
/// assert_eq!(err.to_string(), "not well-formed: mismatched end tag: expected </a>");
///
/// // Validity errors are recoverable: the caller may report and continue.
/// let invalid = Error::validity("element \"b\" is not allowed here");
/// assert_eq!(invalid.severity(), Severity::Error);
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
  /// I/O failure while reading an entity.
  #[error("I/O error: {message}")]
  Io {
    /// Where the read was taking place, if known.
    location: Location,
    /// What went wrong.
    message: String,
    /// The underlying I/O error.
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
  },
  /// A failure caused by the application code of the entity resolver invoked by the parser.
  ///
  /// This variant allows the application to store its own error as [`source`](std::error::Error::source) because the
  /// parser cannot determine what went wrong inside the application's callback. The caller can retrieve the original
  /// error by down-casting the `source`.
  ///
  #[error("resolver error: {message}")]
  Resolver {
    /// Where the parser was when it called out, if known.
    location: Location,
    /// What went wrong.
    message: String,
    /// The application's own error, preserved so a caller can downcast to recover it.
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
  },
  /// A failure raised by application code in a SAX-style event handler driven over the parser.
  ///
  /// Like [`Resolver`](Self::Resolver), the parser cannot know what went wrong inside the callback, so the
  /// application's own error is stored as [`source`](std::error::Error::source); a caller can downcast it to recover the
  /// original.
  ///
  #[error("SAX handler error: {message}")]
  SaxHandler {
    /// Where the handler was called, if known.
    location: Location,
    /// What went wrong, taken from the application error's `Display`.
    message: String,
    /// The application's own error, preserved so a caller can downcast to recover it.
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
  },
  /// Malformed input for the character encoding in use, or an unsupported encoding.
  #[error("encoding error: {message}")]
  Encoding {
    /// Where in the entity the bytes were being decoded.
    location: Location,
    /// A message explaining what went wrong, and what the caller can do about it.
    message: String,
    /// The 0-based index, the first byte position that could not be decoded within the byte sequence passed to the
    /// decoder during the call in which the error occurred. `None` if the byte position causing the error does not
    /// exist, for example, if the encoding name is unknown or an entity ends mid_character.
    byte_offset: Option<usize>,
  },
  /// Malformed URI or an unresolvable relative reference.
  #[error("URI error: {message}")]
  Uri {
    /// Where the reference appeared, if known.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// A string that must match an XML production (`Name`, `NCName`, ...) does not.
  #[error("name error: {message}")]
  Name {
    /// Where the name appeared, if known.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// Well-formedness violation.
  #[error("not well-formed: {message}")]
  WellFormedness {
    /// Where the violation was detected.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// Validity constraint violation (recoverable).
  #[error("not valid: {message}")]
  Validity {
    /// Where the violation was detected.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// Namespace constraint violation.
  #[error("namespace error: {message}")]
  Namespace {
    /// Where the violation was detected.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// A resource limit was exceeded: entity expansion, nesting depth, node count.
  #[error("limit exceeded: {message}")]
  Limit {
    /// Where the limit was reached.
    location: Location,
    /// Which limit, and how to raise it.
    message: String,
  },
  /// An XInclude processing error: an inclusion loop, or a failed inclusion with no fallback.
  #[error("XInclude error: {message}")]
  XInclude {
    /// Where the `xi:include` appeared.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// An XPath expression could not be parsed, or failed while being evaluated.
  #[error("XPath error: {message}")]
  XPath {
    /// Where in the expression or the stylesheet the failure lies, if known.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// A stylesheet is not one XSLT allows, or a transformation could not be carried out.
  #[error("XSLT error: {message}")]
  Xslt {
    /// Where in the stylesheet the failure lies, if known.
    location: Location,
    /// What went wrong.
    message: String,
  },
  /// The requested feature was not included in the build; see the crate's feature flags. The message should specify
  /// the name of the missing feature.
  #[error("unsupported feature: {message}")]
  UnsupportedFeature {
    /// Which capability, which feature would supply it, and what the build can do instead. Built
    /// by [`Error::unsupported_feature`] so the wording stays uniform.
    message: String,
  },
  /// A bug in xenolith or a dependent library, or that the API was called in a way prohibited by the API specification.
  #[error("internal error: {message}")]
  Internal {
    /// What was expected that did not hold, and what the caller should do about it.
    message: String,
  },
}

impl Error {
  /// An I/O failure while reading an entity.
  #[must_use]
  pub fn io(message: impl Into<String>) -> Self {
    Self::Io { location: Location::unknown(), message: message.into(), source: None }
  }

  /// A failure raised by application code the parser called into, such as a resolver.
  ///
  /// Wrap an application-specific errors (such as database or network failures) in this way, they are relayed to the
  /// caller via the parser. The application error is stored as [`source`](std::error::Error::source), and its string
  /// representation serves as the error message. If the location is known, append the location information using
  /// [`at`](Self::at).
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::Error;
  ///
  /// let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the catalog is locked");
  /// let err = Error::resolver(cause);
  /// assert!(matches!(err, Error::Resolver { .. }));
  /// assert_eq!(err.to_string(), "resolver error: the catalog is locked");
  /// assert!(std::error::Error::source(&err).is_some());
  /// ```
  #[must_use]
  pub fn resolver(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
    let source = source.into();
    Self::Resolver { location: Location::unknown(), message: source.to_string(), source: Some(source) }
  }

  /// A failure raised by application code in a SAX-style event handler.
  ///
  /// Like [`resolver`](Self::resolver), the application error is stored as [`source`](std::error::Error::source) and its
  /// string representation serves as the message; append a location with [`at`](Self::at) when it is known.
  ///
  #[must_use]
  pub fn sax_handler(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
    let source = source.into();
    Self::SaxHandler { location: Location::unknown(), message: source.to_string(), source: Some(source) }
  }

  /// A character-encoding failure. When the problematic byte has been indicated, specify [`byte_offset`] to indicate
  /// its byte offset.
  ///
  /// [`byte_offset`]: Error::Encoding
  #[must_use]
  pub fn encoding(message: impl Into<String>) -> Self {
    Self::Encoding { location: Location::unknown(), message: message.into(), byte_offset: None }
  }

  /// A malformed URI or an unresolvable relative reference.
  #[must_use]
  pub fn uri(message: impl Into<String>) -> Self {
    Self::Uri { location: Location::unknown(), message: message.into() }
  }

  /// A string that must match an XML name production does not.
  #[must_use]
  pub fn name(message: impl Into<String>) -> Self {
    Self::Name { location: Location::unknown(), message: message.into() }
  }

  /// A well-formedness violation.
  #[must_use]
  pub fn well_formedness(message: impl Into<String>) -> Self {
    Self::WellFormedness { location: Location::unknown(), message: message.into() }
  }

  /// A validity-constraint violation. Recoverable: its [`severity`](Error::severity) is
  /// [`Severity::Error`], so a caller may report it and continue.
  #[must_use]
  pub fn validity(message: impl Into<String>) -> Self {
    Self::Validity { location: Location::unknown(), message: message.into() }
  }

  /// A namespace-constraint violation.
  #[must_use]
  pub fn namespace(message: impl Into<String>) -> Self {
    Self::Namespace { location: Location::unknown(), message: message.into() }
  }

  /// A resource limit exceeded.
  #[must_use]
  pub fn limit(message: impl Into<String>) -> Self {
    Self::Limit { location: Location::unknown(), message: message.into() }
  }

  /// An XInclude processing failure.
  #[must_use]
  pub fn xinclude(message: impl Into<String>) -> Self {
    Self::XInclude { location: Location::unknown(), message: message.into() }
  }

  /// An XPath expression could not be parsed or evaluated.
  #[must_use]
  pub fn xpath(message: impl Into<String>) -> Self {
    Self::XPath { location: Location::unknown(), message: message.into() }
  }

  /// A stylesheet could not be compiled or applied.
  #[must_use]
  pub fn xslt(message: impl Into<String>) -> Self {
    Self::Xslt { location: Location::unknown(), message: message.into() }
  }

  /// For errors where the `Location` field of the source is specified, associate that location with the error. For all
  /// other errors, take no action.
  ///
  #[must_use]
  pub fn at(mut self, location: Location) -> Self {
    match &mut self {
      Self::Io { location: at, .. }
      | Self::Resolver { location: at, .. }
      | Self::SaxHandler { location: at, .. }
      | Self::Encoding { location: at, .. }
      | Self::Uri { location: at, .. }
      | Self::Name { location: at, .. }
      | Self::WellFormedness { location: at, .. }
      | Self::Validity { location: at, .. }
      | Self::Namespace { location: at, .. }
      | Self::Limit { location: at, .. }
      | Self::XInclude { location: at, .. }
      | Self::XPath { location: at, .. }
      | Self::Xslt { location: at, .. } => *at = location,
      Self::UnsupportedFeature { .. } | Self::Internal { .. } => {}
    }
    self
  }

  /// Associates the root cause of an [`Io`](Error::Io) or [`Resolver`](Error::Resolver) error.
  /// For other types of errors that do not have a source, it does nothing.
  ///
  #[must_use]
  pub fn caused_by(mut self, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
    if let Self::Io { source: cause, .. } | Self::Resolver { source: cause, .. } = &mut self {
      *cause = Some(source.into());
    }
    self
  }

  /// Where the error occurred, or an unknown location for the kinds that have none.
  #[must_use]
  pub fn location(&self) -> &Location {
    match self {
      Self::Io { location, .. }
      | Self::Resolver { location, .. }
      | Self::SaxHandler { location, .. }
      | Self::Encoding { location, .. }
      | Self::Uri { location, .. }
      | Self::Name { location, .. }
      | Self::WellFormedness { location, .. }
      | Self::Validity { location, .. }
      | Self::Namespace { location, .. }
      | Self::Limit { location, .. }
      | Self::XInclude { location, .. }
      | Self::XPath { location, .. }
      | Self::Xslt { location, .. } => location,
      Self::UnsupportedFeature { .. } | Self::Internal { .. } => &UNKNOWN_LOCATION,
    }
  }

  /// The severity, in the SAX sense: a validity violation is recoverable, everything else fatal.
  #[must_use]
  pub const fn severity(&self) -> Severity {
    match self {
      Self::Validity { .. } => Severity::Error,
      _ => Severity::Fatal,
    }
  }

  /// Builds an error for a situation that should be unreachable.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::Error;
  ///
  /// assert_eq!(
  ///   Error::internal("the entity stack is empty").to_string(),
  ///   "internal error: the entity stack is empty; this is a bug in xenolith, please report it"
  /// );
  /// ```
  #[must_use]
  pub fn internal(what: impl std::fmt::Display) -> Self {
    Self::Internal { message: format!("{what}; this is a bug in xenolith, please report it") }
  }

  /// Generates an error that is returned when the build is compiled with a feature disabled.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::Error;
  ///
  /// let err = Error::unsupported_feature("decoding \"Shift_JIS\"", "encodings", "this build handles UTF-8 only");
  /// assert!(matches!(err, Error::UnsupportedFeature { .. }));
  /// assert_eq!(
  ///   err.to_string(),
  ///   "unsupported feature: decoding \"Shift_JIS\" needs the `encodings` feature, \
  ///    which this build does not have; this build handles UTF-8 only"
  /// );
  /// ```
  #[must_use]
  pub fn unsupported_feature(capability: impl std::fmt::Display, feature: &str, fallback: &str) -> Self {
    Self::UnsupportedFeature {
      message: format!("{capability} needs the `{feature}` feature, which this build does not have; {fallback}"),
    }
  }

  /// The human-readable detail, without the kind label or the location.
  #[must_use]
  pub fn message(&self) -> &str {
    match self {
      Self::Io { message, .. }
      | Self::Resolver { message, .. }
      | Self::SaxHandler { message, .. }
      | Self::Encoding { message, .. }
      | Self::Uri { message, .. }
      | Self::Name { message, .. }
      | Self::WellFormedness { message, .. }
      | Self::Validity { message, .. }
      | Self::Namespace { message, .. }
      | Self::Limit { message, .. }
      | Self::XInclude { message, .. }
      | Self::XPath { message, .. }
      | Self::Xslt { message, .. }
      | Self::UnsupportedFeature { message, .. }
      | Self::Internal { message, .. } => message,
    }
  }
}

impl From<std::io::Error> for Error {
  fn from(e: std::io::Error) -> Self {
    Self::Io { location: Location::unknown(), message: e.to_string(), source: Some(Box::new(e)) }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn display_prefixes_the_kind_and_location_stays_a_field() {
    let e = Error::well_formedness("mismatched end tag")
      .at(Location { line: 12, column: 5, ..Location::unknown() }.with_system_id("file:///doc.xml"));
    // The location is not in the message; it is read from the field.
    assert_eq!(e.to_string(), "not well-formed: mismatched end tag");
    assert_eq!((e.location().line, e.location().column), (12, 5));
    assert_eq!(e.location().system_id.as_deref(), Some("file:///doc.xml"));
  }

  #[test]
  fn a_kind_without_a_location_reports_an_unknown_one() {
    let e = Error::internal("unreachable");
    assert_eq!(e.to_string(), "internal error: unreachable; this is a bug in xenolith, please report it");
    assert!(e.location().is_unknown());
  }

  #[test]
  fn only_a_validity_violation_is_recoverable() {
    assert_eq!(Error::io("x").severity(), Severity::Fatal);
    assert_eq!(Error::validity("x").severity(), Severity::Error);
    assert!(Severity::Warning < Severity::Error && Severity::Error < Severity::Fatal);
  }
}
