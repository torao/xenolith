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
fn text_of<'a, R: Read>(reader: &'a StreamSource<'_, R>) -> &'a str {
  reader.parser().token_ref().and_then(|e| e.text()).expect("the current event is character data")
}

fn kinds<R: Read>(mut reader: StreamSource<'_, R>) -> Result<Vec<TokenKind>> {
  let mut kinds = Vec::new();
  while let Some(kind) = reader.advance()? {
    kinds.push(kind);
  }
  Ok(kinds)
}

#[test]
fn reads_a_document_from_a_slice() {
  let kinds = kinds(StreamSource::new("<a>x</a>".as_bytes())).unwrap();
  assert_eq!(kinds, [TokenKind::StartElement, TokenKind::Text, TokenKind::EndElement]);
}

#[test]
fn an_explicit_encoding_overrides_sniffing() {
  // 0xE9 is 'é' in ISO-8859-1 but not valid UTF-8: giving the encoding is what makes it read.
  let bytes: &[u8] = b"<a>caf\xE9</a>";
  let mut reader = StreamSource::new(bytes).with_encoding("ISO-8859-1").unwrap();
  let mut text = None;
  while let Some(kind) = reader.advance().unwrap() {
    if kind == TokenKind::Text {
      text = Some(text_of(&reader).to_owned());
    }
  }
  assert_eq!(text.as_deref(), Some("café"));

  // Left to sniff, the same bytes are read as UTF-8, where 0xE9 is a fatal error.
  assert!(matches!(kinds(StreamSource::new(bytes)), Err(Error::Encoding { .. })));

  // An encoding this build cannot provide is refused up front.
  assert!(StreamSource::new(bytes).with_encoding("no-such-encoding").is_err());
}

#[test]
fn an_empty_source_is_a_document_without_a_root() {
  let error = kinds(StreamSource::new(&b""[..])).unwrap_err();
  assert!(error.message().contains("no root element"));
}

#[test]
fn io_errors_are_reported_with_their_cause() {
  let error = kinds(StreamSource::new(Failing)).unwrap_err();
  assert!(matches!(error, Error::Io { .. }));
  assert!(error.message().contains("cannot read"));
  assert!(std::error::Error::source(&error).is_some(), "the io::Error is kept as the cause");
}

#[test]
fn a_source_that_stalls_and_trickles_parses_the_same() {
  // `Interrupted` is a real condition on a pipe; it must not end the document.
  let xml = "<?xml version='1.0'?><a x='1'>text<b/><!--c--></a>";
  let expected = kinds(StreamSource::new(xml.as_bytes())).unwrap();
  let mut reader = StreamSource::new(Trickle::new(xml));
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
  let mut reader = StreamSource::new(xml.as_bytes());
  let mut text = String::new();
  while let Some(kind) = reader.advance().unwrap() {
    if kind == TokenKind::Text {
      text.push_str(text_of(&reader));
    }
  }
  assert_eq!(text.len(), READ_BUFFER_SIZE * 3);
}

