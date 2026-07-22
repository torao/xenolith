//! Errors and source locations.
//!
//! Every error carries a [`Location`] where one is known. XML processors are judged by their
//! diagnostics as much as by their conformance, and a location that is threaded through from
//! the start is far cheaper than one retrofitted later.

use std::fmt;
use std::sync::Arc;

/// Result type used throughout xylograph.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A position in an entity.
///
/// The origin is the *entity*, not the document: a location inside an external entity reports
/// that entity's system identifier and its own line numbering. This is what XML requires, and
/// it is also what base URI resolution needs later.
///
/// # Examples
///
/// ```
/// use xylograph_core::Location;
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

/// Severity of a diagnostic, matching the three levels of SAX's `ErrorHandler`.
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

/// Broad classification of an error.
///
/// Variants are added as later phases land; match with a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
  /// I/O failure while reading an entity.
  Io,
  /// Malformed input for the character encoding in use, or an unsupported encoding.
  Encoding,
  /// Malformed URI or an unresolvable relative reference.
  Uri,
  /// A string that must match an XML production (`Name`, `NCName`, ...) does not.
  Name,
  /// Well-formedness violation.
  WellFormedness,
  /// Validity constraint violation (recoverable).
  Validity,
  /// Namespace constraint violation.
  Namespace,
  /// A resource limit was exceeded: entity expansion, nesting depth, node count.
  Limit,
  /// The requested capability was not compiled in; see the crate's feature flags.
  UnsupportedFeature,
  /// A bug in xylograph.
  Internal,
}

impl fmt::Display for ErrorKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      Self::Io => "I/O error",
      Self::Encoding => "encoding error",
      Self::Uri => "URI error",
      Self::Name => "name error",
      Self::WellFormedness => "not well-formed",
      Self::Validity => "not valid",
      Self::Namespace => "namespace error",
      Self::Limit => "limit exceeded",
      Self::UnsupportedFeature => "unsupported feature",
      Self::Internal => "internal error",
    })
  }
}

/// An error produced anywhere in the xylograph pipeline.
///
/// # Examples
///
/// ```
/// use xylograph_core::{Error, ErrorKind, Location, Severity};
///
/// let err = Error::new(ErrorKind::WellFormedness, "mismatched end tag: expected </a>")
///   .at(Location { line: 12, column: 3, ..Location::unknown() }.with_system_id("file:///doc.xml"));
///
/// assert_eq!(err.kind(), ErrorKind::WellFormedness);
/// assert_eq!(err.severity(), Severity::Fatal); // well-formedness errors stop processing
/// assert_eq!(err.location().line, 12);
/// assert_eq!(
///   err.to_string(),
///   "file:///doc.xml:12:3: not well-formed: mismatched end tag: expected </a>"
/// );
///
/// // Validity errors are recoverable: the caller may report and continue.
/// let invalid = Error::recoverable(ErrorKind::Validity, "element \"b\" is not allowed here");
/// assert_eq!(invalid.severity(), Severity::Error);
/// ```
#[derive(Debug)]
pub struct Error {
  kind: ErrorKind,
  severity: Severity,
  message: String,
  location: Location,
  source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
  /// Creates a fatal error.
  #[must_use]
  pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
    Self { kind, severity: Severity::Fatal, message: message.into(), location: Location::unknown(), source: None }
  }

  /// Creates a recoverable error, as used for validity constraints.
  #[must_use]
  pub fn recoverable(kind: ErrorKind, message: impl Into<String>) -> Self {
    Self { severity: Severity::Error, ..Self::new(kind, message) }
  }

  /// Attaches a source location.
  #[must_use]
  pub fn at(mut self, location: Location) -> Self {
    self.location = location;
    self
  }

  /// Attaches an underlying cause.
  #[must_use]
  pub fn caused_by(mut self, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
    self.source = Some(source.into());
    self
  }

  /// Overrides the severity.
  #[must_use]
  pub fn with_severity(mut self, severity: Severity) -> Self {
    self.severity = severity;
    self
  }

  /// The classification of this error.
  #[must_use]
  pub const fn kind(&self) -> ErrorKind {
    self.kind
  }

  /// The severity of this error.
  #[must_use]
  pub const fn severity(&self) -> Severity {
    self.severity
  }

  /// Where the error occurred.
  #[must_use]
  pub const fn location(&self) -> &Location {
    &self.location
  }

  /// The human-readable description, without location or classification.
  #[must_use]
  pub fn message(&self) -> &str {
    &self.message
  }

  /// Builds the error returned when a capability was not compiled in.
  ///
  /// Requesting a disabled capability must fail loudly rather than silently degrade; see
  /// the feature policy in `ROADMAP.md`.
  #[must_use]
  pub fn unsupported_feature(capability: &str, feature: &str) -> Self {
    Self::new(
      ErrorKind::UnsupportedFeature,
      format!("{capability} is not available: rebuild with the `{feature}` feature"),
    )
  }
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if self.location.is_unknown() {
      write!(f, "{}: {}", self.kind, self.message)
    } else {
      write!(f, "{}: {}: {}", self.location, self.kind, self.message)
    }
  }
}

impl std::error::Error for Error {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    self.source.as_ref().map(|e| &**e as &(dyn std::error::Error + 'static))
  }
}

impl From<std::io::Error> for Error {
  fn from(e: std::io::Error) -> Self {
    Self::new(ErrorKind::Io, e.to_string()).caused_by(e)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn display_includes_location_when_known() {
    let e = Error::new(ErrorKind::WellFormedness, "mismatched end tag")
      .at(Location { line: 12, column: 5, ..Location::unknown() }.with_system_id("file:///doc.xml"));
    assert_eq!(e.to_string(), "file:///doc.xml:12:5: not well-formed: mismatched end tag");
  }

  #[test]
  fn display_omits_unknown_location() {
    let e = Error::new(ErrorKind::Internal, "unreachable");
    assert_eq!(e.to_string(), "internal error: unreachable");
  }

  #[test]
  fn severity_defaults_to_fatal_and_orders() {
    assert_eq!(Error::new(ErrorKind::Io, "x").severity(), Severity::Fatal);
    assert_eq!(Error::recoverable(ErrorKind::Validity, "x").severity(), Severity::Error);
    assert!(Severity::Warning < Severity::Error && Severity::Error < Severity::Fatal);
  }
}
