//! A push interface over the sans-I/O pull parser: SAX-style event callbacks.
//!
//! The parser library uses a pull API: you request the next event. Some code reads more naturally the other way around,
//! with the parser calling the user's code. [`emit`](EventSource::emit) does that, running a [`Reader`] to the end and
//! calling a [`Handler`] for each event, in the shape of SAX's `ContentHandler`. Each callback is handed a small view
//! of its event, a [`StartElementEvent`], a [`CharactersEvent`], and so on, holding just that event's data: text and
//! names borrowed from the parser, and the [`Location`](xenolith_core::Location) for diagnostics.
//!
//! You can consume the events in two ways. The primitive one is to drive a [`Reader`] by hand, calling
//! [`advance`](Reader::advance) and reading each event. The typical approach is push-style: implement a [`Handler`]
//! and let a source [`emit`](EventSource::emit) events into it. It suits code that dispatches on the event kind, or a
//! port of a Java SAX `ContentHandler`. Both use the same parser, so it is a choice of shape, not capability.
//!
//! Note that xenolith does not *implement* the [SAX API](http://www.saxproject.org/ "SAX: Simple API for XML"); this
//! is SAX-*style* push parsing. Only `ContentHandler` has a direct counterpart here, [`Handler`]. The other SAX
//! handler interfaces are already implemented in an appropriate layer within the library, often as queryable data
//! rather than a stream of callbacks. The following guide explains the corresponding features for those migrating from
//! Java.
//!
//! # Examples
//!
//! A `ContentHandler`: [`emit`](EventSource::emit) calls a [`Handler`] for each event.
//!
//! ```
//! use xenolith_parser::Reader;
//! use xenolith_parser::sax::{EndElementEvent, EventSource, Handler, StartElementEvent};
//!
//! #[derive(Default)]
//! struct Depth { max: usize, current: usize }
//!
//! impl Handler for Depth {
//!   fn start_element(&mut self, _event: StartElementEvent<'_>) {
//!     self.current += 1;
//!     self.max = self.max.max(self.current);
//!   }
//!   fn end_element(&mut self, _event: EndElementEvent<'_>) {
//!     self.current -= 1;
//!   }
//! }
//!
//! let mut handler = Depth::default();
//! Reader::new("<a><b><c/></b></a>".as_bytes()).emit(&mut handler)?;
//! assert_eq!(handler.max, 3);
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! # Coming from Java's SAX
//!
//! | Java `org.xml.sax` | Here |
//! | --- | --- |
//! | `ContentHandler` | [`Handler`], driven by [`emit`](EventSource::emit) |
//! | `EntityResolver` | [`UriResolver`](xenolith_core::resolve::UriResolver), given to a reader with [`with_resolver`](Reader::with_resolver) |
//! | `ErrorHandler` | the [`Result`] from [`emit`](EventSource::emit) carries parser errors; an application problem is held by the handler itself |
//! | `DTDHandler`, ext `DeclHandler` | the parsed [`Dtd`](crate::dtd::Dtd) on [`DoctypeEvent::dtd`], in the [`doctype`](Handler::doctype) callback |
//! | ext `LexicalHandler` | [`comment`](Handler::comment), [`cdata`](Handler::cdata), and [`doctype`](Handler::doctype) |
//!
//! ## Resolving external entities (`EntityResolver`)
//!
//! Resolving an external entity is a reader concern, not a content callback: implement
//! [`UriResolver`](xenolith_core::resolve::UriResolver) and hand it to the reader. It is off by default, since resolving
//! external entities is the XML external-entity (XXE) attack surface.
//!
//! ```
//! use std::io::Read;
//!
//! use xenolith_parser::Reader;
//! use xenolith_parser::resolve::{EntityRequest, UriResolver};
//! use xenolith_parser::sax::{EventSource, Handler, StartElementEvent};
//!
//! // The resolver supplies the bytes of any entity the parser requests.
//! struct Catalog;
//! impl UriResolver for Catalog {
//!   fn resolve(&mut self, request: &EntityRequest) -> xenolith_core::Result<Option<Box<dyn Read>>> {
//!     if request.name() == Some("greeting") {
//!       Ok(Some(Box::new(std::io::Cursor::new(&b"<hello/>"[..]))))
//!     } else {
//!       Ok(None)
//!     }
//!   }
//! }
//!
//! // The handler just records the element names it is given.
//! #[derive(Default)]
//! struct Names(Vec<String>);
//! impl Handler for Names {
//!   fn start_element(&mut self, event: StartElementEvent<'_>) {
//!     self.0.push(event.pool.resolve(event.name.local()).to_owned());
//!   }
//! }
//!
//! let xml = "<!DOCTYPE doc [<!ENTITY greeting SYSTEM 'urn:greeting'>]><doc>&greeting;</doc>";
//! let mut reader = Reader::new(xml.as_bytes()).with_resolver(Catalog);
//! let mut names = Names::default();
//! reader.emit(&mut names)?;
//! assert_eq!(names.0, ["doc", "hello"]); // the entity's element was parsed in place
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! ## Errors: the parser's and the application's (`ErrorHandler`)
//!
//! A handler has no error channel. [`emit`](EventSource::emit) returns only the parser's
//! [`Error`](xenolith_core::Error), and its [`severity`](xenolith_core::Error::severity) draws SAX's line between a
//! recoverable violation ([`Severity::Error`](xenolith_core::Severity), a validity error) and a fatal one
//! ([`Severity::Fatal`](xenolith_core::Severity)). An application-level problem is the handler's own to hold. A handler
//! records it in a field and stops the run through [`should_continue`](Handler::should_continue). The caller, which
//! still owns the handler, reads it back after [`emit`](EventSource::emit) returns.
//!
//! ```
//! use xenolith_parser::Reader;
//! use xenolith_parser::sax::{EventSource, Handler};
//! use xenolith_core::Severity;
//!
//! struct Quiet;
//! impl Handler for Quiet {} // every default runs
//!
//! // A mismatched end tag is a well-formedness violation, which is fatal.
//! let error = Reader::new("<a></b>".as_bytes()).emit(&mut Quiet).unwrap_err();
//! assert_eq!(error.severity(), Severity::Fatal);
//! ```
//!
//! For application-level issues that do not involve the parser, the handler retains the relevant result and any
//! problem it detects in its own fields, and returns `false` from [`should_continue`](Handler::should_continue) to
//! stop the run once it has a problem. The caller still owns the handler, so it reads both back after
//! [`emit`](EventSource::emit) returns.
//!
//! ```
//! use xenolith_parser::Reader;
//! use xenolith_parser::sax::{CharactersEvent, EventSource, Handler, StartElementEvent};
//!
//! // Sums the numbers in `<n>` elements. A value that is not a number is an application error: the handler records it
//! // and stops, and the caller reads the outcome back.
//! #[derive(Default)]
//! struct Sum {
//!   in_number: bool,
//!   total: i64,
//!   not_a_number: Option<String>,
//! }
//!
//! impl Handler for Sum {
//!   fn start_element(&mut self, event: StartElementEvent<'_>) {
//!     self.in_number = event.pool.resolve(event.name.local()) == "n";
//!   }
//!   fn characters(&mut self, event: CharactersEvent<'_>) {
//!     if self.in_number {
//!       match event.text.trim().parse::<i64>() {
//!         Ok(value) => self.total += value,
//!         Err(_) => self.not_a_number = Some(event.text.to_owned()),
//!       }
//!     }
//!   }
//!   fn should_continue(&self) -> bool {
//!     self.not_a_number.is_none() // stop as soon as a problem is recorded
//!   }
//! }
//!
//! let mut sum = Sum::default();
//! Reader::new("<data><n>2</n><n>x</n><n>3</n></data>".as_bytes()).emit(&mut sum)?;
//! // The run stopped at the offending value, and the handler carries both the result so far and the error.
//! assert_eq!(sum.total, 2);
//! assert_eq!(sum.not_a_number.as_deref(), Some("x"));
//! # Ok::<(), xenolith_core::Error>(())
//! ```
//!
//! ## Inspecting the DTD (`DTDHandler`, `DeclHandler`)
//!
//! Notations, unparsed entities, and the element, attribute, and entity declarations are not pushed one event at a
//! time; the parser reads the whole DTD into a [`Dtd`](crate::dtd::Dtd) and hands it to the [`doctype`](Handler::doctype) callback on
//! [`DoctypeEvent::dtd`]. The parser finishes the `DOCTYPE` and both subsets before that callback fires, so the DTD is
//! already complete.
//!
//! Because the whole DTD is in hand by then, a handler that wants only the DTD can stop right there:
//! [`should_continue`](Handler::should_continue) returns `false` after the `DOCTYPE`, and
//! [`emit`](EventSource::emit) returns without reading the rest of the document.
//!
//! ```
//! use xenolith_parser::Reader;
//! use xenolith_parser::dtd::GeneralEntity;
//! use xenolith_parser::sax::{DoctypeEvent, EventSource, Handler};
//!
//! #[derive(Default)]
//! struct Dtds { gif_is_a_notation: bool, logo_is_unparsed: bool, done: bool }
//!
//! impl Handler for Dtds {
//!   fn doctype(&mut self, event: DoctypeEvent<'_>) {
//!     // DTDHandler.notationDecl: a NOTATION was declared.
//!     if let Some(gif) = event.pool.get("gif") {
//!       self.gif_is_a_notation = event.dtd.has_notation(gif);
//!     }
//!     // DTDHandler.unparsedEntityDecl: an NDATA entity that refers to a notation.
//!     if let Some(logo) = event.pool.get("logo") {
//!       self.logo_is_unparsed = matches!(event.dtd.general_entity(logo), Some(GeneralEntity::Unparsed { .. }));
//!     }
//!     self.done = true; // the DTD is all we wanted
//!   }
//!   fn should_continue(&self) -> bool {
//!     !self.done // stop right after the DOCTYPE, without reading the document body
//!   }
//! }
//!
//! let doc = "<!DOCTYPE doc [\
//!   <!NOTATION gif PUBLIC '-//example//NOTATION GIF//EN'>\
//!   <!ENTITY logo SYSTEM 'urn:logo' NDATA gif>\
//! ]><doc/>";
//! let mut dtds = Dtds::default();
//! Reader::new(doc.as_bytes()).emit(&mut dtds)?;
//! assert!(dtds.gif_is_a_notation && dtds.logo_is_unparsed);
//! # Ok::<(), xenolith_core::Error>(())
//! ```

