//! A driver for asynchronous sources, behind the `async` feature.
//!
//! [`AsyncReader`] is the asynchronous counterpart to [`Reader`](crate::Reader): it drives the same [`Parser`], reading
//! the next chunk with an `.await` when the parser needs input and fetching an external entity through an
//! [`AsyncUriResolver`] when it needs one, so the caller only awaits [`advance`](AsyncReader::advance). The runtime is
//! the caller's: any executor drives it, and the source is any [`futures_io::AsyncRead`] (a tokio reader through its
//! `.compat()` adapter).
//!

use std::pin::Pin;

use futures_io::AsyncRead;

use xenolith_core::error::{Error, Location, Result};

use crate::async_resolve::{AsyncEntityReader, AsyncUriResolver};
use crate::config::{Bounds, ParserConfig};
use crate::entity::{Entity, Limits};
use crate::event::Event;
use crate::parser::{EventKind, Parser, Progress};
use xenolith_core::resolve::RequestKind;
use xenolith_core::stream::CharStream;

/// The read buffer's size, and so how many bytes are read from the source at a time.
///
const READ_BUFFER_SIZE: usize = 8 * 1024;

/// Reads a chunk from a [`futures_io::AsyncRead`] into `buf`, without a runtime's extension trait.
///
async fn read_bytes<R: AsyncRead + Unpin + ?Sized>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
  std::future::poll_fn(|cx| Pin::new(&mut *reader).poll_read(cx, buf)).await
}

/// Reads a document from anything that implements [`AsyncRead`].
///
/// # Examples
///
/// ```
/// # pollster::block_on(async {
/// use xenolith_parser::{AsyncReader, EventKind};
///
/// let mut reader = AsyncReader::new(&b"<doc>text</doc>"[..]);
/// let mut names = Vec::new();
/// while let Some(kind) = reader.advance().await? {
///   if kind == EventKind::StartElement {
///     names.push(reader.parser().local_name().to_owned());
///   }
/// }
/// assert_eq!(names, ["doc"]);
/// # Ok::<(), xenolith_core::Error>(())
/// # }).unwrap();
/// ```
pub struct AsyncReader<R, Resolver = NoResolver> {
  source: R,
  /// Streamed external general entities, innermost last; read in preference to `source`.
  entities: Vec<AsyncEntitySource>,
  parser: Parser,
  /// Reused across reads, `READ_BUFFER_SIZE` bytes long.
  buffer: Vec<u8>,
  /// Whether `source` has reached its end; after that the document is fed nothing more.
  finished: bool,
  resolver: Resolver,
}

/// A streamed external general entity: the resolver's reader and whether it has hit its end.
///
struct AsyncEntitySource {
  reader: AsyncEntityReader,
  finished: bool,
}

impl<R, Resolver> std::fmt::Debug for AsyncReader<R, Resolver> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("AsyncReader")
      .field("finished", &self.finished)
      .field("open_entities", &self.entities.len())
      .finish_non_exhaustive()
  }
}

/// The default resolver of an [`AsyncReader`] declines every external entity, so a reference to one is a fatal error
/// until [`AsyncReader::with_resolver`] supplies a real one.
///
#[derive(Clone, Copy, Debug, Default)]
pub struct NoResolver;

impl AsyncUriResolver for NoResolver {
  async fn resolve(&mut self, request: &xenolith_core::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
    let message = format!("{request}: no resolver is configured; attach one with with_resolver to allow this");
    Err(Error::well_formedness(message))
  }
}

impl<R: AsyncRead + Unpin> AsyncReader<R> {
  /// Reads a document whose encoding is determined from its bytes.
  ///
  #[must_use]
  pub fn new(source: R) -> Self {
    Self::with_document(source, Entity::document(CharStream::new()), Limits::default())
  }

  /// Reads a document with its system identifier already known.
  ///
  /// The identifier appears in every diagnostic and is the base URI for relative references.
  ///
  #[must_use]
  pub fn with_system_id(source: R, system_id: &str) -> Self {
    let document = Entity::document(CharStream::new().with_system_id(system_id));
    Self::with_document(source, document, Limits::default())
  }

