//! Errors and source locations.
//!

#[cfg(test)]
mod test;

use std::fmt;
use std::sync::Arc;

use crate::dom::DomException;

/// Result type used throughout xenolith.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Indicates a position within an entity.
///
/// The XML/SAX specification requires that if an error occurs while the parser is reading the content of an external
/// entity referenced from an XML document, the parser must report the position within that external entity rather than
/// the position in the original XML document where the reading began. The position within the external entity is
/// reported as the entity's system identifier followed by a line number starting at 1.
///
/// # Examples
///
/// ```
/// use xenolith::Location;
///
/// // A reader starts at the first character of the entity and updates its position as it advances.
/// let mut at = Location::new().with_system_id("file:///docs/part.ent");
/// for c in "<p>hi\nthere".chars() {
///   at.advance(c);
/// }
/// assert_eq!((at.line, at.column, at.offset), (2, 6, 11));
/// assert_eq!(at.to_string(), "file:///docs/part.ent:2:6");
///
/// // The position for unknown position remains unknown regardless of what is read.
/// let mut nowhere = Location::unknown();
/// nowhere.advance('\n');
/// assert!(nowhere.is_unknown());
/// assert_eq!(nowhere.to_string(), "<unknown>");
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
  /// The initial starting position for the entity (line 1, column 1, offset 0). No identifier is set. Neither the
  /// public identifier nor the system identifier is set.
  ///
  /// A starting point for entity reading by the reader is established, and the positional information is updated as
  /// [`advance`](Self::advance) progresses through the characters. An identifier for the entity is assigned using
  /// [`with_system_id`](Self::with_system_id).
  ///
  #[must_use]
  pub const fn new() -> Self {
    Self { system_id: None, public_id: None, line: 1, column: 1, offset: 0 }
  }

  /// A location with no location information.
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

  /// Returns a copy with `public_id` set.
  ///
  /// The public identifier is a name assigned to an entity in a declaration — such as `-//W3C//DTD XHTML 1.0
  /// Strict//EN` — and does not indicate the source from which the entity is loaded. Use a system identifier to
  /// specify the location of the entity, such as a file URI.
  ///
  #[must_use]
  pub fn with_public_id(mut self, public_id: impl Into<Arc<str>>) -> Self {
    self.public_id = Some(public_id.into());
    self
  }

  /// True if indicating an unknown position.
  #[must_use]
  pub fn is_unknown(&self) -> bool {
    self.line == 0 && self.column == 0 && self.offset == 0
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
  /// An "unknown location" does not advance.
  ///
  pub fn advance(&mut self, c: char) {
    if self.is_unknown() {
      return;
    }
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
    let entity = self.system_id.as_deref().unwrap_or("<unknown>");
    if self.is_unknown() {
      return f.write_str(entity);
    }
    write!(f, "{entity}:{}:{}", self.line, self.column)
  }
}

/// A location shared by errors that have no specific location as "somewhere."
static UNKNOWN_LOCATION: Location = Location::unknown();