use std::io::Read;

use xenolith_core::attr::Attributes;
use xenolith_core::error::Result;

pub use xenolith_core::event::{
  Broadcast, CdataEvent, CharactersEvent, CommentEvent, DoctypeEvent, EndElementEvent, EventSource, Handler,
  ProcessingInstructionEvent, StartElementEvent, XmlSpace,
};

use crate::parser::EventRef;
use crate::reader::Reader;

impl<R: Read> EventSource for Reader<R> {
  fn defaults_xml_id(&self) -> bool {
    self.parser().config().xml_id
  }

  fn emit<H: Handler + ?Sized>(&mut self, handler: &mut H) -> Result<()> {
    handler.start_document();
    loop {
      if self.advance()?.is_none() {
        // Reached the end of the document, so it was read in full.
        handler.end_document();
        return Ok(());
      }
      let parser = self.parser();
      let pool = parser.pool();
      let location = parser.event_location();
      match parser.event_ref() {
        Some(EventRef::StartElement { name, attributes, xml_space, xml_lang }) => {
          let attributes = Attributes::new(&attributes);
          // `base` is a local so the event can borrow the resolved base URI for this one call. Without the `xml-base`
          // feature the parser tracks no base, so the event carries none.
          #[cfg(feature = "xml-base")]
          let base = parser.base_uri();
          #[cfg(not(feature = "xml-base"))]
          let base: Option<String> = None;
          let event = StartElementEvent::new(name, attributes, xml_space, xml_lang, base.as_deref(), pool, location);
          handler.start_element(event);
        }
        Some(EventRef::EndElement { name }) => handler.end_element(EndElementEvent::new(name, pool, location)),
        Some(EventRef::Text(text)) => handler.characters(CharactersEvent::new(text, location)),
        Some(EventRef::CData(text)) => handler.cdata(CdataEvent::new(text, location)),
        Some(EventRef::Comment(text)) => handler.comment(CommentEvent::new(text, location)),
        Some(EventRef::ProcessingInstruction { target, data, data_location }) => {
          handler.processing_instruction(ProcessingInstructionEvent::new(
            target,
            data,
            data_location.clone(),
            location,
          ));
        }
        Some(EventRef::Doctype(_)) => handler.doctype(DoctypeEvent::new(
          parser.doctype_name().map(|n| pool.resolve(n)),
          parser.doctype_public_id(),
          parser.doctype_system_id(),
          parser.dtd().expect("the DTD is fully parsed by the doctype event"),
          pool,
          location,
        )),
        // The XML declaration carries no content a SAX handler models, and `advance` reported an event so there is
        // always a current one.
        _ => {}
      }
      if !handler.should_continue() {
        // A handler that has what it needs stops here; the document was not read in full, so no end_document.
        return Ok(());
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use xenolith_core::error::Error;

  use super::*;

  #[derive(Default)]
  struct Trace(Vec<String>);

  impl Handler for Trace {
    fn start_document(&mut self) {
      self.0.push("start".to_owned());
    }
    fn end_document(&mut self) {
      self.0.push("end".to_owned());
    }
    fn start_element(&mut self, event: StartElementEvent<'_>) {
      self.0.push(format!("<{}>", event.pool.resolve(event.name.local())));
    }
    fn end_element(&mut self, event: EndElementEvent<'_>) {
      self.0.push(format!("</{}>", event.pool.resolve(event.name.local())));
    }
    fn characters(&mut self, event: CharactersEvent<'_>) {
      self.0.push(format!("t:{}", event.text));
    }
    fn comment(&mut self, event: CommentEvent<'_>) {
      self.0.push(format!("!:{}", event.text));
    }
    fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) {
      self.0.push(format!("?:{} {}", event.target, event.data));
    }
  }

  #[test]
  fn parses_events_in_order() {
    let mut trace = Trace::default();
    Reader::new("<a>hi<b/><!--c--><?p d?></a>".as_bytes()).emit(&mut trace).unwrap();
    assert_eq!(trace.0, ["start", "<a>", "t:hi", "<b>", "</b>", "!:c", "?:p d", "</a>", "end"]);
  }

  #[test]
  fn a_not_well_formed_document_is_a_parse_error() {
    let mut trace = Trace::default();
    let error = Reader::new("<a></b>".as_bytes()).emit(&mut trace).unwrap_err();
    assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
  }

  #[test]
  fn each_event_locates_its_start() {
    // Every event's `location` is where its markup begins, not where reading has since reached.
    #[derive(Default)]
    struct At(Vec<(String, u32, u32)>);
    impl Handler for At {
      fn start_element(&mut self, e: StartElementEvent<'_>) {
        self.0.push((format!("<{}>", e.pool.resolve(e.name.local())), e.location.line, e.location.column));
      }
      fn end_element(&mut self, e: EndElementEvent<'_>) {
        self.0.push((format!("</{}>", e.pool.resolve(e.name.local())), e.location.line, e.location.column));
      }
      fn characters(&mut self, e: CharactersEvent<'_>) {
        self.0.push((format!("t:{}", e.text), e.location.line, e.location.column));
      }
      fn comment(&mut self, e: CommentEvent<'_>) {
        self.0.push((format!("!:{}", e.text), e.location.line, e.location.column));
      }
    }
    let mut at = At::default();
    Reader::new("<r>\n  <c/>hi<!--x--></r>".as_bytes()).emit(&mut at).unwrap();
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
    impl Handler for First {
      fn start_element(&mut self, event: StartElementEvent<'_>) {
        self.names.push(event.pool.resolve(event.name.local()).to_owned());
        self.done = true;
      }
      fn should_continue(&self) -> bool {
        !self.done
      }
    }
    let mut first = First::default();
    Reader::new("<a><b/><c/></a>".as_bytes()).emit(&mut first).unwrap();
    assert_eq!(first.names, ["a"], "only the first start element is seen");
  }

  #[test]
  fn a_handler_records_its_own_error_and_stops() {
    // A handler has no error channel; it holds any application problem in its own fields and stops the run through
    // should_continue. The caller reads it back from the handler after parse returns.
    #[derive(Default)]
    struct Reject {
      rejected: Option<String>,
    }
    impl Handler for Reject {
      fn start_element(&mut self, event: StartElementEvent<'_>) {
        if self.rejected.is_none() {
          self.rejected = Some(event.pool.resolve(event.name.local()).to_owned());
        }
      }
      fn should_continue(&self) -> bool {
        self.rejected.is_none()
      }
    }
    let mut reject = Reject::default();
    Reader::new("<a><b/></a>".as_bytes()).emit(&mut reject).unwrap();
    // The first element was recorded, and the run stopped before visiting the child.
    assert_eq!(reject.rejected.as_deref(), Some("a"));
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
    impl Handler for Seen {
      fn doctype(&mut self, event: DoctypeEvent<'_>) {
        self.notation = event.pool.get("gif").is_some_and(|id| event.dtd.has_notation(id));
        self.unparsed = event
          .pool
          .get("logo")
          .is_some_and(|id| matches!(event.dtd.general_entity(id), Some(crate::dtd::GeneralEntity::Unparsed { .. })));
        self.at = Some((event.location.line, event.location.column));
      }
    }
    let doc = "<!DOCTYPE doc [\
      <!NOTATION gif PUBLIC '-//x//NOTATION gif//EN'>\
      <!ENTITY logo SYSTEM 'urn:logo' NDATA gif>\
    ]><doc/>";
    let mut seen = Seen::default();
    Reader::new(doc.as_bytes()).emit(&mut seen).unwrap();
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
    impl Handler for Pi {
      fn processing_instruction(&mut self, e: ProcessingInstructionEvent<'_>) {
        self.target = e.target.to_owned();
        self.data = e.data.to_owned();
        self.data_at = Some((e.data_location.line, e.data_location.column, e.data_location.offset));
      }
    }
    let mut pi = Pi::default();
    Reader::new("<r><?php\n  echo 1; ?></r>".as_bytes()).emit(&mut pi).unwrap();
    assert_eq!(pi.target, "php");
    assert_eq!(pi.data, "echo 1; ");
    // `<?php` is on line 1; the separator's newline puts `data` on line 2, column 3, character offset 11.
    assert_eq!(pi.data_at, Some((2, 3, 11)));
  }

  #[test]
  fn broadcast_runs_several_handlers_in_one_pass() {
    // The fluent no-validation path: attach handlers to the source and run, without building a Dispatch by hand.
    let mut first = Trace::default();
    let mut second = Trace::default();
    Reader::new("<a>hi</a>".as_bytes()).broadcast().with_handler(&mut first).with_handler(&mut second).run().unwrap();
    let expected = ["start", "<a>", "t:hi", "</a>", "end"];
    assert_eq!(first.0, expected);
    assert_eq!(second.0, expected, "both handlers saw the same stream");
  }
}
