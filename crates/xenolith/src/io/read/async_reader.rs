//! a driver for asynchronous sources, enabled by `async` functionality.
//!
//! [`AsyncReader`] is the asynchronous counterpart to [`StreamSource`](crate::io::StreamSource): it drives the same
//! [`Parser`], reading the next chunk with an `.await` when the parser needs input and fetching an external entity
//! through an [`AsyncUriResolver`] when it needs one, so the caller only awaits [`advance`](AsyncReader::advance). The
//! runtime is the caller's: any executor drives it, and the source is any [`futures_io::AsyncRead`] (a tokio reader
//! through its `.compat()` adapter).
//!

#[cfg(test)]
mod test;

use std::pin::Pin;

use futures_io::AsyncRead;

use crate::error::{Error, Location, Result};

use crate::io::parse::config::ParserConfig;
use crate::io::parse::entity::Entity;
use crate::io::parse::{Parser, Progress, TokenKind};
use crate::io::read::async_resolve::{AsyncEntityReader, AsyncUriResolver};
use crate::io::resolve::RequestKind;
use crate::io::stream::CharStream;

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
/// use xenolith::io::{AsyncReader, TokenKind};
///
/// let mut reader = AsyncReader::new(&b"<doc>text</doc>"[..]);
/// let mut names = Vec::new();
/// while let Some(kind) = reader.advance().await? {
///   if kind == TokenKind::StartElement {
///     names.push(reader.parser().local_name().to_owned());
///   }
/// }
/// assert_eq!(names, ["doc"]);
/// # Ok::<(), xenolith::Error>(())
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

/// The default resolver of an [`AsyncReader`] refuses every external entity, so a reference to one is a fatal error
/// until [`AsyncReader::with_resolver`] supplies a real one.
///
#[derive(Clone, Copy, Debug, Default)]
pub struct NoResolver;

impl AsyncUriResolver for NoResolver {
  async fn resolve(&mut self, request: &crate::io::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
    let message = format!("{request}: no resolver is configured; attach one with with_resolver to allow this");
    Err(Error::well_formedness(message))
  }
}

impl<R: AsyncRead + Unpin> AsyncReader<R> {
  /// Reads a document whose encoding is determined from its bytes.
  ///
  #[must_use]
  pub fn new(source: R) -> Self {
    Self::with_document(source, Entity::document(CharStream::new()))
  }

  /// Reads a document with its system identifier already known.
  ///
  /// The identifier appears in every diagnostic and is the base URI for relative references.
  ///
  #[must_use]
  pub fn with_system_id(source: R, system_id: &str) -> Self {
    let document = Entity::document(CharStream::new().with_system_id(system_id));
    Self::with_document(source, document)
  }

  /// Reads a document from a prepared document [`Entity`].
  ///
  /// The other constructors build on this one. The [`Entity`] carries the document's [`CharStream`], so its encoding
  /// and system identifier are already set on that stream. Reach for this when [`new`](Self::new) and
  /// [`with_system_id`](Self::with_system_id) do not fit.
  ///
  #[must_use]
  pub fn with_document(source: R, document: Entity) -> Self {
    Self {
      source,
      entities: Vec::new(),
      parser: Parser::with_document(document),
      buffer: vec![0; READ_BUFFER_SIZE],
      finished: false,
      resolver: NoResolver,
    }
  }

  /// Fixes the encoding of the document, skipping detection.
  ///
  /// The asynchronous counterpart to [`StreamSource::with_encoding`](crate::io::StreamSource::with_encoding):
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

  /// Sets the parser's settings: the extensions it applies, its limits, and the length of text fragments.
  ///
  /// See [`ParserConfig`] for each setting and its default.
  /// Call before the first [`advance`](Self::advance), and before [`with_resolver`](Self::with_resolver).
  ///
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.parser.set_config(config);
    self
  }

  /// Attaches a resolver for external entities.
  ///
  /// Without one, a reference to an external entity is a fatal error: the safe default, since resolving external
  /// entities is the XML external-entity (XXE) attack surface. See [`AsyncUriResolver`] and the note on
  /// [`StreamSource::with_resolver`](crate::io::StreamSource::with_resolver).
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
  /// Returns the token's [`TokenKind`], or `None` at the end of the document; the token's data is then read through
  /// [`parser`](Self::parser)'s [`token_ref`](Parser::token_ref). Where [`Parser::advance`] can also stop to request
  /// input or an entity, this loops until it has a token, feeding the parser from the source and resolving entities
  /// through the resolver itself, so the caller sees only tokens.
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
  ///   Ok::<(), xenolith::Error>(())
  /// })
  /// .await??;
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`Error::Io`] if reading the source or an external entity fails, and whatever the parser reports for a
  /// document that breaks the rules, including an unresolved external entity.
  ///
  pub async fn advance(&mut self) -> Result<Option<TokenKind>> {
    loop {
      match self.parser.advance()? {
        Progress::Token(kind) => return Ok(Some(kind)),
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
  /// are read whole and added to the DTD text with [`provide_entity`](Parser::provide_entity).
  ///
  async fn resolve_entity(&mut self) -> Result<()> {
    let request = self.parser.pending_entity().expect("the parser requested an entity");
    let kind = request.kind();
    // As in the blocking reader: the resolver is handed a request and no position, so the start of the construct that
    // called for the entity is added to a failure of its own.
    let at = self.parser.event_location();
    let resolved = self.resolver.resolve(request).await;
    let Some(reader) = resolved.map_err(|error| error.or_at(at))? else {
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
      // The DTD-side kinds are added to the DTD text, so they are read whole.
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

  /// The parser, for reading the current token through [`token_ref`](Parser::token_ref) and its surrounding context.
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
