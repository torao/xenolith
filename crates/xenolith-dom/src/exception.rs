//! `DOMException` and its code system.
//!
//! The W3C DOM specification reports a failed operation by raising a `DOMException` carrying a numeric code. Those
//! codes are listed here as [`ExceptionCode`]. A fallible DOM method returns `Result<_, DomException>` rather than
//! throwing. All Level 3 codes are defined, not just the ones raised so far, so later code can return any of them
//! without changing this enum.
//!

use std::fmt;

/// The reason a DOM operation failed, as a `DOMException` code (DOM Level 3 Core).
///
/// The variants keep the W3C DOM specification's own constant names rather than Rust's usual `UpperCamelCase`.
///
/// <https://www.w3.org/TR/2003/WD-DOM-Level-3-Core-20030226/DOM3-Core.html#core-ID-258A00AF>
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
#[allow(non_camel_case_types)] // intentionally, the W3C DOM specification's constant names: INDEX_SIZE_ERR and the rest
pub enum ExceptionCode {
  /// An index or size was negative or past the allowed range.
  INDEX_SIZE_ERR = 1,
  /// A range of text does not fit in a `DOMString`.
  DOMSTRING_SIZE_ERR = 2,
  /// A node was inserted somewhere it does not belong.
  HIERARCHY_REQUEST_ERR = 3,
  /// A node was used in a document other than the one that created it.
  WRONG_DOCUMENT_ERR = 4,
  /// A name contains a character that is not allowed.
  INVALID_CHARACTER_ERR = 5,
  /// Data was set on a node that does not support it.
  NO_DATA_ALLOWED_ERR = 6,
  /// An operation would modify a read-only node.
  NO_MODIFICATION_ALLOWED_ERR = 7,
  /// A referenced node or object does not exist.
  NOT_FOUND_ERR = 8,
  /// The implementation does not support the requested operation or object.
  NOT_SUPPORTED_ERR = 9,
  /// An attribute already in use elsewhere was set again.
  INUSE_ATTRIBUTE_ERR = 10,
  /// The object is in a state that does not allow the operation.
  INVALID_STATE_ERR = 11,
  /// A string did not match the expected syntax.
  SYNTAX_ERR = 12,
  /// An object cannot be modified in the way requested.
  INVALID_MODIFICATION_ERR = 13,
  /// A namespace error, as defined by Namespaces in XML.
  NAMESPACE_ERR = 14,
  /// A parameter or operation is not supported by the underlying object.
  INVALID_ACCESS_ERR = 15,
  /// A call to a method such as `insertBefore` or `removeChild` would make the node invalid.
  VALIDATION_ERR = 16,
  /// The type of an object is incompatible with the expected type.
  TYPE_MISMATCH_ERR = 17,
}

/// A failed DOM operation: a [code](ExceptionCode) and a message naming what went wrong.
///
/// <https://www.w3.org/TR/2003/WD-DOM-Level-3-Core-20030226/DOM3-Core.html#core-ID-17189187>
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomException {
  code: ExceptionCode,
  message: String,
}

impl DomException {
  /// Creates an exception with the given code and message.
  ///
  #[must_use]
  pub fn new(code: ExceptionCode, message: impl Into<String>) -> Self {
    Self { code, message: message.into() }
  }

  /// The exception code.
  ///
  #[must_use]
  pub const fn code(&self) -> ExceptionCode {
    self.code
  }

  /// The human-readable description.
  ///
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn display_uses_the_spec_constant_name() {
    // The value a caller sees must read as the code other DOM implementations report, not as a Rust identifier.
    let error = DomException::new(ExceptionCode::HIERARCHY_REQUEST_ERR, "a text node cannot have children");
    assert_eq!(error.to_string(), "HIERARCHY_REQUEST_ERR: a text node cannot have children");
  }
}