  /// Reads a document from a prepared document [`Entity`] and explicit [`Limits`].
  ///
  /// The other constructors build on this one. The [`Entity`] carries the document's [`CharStream`], so its encoding
  /// and system identifier are already set on that stream; [`Limits`] cap the whole-document work, such as total
  /// entity expansion and nesting depth, where [`with_bounds`](Self::with_bounds) caps every single token. Reach for
  /// this when the defaults [`new`](Self::new) uses do not fit.
  ///
  #[must_use]
  pub fn with_document(source: R, document: Entity, limits: Limits) -> Self {
    Self {
      source,
      entities: Vec::new(),
      parser: Parser::with_document(document, limits),
      buffer: vec![0; READ_BUFFER_SIZE],
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
  ///
  pub fn with_encoding(mut self, encoding: &str) -> Result<Self> {
    self.parser.set_encoding(encoding)?;
    Ok(self)
  }

  /// Sets the parser configuration: whether `xml:base` base-URI tracking and `xml:id` ID-typing are done.
  ///
  /// See [`ParserConfig`] for what each does and its default, which follows the features this build was compiled with.
  /// Call before the first [`advance`](Self::advance), and before [`with_resolver`](Self::with_resolver).
  ///
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.parser.set_config(config);
    self
  }

  /// Sets the per-token byte-length bounds the scanner enforces.
  ///
  /// Each bound caps the size of one token, so an oversized name, attribute, or comment cannot be buffered without
  /// limit. See [`Bounds`]. Call before the first [`advance`](Self::advance). The default caps each token generously.
  ///
  #[must_use]
  pub fn with_bounds(mut self, bounds: Bounds) -> Self {
    self.parser.set_bounds(bounds);
    self
  }

  /// Attaches a resolver for external entities.
  ///
  /// Without one, a reference to an external entity is a fatal error: the safe default, since resolving external
  /// entities is the XML external-entity (XXE) attack surface. See [`AsyncUriResolver`] and the note on
  /// [`Reader::with_resolver`](crate::Reader::with_resolver).
  ///
  #[must_use]
  pub fn with_resolver<Resolver: AsyncUriResolver>(self, resolver: Resolver) -> AsyncReader<R, Resolver> {
    AsyncReader {
      source: self.source,
      entities: self.entities,
      parser: self.parser,
      buffer: self.buffer,
      finished: self.finished,
      resolver,
    }
  }
}

impl<R: AsyncRead + Unpin, Resolver: AsyncUriResolver> AsyncReader<R, Resolver> {
  /// Advances to the next event, reading from the source and resolving entities as needed.
  ///
  /// Returns the event's [`EventKind`], or `None` at the end of the document; the event's data is then read through
  /// [`parser`](Self::parser)'s [`event_ref`](Parser::event_ref). Where [`Parser::advance`] can also stop to request
  /// input or an entity, this loops until it has an event, feeding the parser from the source and resolving entities
  /// through the resolver itself, so the caller sees only events.
  ///
  /// # Cancel safety
  ///
  /// This method is **not** cancel safe. Dropping the future between a read and the parser consuming it loses those
  /// bytes, and the reader cannot be used again. Put the timeout or the `select!` branch around the whole run, not a
  /// single call:
  ///
  /// ```ignore
  /// // `timeout` here stands for your runtime's timeout.
  ///
  /// // NG: a timeout on one advance() may drop it mid-read, spending the reader.
  /// let kind = timeout(limit, reader.advance()).await??;
  ///
  /// // OK: the timeout covers the whole run.
  /// timeout(limit, async {
  ///   while let Some(kind) = reader.advance().await? {
  ///     // handle the event
  ///   }
  ///   Ok::<(), xenolith_core::Error>(())
  /// })
  /// .await??;
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`Error::Io`] if reading the source or an external entity fails, and whatever the parser reports for a
  /// document that breaks the rules, including an unresolved external entity.
  ///
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

