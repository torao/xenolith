//! An XML 1.0 processor library: a namespace-aware parser, a DOM tree, DTD validation, and serialization.
//!
//! This crate implements XML 1.0 Fifth Edition and Namespaces in XML 1.0. It reads XML from a byte stream and reports
//! it as an event stream, validates the event stream, constructs a tree from those events, validates documents against
//! a DTD, and writes out trees or event streams as text. It includes functionality for character decoding, entity
//! resolution, DTD processing, and checking constraints regarding well-formedness and validity.
//!
//! What it does:
//!
//! - Reads a document as events, pulled one at a time or pushed to handlers.
//! - Builds a DOM tree from events, and reports a tree back as events.
//! - Checks a document against the DTD it declares, or against a DTD held as a schema of its own.
//! - Writes a tree, or a sequence of events, as well-formed XML text.
//!
//! Nothing is fetched unless the caller says how. An external entity or an external DTD subset is read only through a
//! [`UriResolver`](io::resolve::UriResolver) the caller installs, and a document that refers to one without a resolver
//! is refused with a reason rather than read as though the reference were absent. Entity expansion is bounded by
//! [`Limits`](io::Limits), whatever the document declares.
//!
//! # Key features
//!
//! - [`event`]: Event-related vocabulary. This includes the [`EventRef`](event::EventRef),
//!   [`EventHandler`](event::EventHandler), and [`EventSource`](event::EventSource) traits shared by all producers and
//!   consumers; the strict check that separates XML from Loose XML; and the [`Validator`] contract implemented by
//!   schemas. Events store names and text as `&str`.
//! - [`io`]: Input/output processing. It employs a *sans-I/O pattern* where the [`Parser`](io::Parser) itself does not
//!   manage I/O; instead, a [`StreamSource`](io::StreamSource) drives the parser via [`std::io::Read`]. When the
//!   `async` feature is enabled, an `AsyncReader` performs similar processing asynchronously. It also covers character
//!   decoding, character streams per entity, XML and text declarations, entity resolution, and [`io::write`] for
//!   serialization.
//! - [`dtd`]: The Document Type Definition. [`dtd::model`] stores declarations as data, while [`dtd::read`] parses
//!   them from a `DOCTYPE` or a standalone DTD. [`dtd::validate`] allows for document validation based on a DTD.
//! - [`dom`]: Document Object Model. It maintains a collection of nodes — identified by [`NodeId`](dom::NodeId) (which
//!   implements the `Copy` trait) — using an *arena structure*. Tools are provided such as [`dom::build`] for
//!   constructing a tree from events, and [`DomSource`](dom::DomSource) for outputting a tree as events.
//!
//! Primitives shared across these four modules:
//!
//! - [`error`]: Errors; each holds the [`Location`] (positional information) within the entity where the error
//!   occurred.
//! - [`chars`]: Character classes and name production rules from XML 1.0 Fifth Edition.
//! - [`name`]: Interned names using a string pool, [`QName`], and [`ExpandedName`].
//! - [`attr`]: [`Attributes`]; a view of element attributes that is independent of how the attributes were generated.
//! - [`uri`]: Reference and resolution handling based on RFC 3986; forms the basis for base URI handling.
//!
//! XPath, XSLT, and XInclude are provided as separate crates built upon this crate.
//!
//! # Getting Started
//!
//! The [`Reader`] reads a byte stream to generate events or a tree, and the [`Writer`] writes out a tree. These
//! components should be integrated with the aforementioned modules to ensure strict lexical validation and serve as
//! the primary entry points for usage, unless a specific component needs to operate in isolation.
//!
//! At the next layer down, it is able to construct an event pipeline using an [`EventSource`](event::EventSource) —
//! which generates [`EventRef`](event::EventRef)s — and an [`EventHandler`](event::EventHandler) — which receives them.
//! [`io::StreamSource`] wraps any [`std::io::Read`] implementation and passes generated events to registered handlers.
//! Handlers can include components such as [`DomBuilder`](dom::build::DomBuilder) (for tree construction),
//! [`Validator`] (for event stream validation), or custom implementations of [`EventHandler`](event::EventHandler).
//! Using [`emit`](event::EventCursor::emit) triggers a SAX-style "push" behavior that processes the entire stream,
//! whereas using [`next`](event::EventCursor::next) enables a StAX-style "pull" behavior that retrieves events one by
//! one.
//! At an even lower layer, [`io::Parser`] allows the caller to manually supply byte data via
//! [`feed`](io::Parser::feed) to extract the next event.
//!
//!
//! # Examples
//!
//! You can also use the parser and primitives independently. They support operations such as identifying the encoding,
//! decoding byte sequences, validating names, and resolving relative references based on the entity's URI.
//!
//! ```
//! use xenolith::io::encoding;
//! use xenolith::{NamePool, UriReference, chars};
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
//! # Ok::<(), xenolith::Error>(())
//! ```
//!
//! # Feature flags
//!
//! Each feature gate corresponds to crates implemented as an external library rather than part of the core XML
//! functionality; thus, you can choose whether to include them in the compilation via flags.
//!
//! - `encodings` (default): Enables character encodings supported by [`encoding_rs`]. If this is not specified, only
//!   UTF-8, UTF-16, US-ASCII, and ISO-8859-1 are available; attempting to use other encodings will result in an
//!   [`Error::UnsupportedFeature`] error.
//! - `async`: Enables an `AsyncReader` based on `futures_io::AsyncRead` that is agnostic of the runtime. Applications
//!   can drive this using any execution environment (executor) and provide their own asynchronous I/O implementation.
//!   Adding the `tokio` feature enables the `async` feature and includes an adapter that bridges the reader with
//!   `tokio`'s own `AsyncRead` trait.
//!
//! # Specifications
//!
//! These were implemented based on the following documents. The dates listed indicate when the documents were last
//! updated at the time of access.
//!
//! - [XML 1.0 (Fifth Edition)] — W3C Recommendation (26 November 2008). The entirety of [`io`] and [`dtd`] (documents,
//!   DTDs, entities, and well-formedness constraints); [`chars`] holds character classes and production rules, and
//!   [`io::encoding`] holds the rules from §4.3.3 for determining document encoding.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation (8 December 2009). Prefix resolution and namespace
//!   constraints; [`name`] holds models for `QName`, prefixes, and expanded-names.
//! - [XML Base (Second Edition)] — W3C Recommendation (28 January 2009). Computation of per-node base URIs based on
//!   `xml:base` and entity system identifiers; accessed via [`Parser::base_uri`](io::Parser::base_uri).
//! - [xml:id 1.0] — W3C Recommendation (9 September 2005). `xml:id` as an ID-type attribute with token normalization;
//!   accessed via [`Parser::xml_id`](io::Parser::xml_id).
//! - [DOM Level 3 Core] — W3C Recommendation (7 April 2004). The structure presented by [`dom`] (not the IDL itself).
//! - [RFC 3986] — Uniform Resource Identifier (URI): Generic Syntax (January 2005). [`uri`] refers to the reference
//!   resolution described in §5.3.
//!
//! The parser has been verified against the [W3C XML Conformance Test Suite]. Please refer to the developer guide for
//! instructions on how to run it.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/
//! [xml:id 1.0]: https://www.w3.org/TR/2005/REC-xml-id-20050909/
//! [DOM Level 3 Core]: https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/
//! [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986
//! [W3C XML Conformance Test Suite]: https://www.w3.org/XML/Test/
//! [`encoding_rs`]: https://docs.rs/encoding_rs
//!

pub mod _design;
pub mod attr;
pub mod chars;
pub mod dom;
pub mod dtd;
pub mod error;
pub mod event;
pub mod io;
pub mod name;
pub mod uri;

mod facade;

pub use attr::{Attribute, AttributeList, AttributeRef, Attributes};
pub use error::{Error, Location, Result};
pub use event::validate::{Schema, Validator, ValidityError};
pub use facade::{Reader, Writer};
pub use name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};
pub use uri::UriReference;
