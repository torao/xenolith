//! A push interface over the sans-I/O pull parser: SAX-style event callbacks.
//!
//! The parser library is based on a pull API: you request the next event. Some code reads more naturally the other
//! way round, with the parser calling user's code. [`parse`] does that, running a [`Reader`] to the end and calling a
//! [`Handler`] for each event, in the shape of SAX's `ContentHandler`. Each callback is handed a small view of its
//! event, a [`StartElementEvent`], a [`CharactersEvent`], and so on, holding just that event's data: text and names
//! borrowed from the parser, and the [`Location`] for diagnostics.
//!
//! Driving a [`Reader`] yourself is the usual choice; reach for [`parse`] when your code is a handler that dispatches
//! on the event kind, or when porting a Java SAX `ContentHandler`. The two run the same parser, so it is a choice of
//! shape, not capability.
//!
//! xylogue does not *implement* the [SAX API](http://www.saxproject.org/ "SAX: Simple API for XML"); this is
//! SAX-*style* push parsing. Only `ContentHandler` has a direct counterpart here, [`Handler`]. The capabilities
//! provided by other SAX handler interfaces are already implemented in an appropriate layer within the library, often
//! as queryable data rather than a stream of callbacks. The following guide explains the corresponding features for
//! those migrating from Java.
//!
//! # Examples
//!
//! A `ContentHandler`: [`parse`] calls a [`Handler`] for each event.
//!
//! ```
//! use std::convert::Infallible;
//!
//! use xylogue_parser::Reader;
//! use xylogue_parser::sax::{EndElementEvent, Handler, StartElementEvent, parse};
//!
//! #[derive(Default)]
//! struct Depth { max: usize, current: usize }
//!
//! impl Handler for Depth {
//!   type Error = Infallible;
//!   fn start_element(&mut self, _event: StartElementEvent<'_>) -> Result<(), Infallible> {
//!     self.current += 1;
//!     self.max = self.max.max(self.current);
//!     Ok(())
//!   }
//!   fn end_element(&mut self, _event: EndElementEvent<'_>) -> Result<(), Infallible> {
//!     self.current -= 1;
//!     Ok(())
//!   }
//! }
//!
//! let mut handler = Depth::default();
//! parse(&mut Reader::new("<a><b><c/></b></a>".as_bytes()), &mut handler)?;
//! assert_eq!(handler.max, 3);
//! # Ok::<(), xylogue_core::Error>(())
//! ```
//!
//! # Coming from Java's SAX
//!
//! | Java `org.xml.sax` | Here |
//! | --- | --- |
//! | `ContentHandler` | [`Handler`], driven by [`parse`] |
//! | `EntityResolver` | [`UriResolver`](crate::resolve::UriResolver), given to a reader with [`with_resolver`](Reader::with_resolver) |
//! | `ErrorHandler` | the [`Result`] from [`parse`], with [`severity`](Error::severity) telling recoverable from fatal |
//! | `DTDHandler`, ext `DeclHandler` | the parsed [`Dtd`] on [`DoctypeEvent::dtd`], in the [`doctype`](Handler::doctype) callback |
//! | ext `LexicalHandler` | [`comment`](Handler::comment), [`cdata`](Handler::cdata), and [`doctype`](Handler::doctype) |
//!
//! ## Resolving external entities (`EntityResolver`)
//!
//! Resolving an external entity is a reader concern, not a content callback: implement
//! [`UriResolver`](crate::resolve::UriResolver) and hand it to the reader. It is off by default, since resolving
//! external entities is the XML external-entity (XXE) attack surface.
//!
//! ```
//! use std::convert::Infallible;
//! use std::io::Read;
//!
//! use xylogue_parser::Reader;
//! use xylogue_parser::resolve::{EntityRequest, UriResolver};
//! use xylogue_parser::sax::{Handler, StartElementEvent, parse};
//!
//! // The resolver supplies the bytes of any entity the parser requests.
//! struct Catalog;
//! impl UriResolver for Catalog {
//!   fn resolve(&mut self, request: &EntityRequest) -> xylogue_core::Result<Option<Box<dyn Read>>> {
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
//!   type Error = Infallible;
//!   fn start_element(&mut self, event: StartElementEvent<'_>) -> Result<(), Infallible> {
//!     self.0.push(event.pool.resolve(event.name.local()).to_owned());
//!     Ok(())
//!   }
//! }
//!
//! let xml = "<!DOCTYPE doc [<!ENTITY greeting SYSTEM 'urn:greeting'>]><doc>&greeting;</doc>";
//! let mut reader = Reader::new(xml.as_bytes()).with_resolver(Catalog);
//! let mut names = Names::default();
//! parse(&mut reader, &mut names)?;
//! assert_eq!(names.0, ["doc", "hello"]); // the entity's element was parsed in place
//! # Ok::<(), xylogue_core::Error>(())
//! ```
//!
//! ## Errors and their severity (`ErrorHandler`)
//!
//! There is no error callback: [`parse`] returns the parser's [`Error`], and its [`severity`](Error::severity) draws
//! SAX's line between a recoverable violation ([`Severity::Error`](xylogue_core::Severity), a validity error) and a
//! fatal one ([`Severity::Fatal`](xylogue_core::Severity)). A handler's own error comes back wrapped in
//! [`Error::SaxHandler`].
//!
//! ```
//! use std::convert::Infallible;
//!
//! use xylogue_parser::Reader;
//! use xylogue_parser::sax::{Handler, parse};
//! use xylogue_core::Severity;
//!
//! struct Quiet;
//! impl Handler for Quiet {
//!   type Error = Infallible; // overrides nothing, so every default runs
//! }
//!
//! // A mismatched end tag is a well-formedness violation, which is fatal.
//! let error = parse(&mut Reader::new("<a></b>".as_bytes()), &mut Quiet).unwrap_err();
//! assert_eq!(error.severity(), Severity::Fatal);
//! ```
//!
//! ## Inspecting the DTD (`DTDHandler`, `DeclHandler`)
//!
//! Notations, unparsed entities, and the element, attribute, and entity declarations are not pushed one event at a
//! time; the parser reads the whole DTD into a [`Dtd`] and hands it to the [`doctype`](Handler::doctype) callback on
//! [`DoctypeEvent::dtd`]. The parser finishes the `DOCTYPE`, both subsets, before that callback fires, so the DTD is
//! already complete.
//!
//! Because the whole DTD is in hand by then, a handler that wants only the DTD can stop right there:
//! [`should_continue`](Handler::should_continue) returns `false` after the `DOCTYPE`, and [`parse`] returns without
//! reading the rest of the document.
//!
//! ```
//! use std::convert::Infallible;
//!
//! use xylogue_parser::Reader;
//! use xylogue_parser::dtd::GeneralEntity;
//! use xylogue_parser::sax::{DoctypeEvent, Handler, parse};
//!
//! #[derive(Default)]
//! struct Dtds { gif_is_a_notation: bool, logo_is_unparsed: bool, done: bool }
//!
//! impl Handler for Dtds {
//!   type Error = Infallible;
//!   fn doctype(&mut self, event: DoctypeEvent<'_>) -> Result<(), Infallible> {
//!     // DTDHandler.notationDecl: a NOTATION was declared.
//!     if let Some(gif) = event.pool.get("gif") {
//!       self.gif_is_a_notation = event.dtd.has_notation(gif);
//!     }
//!     // DTDHandler.unparsedEntityDecl: an NDATA entity that names a notation.
//!     if let Some(logo) = event.pool.get("logo") {
//!       self.logo_is_unparsed = matches!(event.dtd.general_entity(logo), Some(GeneralEntity::Unparsed { .. }));
//!     }
//!     self.done = true; // the DTD is all we wanted
//!     Ok(())
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
//! parse(&mut Reader::new(doc.as_bytes()), &mut dtds)?;
//! assert!(dtds.gif_is_a_notation && dtds.logo_is_unparsed);
//! # Ok::<(), xylogue_core::Error>(())
//! ```

