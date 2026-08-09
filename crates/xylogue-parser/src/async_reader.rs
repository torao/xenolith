//! Driver for asynchronous sources, behind the `tokio` feature.
//!
//! The same [`Parser`] as everywhere else. Only the answer to `Progress::NeedMoreInput`
//! differs: a `.await` instead of a blocking read. Nothing about parsing is duplicated here,
//! which is the whole point of keeping I/O out of the core.

use tokio::io::{AsyncRead, AsyncReadExt};

use xylogue_core::error::{Error, Location, Result};

use crate::config::ParserConfig;
use crate::entity::{Entity, Limits};
use crate::event::Event;
use crate::parser::{EventKind, Parser, Progress};
use crate::resolve::AsyncUriResolver;
use crate::stream::CharStream;

/// How many bytes to read from the source at a time.
const CHUNK: usize = 8 * 1024;

/// Reads a document from anything that implements [`AsyncRead`].
///
/// # Examples
///
/// ```
/// # tokio_test::block_on(async {
/// use xylogue_parser::{AsyncReader, EventKind};
///
/// let mut reader = AsyncReader::new(&b"<doc>text</doc>"[..]);
/// let mut names = Vec::new();
/// while let Some(kind) = reader.advance().await? {
///   if kind == EventKind::StartElement {
///     names.push(reader.parser().local_name().to_owned());
///   }
/// }
/// assert_eq!(names, ["doc"]);
/// # Ok::<(), xylogue_core::Error>(())
/// # }).unwrap();
/// ```
pub struct AsyncReader<R, Resolver = NoResolver> {
  source: R,
  parser: Parser,
  buffer: Vec<u8>,
  finished: bool,
  resolver: Resolver,
}

impl<R, Resolver> std::fmt::Debug for AsyncReader<R, Resolver> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("AsyncReader").field("finished", &self.finished).finish_non_exhaustive()
  }
}

/// The default resolver of an [`AsyncReader`]: it declines every external entity, so a
/// reference to one is a fatal error until [`AsyncReader::with_resolver`] supplies a real one.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoResolver;

impl AsyncUriResolver for NoResolver {
  async fn resolve(&mut self, request: &crate::resolve::EntityRequest) -> Result<Option<Vec<u8>>> {
    let message = format!("{request}: no resolver is configured; attach one with with_resolver to allow this");
    Err(Error::well_formedness(message))
  }
}

impl<R: AsyncRead + Unpin> AsyncReader<R> {
  /// Reads a document whose encoding is determined from its bytes.
  #[must_use]
  pub fn new(source: R) -> Self {
    Self::with_document(source, Entity::document(CharStream::new()), Limits::default())
  }

  /// Reads a document with its system identifier already known.
  ///
  /// The identifier appears in every diagnostic and is the base URI for relative references.
  #[must_use]
  pub fn with_system_id(source: R, system_id: &str) -> Self {
    let document = Entity::document(CharStream::new().with_system_id(system_id));
    Self::with_document(source, document, Limits::default())
  }

  /// Reads a document over a prepared entity, with explicit limits.
  #[must_use]
  pub fn with_document(source: R, document: Entity, limits: Limits) -> Self {
    Self {
      source,
      parser: Parser::with_document(document, limits),
      buffer: vec![0; CHUNK],
      finished: false,
      resolver: NoResolver,
    }
  }

  /// Fixes the encoding of the document, skipping detection.
  ///
  /// The asynchronous counterpart to [`Reader::with_encoding`](crate::Reader::with_encoding):
  /// give the encoding here when it is dictated from outside rather than sniffed. Call before the
  /// first [`advance`](Self::advance), and before [`with_resolver`](Self::with_resolver).
  /// Detection is skipped, so a leading byte-order mark is not stripped.
  ///
  /// # Errors
  ///
  /// Returns an error if `encoding` is not one this build can decode.
  pub fn with_encoding(mut self, encoding: &str) -> Result<Self> {
    self.parser.set_encoding(encoding)?;
    Ok(self)
  }

