//! XML 1.0 parsing for xylograph.
//!
//! The parser holds no I/O of its own. It is fed bytes and asked to make progress, so the
//! same core drives a blocking reader, an async reader, or an in-memory slice, and can stop
//! mid-document to ask for an entity that someone else fetches. See `ROADMAP.md`, decision 7.
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
//! use xylograph_parser::{EventKind, Reader};
//!
//! let mut reader = Reader::with_system_id("<doc>text</doc>".as_bytes(), "file:///doc.xml");
//! while let Some(kind) = reader.advance()? {
//!   if kind == EventKind::Text {
//!     assert_eq!(reader.parser().text(), "text");
//!   }
//! }
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! The rest exists underneath it:
//!
//! - [`Parser`] — the core: feed it bytes, call [`advance`](Parser::advance), read the event
//!   through the accessors. Drive this directly to control where the bytes come from.
//! - [`AsyncReader`] — the same, over [`tokio::io::AsyncRead`], behind the `tokio` feature.
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
//! The accessors on [`Parser`] borrow from its buffers, so an event is readable only until
//! the next [`advance`](Parser::advance) and cannot be collected. That costs nothing per
//! event, which matters for a large document. When events need to outlive the call — to be
//! collected, compared or sent elsewhere — [`Event::capture`] copies one, and
//! [`Reader::events`] gives an iterator of them.
//!
//! # Feeding the parser by hand
//!
//! Nothing is consumed until a complete token has been recognized, which is what lets the
//! same scan be retried after more input arrives:
//!
//! ```
//! use xylograph_parser::{CharStream, Entity, EntityStack, Limits};
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
//! assert!(stream.is_exhausted());
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! # Feature flags
//!
//! - `encodings` (default): encodings beyond UTF-8/UTF-16/US-ASCII/ISO-8859-1.
//! - `tokio`: [`AsyncReader`], over `tokio`'s `AsyncRead`. Off by default; only
//!   `tokio`'s `io-util` is pulled in, and the runtime remains the caller's choice.
//! - `xml-base`: per-node base URI computation from `xml:base` and the entity's system
//!   identifier (XML Base); read it with [`Parser::base_uri`].
//! - `xml-id`: `xml:id` as an ID-typed attribute, with tokenized normalization; read it with
//!   [`Parser::xml_id`]. Both are switched per parser through [`ParserConfig`].

#[cfg(feature = "tokio")]
pub mod async_reader;
pub mod config;
pub mod dtd;
pub mod entity;
pub mod event;
mod namespace;
pub mod parser;
pub mod reader;
pub mod resolve;
pub mod sax;
mod scan;
pub mod stream;

#[cfg(feature = "tokio")]
pub use async_reader::{AsyncReader, NoResolver};
pub use config::ParserConfig;
pub use dtd::Dtd;
pub use entity::{Entity, EntityKind, EntityStack, Limits};
pub use event::{Attribute, Event};
pub use parser::{AttributeRef, EventKind, Events, Parser, Progress, XmlSpace};
pub use reader::{Reader, ReaderEvents};
#[cfg(feature = "tokio")]
pub use resolve::AsyncUriResolver;
pub use resolve::{EntityRequest, RequestKind, UriResolver};
pub use stream::CharStream;
