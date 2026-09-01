//! Validation for xenolith.
//!
//! This crate provides a schema-agnostic contract. It defines a [`Validator`] that checks a document's events and
//! reports results via an [`ErrorListener`]. While a DTD validator serves as the initial implementation, the system is
//! not limited to DTDs; validators for RELAX NG, XML Schema, or even custom rules defined by the caller can be used,
//! provided they implement the same trait.
//!
//! # The boundary
//!
//! A validator checks constraints over the event stream. It does *not* expand entities, supply attribute defaults, or
//! type-check the way parsing does. Those belong to [`xenolith_parser`], which has already done them by the time an
//! event arrives. A namespace-aware schema (RELAX NG, XSD) or a custom rule can therefore sit on the same interface as
//! the DTD. All of them need only the events and the names.
//!
//! # Well-formed versus valid
//!
//! A well-formedness error is fatal and stops parsing, and the parser raises it. A *validity* error is recoverable.
//! The document is still a tree, just not one the schema allows, so a validator reports the error and continues, and
//! the [`ErrorListener`] decides whether to keep going. This corresponds to Java's `setValidating(true)`.
//!
//! # Examples
//!
//! ```
//! use xenolith_parser::Reader;
//! use xenolith_validate::Validatable;
//!
//! // The element `c` is used but never declared: a validity error, reported, not thrown.
//! let xml = "<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>";
//! let report = Reader::new(xml.as_bytes()).with_validation().validating_dtd().run()?;
//! assert!(!report.is_valid());
//! assert!(report.errors().iter().any(|e| e.to_string().contains("c")));
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! # Specifications
//!
//! Implemented from this document, at the version linked. The link is dated, so the exact version read can be found:
//!
//! - [XML 1.0 (Fifth Edition)], W3C Recommendation 26 November 2008. The DTD validator checks its
//!   [validity constraints]. The determinism requirement on content models is in [Appendix E].
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [validity constraints]: https://www.w3.org/TR/2008/REC-xml-20081126/#dt-validity-constraint
//! [Appendix E]: https://www.w3.org/TR/2008/REC-xml-20081126/#determinism

mod content;
pub mod dtd;
mod handler;
#[cfg(feature = "xml-id")]
pub mod ids;
mod validation;

pub use handler::ValidatingHandler;
#[cfg(feature = "xml-id")]
pub use ids::XmlIdValidator;
pub use validation::{Report, Validatable, ValidatingSource};
// The validation contract is core vocabulary, re-exported here so this crate reads as one API.
pub use xenolith_core::validate::{CollectErrors, ErrorListener, FailFast, Validator, ValidityError};

/// A compiled schema that yields a fresh [`Validator`] for each document.
///
/// The DTD schema is the document's own DTD, built when its `DOCTYPE` is read. An external schema (RELAX NG, XSD) is
/// compiled ahead of time and reused.
///
pub trait Schema {
  /// Returns a new validator for a single document.
  ///
  fn validator(&self) -> Box<dyn Validator>;
}