use std::io::Read;

use xylogue_core::error::{Error, Location, Result};
use xylogue_core::name::{NamePool, QName};

use crate::dtd::Dtd;
use crate::parser::{Attributes, EventRef, XmlSpace};
use crate::reader::Reader;

/// A start element and the scope around it, borrowed from the parser for one callback.
///
/// `name` is an interned [`QName`]; resolve it, and the attribute names, to strings through `pool`.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StartElementEvent<'a> {
  /// The element's qualified name.
  pub name: QName,
  /// The attributes, in document order, namespace declarations included.
  pub attributes: Attributes<'a>,
  /// The `xml:space` in effect inside this element.
  pub xml_space: XmlSpace,
  /// The `xml:lang` in effect inside this element, if any.
  pub xml_lang: Option<&'a str>,
  /// The name pool, for resolving `name` and the attribute names to strings.
  pub pool: &'a NamePool,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// An end element, borrowed from the parser for one callback.
///
/// `name` is an interned [`QName`]; resolve it to a string through `pool`.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EndElementEvent<'a> {
  /// The element's qualified name.
  pub name: QName,
  /// The name pool, for resolving `name` to a string.
  pub pool: &'a NamePool,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// A run of character data, with references expanded.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CharactersEvent<'a> {
  /// The text.
  pub text: &'a str,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// The content of a CDATA section, reported apart from ordinary character data.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CdataEvent<'a> {
  /// The section's text: everything between `<![CDATA[` and `]]>`. References are not expanded and nothing is
  /// trimmed, so `<![CDATA[ a<b ]]>` gives ` a<b `.
  pub text: &'a str,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// A comment, without its delimiters.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommentEvent<'a> {
  /// The comment's text: everything between `<!--` and `-->`, verbatim, so `<!-- note -->` gives ` note ` with its
  /// surrounding spaces.
  pub text: &'a str,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// A processing instruction.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProcessingInstructionEvent<'a> {
  /// The target: the name right after `<?`, ending at the first whitespace, or at `?>` when there is no data.
  pub target: &'a str,
  /// Everything after the target and the whitespace separating it, up to `?>`. That one run of separating whitespace is
  /// dropped and nothing else is trimmed; it is empty when the instruction is only a target. So `<?php echo 1; ?>`
  /// gives target `php` and data `echo 1; `, the trailing space kept.
  pub data: &'a str,
  /// Where `data` begins in the source. `location` marks the `<?`, but the dropped separator whitespace hides where
  /// `data` starts, so this anchors it: a handler that parses `data` as a foreign language adds the position it finds
  /// within `data` to this to map back to the document.
  pub data_location: Location,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// A document type declaration: its name, external identifiers, and the parsed DTD.
///
/// By this event, the whole DTD has been read, including its internal subset, external subset, and any external
/// parameter entities, so `dtd` is the counterpart of SAX's `DTDHandler` and `DeclHandler`: it reaches notations,
/// unparsed entities, and the element, attribute, and entity declarations through it, interning a name with `pool`
/// to look it up.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DoctypeEvent<'a> {
  /// The root element name the declaration gave, if it gave one.
  pub name: Option<&'a str>,
  /// The public identifier, if any.
  pub public_id: Option<&'a str>,
  /// The system identifier, if any.
  pub system_id: Option<&'a str>,
  /// The parsed DTD, both its internal and external subsets.
  pub dtd: &'a Dtd,
  /// The name pool, for interning a name to query `dtd` and resolving the ids it returns.
  pub pool: &'a NamePool,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

/// Receives parser events as they are read. Every method has a default, so handlers override only what they care about.
///
/// Each event callback receives a borrowed `*Event` view of its event, valid only for that call, and returns
/// `Result<(), Self::Error>` so it can abort with an application error when it detects one. [`parse`] wraps that error
/// in [`Error::SaxHandler`], from which a caller downcasts to recover it. After each event [`parse`] calls
/// [`should_continue`](Self::should_continue); returning `false` stops the run early, for a handler that has read all
/// it needs. [`parse`] never calls back for the XML declaration, which a SAX handler models nothing from.
///
pub trait Handler {
  /// The application error that a callback may raise. [`parse`] boxes it into [`Error::SaxHandler`] for a caller to
  /// downcast, so it must be a [`std::error::Error`] that is `Send + Sync + 'static`; use
  /// [`Infallible`](core::convert::Infallible) for a handler that never fails.
  ///
  type Error: std::error::Error + Send + Sync + 'static;

  /// [`parse`] calls this before any other event.
  ///
  fn start_document(&mut self) -> Result<(), Self::Error> {
    Ok(())
  }

  /// [`parse`] calls this after the last event, with the document read to its end. It does not call this when a
  /// handler has stopped the run early through [`should_continue`](Self::should_continue).
  ///
  fn end_document(&mut self) -> Result<(), Self::Error> {
    Ok(())
  }

  /// [`parse`] calls this at the start of an element.
  ///
  fn start_element(&mut self, event: StartElementEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this at the end of an element, the implied end of an empty one included.
  ///
  fn end_element(&mut self, event: EndElementEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this for a run of character data. One run may arrive as several calls, so a handler that wants a
  /// maximal run coalesces them.
  ///
  fn characters(&mut self, event: CharactersEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this for a CDATA section's content, which it keeps separate from ordinary character data.
  ///
  fn cdata(&mut self, event: CdataEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this for a comment.
  ///
  fn comment(&mut self, event: CommentEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this for a processing instruction.
  ///
  fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this for the document type declaration.
  ///
  fn doctype(&mut self, event: DoctypeEvent<'_>) -> Result<(), Self::Error> {
    let _ = event;
    Ok(())
  }

  /// [`parse`] calls this after each event and stops the run early if it returns `false`, before the document ends.
  /// The default keeps going.
  ///
  fn should_continue(&self) -> bool {
    true
  }
}

/// Runs `reader` to the end, or until a handler stops it, calling `handler` for each event.
///
/// It calls [`start_document`](Handler::start_document) first, an event callback for each event as it reads, and
/// [`end_document`](Handler::end_document) when it reaches the end. A handler that stops the run early through
/// [`should_continue`](Handler::should_continue) ends it before that final call, which is not an error.
///
/// # Errors
///
/// Returns the parser's error if the document is not well-formed or reading fails, and [`Error::SaxHandler`], wrapping
/// the handler's own error, if a callback raises one.
///
pub fn parse<R: Read, H: Handler>(reader: &mut Reader<R>, handler: &mut H) -> Result<()> {
  handler.start_document().map_err(Error::sax_handler)?;
  loop {
    if reader.advance()?.is_none() {
      // Reached the end of the document, so it was read in full.
      return handler.end_document().map_err(Error::sax_handler);
    }
    let parser = reader.parser();
    let pool = parser.pool();
    let location = parser.event_location();
    // The event takes `location` by value; keep a copy to place on a handler error.
    let at = location.clone();
    match parser.event_ref() {
      Some(EventRef::StartElement { name, attributes, xml_space, xml_lang }) => {
        handler.start_element(StartElementEvent { name, attributes, xml_space, xml_lang, pool, location })
      }
      Some(EventRef::EndElement { name }) => handler.end_element(EndElementEvent { name, pool, location }),
      Some(EventRef::Text(text)) => handler.characters(CharactersEvent { text, location }),
      Some(EventRef::CData(text)) => handler.cdata(CdataEvent { text, location }),
      Some(EventRef::Comment(text)) => handler.comment(CommentEvent { text, location }),
      Some(EventRef::ProcessingInstruction { target, data, data_location }) => {
        handler.processing_instruction(ProcessingInstructionEvent {
          target,
          data,
          data_location: data_location.clone(),
          location,
        })
      }
      Some(EventRef::Doctype(_)) => handler.doctype(DoctypeEvent {
        name: parser.doctype_name().map(|n| pool.resolve(n)),
        public_id: parser.doctype_public_id(),
        system_id: parser.doctype_system_id(),
        dtd: parser.dtd().expect("the DTD is fully parsed by the doctype event"),
        pool,
        location,
      }),
      // The XML declaration carries no content a SAX handler models, and `advance` reported an event so there is
      // always a current one.
      _ => Ok(()),
    }
    .map_err(|e| Error::sax_handler(e).at(at))?;
    if !handler.should_continue() {
      // A handler that has what it needs stops here; the document was not read in full, so no end_document.
      return Ok(());
    }
  }
}

#[cfg(test)]
mod tests {
  use std::convert::Infallible;

  use super::*;

  #[derive(Default)]
  struct Trace(Vec<String>);

  impl Handler for Trace {
    type Error = Infallible;
    fn start_document(&mut self) -> Result<(), Infallible> {
      self.0.push("start".to_owned());
      Ok(())
    }
    fn end_document(&mut self) -> Result<(), Infallible> {
      self.0.push("end".to_owned());
      Ok(())
    }
    fn start_element(&mut self, event: StartElementEvent<'_>) -> Result<(), Infallible> {
      self.0.push(format!("<{}>", event.pool.resolve(event.name.local())));
      Ok(())
    }
    fn end_element(&mut self, event: EndElementEvent<'_>) -> Result<(), Infallible> {
      self.0.push(format!("</{}>", event.pool.resolve(event.name.local())));
      Ok(())
    }
    fn characters(&mut self, event: CharactersEvent<'_>) -> Result<(), Infallible> {
      self.0.push(format!("t:{}", event.text));
      Ok(())
    }
    fn comment(&mut self, event: CommentEvent<'_>) -> Result<(), Infallible> {
      self.0.push(format!("!:{}", event.text));
      Ok(())
    }
    fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) -> Result<(), Infallible> {
      self.0.push(format!("?:{} {}", event.target, event.data));
      Ok(())
    }
  }

  #[test]
  fn parses_events_in_order() {
    let mut trace = Trace::default();
    parse(&mut Reader::new("<a>hi<b/><!--c--><?p d?></a>".as_bytes()), &mut trace).unwrap();
    assert_eq!(trace.0, ["start", "<a>", "t:hi", "<b>", "</b>", "!:c", "?:p d", "</a>", "end"]);
  }

  #[test]
  fn a_not_well_formed_document_is_a_parse_error() {
    let mut trace = Trace::default();
    let error = parse(&mut Reader::new("<a></b>".as_bytes()), &mut trace).unwrap_err();
    assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
  }

  #[test]
  fn each_event_locates_its_start() {
    // Every event's `location` is where its markup begins, not where reading has since reached.
    #[derive(Default)]
    struct At(Vec<(String, u32, u32)>);
    impl Handler for At {
      type Error = Infallible;
      fn start_element(&mut self, e: StartElementEvent<'_>) -> Result<(), Infallible> {
        self.0.push((format!("<{}>", e.pool.resolve(e.name.local())), e.location.line, e.location.column));
        Ok(())
      }
      fn end_element(&mut self, e: EndElementEvent<'_>) -> Result<(), Infallible> {
        self.0.push((format!("</{}>", e.pool.resolve(e.name.local())), e.location.line, e.location.column));
        Ok(())
      }
      fn characters(&mut self, e: CharactersEvent<'_>) -> Result<(), Infallible> {
        self.0.push((format!("t:{}", e.text), e.location.line, e.location.column));
        Ok(())
      }
      fn comment(&mut self, e: CommentEvent<'_>) -> Result<(), Infallible> {
        self.0.push((format!("!:{}", e.text), e.location.line, e.location.column));
        Ok(())
      }
    }
    let mut at = At::default();
    parse(&mut Reader::new("<r>\n  <c/>hi<!--x--></r>".as_bytes()), &mut at).unwrap();
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
      type Error = Infallible;
      fn start_element(&mut self, event: StartElementEvent<'_>) -> Result<(), Infallible> {
        self.names.push(event.pool.resolve(event.name.local()).to_owned());
        self.done = true;
        Ok(())
      }
      fn should_continue(&self) -> bool {
        !self.done
      }
    }
    let mut first = First::default();
    parse(&mut Reader::new("<a><b/><c/></a>".as_bytes()), &mut first).unwrap();
    assert_eq!(first.names, ["a"], "only the first start element is seen");
  }

  #[test]
  fn a_handler_error_aborts_the_run() {
    #[derive(Debug, PartialEq)]
    struct NotAllowed(String);
    impl std::fmt::Display for NotAllowed {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "element {} is not allowed", self.0)
      }
    }
    impl std::error::Error for NotAllowed {}

    struct Reject;
    impl Handler for Reject {
      type Error = NotAllowed;
      fn start_element(&mut self, event: StartElementEvent<'_>) -> Result<(), NotAllowed> {
        Err(NotAllowed(event.pool.resolve(event.name.local()).to_owned()))
      }
    }
    let error = parse(&mut Reader::new("<a/>".as_bytes()), &mut Reject).unwrap_err();
    assert!(matches!(error, Error::SaxHandler { .. }));
    assert_eq!(error.to_string(), "SAX handler error: element a is not allowed");
    // The application's own error is preserved as the source, so a caller can downcast to recover it.
    let source = std::error::Error::source(&error).expect("the app error is kept as the source");
    assert_eq!(source.downcast_ref::<NotAllowed>().expect("downcasts to NotAllowed").0, "a");
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
      type Error = Infallible;
      fn doctype(&mut self, event: DoctypeEvent<'_>) -> Result<(), Infallible> {
        self.notation = event.pool.get("gif").is_some_and(|id| event.dtd.has_notation(id));
        self.unparsed = event
          .pool
          .get("logo")
          .is_some_and(|id| matches!(event.dtd.general_entity(id), Some(crate::dtd::GeneralEntity::Unparsed { .. })));
        self.at = Some((event.location.line, event.location.column));
        Ok(())
      }
    }
    let doc = "<!DOCTYPE doc [\
      <!NOTATION gif PUBLIC '-//x//NOTATION gif//EN'>\
      <!ENTITY logo SYSTEM 'urn:logo' NDATA gif>\
    ]><doc/>";
    let mut seen = Seen::default();
    parse(&mut Reader::new(doc.as_bytes()), &mut seen).unwrap();
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
      type Error = Infallible;
      fn processing_instruction(&mut self, e: ProcessingInstructionEvent<'_>) -> Result<(), Infallible> {
        self.target = e.target.to_owned();
        self.data = e.data.to_owned();
        self.data_at = Some((e.data_location.line, e.data_location.column, e.data_location.offset));
        Ok(())
      }
    }
    let mut pi = Pi::default();
    parse(&mut Reader::new("<r><?php\n  echo 1; ?></r>".as_bytes()), &mut pi).unwrap();
    assert_eq!(pi.target, "php");
    assert_eq!(pi.data, "echo 1; ");
    // `<?php` is on line 1; the separator's newline puts `data` on line 2, column 3, character offset 11.
    assert_eq!(pi.data_at, Some((2, 3, 11)));
  }
}
