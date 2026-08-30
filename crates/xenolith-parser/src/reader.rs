//! A blocking driver that feeds the parser from a source of bytes.
//!
//! [`Parser`] reads nothing on its own; it requests what it needs, and [`Reader`] wraps a [`std::io::Read`] and
//! answers. It reads the next chunk when the parser needs input, and fetches an external entity through a
//! [`UriResolver`] when it needs one, so the caller only calls [`advance`](Reader::advance) and reads each event.
//! To take owned events instead, [`events`](Reader::events) gives an iterator of them, and for the push style, with
//! the parser calling a handler, see the [`sax`](crate::sax) module.
//!

use std::io::Read;

use xenolith_core::error::{Error, Location, Result};

use crate::config::{Bounds, ParserConfig};
use crate::entity::{Entity, Limits};
use crate::event::Event;
use crate::parser::{EventKind, Parser, Progress};
use crate::resolve::{RequestKind, UriResolver};
use crate::stream::CharStream;

/// The read buffer's size, and so how many bytes are read from the source at a time.
///
const READ_BUFFER_SIZE: usize = 8 * 1024;

/// Reads a document from anything that implements [`Read`].
///
/// # Examples
///
/// ```
/// use xenolith_parser::{EventKind, Reader};
///
/// let mut reader = Reader::new("<doc>text</doc>".as_bytes());
/// let mut depth = 0;
/// while let Some(kind) = reader.advance()? {
///   match kind {
///     EventKind::StartElement => depth += 1,
///     EventKind::Text => assert_eq!(reader.parser().event_ref().and_then(|e| e.text()), Some("text")),
///     _ => {}
///   }
/// }
/// assert_eq!(depth, 1);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// Or, when owning the events is more convenient than borrowing them:
///
/// ```
/// use xenolith_parser::{Event, Reader};
///
/// let events: Vec<Event> = Reader::new("<a><b/></a>".as_bytes()).events().collect::<Result<_, _>>()?;
/// assert_eq!(events.len(), 4);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
pub struct Reader<R> {
  source: R,
  /// Streamed external general entities, innermost last; read in preference to `source`.
  entities: Vec<EntitySource>,
  parser: Parser,
  /// Reused across reads, `READ_BUFFER_SIZE` bytes long.
  buffer: Vec<u8>,
  /// Whether `source` has reached its end; once set, the document is fed nothing more.
  finished: bool,
  resolver: Option<Box<dyn UriResolver>>,
}

/// A streamed external general entity: the resolver's reader and whether it has hit its end.
struct EntitySource {
  reader: Box<dyn Read>,
  finished: bool,
}

impl<R> std::fmt::Debug for Reader<R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Reader")
      .field("finished", &self.finished)
      .field("open_entities", &self.entities.len())
      .field("has_resolver", &self.resolver.is_some())
      .finish_non_exhaustive()
  }
}

impl<R: Read> Reader<R> {
  /// Reads a document whose encoding is determined from its bytes.
  #[must_use]
  pub fn new(source: R) -> Self {
    Self::with_document(source, Entity::document(CharStream::new()), Limits::default())
  }

