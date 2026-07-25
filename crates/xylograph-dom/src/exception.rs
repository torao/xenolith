//! `DOMException` and its code system.
//!
//! The DOM reports a failed operation by raising a `DOMException` carrying a numeric code. The
//! same codes appear here as [`ExceptionCode`]; a fallible DOM method returns
//! `Result<_, DomException>` rather than throwing. The full Level 3 code list is defined so the
//! set does not shift as later phases begin to raise more of them.

use std::fmt;

/// The reason a DOM operation failed, as a `DOMException` code (DOM Level 3 Core).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ExceptionCode {
  /// An index or size was negative or past the allowed range.
  IndexSize = 1,
  /// A range of text does not fit in a `DOMString`.
  DomstringSize = 2,
  /// A node was inserted somewhere it does not belong.
  HierarchyRequest = 3,
  /// A node was used in a document other than the one that created it.
  WrongDocument = 4,
  /// A name contains a character not allowed in it.
  InvalidCharacter = 5,
  /// Data was set on a node that does not support it.
  NoDataAllowed = 6,
  /// An attempt was made to modify a node that is read-only.
  NoModificationAllowed = 7,
  /// A node or object referenced does not exist.
  NotFound = 8,
  /// The implementation does not support the requested operation or object.
  NotSupported = 9,
  /// An attribute already in use elsewhere was set again.
  InuseAttribute = 10,
  /// The object is in a state that does not allow the operation.
  InvalidState = 11,
  /// A string did not match the expected syntax.
  Syntax = 12,
  /// An object cannot be modified in the way requested.
  InvalidModification = 13,
  /// A namespace error, as defined by Namespaces in XML.
  Namespace = 14,
  /// A parameter or operation is not supported by the underlying object.
  InvalidAccess = 15,
  /// A call to a method such as `insertBefore` or `removeChild` would make the node invalid.
  Validation = 16,
  /// The type of an object is incompatible with the expected type.
  TypeMismatch = 17,
}

/// A failed DOM operation: a [code](ExceptionCode) and a message naming what went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomException {
  code: ExceptionCode,
  message: String,
}

impl DomException {
  /// Creates an exception with a code and a message.
  #[must_use]
  pub fn new(code: ExceptionCode, message: impl Into<String>) -> Self {
    Self { code, message: message.into() }
  }

  /// The exception code.
  #[must_use]
  pub const fn code(&self) -> ExceptionCode {
    self.code
  }

  /// The human-readable description.
  #[must_use]
  pub fn message(&self) -> &str {
    &self.message
  }
}

impl fmt::Display for DomException {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{:?}: {}", self.code, self.message)
  }
}

impl std::error::Error for DomException {}

/// The result of a fallible DOM operation.
pub type Result<T> = std::result::Result<T, DomException>;