/// An error that occurred at some point in the xenolith pipeline.
///
/// Each variant represents a distinct failure type and conveys the information intended for that failure. As the
/// number of error types may increase with future fixes or the implementation of subsequent phases, a wildcard arm
/// should be used to catch unexpected errors in order to ensure backward compatibility.
///
/// # Examples
///
/// ```
/// use xenolith::{Error, Location};
///
/// let err = Error::well_formedness("mismatched end tag: expected </a>")
///   .at(Location { line: 12, column: 3, ..Location::unknown() }.with_system_id("file:///doc.xml"));
///
/// assert!(matches!(err, Error::WellFormedness { .. }));
/// assert_eq!(err.location().line, 12);
/// assert_eq!(err.to_string(), "not well-formed: mismatched end tag: expected </a>");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
  /// A failure caused while reading or writing an entity.
  #[error("I/O error: {message}")]
  Io {
    /// The location where the read or write operation was taking place (if known).
    location: Location,
    /// Description of the problem.
    message: String,
    /// The underlying I/O error.
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
  },

  /// A failure caused by the application's implementation of the entity resolver invoked by the parser.
  ///
  /// This variant allows the application to store its own error as a [`source`](std::error::Error::source) because
  /// the parser cannot determine exactly what occurred within the application callback. The caller can retrieve the
  /// original error by down-casting the `source`.
  ///
  #[error("resolver error: {message}")]
  Resolver {
    /// The location where the parser was positioned at the time of the call (if known).
    location: Location,
    /// Description of the problem.
    message: String,
    /// An application-specific exception; it is preserved so that the caller can downcast and recover from it.
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
  },

  /// Malformed input for the character encoding in use, or the encoding is unsupported.
  #[error("encoding error: {message}")]
  Encoding {
    /// The location within the entity where decoding failed.
    location: Location,
    /// A message explaining the issue and what the caller can do about it.
    message: String,
    /// The *byte position* (zero-based index) of the first byte that could not be decoded within the byte sequence
    /// passed to the decoder during the call that caused the error. `None` if there is no specific byte position
    /// causing the error (e.g., if the encoding name is unknown or the entity ends in the middle of a character).
    byte_offset: Option<usize>,
  },

  /// Malformed URI or unresolvable relative reference.
  #[error("URI error: {message}")]
  Uri {
    /// The location of the reference (if known).
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// A failure indicating that a string does not conform to the required XML generation rules (such as `Name` or
  /// `NCName`).
  #[error("name error: {message}")]
  Name {
    /// The location where the name appeared (if known).
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// Well-formedness violation.
  #[error("not well-formed: {message}")]
  WellFormedness {
    /// The location where the well-formedness violation was detected (if known).
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// Validity constraint violation (recoverable).
  #[error("not valid: {message}")]
  Validity {
    /// The location where the violation was detected (if known).
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// Invalid DOM operation.
  ///
  /// This wraps to report a [`DomException`]. It primarily occurs when [`DomBuilder`](crate::dom::build::DomBuilder)
  /// is building a DOM based on a non-strict event sequence.
  #[error("DOM error: {message}")]
  Dom {
    /// The location where the violation was detected (if known).
    location: Location,
    /// Description of the problem, which is the exception's own, its code included.
    message: String,
    /// The exception as the DOM raised it, so its [`code`](DomException::code) can be matched on. It is this error's
    /// [`source`](std::error::Error::source) as well.
    #[source]
    exception: DomException,
  },

  /// Namespace constraint violation.
  #[error("namespace error: {message}")]
  Namespace {
    /// The location where the violation was detected.
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// A resource limit have been exceeded (entity expansion, nesting depth, number of nodes).
  #[error("limit exceeded: {message}")]
  Limit {
    /// The location where the limit was reached.
    location: Location,
    /// Description on which limit was reached and how to increase it.
    message: String,
  },

  /// An XInclude processing error: An inclusion loop or a failed inclusion with no fallback.
  #[error("XInclude error: {message}")]
  XInclude {
    /// The location where the `xi:include` appeared.
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// A failure to parse the XPath expression or a failure during evaluation.
  #[error("XPath error: {message}")]
  XPath {
    /// The location of the error within the expression or style sheet (if known).
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// A stylesheet is not permitted for XSLT, or the transformation could not be performed.
  #[error("XSLT error: {message}")]
  Xslt {
    /// The location in the style sheet where the problem occurred (if known).
    location: Location,
    /// Description of the problem.
    message: String,
  },

  /// The requested feature is not included in the build. Please check the crate's feature flags; the message should
  /// indicate the name of the missing feature.
  #[error("unsupported feature: {message}")]
  UnsupportedFeature {
    /// Indicates which feature or capability provides it, and what can be done with the build instead. To ensure
    /// consistent wording, it is generated by [`Error::unsupported_feature`].
    message: String,
  },

  /// A bug occurred in the xenolith implementation or a dependent library, or the API was called in a manner
  /// prohibited by the API specification.
  #[error("internal error: {message}")]
  Internal {
    /// An explanation of what was expected versus what actually occurred, and the appropriate response for the caller
    /// to take. [`Error::internal`] words it as the former case, [`Error::misuse`] as the latter.
    message: String,
  },
}

impl Error {
  /// An I/O failure that occurred while reading or writing an entity, carrying neither location information nor
  /// underlying I/O error details.
  #[must_use]
  pub fn io(message: impl Into<String>) -> Self {
    Self::Io { location: Location::unknown(), message: message.into(), source: None }
  }

  /// This error is raised by an application resolver implementation invoked by the parser.
  ///
  /// You can use this function to wrap application-specific errors — such as database or network failures —
  /// encountered within the resolver implementation. These errors are propagated to the caller via the parser. The
  /// application error is stored as the [`source`](std::error::Error::source), and its string representation serves as
  /// the error message.
  ///
  /// If the location of the error can be identified, attach location information using [`at`](Self::at). If no
  /// location information is provided, the location information known to the caller invoking the resolver will be
  /// attached instead.
  ///
  #[must_use]
  pub fn resolver(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
    let source = source.into();
    Self::Resolver { location: Location::unknown(), message: source.to_string(), source: Some(source) }
  }

  /// A character encoding failure.
  ///
  /// If the problematic byte has been identified and its location is known, specify the `byte_offset` to indicate that
  /// byte offset.
  ///
  #[must_use]
  pub fn encoding(message: impl Into<String>, byte_offset: Option<usize>) -> Self {
    Self::Encoding { location: Location::unknown(), message: message.into(), byte_offset }
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

  /// A validity constraint violation. It is used by validators that reject the document as soon as the first violation
  /// is detected.
  ///
  /// Validators that collect violations continue to return `Ok` while storing each violation as a
  /// [`ValidityError`](crate::ValidityError), allowing the validation process to proceed to the end of the document.
  /// Ultimately, [`ValidityError::to_error`](crate::ValidityError::to_error) produces this variant.
  ///
  #[must_use]
  pub fn validity(message: impl Into<String>) -> Self {
    Self::Validity { location: Location::unknown(), message: message.into() }
  }

  /// A namespace-constraint violation.
  #[must_use]
  pub fn namespace(message: impl Into<String>) -> Self {
    Self::Namespace { location: Location::unknown(), message: message.into() }
  }

  /// A resource limit was exceeded.
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

  /// The location where the error occurred, or an unknown location for types where no such location exists.
  #[must_use]
  pub fn location(&self) -> &Location {
    match self {
      Self::Io { location, .. }
      | Self::Resolver { location, .. }
      | Self::Encoding { location, .. }
      | Self::Uri { location, .. }
      | Self::Name { location, .. }
      | Self::WellFormedness { location, .. }
      | Self::Validity { location, .. }
      | Self::Dom { location, .. }
      | Self::Namespace { location, .. }
      | Self::Limit { location, .. }
      | Self::XInclude { location, .. }
      | Self::XPath { location, .. }
      | Self::Xslt { location, .. } => location,
      Self::UnsupportedFeature { .. } | Self::Internal { .. } => &UNKNOWN_LOCATION,
    }
  }

  /// For error variants where the `Location` field is defined, set that location. For other errors, take no action.
  #[must_use]
  pub fn at(mut self, location: Location) -> Self {
    match &mut self {
      Self::Io { location: at, .. }
      | Self::Resolver { location: at, .. }
      | Self::Encoding { location: at, .. }
      | Self::Uri { location: at, .. }
      | Self::Name { location: at, .. }
      | Self::WellFormedness { location: at, .. }
      | Self::Validity { location: at, .. }
      | Self::Dom { location: at, .. }
      | Self::Namespace { location: at, .. }
      | Self::Limit { location: at, .. }
      | Self::XInclude { location: at, .. }
      | Self::XPath { location: at, .. }
      | Self::Xslt { location: at, .. } => *at = location,
      Self::UnsupportedFeature { .. } | Self::Internal { .. } => {}
    }
    self
  }

  /// Adds the `Location` to the error if it does not include any location information. Otherwise, if the information
  /// is already present, it is preserved.
  ///
  /// While the layer catching an error from a lower layer often knows where the call was made, the code that triggered
  /// the error may not. This method lets the caller attach the location information it has.
  ///
  #[must_use]
  pub fn or_at(self, location: Location) -> Self {
    // Set the location only if nothing is known, so an error carrying just a system ID keeps it.
    if *self.location() == Location::unknown() { self.at(location) } else { self }
  }

  /// Associate the underlying cause for error types that have a cause. For other error types that do not have a cause,
  /// this method does nothing.
  ///
  #[must_use]
  pub fn caused_by(mut self, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
    if let Self::Io { source: cause, .. } | Self::Resolver { source: cause, .. } = &mut self {
      *cause = Some(source.into());
    }
    self
  }

  /// Generates an error for a DOM operation where the requested action was refused.
  #[must_use]
  pub fn dom(exception: DomException) -> Self {
    Self::Dom { location: Location::unknown(), message: exception.to_string(), exception }
  }

  /// Generates an error for a situation that should be unreachable.
  #[must_use]
  pub fn internal(what: impl std::fmt::Display) -> Self {
    Self::Internal { message: format!("{what}; this is a bug in xenolith, please report it") }
  }

  /// Generates an error for API calls that are not permitted, such as attempting to write an attribute without an open
  /// start tag.
  #[must_use]
  pub fn misuse(what: impl std::fmt::Display) -> Self {
    Self::Internal { message: what.to_string() }
  }

  /// Generates an error that is returned when a build is compiled with specific features disabled.
  #[must_use]
  pub fn unsupported_feature(capability: impl std::fmt::Display, feature: &str, fallback: &str) -> Self {
    Self::UnsupportedFeature {
      message: format!("{capability} needs the `{feature}` feature, which this build does not have; {fallback}"),
    }
  }

  /// The human-readable message.
  #[must_use]
  pub fn message(&self) -> &str {
    match self {
      Self::Io { message, .. }
      | Self::Resolver { message, .. }
      | Self::Encoding { message, .. }
      | Self::Uri { message, .. }
      | Self::Name { message, .. }
      | Self::WellFormedness { message, .. }
      | Self::Validity { message, .. }
      | Self::Dom { message, .. }
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

/// An [`Error::Dom`] is generated from code that reports a [`DomException`] using the `?` operator. In particular,
/// when using [`DomBuilder`](crate::dom::build::DomBuilder), a rejection on the DOM side becomes the rejection reason
/// for the operation itself.
///
/// Since DOM operations typically do not include location information regarding where an error occurred, the caller
/// must use [`or_at`](Error::or_at) to specify this information if it is known.
impl From<DomException> for Error {
  fn from(exception: DomException) -> Self {
    Self::dom(exception)
  }
}