  /// Resolves the entity the parser requested and returns its bytes through the configured resolver.
  ///
  /// The default [`NoResolver`] refuses every external entity as not well-formed, the safe default against the XML
  /// external-entity (XXE) attack. A resolver that returns `None` for the request declines it with
  /// [`decline_entity`](Parser::decline_entity), which the parser reports as a fatal error.
  ///
  /// When the resolver does return a reader, how it is consumed depends on the request kind. A general entity is
  /// streamed: it is opened onto the parser and pumped chunk by chunk by [`fill`](Self::fill), so its bytes are never
  /// held all at once. The DTD-side kinds (an external subset or a parameter entity) have no streaming form, so they
  /// are read whole and spliced in with [`provide_entity`](Parser::provide_entity).
  ///
  async fn resolve_entity(&mut self) -> Result<()> {
    let request = self.parser.pending_entity().expect("the parser requested an entity");
    let kind = request.kind();
    let Some(reader) = self.resolver.resolve(request).await? else {
      // The resolver does not have this entity; declining lets the parser decide the error.
      return self.parser.decline_entity();
    };
    match kind {
      RequestKind::GeneralEntity => {
        // Open the entity, then let `fill` pump its reader chunk by chunk.
        self.parser.begin_entity()?;
        self.entities.push(AsyncEntitySource { reader, finished: false });
        Ok(())
      }
      // The DTD-side kinds are spliced into the DTD text, so they are read whole.
      RequestKind::ExternalSubset | RequestKind::ParameterEntity => {
        let mut reader = reader;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.map_err(|e| {
          let at = self.parser.location();
          Error::io(format!("cannot read an external entity: {e}")).at(at).caused_by(e)
        })?;
        self.parser.provide_entity(&bytes)
      }
    }
  }

  /// Reads one chunk from the innermost open source and feeds it to the parser; this is what answers
  /// [`Progress::NeedMoreInput`].
  ///
  /// The innermost source is a streamed external entity when one is open; otherwise, it is the document. A zero-byte
  /// read marks the end of that source: the chunk is fed with the `last` flag set, and the source is then retired,
  /// popping an exhausted entity or marking the document finished. A short read is not the end, so whatever arrived
  /// is fed, and the next call reads again.
  ///
  async fn fill(&mut self) -> Result<()> {
    let (read, from_entity) = match self.entities.last_mut() {
      Some(top) => {
        if top.finished {
          return Err(Error::internal("the parser requested input beyond the end of an entity"));
        }
        (top.reader.read(&mut self.buffer).await, true)
      }
      None => {
        if self.finished {
          // The parser requested input after the document ended; only a bug in the parser or a hand-written driver
          // gets here, since `feed(.., true)` settles the question.
          return Err(Error::internal("the parser requested input beyond the end of the document"));
        }
        (read_bytes(&mut self.source, &mut self.buffer).await, false)
      }
    };
    let read = read.map_err(|e| {
      let at = self.parser.location();
      let what = if from_entity { "an external entity" } else { "the document" };
      Error::io(format!("cannot read {what}: {e}")).at(at).caused_by(e)
    })?;
    let last = read == 0;
    self.parser.feed(&self.buffer[..read], last)?;
    if from_entity {
      // The same entity is still innermost (the borrow above was released to feed the parser); mark it finished, and
      // drop it when its last bytes are in.
      self.entities.last_mut().expect("an entity source was open").finished = last;
      if last {
        self.entities.pop();
      }
    } else {
      self.finished = last;
    }
    Ok(())
  }

