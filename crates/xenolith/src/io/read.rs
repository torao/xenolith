//! A blocking driver that supplies data from a byte stream to the parser.
//!
//! The [`Parser`] itself does not read data; instead, it requests the data it needs. Meanwhile, the [`StreamSource`]
//! wraps a [`std::io::Read`] implementation and fulfills those requests. It reads the next chunk when the parser
//! requires input and fetches external entities via the [`UriResolver`] when needed, allowing the caller to focus
//! solely on handling events.
//!
//! This can be viewed as an [`EventSource`] powered by the parser. After registering a handler to process the
//! document's event sequence, you can either advance processing to the end using [`emit`](EventCursor::emit) or
//! retrieve (pull) events one by one using [`next`](EventCursor::next). In either case, the registered handler
//! receives the events. [`advance`](StreamSource::advance) is the underlying low-level method that drives the parser
//! and makes the current event available for reading.

#[cfg(feature = "async")]
pub mod async_reader;
#[cfg(feature = "async")]
pub mod async_resolve;

#[cfg(test)]
mod test;

#[cfg(feature = "async")]
pub use async_reader::{AsyncReader, NoResolver};
#[cfg(feature = "async")]
pub use async_resolve::{AsyncEntityReader, AsyncUriResolver};

use std::io::Read;

use crate::error::{Error, Location, Result};

use crate::attr::Attributes;
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, Dispatch, DoctypeEventRef, EndElementEventRef, EventCursor,
  EventHandler, EventSource, Outcome, ProcessingInstructionEventRef, StartElementEventRef,
};
use crate::io::resolve::{RequestKind, UriResolver};
use crate::io::stream::CharStream;

use crate::io::parse::config::ParserConfig;
use crate::io::parse::entity::Entity;
use crate::io::parse::{Parser, Progress, TokenKind, TokenRef};

/// The size of the read buffer — that is, the number of bytes read from the source at one time.
const READ_BUFFER_SIZE: usize = 8 * 1024;

/// Reads a document from anything that implements [`Read`], as an [`EventSource`].
///
/// # Examples
///
/// ```
/// use xenolith::io::{TokenKind, StreamSource};
///
/// let mut reader = StreamSource::new("<doc>text</doc>".as_bytes());
/// let mut depth = 0;
/// while let Some(kind) = reader.advance()? {
///   match kind {
///     TokenKind::StartElement => depth += 1,
///     TokenKind::Text => assert_eq!(reader.parser().token_ref().and_then(|e| e.text()), Some("text")),
///     _ => {}
///   }
/// }
/// assert_eq!(depth, 1);
/// # Ok::<(), xenolith::Error>(())
/// ```
pub struct StreamSource<'h, R> {
  source: R,
  /// Streamed external general entities, innermost last; read in preference to `source`.
  entities: Vec<EntitySource>,
  parser: Parser,
  /// Reused across reads, `READ_BUFFER_SIZE` bytes long.
  buffer: Vec<u8>,
  /// Whether `source` has reached its end; once set, the document is fed nothing more.
  finished: bool,
  resolver: Option<Box<dyn UriResolver>>,
  /// The handlers this source was built with, which every event reaches.
  dispatch: Dispatch<'h>,
  /// Where the cursor has reached, so `next` knows whether the document has begun or ended.
  step: Step,
  /// The base URI of the current event, held here rather than computed into a local so that the event handed out can
  /// borrow it. It stays `None` when the parser is configured to track no base.
  base: Option<String>,
}

/// Where a [`StreamSource`] has reached: the document's own start and end are events of their own, so the cursor
/// remembers which side of the document it is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
  /// Nothing reported yet; the next event is [`StartDocument`](crate::event::EventRef::StartDocument).
  Before,
  /// The document is being read.
  Reading,
  /// The document was read to its end and [`EndDocument`](crate::event::EventRef::EndDocument) reported; the next
  /// call tells the handlers the run completed.
  Ended,
  /// Every handler has finished early; the next call tells the handlers the run stopped.
  Stopped,
  /// The run is over, and the handlers have been told how it ended.
  Done,
}

/// A streamed external general entity: the resolver's reader and whether it has hit its end.
struct EntitySource {
  reader: Box<dyn Read>,
  finished: bool,
}

