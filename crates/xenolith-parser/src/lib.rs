//! XML 1.0 parsing for xenolith.
//!
//! The parser holds no I/O of its own. It is fed bytes and driven to make progress, so the
//! same core drives a blocking reader, an async reader, or an in-memory slice, and can stop
//! mid-document to request an entity that someone else fetches.
//!
//! It reads the DTD in full — internal subset, external subset, and internal and external
//! parameter entities: general entities are resolved in content and attributes, declared
//! attribute defaults are supplied, tokenized attribute values are normalized, and the
//! standalone and parameter-entity nesting constraints are enforced. Anything external — the
//! external subset, an external entity — is fetched through a [`resolve::UriResolver`] given to
//! the reader, off by default, since that is the XXE attack surface.
//!
//! # Where to start
//!
//! [`Reader`] is the ordinary way in: give it anything that implements [`std::io::Read`] and
//! call [`advance`](Reader::advance).
//!
//! ```
//! use xenolith_parser::{EventKind, Reader};
//!
//! let mut reader = Reader::with_system_id("<doc>text</doc>".as_bytes(), "file:///doc.xml");
//! while let Some(kind) = reader.advance()? {
//!   if kind == EventKind::Text {
//!     assert_eq!(reader.parser().event_ref().and_then(|e| e.text()), Some("text"));
//!   }
//! }
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! That pull loop is the usual choice. For the push style, with the parser calling you, implement a
//! [`Handler`](sax::Handler) and run a source through it with [`emit`](sax::EventSource::emit); see the [`sax`] module.
//! Both run the same parser, so it is a choice of shape, not capability: reach for [`sax`] when your code is a handler
//! that dispatches on the event kind, or when porting a Java SAX `ContentHandler`, and stay with [`Reader`] otherwise.
//!
//! The other items sit beneath it:
//!
//! - [`Parser`] — the core: feed it bytes, call [`advance`](Parser::advance), read the event
//!   through [`event_ref`](Parser::event_ref). Drive this directly to control where the bytes come from.
//! - [`AsyncReader`] — the same, over [`futures_io::AsyncRead`], behind the `async` feature.
//! - [`Event`] — an event that owns its data, for when the borrow is in the way.
//! - [`resolve`] — [`resolve::UriResolver`] and the request the parser hands out when it needs
//!   an external entity; give a resolver to a reader with `with_resolver`.
//! - [`CharStream`] — bytes to characters for one entity: decoding, line-end normalization,
//!   `Char` checking, and position tracking.
//! - [`Entity`], [`EntityStack`] — the entities being read, innermost last, with base URIs
//!   and the [`Limits`] that keep a hostile document bounded.
//!
//! # Borrowed or owned
//!
//! The [`EventRef`] from [`Parser::event_ref`] borrows the parser's buffers, so an event is
//! readable only until the next [`advance`](Parser::advance) and cannot be collected. That costs
//! nothing per event, which matters for a large document. When events need to outlive the call — to
//! be collected, compared or sent elsewhere — [`Event::capture`] copies one, and
//! [`Reader::events`] gives an iterator of them.
//!
//! # Feeding the parser by hand
//!
//! Nothing is consumed until a complete token has been recognized, which is what lets the
//! same scan be retried after more input arrives:
//!
//! ```
//! use xenolith_parser::{CharStream, Entity, EntityStack, Limits};
//!
//! // The encoding is given here so the example is not also about sniffing; `CharStream::new`
//! // would hold the first bytes back until it can determine the encoding.
//! let document = Entity::document(CharStream::with_encoding("UTF-8")?);
//! let mut stack = EntityStack::new(document, Limits::default());
//!
//! // A first chunk that ends in the middle of a tag.
//! stack.feed(b"<doc", false)?;
//! assert_eq!(stack.current().stream().remainder(), "<doc");
//! assert!(!stack.current().stream().remainder().contains('>'), "the tag is incomplete: consume nothing");
//!
//! // After the rest arrives, the token is rescanned from its start and consumed.
//! stack.feed(b"/>", true)?;
//! let stream = stack.current_mut().stream_mut();
//! assert_eq!(stream.remainder(), "<doc/>");
//! stream.advance("<doc/>".len());
//! assert!(stream.is_fully_read());
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! # Feature flags
//!
//! - `encodings` (default): encodings beyond UTF-8/UTF-16/US-ASCII/ISO-8859-1.
//! - `async`: the runtime-agnostic [`AsyncReader`], over `futures_io::AsyncRead`. Off by
//!   default; an application can drive it with any executor and supply its own async I/O.
//! - `tokio`: adapters that bridge `tokio`'s own `AsyncRead` to the async driver, chiefly
//!   [`AsyncEntityReader::from_tokio`](async_resolve::AsyncEntityReader::from_tokio). Enables `async`;
//!   off by default.
//! - `xml-base`: per-node base URI computation from `xml:base` and the entity's system
//!   identifier (XML Base); read it with [`Parser::base_uri`].
//! - `xml-id`: `xml:id` as an ID-typed attribute, with tokenized normalization; read it with
//!   [`Parser::xml_id`]. Both are switched per parser through [`ParserConfig`].

//! # Specifications
//!
//! Implemented from these documents. The links are dated so the exact version read can be found:
//!
//! - [XML 1.0 (Fifth Edition)] — W3C Recommendation 26 November 2008. The whole of the parser:
//!   documents, the DTD, entities, and the well-formedness constraints.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation 8 December 2009. Prefix
//!   resolution and the namespace constraints.
//! - [XML Base (Second Edition)] — W3C Recommendation 28 January 2009. Behind the `xml-base`
//!   feature; see [`Parser::base_uri`].
//! - [xml:id 1.0] — W3C Recommendation 9 September 2005. Behind the `xml-id` feature; see
//!   [`Parser::xml_id`].
//!
//! The suite the parser is measured against is the [W3C XML Conformance Test Suite]; see the
//! README for how to run it.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/
//! [xml:id 1.0]: https://www.w3.org/TR/2005/REC-xml-id-20050909/
//! [W3C XML Conformance Test Suite]: https://www.w3.org/XML/Test/

#[cfg(feature = "async")]
pub mod async_reader;
#[cfg(feature = "async")]
pub mod async_resolve;
pub mod config;
pub mod entity;
pub mod event;
mod namespace;
pub mod parser;
pub mod reader;
pub mod sax;
mod scan;

#[cfg(feature = "async")]
pub use async_reader::{AsyncReader, NoResolver};
pub use config::{Bounds, ParserConfig};
// The DTD model and its parser are their own crate, usable without a document parser. They are re-exported here
// because this crate hands `Dtd` values out, through `Parser::dtd` and the `doctype` callback.
#[cfg(feature = "async")]
pub use async_resolve::{AsyncEntityReader, AsyncUriResolver};
pub use entity::{Entity, EntityKind, EntityStack, Limits};
pub use event::{Attribute, Event};
pub use parser::{Attributes, EventKind, EventRef, Events, Parser, Progress, XmlSpace};
pub use reader::{Reader, ReaderEvents};
pub use resolve::{EntityRequest, RequestKind, UriResolver};
pub use stream::CharStream;
pub use xenolith_core::attr::AttributeRef;
pub use xenolith_core::{resolve, stream};
pub use xenolith_dtd as dtd;
pub use xenolith_dtd::Dtd;