  /// Collects every remaining event into a `Vec`.
  ///
  /// This does not implement `Stream`, which is not yet in the standard library; collecting is the honest alternative
  /// until it is. Loop over [`advance`](Self::advance) instead when the whole document should not be held in memory at
  /// once.
  ///
  /// # Errors
  ///
  /// Stops and returns the first error.
  ///
  pub async fn events(&mut self) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    while self.advance().await?.is_some() {
      events.push(Event::capture(&self.parser)?);
    }
    Ok(events)
  }

  /// The parser, for reading the current event through [`event_ref`](Parser::event_ref) and its surrounding context.
  ///
  #[must_use]
  pub const fn parser(&self) -> &Parser {
    &self.parser
  }

  /// The parser's current position, at the end of the event just reported; [`Parser::event_location`] gives where the
  /// current event begins.
  ///
  #[must_use]
  pub fn location(&self) -> Location {
    self.parser.location()
  }

  /// Recovers ownership of the source for closing it, returning it to a pool, or reusing the connection once parsing
  /// is done.
  ///
  /// Like [`BufReader::into_inner`](std::io::BufReader::into_inner), it discards the reader's internal buffer: the
  /// reader reads ahead in chunks, so bytes already read but not yet parsed are lost. It is therefore not a way to
  /// keep reading the same byte stream from just after the document; use it when you don't want the leftover bytes.
  ///
  pub fn into_inner(self) -> R {
    self.source
  }
}

#[cfg(test)]
mod tests {
  use std::pin::Pin;
  use std::task::{Context, Poll};

  use super::*;

