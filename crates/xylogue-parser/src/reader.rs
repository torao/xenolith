//! Drivers that feed the parser from a source of bytes.
//!
//! [`Parser`] never reads anything itself, so a driver is the piece that does: it answers
//! [`Progress::NeedMoreInput`] by fetching more and asking again. That is the whole of the
//! difference between the blocking reader here and the asynchronous one behind the `tokio`
//! feature, which is why there is only one parser and not two.

use std::io::Read;

use xylogue_core::error::{Error, Location, Result};

use crate::config::ParserConfig;
use crate::entity::{Entity, Limits};
use crate::event::Event;
use crate::parser::{EventKind, Parser, Progress};
use crate::resolve::UriResolver;
use crate::stream::CharStream;

/// How many bytes to read from the source at a time.
const CHUNK: usize = 8 * 1024;

/// Reads a document from anything that implements [`Read`].
///
/// # Examples
///
/// ```
/// use xylogue_parser::{EventKind, Reader};
///
/// let mut reader = Reader::new("<doc>text</doc>".as_bytes());
/// let mut depth = 0;
/// while let Some(kind) = reader.advance()? {
///   match kind {
///     EventKind::StartElement => depth += 1,
///     EventKind::Text => assert_eq!(reader.parser().text(), "text"),
///     _ => {}
///   }
/// }
/// assert_eq!(depth, 1);
/// # Ok::<(), xylogue_core::Error>(())
/// ```
///
/// Or, when owning the events is more convenient than borrowing them:
///
/// ```
/// use xylogue_parser::{Event, Reader};
///
/// let events: Vec<Event> = Reader::new("<a><b/></a>".as_bytes()).events().collect::<Result<_, _>>()?;
/// assert_eq!(events.len(), 4);
/// # Ok::<(), xylogue_core::Error>(())
/// ```
pub struct Reader<R> {
  source: R,
  parser: Parser,
  buffer: Vec<u8>,
  finished: bool,
  resolver: Option<Box<dyn UriResolver>>,
}

impl<R> std::fmt::Debug for Reader<R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Reader")
      .field("finished", &self.finished)
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
  /// use xylogue_parser::Reader;
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

  /// Reads a document over a prepared entity, with explicit limits.
  #[must_use]
  pub fn with_document(source: R, document: Entity, limits: Limits) -> Self {
    Self {
      source,
      parser: Parser::with_document(document, limits),
      buffer: vec![0; CHUNK],
      finished: false,
      resolver: None,
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
  /// use xylogue_parser::Reader;
  ///
  /// let mut reader = Reader::new("<a/>".as_bytes()).with_encoding("US-ASCII")?;
  /// assert!(reader.advance()?.is_some());
  /// # Ok::<(), xylogue_core::Error>(())
  /// ```
  pub fn with_encoding(mut self, encoding: &str) -> Result<Self> {
    self.parser.set_encoding(encoding)?;
    Ok(self)
  }

  /// Sets the parser configuration: the optional `xml:base` and `xml:id` processing.
  ///
  /// See [`ParserConfig`]. Call before the first [`advance`](Self::advance).
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.parser.set_config(config);
    self
  }

  /// Advances to the next event, reading from the source as needed.
  ///
  /// Returns `None` at the end of the document. The event itself is read through
  /// [`parser`](Self::parser).
  ///
  /// # Errors
  ///
  /// Returns [`Error::Io`] if the source fails, and whatever the parser reports for a
  /// document that breaks the rules.
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

  /// Resolves the entity the parser asked for, through the configured resolver.
  fn resolve_entity(&mut self) -> Result<()> {
    let request = self.parser.pending_entity().expect("the parser asked for an entity");
    match &mut self.resolver {
      Some(resolver) => match resolver.resolve(request)? {
        Some(bytes) => self.parser.provide_entity(&bytes),
        None => self.parser.decline_entity(),
      },
      // No resolver: external entities are refused, which is the safe default for untrusted
      // input. The message names the way to opt in.
      None => {
        let at = self.parser.location();
        let message = format!("{request}: no resolver is configured; call Reader::with_resolver to allow this");
        Err(Error::well_formedness(message).at(at))
      }
    }
  }

  /// Reads one chunk from the source into the parser.
  fn fill(&mut self) -> Result<()> {
    if self.finished {
      // The parser asked for input after the entity ended; only a bug in the parser or in a
      // hand-written driver gets here, since `feed(.., true)` settles the question.
      return Err(Error::internal("the parser asked for input beyond the end of the document"));
    }
    let read = self.source.read(&mut self.buffer).map_err(|e| {
      let at = self.parser.location();
      Error::io(format!("cannot read the document: {e}")).at(at).caused_by(e)
    })?;
    self.finished = read == 0;
    self.parser.feed(&self.buffer[..read], self.finished)
  }

  /// Iterates over the remaining events, copying each one.
  ///
  /// Iteration stops after the first error.
  pub fn events(self) -> ReaderEvents<R> {
    ReaderEvents { reader: self, done: false }
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
      Ok(Some(_)) => Some(Ok(Event::capture(self.reader.parser()))),
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
        text = Some(reader.parser().text().to_owned());
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
    let xml = format!("<a>{}</a>", "x".repeat(CHUNK * 3));
    let mut reader = Reader::new(xml.as_bytes());
    let mut text = String::new();
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::Text {
        text.push_str(reader.parser().text());
      }
    }
    assert_eq!(text.len(), CHUNK * 3);
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

  /// A resolver keyed on the entity name, standing in for a catalogue or a filesystem.
  struct Fixtures(std::collections::HashMap<&'static str, &'static [u8]>);

  impl UriResolver for Fixtures {
    fn resolve(&mut self, request: &crate::resolve::EntityRequest) -> Result<Option<Vec<u8>>> {
      Ok(request.name().and_then(|name| self.0.get(name)).map(|bytes| bytes.to_vec()))
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
        text.push_str(reader.parser().text());
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
}
