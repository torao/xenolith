//! XML 1.0 parsing for xylograph.
//!
//! The parser holds no I/O of its own. It is fed bytes and asked to make progress, so the
//! same core drives a blocking reader, an async reader, or an in-memory slice, and can stop
//! mid-document to ask for an entity that someone else fetches. See `ROADMAP.md`, decision 7.
//!
//! This phase provides the layer below the tokenizer:
//!
//! - [`CharStream`] — bytes to characters for one entity: decoding, line-end normalization,
//!   `Char` checking, and position tracking.
//! - [`Entity`], [`EntityStack`] — the entities being read, innermost last, with base URIs
//!   and the [`Limits`] that keep expansion bounded.
//!
//! # Examples
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

pub mod entity;
pub mod stream;

pub use entity::{Entity, EntityKind, EntityStack, Limits};
pub use stream::CharStream;