  /// A source that yields to the executor before every byte, as a socket would. Runtime-agnostic:
  /// it drives on any executor, so the tests need no tokio.
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
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
      let this = self.get_mut();
      if this.at == this.bytes.len() {
        return Poll::Ready(Ok(0));
      }
      this.ready = !this.ready;
      if !this.ready {
        // Pending once per byte, so the parser really has to survive being suspended.
        cx.waker().wake_by_ref();
        return Poll::Pending;
      }
      buf[0] = this.bytes[this.at];
      this.at += 1;
      Poll::Ready(Ok(1))
    }
  }

  /// An owned `futures_io::AsyncRead` over a byte buffer, for entity content built at run time.
  struct Bytes {
    data: Vec<u8>,
    at: usize,
  }

  impl AsyncRead for Bytes {
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
      let this = self.get_mut();
      let n = (this.data.len() - this.at).min(buf.len());
      buf[..n].copy_from_slice(&this.data[this.at..this.at + n]);
      this.at += n;
      Poll::Ready(Ok(n))
    }
  }

  async fn kinds<R: AsyncRead + Unpin>(mut reader: AsyncReader<R>) -> Result<Vec<EventKind>> {
    let mut kinds = Vec::new();
    while let Some(kind) = reader.advance().await? {
      kinds.push(kind);
    }
    Ok(kinds)
  }

  /// The character data of the reader's current event, for the tests that collect a run by hand.
  fn text_of<R: AsyncRead + Unpin, Res: AsyncUriResolver>(reader: &AsyncReader<R, Res>) -> &str {
    reader.parser().event_ref().and_then(|e| e.text()).expect("the current event is character data")
  }

  #[test]
  fn reads_a_document() {
    pollster::block_on(async {
      let kinds = kinds(AsyncReader::new(&b"<a>x</a>"[..])).await.unwrap();
      assert_eq!(kinds, [EventKind::StartElement, EventKind::Text, EventKind::EndElement]);
    });
  }

  #[test]
  fn a_source_that_pends_between_bytes_parses_the_same() {
    pollster::block_on(async {
      let xml = "<?xml version='1.0'?><a x='1'>text<b/><!--c--></a>";
      let expected = kinds(AsyncReader::new(xml.as_bytes())).await.unwrap();
      assert_eq!(kinds(AsyncReader::new(Trickle::new(xml))).await.unwrap(), expected);
    });
  }

  #[test]
  fn matches_the_blocking_reader_event_for_event() {
    pollster::block_on(async {
      // The two drivers share a parser; this is the assertion that keeps it that way.
      let xml = "<?xml version='1.0'?><a xmlns:p='urn:p' x='1'><p:b/>text<![CDATA[<]]></a>";
      let blocking: Vec<Event> = crate::Reader::new(xml.as_bytes()).events().collect::<Result<_>>().unwrap();
      let asynchronous = AsyncReader::new(Trickle::new(xml)).events().await.unwrap();
      assert_eq!(asynchronous, blocking);
    });
  }

  #[test]
  fn errors_carry_their_position() {
    pollster::block_on(async {
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
    });
  }

  #[test]
  fn a_document_larger_than_the_buffer_is_read_in_full() {
    pollster::block_on(async {
      let xml = format!("<a>{}</a>", "x".repeat(READ_BUFFER_SIZE * 3));
      let mut reader = AsyncReader::new(xml.as_bytes());
      let mut text = String::new();
      while let Some(kind) = reader.advance().await.unwrap() {
        if kind == EventKind::Text {
          text.push_str(text_of(&reader));
        }
      }
      assert_eq!(text.len(), READ_BUFFER_SIZE * 3);
    });
  }

  /// A resolver over a runtime-agnostic reader, needing no tokio.
  struct AsyncFixture(&'static [u8]);

  impl AsyncUriResolver for AsyncFixture {
    async fn resolve(&mut self, _request: &xenolith_core::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
      // A real resolver would await a socket or a file here.
      Ok(Some(AsyncEntityReader::from_async_read(self.0)))
    }
  }

  #[test]
  fn an_external_entity_is_resolved_asynchronously() {
    pollster::block_on(async {
      let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
      let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(AsyncFixture(b"<b>in</b>"));
      let events = reader.events().await.unwrap();
      let names: Vec<_> = events.iter().filter_map(Event::name).map(|n| n.local()).collect();
      // <doc>, <b>, </b>, </doc>: four names.
      assert_eq!(names.len(), 4);
    });
  }

  #[test]
  fn without_a_resolver_an_external_entity_is_refused() {
    pollster::block_on(async {
      let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
      let error = AsyncReader::new(xml.as_bytes()).events().await.unwrap_err();
      assert!(error.message().contains("no resolver is configured"));
    });
  }

  /// A resolver that owns its bytes, for content generated at run time.
  struct OwnedAsyncEntity(&'static str, Vec<u8>);

  impl AsyncUriResolver for OwnedAsyncEntity {
    async fn resolve(&mut self, request: &xenolith_core::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
      if request.name() == Some(self.0) {
        Ok(Some(AsyncEntityReader::from_async_read(Bytes { data: self.1.clone(), at: 0 })))
      } else {
        Ok(None)
      }
    }
  }

  #[test]
  fn a_large_external_general_entity_streams_across_chunks() {
    pollster::block_on(async {
      // The entity is larger than one read buffer, so it is pulled through several `fill` chunks
      // rather than materialized whole; the reassembled text proves every byte arrived.
      let body = "y".repeat(READ_BUFFER_SIZE * 2 + 100);
      let entity = format!("<b>{body}</b>");
      let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
      let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(OwnedAsyncEntity("e", entity.into_bytes()));
      let mut text = String::new();
      while let Some(kind) = reader.advance().await.unwrap() {
        if kind == EventKind::Text {
          text.push_str(text_of(&reader));
        }
      }
      assert_eq!(text.len(), READ_BUFFER_SIZE * 2 + 100);
    });
  }

  /// The `tokio` feature's adapter: a reader implementing tokio's own `AsyncRead` is bridged in.
  #[cfg(feature = "tokio")]
  #[test]
  fn from_tokio_adapts_a_tokio_reader() {
    struct TokioFixture(&'static [u8]);

    impl AsyncUriResolver for TokioFixture {
      async fn resolve(
        &mut self,
        _request: &xenolith_core::resolve::EntityRequest,
      ) -> Result<Option<AsyncEntityReader>> {
        Ok(Some(AsyncEntityReader::from_tokio(std::io::Cursor::new(self.0.to_vec()))))
      }
    }

    pollster::block_on(async {
      let xml = "<!DOCTYPE d [<!ENTITY e SYSTEM 'e.ent'>]><d>&e;</d>";
      let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(TokioFixture(b"<b/>"));
      let events = reader.events().await.unwrap();
      let names: Vec<_> = events.iter().filter_map(Event::name).map(|n| n.local()).collect();
      assert_eq!(names.len(), 4); // start and end of <d> and <b>, the entity's content parsed in place
    });
  }
}