impl<R> std::fmt::Debug for StreamSource<'_, R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("StreamSource")
      .field("finished", &self.finished)
      .field("open_entities", &self.entities.len())
      .field("has_resolver", &self.resolver.is_some())
      .finish_non_exhaustive()
  }
}

impl<'h, R: Read> StreamSource<'h, R> {
  /// Reads a document whose encoding is determined from its bytes.
  #[must_use]
  pub fn new(source: R) -> Self {
    Self::with_document(source, Entity::document(CharStream::new()))
  }

  /// Reads a document with its system identifier already known.
  ///
  /// Worth doing whenever it is: the identifier appears in every diagnostic, and it is the
  /// base URI against which relative references resolve.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::io::StreamSource;
  ///
  /// let mut reader = StreamSource::with_system_id("<a>".as_bytes(), "file:///doc.xml");
  /// let error = reader.advance().and_then(|_| reader.advance()).unwrap_err();
  /// assert_eq!(error.location().system_id.as_deref(), Some("file:///doc.xml"));
  /// // The location is a field on the error; its `Display` is the message alone, so a caller
  /// // that wants a position-prefixed line composes the two itself.
  /// assert!(error.to_string().starts_with("not well-formed:"));
  /// ```
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
  #[must_use]
  pub fn with_document(source: R, document: Entity) -> Self {
    Self {
      source,
      entities: Vec::new(),
      parser: Parser::with_document(document),
      buffer: vec![0; READ_BUFFER_SIZE],
      finished: false,
      resolver: None,
      dispatch: Dispatch::new(),
      step: Step::Before,
      base: None,
    }
  }

  /// Sets the resolver used for external entities.
  ///
  /// Without one, a reference to an external entity is a fatal error — the safe default, since
  /// resolving external entities is the XML external-entity (XXE) attack surface. Supply a
  /// resolver only for trusted input; see [`UriResolver`].
  #[must_use]
  pub fn with_resolver(mut self, resolver: impl UriResolver + 'static) -> Self {
    self.resolver = Some(Box::new(resolver));
    self
  }

  /// Fixes the encoding of the document, skipping detection.
  ///
  /// By default the encoding is sniffed from a byte-order mark and the declaration; give it here
  /// when it is dictated from outside — an HTTP `Content-Type`, say, or a caller who knows the
  /// file. Call before the first [`advance`](Self::advance). Detection is skipped, so a leading
  /// byte-order mark is not stripped; leave the encoding unset when the input may carry one. Any
  /// system identifier already set with [`with_system_id`](Self::with_system_id) is kept.
  ///
  /// # Errors
  ///
  /// Returns an error if `encoding` is not one this build can decode.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::io::StreamSource;
  ///
  /// let mut reader = StreamSource::new("<a/>".as_bytes()).with_encoding("US-ASCII")?;
  /// assert!(reader.advance()?.is_some());
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  pub fn with_encoding(mut self, encoding: &str) -> Result<Self> {
    self.parser.set_encoding(encoding)?;
    Ok(self)
  }

