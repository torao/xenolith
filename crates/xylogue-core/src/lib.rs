//! Core primitives shared by every layer of xylogue.
//!
//! This crate deliberately knows nothing about parsing, trees or transformation. It provides
//! the vocabulary the rest of the workspace agrees on:
//!
//! - [`error`] — errors carrying a [`Location`] in the *entity* they occurred in.
//! - [`chars`] — the character classes of XML 1.0 Fifth Edition.
//! - [`name`] — interned names, [`QName`] and [`ExpandedName`].
//! - [`uri`] — RFC 3986 references and resolution, the basis of base URI handling.
//! - [`encoding`] — the [`Decoder`](encoding::Decoder) seam and the built-in encodings.
//!
//! # Examples
//!
//! Reading an entity involves all of them: sniff the encoding, decode the bytes, check the
//! names, and resolve relative references against the entity's own URI.
//!
//! ```
//! use xylogue_core::{NamePool, UriReference, chars, encoding};
//!
//! let bytes = b"\xEF\xBB\xBF<doc href='sub/part.xml'/>";
//!
//! // 1. Sniff, then skip the byte-order mark the decoder must not see.
//! let detected = encoding::detect(bytes);
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
//! # Ok::<(), xylogue_core::Error>(())
//! ```
//!
//! # Feature flags
//!
//! - `encodings` (default): encodings beyond UTF-8/UTF-16/US-ASCII/ISO-8859-1, via
//!   `encoding_rs`. With it off, [`encoding::decoder_for`] reports
//!   [`ErrorKind::UnsupportedFeature`] rather than silently falling back.

//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XML 1.0 (Fifth Edition)] — W3C Recommendation 26 November 2008. [`chars`] is its character
//!   classes and productions; [`encoding`] its §4.3.3 rules for reading a document's encoding.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation 8 December 2009. [`name`] is
//!   its `QName`, prefix and expanded-name model.
//! - [RFC 3986] — Uniform Resource Identifier (URI): Generic Syntax, January 2005. [`uri`] is
//!   its §5.3 reference resolution.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986

pub mod chars;
pub mod encoding;
pub mod error;
pub mod name;
pub mod uri;

pub use error::{Error, ErrorKind, Location, Result, Severity};
pub use name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};
pub use uri::UriReference;