  /// Reads a document with its system identifier already known.
  ///
  /// Worth doing whenever it is: the identifier appears in every diagnostic, and it is the
  /// base URI against which relative references resolve.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_parser::Reader;
  ///
  /// let mut reader = Reader::with_system_id("<a>".as_bytes(), "file:///doc.xml");
  /// let error = reader.advance().and_then(|_| reader.advance()).unwrap_err();
  /// assert_eq!(error.location().system_id.as_deref(), Some("file:///doc.xml"));
  /// // The location is a field on the error; its `Display` is the message alone, so a caller
  /// // that wants a position-prefixed line composes the two itself.
  /// assert!(error.to_string().starts_with("not well-formed:"));
  /// ```
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
      resolver: None,
    }
  }

  /// Sets the resolver used for external entities.
  ///
  /// Without one, a reference to an external entity is a fatal error — the safe default, since
  /// resolving external entities is the XML external-entity (XXE) attack surface. Supply a
  /// resolver only for trusted input; see [`UriResolver`].
  ///
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
  /// use xenolith_parser::Reader;
  ///
  /// let mut reader = Reader::new("<a/>".as_bytes()).with_encoding("US-ASCII")?;
  /// assert!(reader.advance()?.is_some());
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  pub fn with_encoding(mut self, encoding: &str) -> Result<Self> {
    self.parser.set_encoding(encoding)?;
    Ok(self)
  }

  /// Sets the parser configuration: whether `xml:base` base-URI tracking and `xml:id` ID-typing are done.
  ///
  /// See [`ParserConfig`] for what each does and its default, which follows the features this build was compiled with.
  /// Call before the first [`advance`](Self::advance).
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

  /// Advances to the next event, reading from the source and resolving entities as needed.
  ///
  /// Returns the event's [`EventKind`], or `None` at the end of the document; the event's data is then read through
  /// [`parser`](Self::parser)'s [`event_ref`](Parser::event_ref). Where [`Parser::advance`] can also stop to request
  /// input or an entity, this loops until it has an event, feeding the parser from the source and resolving entities
  /// through the resolver itself, so the caller sees only events.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Io`] if reading the source or an external entity fails, and whatever the parser reports for a
  /// document that breaks the rules, including an unresolved external entity.
  ///
  pub fn advance(&mut self) -> Result<Option<EventKind>> {
    loop {
      match self.parser.advance()? {
        Progress::Event(kind) => return Ok(Some(kind)),
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
  /// are read whole and spliced in with [`provide_entity`](Parser::provide_entity).
  ///
  fn resolve_entity(&mut self) -> Result<()> {
    let request = self.parser.pending_entity().expect("the parser requested an entity");
    let kind = request.kind();
    let Some(resolver) = &mut self.resolver else {
      // Refused here rather than resolved; the message names the opt-in so the caller knows how to allow it.
      let at = self.parser.location();
      let message = format!("{request}: no resolver is configured; call Reader::with_resolver to allow this");
      return Err(Error::well_formedness(message).at(at));
    };
    let Some(reader) = resolver.resolve(request)? else {
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
      // The DTD-side kinds are spliced into the DTD text, so they are read whole.
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
  ///
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

  /// Consumes the reader and iterates over its remaining events, each copied into an owned [`Event`].
  ///
  /// Iteration stops after the first error, yielding it as the last item.
  ///
  pub fn events(self) -> ReaderEvents<R> {
    ReaderEvents { reader: self, done: false }
  }

  /// The parser, for reading the current event through [`event_ref`](Parser::event_ref) and its surrounding context.
  ///
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
  ///
  pub fn into_inner(self) -> R {
    self.source
  }
}

/// Iterator over the events of a [`Reader`]; see [`Reader::events`].
#[derive(Debug)]
pub struct ReaderEvents<R> {
  reader: Reader<R>,
  done: bool,
}

impl<R: Read> Iterator for ReaderEvents<R> {
  type Item = Result<Event>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.done {
      return None;
    }
    match self.reader.advance() {
      Ok(Some(_)) => Some(Event::capture(self.reader.parser())),
      Ok(None) => {
        self.done = true;
        None
      }
      Err(e) => {
        self.done = true;
        Some(Err(e))
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::io;

  use super::*;

  /// A source that hands over one byte at a time, and stalls in between.
  ///
  /// Both are worth forcing: a driver that assumes a full buffer, or that treats a short read
  /// as the end of the document, passes every test against a slice and fails against a pipe.
  struct Trickle {
    bytes: Vec<u8>,
    at: usize,
    stall: bool,
  }

  impl Trickle {
    fn new(text: &str) -> Self {
      Self { bytes: text.as_bytes().to_vec(), at: 0, stall: false }
    }
  }

  impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      if self.at == self.bytes.len() {
        return Ok(0);
      }
      self.stall = !self.stall;
      if self.stall {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "not yet"));
      }
      buf[0] = self.bytes[self.at];
      self.at += 1;
      Ok(1)
    }
  }

  struct Failing;

  impl Read for Failing {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
      Err(io::Error::new(io::ErrorKind::PermissionDenied, "nope"))
    }
  }

  /// The character data of the reader's current event, for the tests that collect a run by hand.
  fn text_of<R: Read>(reader: &Reader<R>) -> &str {
    reader.parser().event_ref().and_then(|e| e.text()).expect("the current event is character data")
  }

  fn kinds<R: Read>(mut reader: Reader<R>) -> Result<Vec<EventKind>> {
    let mut kinds = Vec::new();
    while let Some(kind) = reader.advance()? {
      kinds.push(kind);
    }
    Ok(kinds)
  }

  #[test]
  fn reads_a_document_from_a_slice() {
    let kinds = kinds(Reader::new("<a>x</a>".as_bytes())).unwrap();
    assert_eq!(kinds, [EventKind::StartElement, EventKind::Text, EventKind::EndElement]);
  }

  #[test]
  fn an_explicit_encoding_overrides_sniffing() {
    // 0xE9 is 'é' in ISO-8859-1 but not valid UTF-8: naming the encoding is what makes it read.
    let bytes: &[u8] = b"<a>caf\xE9</a>";
    let mut reader = Reader::new(bytes).with_encoding("ISO-8859-1").unwrap();
    let mut text = None;
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::Text {
        text = Some(text_of(&reader).to_owned());
      }
    }
    assert_eq!(text.as_deref(), Some("café"));

    // Left to sniff, the same bytes are read as UTF-8, where 0xE9 is a fatal error.
    assert!(matches!(kinds(Reader::new(bytes)), Err(Error::Encoding { .. })));

    // An encoding this build cannot provide is refused up front.
    assert!(Reader::new(bytes).with_encoding("no-such-encoding").is_err());
  }

  #[test]
  fn an_empty_source_is_a_document_without_a_root() {
    let error = kinds(Reader::new(&b""[..])).unwrap_err();
    assert!(error.message().contains("no root element"));
  }

  #[test]
  fn io_errors_are_reported_with_their_cause() {
    let error = kinds(Reader::new(Failing)).unwrap_err();
    assert!(matches!(error, Error::Io { .. }));
    assert!(error.message().contains("cannot read"));
    assert!(std::error::Error::source(&error).is_some(), "the io::Error is kept as the cause");
  }

  #[test]
  fn a_source_that_stalls_and_trickles_parses_the_same() {
    // `Interrupted` is a real condition on a pipe; it must not end the document.
    let xml = "<?xml version='1.0'?><a x='1'>text<b/><!--c--></a>";
    let expected = kinds(Reader::new(xml.as_bytes())).unwrap();
    let mut reader = Reader::new(Trickle::new(xml));
    let mut got = Vec::new();
    loop {
      match reader.advance() {
        Ok(Some(kind)) => got.push(kind),
        Ok(None) => break,
        // Retrying an interruption is the caller's business; here it stands in for a stall.
        Err(Error::Io { .. }) => continue,
        Err(e) => panic!("{e}"),
      }
    }
    assert_eq!(got, expected);
  }

  #[test]
  fn a_document_larger_than_the_buffer_is_read_in_full() {
    let xml = format!("<a>{}</a>", "x".repeat(READ_BUFFER_SIZE * 3));
    let mut reader = Reader::new(xml.as_bytes());
    let mut text = String::new();
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::Text {
        text.push_str(text_of(&reader));
      }
    }
    assert_eq!(text.len(), READ_BUFFER_SIZE * 3);
  }

  #[test]
  fn the_system_id_reaches_the_diagnostics() {
    let mut reader = Reader::with_system_id("<a>&nosuch;</a>".as_bytes(), "file:///doc.xml");
    let error = loop {
      match reader.advance() {
        Ok(Some(_)) => {}
        Ok(None) => panic!("expected a failure"),
        Err(e) => break e,
      }
    };
    assert_eq!(error.location().system_id.as_deref(), Some("file:///doc.xml"));
  }

  #[test]
  fn events_can_be_collected() {
    let events: Vec<Event> = Reader::new("<a>x</a>".as_bytes()).events().collect::<Result<_>>().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[1].text(), Some("x"));
  }

  #[test]
  fn the_event_iterator_stops_at_the_first_error() {
    let events: Vec<_> = Reader::new("<a></b>".as_bytes()).events().collect();
    assert_eq!(events.len(), 2);
    assert!(events[1].is_err());
  }

  #[test]
  fn the_source_can_be_taken_back() {
    let reader = Reader::new("<a/>rest".as_bytes());
    assert!(!reader.into_inner().is_empty());
  }

  /// A resolver keyed on the entity name, standing in for a catalog or a filesystem.
  struct Fixtures(std::collections::HashMap<&'static str, &'static [u8]>);

  impl UriResolver for Fixtures {
    fn resolve(&mut self, request: &crate::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
      let entry = request.name().and_then(|name| self.0.get(name)).map(|bytes| bytes.to_vec());
      Ok(entry.map(|bytes| Box::new(std::io::Cursor::new(bytes)) as Box<dyn Read>))
    }
  }

  #[test]
  fn an_external_entity_is_resolved_through_the_resolver() {
    let fixtures = Fixtures([("chap", &b"<title>Ch. 1</title>"[..])].into_iter().collect());
    let xml = "<!DOCTYPE doc [<!ENTITY chap SYSTEM 'chap1.xml'>]><doc>&chap;</doc>";
    let mut reader = Reader::new(xml.as_bytes()).with_resolver(fixtures);

    let mut names = Vec::new();
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::StartElement {
        names.push(reader.parser().local_name().to_owned());
      }
    }
    // The entity's content — an element — was parsed in place.
    assert_eq!(names, ["doc", "title"]);
  }

  #[test]
  fn a_text_declaration_on_an_external_entity_is_stripped() {
    let fixtures = Fixtures([("e", &b"<?xml version='1.0' encoding='UTF-8'?>text"[..])].into_iter().collect());
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let mut reader = Reader::new(xml.as_bytes()).with_resolver(fixtures);
    let mut text = String::new();
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::Text {
        text.push_str(text_of(&reader));
      }
    }
    assert_eq!(text, "text", "the text declaration is not reported as a processing instruction");
  }

  #[test]
  fn without_a_resolver_an_external_entity_is_refused() {
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let error = kinds(Reader::new(xml.as_bytes())).unwrap_err();
    assert!(error.message().contains("no resolver is configured"), "{}", error.message());
  }

  #[test]
  fn a_declined_entity_is_a_fatal_error() {
    let fixtures = Fixtures(std::collections::HashMap::new()); // resolves nothing
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let mut reader = Reader::new(xml.as_bytes()).with_resolver(fixtures);
    let error = loop {
      match reader.advance() {
        Ok(Some(_)) => {}
        Ok(None) => panic!("expected a failure"),
        Err(e) => break e,
      }
    };
    assert!(error.message().contains("could not be resolved"));
  }

  /// A resolver that owns its bytes, for content generated at run time.
  struct OwnedEntity(&'static str, Vec<u8>);

  impl UriResolver for OwnedEntity {
    fn resolve(&mut self, request: &crate::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
      if request.name() == Some(self.0) { Ok(Some(Box::new(std::io::Cursor::new(self.1.clone())))) } else { Ok(None) }
    }
  }

  #[test]
  fn a_large_external_general_entity_streams_across_chunks() {
    // The entity is larger than one read buffer, so it is pulled through several `fill` chunks
    // rather than materialized whole; the reassembled text proves every byte arrived.
    let body = "y".repeat(READ_BUFFER_SIZE * 2 + 100);
    let entity = format!("<b>{body}</b>");
    let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
    let mut reader = Reader::new(xml.as_bytes()).with_resolver(OwnedEntity("e", entity.into_bytes()));
    let mut text = String::new();
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::Text {
        text.push_str(text_of(&reader));
      }
    }
    assert_eq!(text.len(), READ_BUFFER_SIZE * 2 + 100);
  }

  /// A reader that never ends. Reading it to completion would hang forever, so a parse that
  /// finishes at all proves the expansion limit stopped it after only a chunk or two — the
  /// streaming design at work: the whole entity is never materialized.
  struct Endless;

  impl Read for Endless {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
      buf.fill(b'y');
      Ok(buf.len())
    }
  }

  #[test]
  fn a_streamed_entity_is_stopped_mid_stream_by_the_expansion_limit() {
    struct EndlessResolver;
    impl UriResolver for EndlessResolver {
      fn resolve(&mut self, _request: &crate::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
        Ok(Some(Box::new(Endless)))
      }
    }
    let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
    let limits = Limits::default().with_max_expansion_chars(1024);
    let mut reader =
      Reader::with_document(xml.as_bytes(), Entity::document(CharStream::new()), limits).with_resolver(EndlessResolver);
    let error = loop {
      match reader.advance() {
        Ok(Some(_)) => {}
        Ok(None) => panic!("an endless entity should not parse to the end"),
        Err(e) => break e,
      }
    };
    assert!(error.message().contains("Limits::max_expansion_chars"), "{}", error.message());
  }
}