  /// Sets the parser configuration: the optional `xml:base` and `xml:id` processing.
  ///
  /// See [`ParserConfig`]. Call before the first [`advance`](Self::advance), and before
  /// [`with_resolver`](Self::with_resolver).
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.parser.set_config(config);
    self
  }

  /// Attaches a resolver for external entities.
  ///
  /// Without one, a reference to an external entity is a fatal error — the safe default. See
  /// [`AsyncUriResolver`] and the note on [`Reader::with_resolver`](crate::Reader::with_resolver).
  #[must_use]
  pub fn with_resolver<Resolver: AsyncUriResolver>(self, resolver: Resolver) -> AsyncReader<R, Resolver> {
    AsyncReader { source: self.source, parser: self.parser, buffer: self.buffer, finished: self.finished, resolver }
  }
}

impl<R: AsyncRead + Unpin, Resolver: AsyncUriResolver> AsyncReader<R, Resolver> {
  /// Advances to the next event, reading from the source as needed.
  ///
  /// Returns `None` at the end of the document. The event itself is read through
  /// [`parser`](Self::parser).
  ///
  /// # Cancel safety
  ///
  /// This method is **not** cancel safe. Dropping the future between a read and the parser
  /// consuming it loses those bytes, and the reader cannot be used again. Wrap the whole
  /// document in the timeout or the `select!` branch, not a single call.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Io`] if the source fails, and whatever the parser reports for a
  /// document that breaks the rules.
  pub async fn advance(&mut self) -> Result<Option<EventKind>> {
    loop {
      match self.parser.advance()? {
        Progress::Event(kind) => return Ok(Some(kind)),
        Progress::Eof => return Ok(None),
        Progress::NeedMoreInput => self.fill().await?,
        Progress::NeedEntity => self.resolve_entity().await?,
      }
    }
  }

  /// Resolves the entity the parser asked for, through the configured resolver.
  async fn resolve_entity(&mut self) -> Result<()> {
    let request = self.parser.pending_entity().expect("the parser asked for an entity");
    match self.resolver.resolve(request).await? {
      Some(bytes) => self.parser.provide_entity(&bytes),
      None => self.parser.decline_entity(),
    }
  }

  /// Reads one chunk from the source into the parser.
  async fn fill(&mut self) -> Result<()> {
    if self.finished {
      return Err(Error::internal("the parser asked for input beyond the end of the document"));
    }
    let read = self.source.read(&mut self.buffer).await.map_err(|e| {
      let at = self.parser.location();
      Error::io(format!("cannot read the document: {e}")).at(at).caused_by(e)
    })?;
    self.finished = read == 0;
    self.parser.feed(&self.buffer[..read], self.finished)
  }

  /// Collects every remaining event.
  ///
  /// `Stream` is not implemented because it is not yet in the standard library; collecting is
  /// the honest alternative until it is. Loop over [`advance`](Self::advance) when the whole
  /// document should not be held in memory at once.
  ///
  /// # Errors
  ///
  /// Stops and returns the first error.
  pub async fn events(&mut self) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    while self.advance().await?.is_some() {
      events.push(Event::capture(&self.parser));
    }
    Ok(events)
  }

  /// The parser, for reading the current event.
  #[must_use]
  pub const fn parser(&self) -> &Parser {
    &self.parser
  }

  /// The current position.
  #[must_use]
  pub fn location(&self) -> Location {
    self.parser.location()
  }

  /// Returns the source, discarding whatever has not been parsed.
  pub fn into_inner(self) -> R {
    self.source
  }
}

#[cfg(test)]
mod tests {
  use std::pin::Pin;
  use std::task::{Context, Poll};

  use tokio::io::ReadBuf;

  use super::*;

  /// A source that yields to the runtime before every byte, as a socket would.
  struct Trickle {
    bytes: Vec<u8>,
    at: usize,
    ready: bool,
  }

  impl Trickle {
    fn new(text: &str) -> Self {
      Self { bytes: text.as_bytes().to_vec(), at: 0, ready: false }
    }
  }

