//! Bidirectional conversion between byte sequences and events: handles parsing on input and serialization on output.
//!
//! Definitions in the [`event`](crate::event) module operate on `&str` and [`Location`](crate::Location) rather than
//! directly handling byte sequences. This module manages byte-level operations, such as encoding detection and
//! decoding, newline normalization, character validation, location tracking, markup tokenization, and external entity
//! retrieval, as well as output-side processing like escaping and encoding.
//!
//! - [`parse`]: A sans-I/O [`Parser`] that performs no I/O itself; it requests necessary inputs or external entities
//!   from the caller, which then provides them.
//! - [`read`]: A set of drivers that perform actual I/O. [`StreamSource`] reads from [`std::io::Read`] and functions
//!   as an [`EventSource`](crate::event::EventSource). When the `async` feature is enabled, `AsyncReader` performs
//!   similar operations via `futures_io::AsyncRead`.
//! - [`encoding`]: Provides decoders, encoders, and functionality for detecting encoding based on BOMs (Byte Order
//!   Marks) or declarations.
//! - [`stream`]: [`CharStream`]; represents a stream of characters decoded from a single entity.
//! - [`decl`]: Scans XML declarations and text declarations.
//! - [`resolve`]: [`UriResolver`]; a trait implemented by the application to retrieve external entities.
//! - [`write`](mod@write): Provides a [`WriterSource`] for an application writing a document by hand, and an
//!   [`XmlWriter`] that writes the events it is given as XML text.
//!
//! The parser processes DTDs via the [`dtd`](crate::dtd) module. This includes processing internal subsets, external
//! subsets, and both internal and external parameter entities. It resolves general entity references within content
//! and attribute values, applies default values for declared attributes, and normalizes tokenized attribute values.
//! It also enforces constraints regarding standalone documents and parameter entity nesting.
//!
//! External resources (such as external subsets or external entities) are retrieved solely via the [`UriResolver`]
//! provided to the reader. To prevent XXE (XML External Entity) attacks, no resolver is configured by default. If the
//! document references an external resource that cannot be retrieved, an error identifying the request occurs, and
//! parsing fails; the reference is not skipped.

pub mod decl;
pub mod encoding;
pub mod parse;
pub mod read;
pub mod resolve;
pub mod stream;
pub mod write;

pub use parse::{
  Attributes, DocumentLimits, Entity, EntityKind, EntityLimits, EntityStack, Extensions, Limits, Parser, ParserConfig,
  Progress, TokenKind, TokenLimits, TokenRef,
};
pub use read::StreamSource;
#[cfg(feature = "async")]
pub use read::{AsyncEntityReader, AsyncReader, AsyncUriResolver, NoResolver};
pub use resolve::{EntityRequest, RequestKind, UriResolver};
pub use stream::CharStream;
pub use write::{WriterSource, XmlWriter};

// Types associated with `TokenRef`. These are re-exported so that callers handling tokens can import them from here.
pub use crate::attr::AttributeRef;
pub use crate::event::XmlSpace;