  /// Sets the parser's settings: the extensions it applies, its limits, and the length of text fragments.
  ///
  /// See [`ParserConfig`] for each setting and its default. Call before the first [`advance`](Self::advance).
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.parser.set_config(config);
    self
  }

  /// Advances to the next event, reading from the source and resolving entities as needed.
  ///
  /// Returns the token's [`TokenKind`], or `None` at the end of the document; the token's data is then read through
  /// [`parser`](Self::parser)'s [`token_ref`](Parser::token_ref). Where [`Parser::advance`] can also stop to request
  /// input or an entity, this loops until it has a token, feeding the parser from the source and resolving entities
  /// through the resolver itself, so the caller sees only tokens.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Io`] if reading the source or an external entity fails, and whatever the parser reports for a
  /// document that breaks the rules, including an unresolved external entity.
  pub fn advance(&mut self) -> Result<Option<TokenKind>> {
    loop {
      match self.parser.advance()? {
        Progress::Token(kind) => return Ok(Some(kind)),
        Progress::Eof => return Ok(None),
        Progress::NeedMoreInput => self.fill()?,
        Progress::NeedEntity => self.resolve_entity()?,
      }
    }
  }

  /// Resolves the entity the parser requested and hands its bytes back, through the configured resolver.
  ///
  /// With no resolver configured, every external entity is refused as not well-formed, the safe default against the
  /// XML external-entity (XXE) attack. A resolver that returns `None` for the request declines it with
  /// [`decline_entity`](Parser::decline_entity), which the parser reports as a fatal error.
  ///
  /// When the resolver does return a reader, how it is consumed depends on the request kind. A general entity is
  /// streamed: it is opened onto the parser and pumped chunk by chunk by [`fill`](Self::fill), so its bytes are never
  /// held all at once. The DTD-side kinds (an external subset or a parameter entity) have no streaming form, so they
  /// are read whole and added to the DTD text with [`provide_entity`](Parser::provide_entity).
  fn resolve_entity(&mut self) -> Result<()> {
    let request = self.parser.pending_entity().expect("the parser requested an entity");
    let kind = request.kind();
    let Some(resolver) = &mut self.resolver else {
      // Refused here rather than resolved; the message gives the opt-in so the caller knows how to allow it.
      let at = self.parser.location();
      let message = format!("{request}: no resolver is configured; call StreamSource::with_resolver to allow this");
      return Err(Error::well_formedness(message).at(at));
    };
    // The resolver is handed a request, not a position, so a failure of its own carries none. The position added here
    // is where the construct that called for the entity begins, the reference or the `DOCTYPE`, rather than the
    // parser's current position just past it.
    let at = self.parser.event_location();
    let Some(reader) = resolver.resolve(request).map_err(|error| error.or_at(at))? else {
      // The resolver does not have this entity; declining lets the parser decide the error.
      return self.parser.decline_entity();
    };
    match kind {
      RequestKind::GeneralEntity => {
        // Open the entity, then let `fill` pump its reader chunk by chunk.
        self.parser.begin_entity()?;
        self.entities.push(EntitySource { reader, finished: false });
        Ok(())
      }
      // The DTD-side kinds are added to the DTD text, so they are read whole.
      RequestKind::ExternalSubset | RequestKind::ParameterEntity => {
        let mut reader = reader;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).map_err(|e| {
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
  /// The innermost source is a streamed external entity when one is open, otherwise the document. A read of zero
  /// bytes marks the end of that source: the chunk is fed with the `last` flag set, and the source is then retired,
  /// popping an exhausted entity or marking the document finished. A short read is not the end, so whatever arrived
  /// is fed and the next call reads again.
  fn fill(&mut self) -> Result<()> {
    let (read, from_entity) = match self.entities.last_mut() {
      Some(top) => {
        if top.finished {
          return Err(Error::internal("the parser requested input beyond the end of an entity"));
        }
        (top.reader.read(&mut self.buffer), true)
      }
      None => {
        if self.finished {
          // The parser requested input after the document ended; only a bug in the parser or a hand-written driver
          // gets here, since `feed(.., true)` settles the question.
          return Err(Error::internal("the parser requested input beyond the end of the document"));
        }
        (self.source.read(&mut self.buffer), false)
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
  #[must_use]
  pub const fn parser(&self) -> &Parser {
    &self.parser
  }

  /// The parser's current position, at the end of the event just reported; [`Parser::event_location`] gives where the
  /// current event begins.
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
  pub fn into_inner(self) -> R {
    self.source
  }

  /// Drives the parser until it has an event this vocabulary models, answering its requests for input and entities.
  ///
  /// Returns `false` at the end of the document. The XML declaration is stepped over: it says how the bytes were
  /// encoded rather than what the document holds, and no handler models it.
  fn advance_to_event(&mut self) -> Result<bool> {
    loop {
      match self.parser.advance()? {
        Progress::Token(TokenKind::XmlDeclaration) => {}
        Progress::Token(_) => return Ok(true),
        Progress::Eof => return Ok(false),
        Progress::NeedMoreInput => self.fill()?,
        Progress::NeedEntity => self.resolve_entity()?,
      }
    }
  }
}

/// The current event of `parser`, as the shared vocabulary sees it.
///
/// It borrows the parser rather than any local, which is what lets a cursor hand the event back to its caller: the
/// attributes come from the parser as an [`AttributeList`](crate::attr::AttributeList), and `base` is the
/// caller's own field. `None` is the XML declaration, which nothing models.
fn current_event<'a>(parser: &'a Parser, base: Option<&'a str>) -> Option<crate::event::EventRef<'a>> {
  use crate::event::EventRef;
  let location = parser.event_location();
  Some(match parser.token_ref()? {
    // The name arrives as its parts, each borrowed from the parser, which is what the shared vocabulary carries.
    TokenRef::StartElement { xml_space, xml_lang, .. } => EventRef::StartElement(StartElementEventRef::new(
      parser.prefix(),
      parser.local_name(),
      parser.namespace_uri(),
      Attributes::new(parser),
      xml_space,
      xml_lang,
      base,
      location,
    )),
    TokenRef::EndElement { .. } => EventRef::EndElement(EndElementEventRef::new(
      parser.prefix(),
      parser.local_name(),
      parser.namespace_uri(),
      location,
    )),
    TokenRef::Text(text) => EventRef::Characters(CharactersEventRef::new(text, location)),
    TokenRef::CData(text) => EventRef::Cdata(CdataEventRef::new(text, location)),
    TokenRef::Comment(text) => EventRef::Comment(CommentEventRef::new(text, location)),
    TokenRef::ProcessingInstruction { target, data, data_location } => {
      EventRef::ProcessingInstruction(ProcessingInstructionEventRef::new(target, data, data_location.clone(), location))
    }
    // The DTD is keyed by interned names, so this is the one event that still carries the pool they belong to.
    TokenRef::Doctype(_) => EventRef::Doctype(DoctypeEventRef::new(
      parser.doctype_name().map(|name| parser.pool().resolve(name)),
      parser.doctype_public_id(),
      parser.doctype_system_id(),
      parser.dtd().expect("the DTD is fully parsed by the doctype event"),
      parser.pool(),
      location,
    )),
    TokenRef::XmlDeclaration { .. } => return None,
  })
}

impl<'h, R: Read> EventSource<'h> for StreamSource<'h, R> {
  fn with_handler(mut self, handler: &'h mut dyn EventHandler) -> Self {
    self.dispatch = self.dispatch.with_handler(handler);
    self
  }
}

impl<'h, R: Read> EventCursor<'h> for StreamSource<'h, R> {
  fn next(&mut self) -> Result<Option<crate::event::EventRef<'_>>> {
    use crate::event::EventRef;
    match self.step {
      Step::Done => return Ok(None),
      Step::Ended | Step::Stopped => {
        let outcome = if self.step == Step::Ended { Outcome::Completed } else { Outcome::Stopped };
        self.step = Step::Done;
        self.dispatch.finish(outcome);
        return Ok(None);
      }
      Step::Before => {
        self.step = Step::Reading;
        let event = EventRef::StartDocument;
        if let Err(error) = self.dispatch.handle(&event) {
          self.step = Step::Done;
          return Err(self.dispatch.fail(error));
        }
        if !self.dispatch.should_continue() {
          self.step = Step::Stopped;
        }
        return Ok(Some(event));
      }
      Step::Reading => {}
    }
    let more = match self.advance_to_event() {
      Ok(more) => more,
      Err(error) => {
        self.step = Step::Done;
        return Err(self.dispatch.fail(error));
      }
    };
    if !more {
      // The document was read in full, so its end is the last event.
      self.step = Step::Ended;
      let event = EventRef::EndDocument;
      if let Err(error) = self.dispatch.handle(&event) {
        self.step = Step::Done;
        return Err(self.dispatch.fail(error));
      }
      return Ok(Some(event));
    }
    {
      // Into the field rather than a local, so the event handed back can borrow it.
      let base = self.parser.base_uri();
      self.base = base;
    }
    let event = current_event(&self.parser, self.base.as_deref()).expect("the parser reported an event it models");
    if let Err(error) = self.dispatch.handle(&event) {
      self.step = Step::Done;
      return Err(self.dispatch.fail(error));
    }
    if !self.dispatch.should_continue() {
      // A handler has all it wanted; the document was not read in full, so no EndDocument follows.
      self.step = Step::Stopped;
    }
    Ok(Some(event))
  }
}