  impl AsyncRead for Trickle {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
      if self.at == self.bytes.len() {
        return Poll::Ready(Ok(()));
      }
      self.ready = !self.ready;
      if !self.ready {
        // Pending once per byte, so the parser really has to survive being suspended.
        cx.waker().wake_by_ref();
        return Poll::Pending;
      }
      let byte = self.bytes[self.at];
      self.at += 1;
      buf.put_slice(&[byte]);
      Poll::Ready(Ok(()))
    }
  }

  async fn kinds<R: AsyncRead + Unpin>(mut reader: AsyncReader<R>) -> Result<Vec<EventKind>> {
    let mut kinds = Vec::new();
    while let Some(kind) = reader.advance().await? {
      kinds.push(kind);
    }
    Ok(kinds)
  }

  #[tokio::test]
  async fn reads_a_document() {
    let kinds = kinds(AsyncReader::new(&b"<a>x</a>"[..])).await.unwrap();
    assert_eq!(kinds, [EventKind::StartElement, EventKind::Text, EventKind::EndElement]);
  }

  #[tokio::test]
  async fn a_source_that_pends_between_bytes_parses_the_same() {
    let xml = "<?xml version='1.0'?><a x='1'>text<b/><!--c--></a>";
    let expected = kinds(AsyncReader::new(xml.as_bytes())).await.unwrap();
    assert_eq!(kinds(AsyncReader::new(Trickle::new(xml))).await.unwrap(), expected);
  }

  #[tokio::test]
  async fn matches_the_blocking_reader_event_for_event() {
    // The two drivers share a parser; this is the assertion that keeps it that way.
    let xml = "<?xml version='1.0'?><a xmlns:p='urn:p' x='1'><p:b/>text<![CDATA[<]]></a>";
    let blocking: Vec<Event> = crate::Reader::new(xml.as_bytes()).events().collect::<Result<_>>().unwrap();
    let asynchronous = AsyncReader::new(Trickle::new(xml)).events().await.unwrap();
    assert_eq!(asynchronous, blocking);
  }

  #[tokio::test]
  async fn errors_carry_their_position() {
    let mut reader = AsyncReader::with_system_id(&b"<a>\n&nosuch;</a>"[..], "file:///doc.xml");
    let error = loop {
      match reader.advance().await {
        Ok(Some(_)) => {}
        Ok(None) => panic!("expected a failure"),
        Err(e) => break e,
      }
    };
    assert_eq!(error.location().system_id.as_deref(), Some("file:///doc.xml"));
    assert_eq!(error.location().line, 2);
  }

  #[tokio::test]
  async fn a_document_larger_than_the_buffer_is_read_in_full() {
    let xml = format!("<a>{}</a>", "x".repeat(CHUNK * 3));
    let events = AsyncReader::new(xml.as_bytes()).events().await.unwrap();
    assert_eq!(events[1].text().map(str::len), Some(CHUNK * 3));
  }

  struct AsyncFixture(&'static [u8]);

  impl AsyncUriResolver for AsyncFixture {
    async fn resolve(&mut self, _request: &crate::resolve::EntityRequest) -> Result<Option<Vec<u8>>> {
      // A real resolver would await a socket or a file here.
      Ok(Some(self.0.to_vec()))
    }
  }

  #[tokio::test]
  async fn an_external_entity_is_resolved_asynchronously() {
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(AsyncFixture(b"<b>in</b>"));
    let events = reader.events().await.unwrap();
    let names: Vec<_> = events.iter().filter_map(Event::name).map(|n| n.local()).collect();
    // <doc>, <b>, </b>, </doc>: four names.
    assert_eq!(names.len(), 4);
  }

  #[tokio::test]
  async fn without_a_resolver_an_external_entity_is_refused() {
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let error = AsyncReader::new(xml.as_bytes()).events().await.unwrap_err();
    assert!(error.message().contains("no resolver is configured"));
  }
}
