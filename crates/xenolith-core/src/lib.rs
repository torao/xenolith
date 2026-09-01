//! Core primitives shared by every layer of xenolith.
//!
//! This crate provides vocabulary that is commonly used in order parts of the workspace. It deliberately does not deal
//! with concepts such as parsing, tree structures, or transformations:
//!
//! - [`error`] — errors carrying a [`Location`] in the *entity* they occurred in.
//! - [`chars`] — the character classes of XML 1.0 Fifth Edition.
//! - [`name`] — interned names, [`QName`] and [`ExpandedName`].
//! - [`attr`] — [`Attributes`], a source-independent view of an element's attributes.
//! - [`validate`] — the [`Validator`] contract and the errors it reports, for any source to target.
//! - [`uri`] — RFC 3986 references and resolution, the basis of base URI handling.
//! - [`encoding`] — the [`Decoder`](encoding::Decoder) seam and the built-in encodings.
//!
//! # Examples
//!
//! Reading an entity involves all of them: sniff the encoding, decode the bytes, check the
//! names, and resolve relative references against the entity's own URI.
//!
//! ```
//! use xenolith_core::{NamePool, UriReference, chars, encoding};
//!
//! let bytes = b"\xEF\xBB\xBF<doc href='sub/part.xml'/>";
//!
//! // 1. Sniff, then skip the byte-order mark the decoder must not see.
//! let detected = encoding::detect(bytes).or_default();
//! assert_eq!(detected.encoding, "UTF-8");
//! let mut decoder = encoding::decoder_for(&detected.encoding)?;
//! let mut text = String::new();
//! decoder.decode(&bytes[detected.bom_length..], &mut text, true)?;
//! assert!(text.starts_with("<doc"));
//!
//! // 2. Names are validated against XML 1.0 Fifth Edition, then interned.
//! assert!(chars::is_name("doc"));
//! let mut pool = NamePool::new();
//! let doc = pool.intern("doc");
//! assert_eq!(pool.resolve(doc), "doc");
//!
//! // 3. Relative references resolve against the base URI of the entity.
//! let base = UriReference::parse("file:///docs/main.xml")?;
//! let href = UriReference::parse("sub/part.xml")?;
//! assert_eq!(base.resolve(&href).to_string(), "file:///docs/sub/part.xml");
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! # Feature flags
//!
//! - `encodings` (default): By default, xenolith can utirize the character encodings supported by [`encoding_rs`]. If
//!   this is disabled, only UTF-8, UTF-16, US-ASCII, and ISO-8859-1 will be available; any other encoding will result
//!   in a [`Error::UnsupportedFeature`] being reported via [`encoding::decoder_for`].
//!
//! # Specifications
//!
//! These were implemented based on the following documents. The dates shown are the last modified dates on the
//! documents at the time of access.
//!
//! - [XML 1.0 (Fifth Edition)] — W3C Recommendation (26 November 2008). [`chars`] refers to the character class and
//!   production urles; [`encoding`] refers to the rules in §4.3.3 for determining the encoding of the document.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation (8 December 2009). [`name`] refers to `QName`,
//!   prefix, and expanded-name model.
//! - [RFC 3986] — Uniform Resource Identifier (URI): Generic Syntax (January 2005). [`uri`] refers to §5.3 reference
//!   resolution.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986

pub mod attr;
pub mod chars;
pub mod encoding;
pub mod error;
pub mod name;
pub mod uri;
pub mod validate;

pub use attr::{AttributeList, AttributeRef, Attributes};
pub use error::{Error, Location, Result, Severity};
pub use name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};
pub use uri::UriReference;
pub use validate::{CollectErrors, ErrorListener, FailFast, Validator, ValidityError};