#[test]
fn the_system_id_reaches_the_diagnostics() {
  let mut reader = StreamSource::with_system_id("<a>&nosuch;</a>".as_bytes(), "file:///doc.xml");
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
fn the_source_can_be_taken_back() {
  let reader = StreamSource::new("<a/>rest".as_bytes());
  assert!(!reader.into_inner().is_empty());
}

/// A resolver keyed on the entity name, standing in for a catalog or a filesystem.
struct Fixtures(std::collections::HashMap<&'static str, &'static [u8]>);

impl UriResolver for Fixtures {
  fn resolve(&mut self, request: &crate::io::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
    let entry = request.name().and_then(|name| self.0.get(name)).map(|bytes| bytes.to_vec());
    Ok(entry.map(|bytes| Box::new(std::io::Cursor::new(bytes)) as Box<dyn Read>))
  }
}

#[test]
fn an_external_entity_is_resolved_through_the_resolver() {
  let fixtures = Fixtures([("chap", &b"<title>Ch. 1</title>"[..])].into_iter().collect());
  let xml = "<!DOCTYPE doc [<!ENTITY chap SYSTEM 'chap1.xml'>]><doc>&chap;</doc>";
  let mut reader = StreamSource::new(xml.as_bytes()).with_resolver(fixtures);

  let mut names = Vec::new();
  while let Some(kind) = reader.advance().unwrap() {
    if kind == TokenKind::StartElement {
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
  let mut reader = StreamSource::new(xml.as_bytes()).with_resolver(fixtures);
  let mut text = String::new();
  while let Some(kind) = reader.advance().unwrap() {
    if kind == TokenKind::Text {
      text.push_str(text_of(&reader));
    }
  }
  assert_eq!(text, "text", "the text declaration is not reported as a processing instruction");
}

#[test]
fn without_a_resolver_an_external_entity_is_refused() {
  let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
  let error = kinds(StreamSource::new(xml.as_bytes())).unwrap_err();
  assert!(error.message().contains("no resolver is configured"), "{}", error.message());
}

#[test]
fn a_declined_entity_is_a_fatal_error() {
  let fixtures = Fixtures(std::collections::HashMap::new()); // resolves nothing
  let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
  let mut reader = StreamSource::new(xml.as_bytes()).with_resolver(fixtures);
  let error = loop {
    match reader.advance() {
      Ok(Some(_)) => {}
      Ok(None) => panic!("expected a failure"),
      Err(e) => break e,
    }
  };
  assert!(error.message().contains("could not be resolved"));
}

/// A resolver that fails the way an application's would, with its own error and no position to report.
struct Broken;

impl UriResolver for Broken {
  fn resolve(&mut self, _request: &crate::io::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
    let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the catalog is locked");
    Err(Error::resolver(cause))
  }
}

#[test]
fn a_failure_in_the_resolver_is_reported_where_the_reference_stood() {
  // The resolver is handed a request and no position, so its own error carries none. The reader knows where the
  // reference was, and a caller needs that to point at the document rather than at the catalog.
  let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]>\n<doc>&e;</doc>";
  let mut reader = StreamSource::with_system_id(xml.as_bytes(), "file:///doc.xml").with_resolver(Broken);
  let error = loop {
    match reader.advance() {
      Ok(Some(_)) => {}
      Ok(None) => panic!("expected the resolver's failure"),
      Err(e) => break e,
    }
  };

  assert!(matches!(error, Error::Resolver { .. }), "{error:?}");
  assert!(error.message().contains("the catalog is locked"));
  let at = error.location();
  assert_eq!(at.system_id.as_deref(), Some("file:///doc.xml"));
  assert_eq!((at.line, at.column), (2, 6), "the reference stands at line 2, after `<doc>`");
}

/// A resolver that owns its bytes, for content generated at run time.
struct OwnedEntity(&'static str, Vec<u8>);

impl UriResolver for OwnedEntity {
  fn resolve(&mut self, request: &crate::io::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
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
  let mut reader = StreamSource::new(xml.as_bytes()).with_resolver(OwnedEntity("e", entity.into_bytes()));
  let mut text = String::new();
  while let Some(kind) = reader.advance().unwrap() {
    if kind == TokenKind::Text {
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
    fn resolve(&mut self, _request: &crate::io::resolve::EntityRequest) -> Result<Option<Box<dyn Read>>> {
      Ok(Some(Box::new(Endless)))
    }
  }
  let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
  let mut config = ParserConfig::default();
  config.limits.entities.max_expansion_chars = Some(1024);
  let mut reader = StreamSource::new(xml.as_bytes()).with_config(config).with_resolver(EndlessResolver);
  let error = loop {
    match reader.advance() {
      Ok(Some(_)) => {}
      Ok(None) => panic!("an endless entity should not parse to the end"),
      Err(e) => break e,
    }
  };
  assert!(error.message().contains("limits.entities.max_expansion_chars"), "{}", error.message());
}

/// Driving handlers with the parser: the events it reports, their order and locations, and how a handler ends a run.
///
/// These were the tests of the `io::sax` module, which was a guide and a second set of names for
/// [`crate::event`](crate::event) and held no code of its own. What they exercise is this reader driving handlers.
mod push {
  use crate::error::{Error, Result};
  use crate::event::{Dispatch, EventCursor, EventHandler, EventRef, EventSource};
  use crate::io::StreamSource;

  #[derive(Default)]
  struct Trace(Vec<String>);

  impl EventHandler for Trace {
    fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
      self.0.push(match event {
        EventRef::StartDocument => "start".to_owned(),
        EventRef::EndDocument => "end".to_owned(),
        EventRef::StartElement(event) => format!("<{}>", event.local),
        EventRef::EndElement(event) => format!("</{}>", event.local),
        EventRef::Characters(event) => format!("t:{}", event.text),
        EventRef::Comment(event) => format!("!:{}", event.text),
        EventRef::ProcessingInstruction(event) => format!("?:{} {}", event.target, event.data),
        EventRef::Cdata(_) | EventRef::Doctype(_) => return Ok(()),
      });
      Ok(())
    }
  }

  #[test]
  fn parses_events_in_order() {
    let mut trace = Trace::default();
    StreamSource::new("<a>hi<b/><!--c--><?p d?></a>".as_bytes()).with_handler(&mut trace).emit().unwrap();
    assert_eq!(trace.0, ["start", "<a>", "t:hi", "<b>", "</b>", "!:c", "?:p d", "</a>", "end"]);
  }

  #[test]
  fn a_not_well_formed_document_is_a_parse_error() {
    let mut trace = Trace::default();
    let error = StreamSource::new("<a></b>".as_bytes()).with_handler(&mut trace).emit().unwrap_err();
    assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
  }

  #[test]
  fn each_event_locates_its_start() {
    // Every event's `location` is where its markup begins, not where reading has since reached.
    #[derive(Default)]
    struct At(Vec<(String, u32, u32)>);
    impl EventHandler for At {
      fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
        let what = match event {
          EventRef::StartElement(e) => format!("<{}>", e.local),
          EventRef::EndElement(e) => format!("</{}>", e.local),
          EventRef::Characters(e) => format!("t:{}", e.text),
          EventRef::Comment(e) => format!("!:{}", e.text),
          _ => return Ok(()),
        };
        let at = event.location().expect("a markup event locates itself");
        self.0.push((what, at.line, at.column));
        Ok(())
      }
    }
    let mut at = At::default();
    StreamSource::new("<r>\n  <c/>hi<!--x--></r>".as_bytes()).with_handler(&mut at).emit().unwrap();
    assert_eq!(
      at.0,
      [
        ("<r>".to_owned(), 1, 1),    // the `<` of <r>
        ("t:\n  ".to_owned(), 1, 4), // the whitespace run starts at the newline after <r>
        ("<c>".to_owned(), 2, 3),    // after the two leading spaces on line 2
        ("</c>".to_owned(), 2, 3),   // the empty element's implied end, at the same `<c/>`
        ("t:hi".to_owned(), 2, 7),   // the first character of the text run
        ("!:x".to_owned(), 2, 9),    // the `<` of <!--x-->
        ("</r>".to_owned(), 2, 17),  // the `<` of </r>
      ]
    );
  }

  #[test]
  fn a_handler_stops_the_run_early() {
    // Collect the first element name, then request a stop; the rest of the document is not visited.
    #[derive(Default)]
    struct First {
      names: Vec<String>,
      done: bool,
    }
    impl EventHandler for First {
      fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
        if let EventRef::StartElement(event) = event {
          self.names.push(event.local.to_owned());
          self.done = true;
        }
        Ok(())
      }
      fn should_continue(&self) -> bool {
        !self.done
      }
    }
    let mut first = First::default();
    StreamSource::new("<a><b/><c/></a>".as_bytes()).with_handler(&mut first).emit().unwrap();
    assert_eq!(first.names, ["a"], "only the first start element is seen");
  }

  #[test]
  fn a_handler_refuses_the_document_and_the_error_comes_back_out() {
    // A handler's own objection travels the same path as the parser's: `handle` returns `Err`, and `emit` hands it to
    // the caller. What the handler collected up to that point is still on the handler the caller owns.
    #[derive(Default)]
    struct Reject {
      seen: Vec<String>,
    }
    impl EventHandler for Reject {
      fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
        if let EventRef::StartElement(event) = event {
          let name = event.local.to_owned();
          if name == "b" {
            return Err(Error::internal("b is not allowed here"));
          }
          self.seen.push(name);
        }
        Ok(())
      }
    }
    let mut reject = Reject::default();
    let error = StreamSource::new("<a><b/></a>".as_bytes()).with_handler(&mut reject).emit().unwrap_err();
    assert!(error.message().contains("not allowed"), "{error}");
    assert_eq!(reject.seen, ["a"], "what it saw before refusing is still there");
  }

  #[test]
  fn the_doctype_callback_reaches_the_dtd() {
    // The whole DTD is parsed by the doctype event, so notations and unparsed entities are reachable there.
    #[derive(Default)]
    struct Seen {
      notation: bool,
      unparsed: bool,
      at: Option<(u32, u32)>,
    }
    impl EventHandler for Seen {
      fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
        if let EventRef::Doctype(event) = event {
          self.notation = event.pool.get("gif").is_some_and(|id| event.dtd.has_notation(id));
          self.unparsed = event.pool.get("logo").is_some_and(|id| {
            matches!(event.dtd.general_entity(id), Some(crate::dtd::read::GeneralEntity::Unparsed { .. }))
          });
          self.at = Some((event.location.line, event.location.column));
        }
        Ok(())
      }
    }
    let doc = "<!DOCTYPE doc [\
      <!NOTATION gif PUBLIC '-//x//NOTATION gif//EN'>\
      <!ENTITY logo SYSTEM 'urn:logo' NDATA gif>\
    ]><doc/>";
    let mut seen = Seen::default();
    StreamSource::new(doc.as_bytes()).with_handler(&mut seen).emit().unwrap();
    assert!(seen.notation, "the NOTATION declaration is reachable");
    assert!(seen.unparsed, "the NDATA entity is reachable");
    // The location is the start of `<!DOCTYPE`, kept across the whole DTD parse, not the `]>` at its end.
    assert_eq!(seen.at, Some((1, 1)));
  }

  #[test]
  fn a_processing_instruction_locates_its_data() {
    // The separator between target and data is dropped, so `data_location` is how a handler finds where `data`
    // begins, even when that separator spans a newline.
    #[derive(Default)]
    struct Pi {
      target: String,
      data: String,
      data_at: Option<(u32, u32, u64)>,
    }
    impl EventHandler for Pi {
      fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
        if let EventRef::ProcessingInstruction(e) = event {
          self.target = e.target.to_owned();
          self.data = e.data.to_owned();
          self.data_at = Some((e.data_location.line, e.data_location.column, e.data_location.offset));
        }
        Ok(())
      }
    }
    let mut pi = Pi::default();
    StreamSource::new("<r><?php\n  echo 1; ?></r>".as_bytes()).with_handler(&mut pi).emit().unwrap();
    assert_eq!(pi.target, "php");
    assert_eq!(pi.data, "echo 1; ");
    // `<?php` is on line 1; the separator's newline puts `data` on line 2, column 3, character offset 11.
    assert_eq!(pi.data_at, Some((2, 3, 11)));
  }

  #[test]
  fn a_dispatch_runs_several_handlers_in_one_pass() {
    let mut first = Trace::default();
    let mut second = Trace::default();
    let mut both = Dispatch::new().with_handler(&mut first).with_handler(&mut second);
    StreamSource::new("<a>hi</a>".as_bytes()).with_handler(&mut both).emit().unwrap();
    drop(both);
    let expected = ["start", "<a>", "t:hi", "</a>", "end"];
    assert_eq!(first.0, expected);
    assert_eq!(second.0, expected, "both handlers saw the same stream");
  }
}

#[test]
fn a_document_that_ends_in_the_middle_of_text_is_refused() {
  // The last run of text is flushed as its own token when the input ends, and that token is reported like any other:
  // a reader that could not name it panicked here instead of reporting the element left open.
  for xml in [
    "<a>text",
    "<a>text
",
    "<!DOCTYPE a [<!ELEMENT a (#PCDATA)>]><a>text",
  ] {
    let error = StreamSource::new(xml.as_bytes()).emit().expect_err("the element is never closed");
    assert!(error.message().contains("is not closed"), "{xml:?}: {error}");
  }
}
