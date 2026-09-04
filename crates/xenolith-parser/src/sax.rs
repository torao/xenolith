//! A push interface over the sans-I/O pull parser: SAX-style event callbacks.
//!
//! The parser library uses a pull API: you request the next event. Some code reads more naturally the other way around,
//! with the parser calling the user's code. [`emit`](EventSource::emit) does that, running a [`Reader`] to the end and
//! calling a [`Handler`] for each event, in the shape of SAX's `ContentHandler`. Each callback is handed a small view
//! of its event, a [`StartElementEvent`], a [`CharactersEvent`], and so on, holding just that event's data: text and
//! names borrowed from the parser, and the [`Location`] for diagnostics.
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
//! | `EntityResolver` | [`UriResolver`](crate::resolve::UriResolver), given to a reader with [`with_resolver`](Reader::with_resolver) |
//! | `ErrorHandler` | the [`Result`] from [`emit`](EventSource::emit) carries parser errors; an application problem is held by the handler itself |
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
//! time; the parser reads the whole DTD into a [`Dtd`] and hands it to the [`doctype`](Handler::doctype) callback on
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
//!     // DTDHandler.unparsedEntityDecl: an NDATA entity that names a notation.
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
use xenolith_core::error::{Location, Result};
use xenolith_core::name::{NamePool, QName};

use crate::config::ParserConfig;
use crate::dtd::Dtd;
use crate::parser::{EventRef, XmlSpace};
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
  /// The base URI in effect at this element (XML Base), resolved from `xml:base` and the document's system identifier,
  /// if either is known.
  pub base_uri: Option<&'a str>,
  /// The name pool, for resolving `name` and the attribute names to strings.
  pub pool: &'a NamePool,
  /// The source position where this event begins, for diagnostics.
  pub location: Location,
}

impl<'a> StartElementEvent<'a> {
  /// Builds a start element event, for a source other than the parser that drives a [`Handler`], for example a tree
  /// walk. A source with no `xml:space`, `xml:lang`, or base URI scope passes [`XmlSpace::default`] and `None`.
  ///
  #[must_use]
  pub fn new(
    name: QName,
    attributes: Attributes<'a>,
    xml_space: XmlSpace,
    xml_lang: Option<&'a str>,
    base_uri: Option<&'a str>,
    pool: &'a NamePool,
    location: Location,
  ) -> Self {
    Self { name, attributes, xml_space, xml_lang, base_uri, pool, location }
  }
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

impl<'a> EndElementEvent<'a> {
  /// Builds an end element event, for a source other than the parser that drives a [`Handler`].
  ///
  #[must_use]
  pub fn new(name: QName, pool: &'a NamePool, location: Location) -> Self {
    Self { name, pool, location }
  }
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

impl<'a> CharactersEvent<'a> {
  /// Builds a character data event, for a source other than the parser that drives a [`Handler`].
  ///
  #[must_use]
  pub fn new(text: &'a str, location: Location) -> Self {
    Self { text, location }
  }
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

impl<'a> CdataEvent<'a> {
  /// Builds a CDATA section event, for a source other than the parser that drives a [`Handler`].
  ///
  #[must_use]
  pub fn new(text: &'a str, location: Location) -> Self {
    Self { text, location }
  }
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

impl<'a> CommentEvent<'a> {
  /// Builds a comment event, for a source other than the parser that drives a [`Handler`].
  ///
  #[must_use]
  pub fn new(text: &'a str, location: Location) -> Self {
    Self { text, location }
  }
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

impl<'a> ProcessingInstructionEvent<'a> {
  /// Builds a processing instruction event, for a source other than the parser that drives a [`Handler`]. A source
  /// that tracks no separate anchor for `data` passes `location` again as `data_location`.
  ///
  #[must_use]
  pub fn new(target: &'a str, data: &'a str, data_location: Location, location: Location) -> Self {
    Self { target, data, data_location, location }
  }
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

/// Receives events from an even source. Every method has a default, so a handler overrides only what it cares about.
///
/// Each callback receives a borrowed `*Event` view of its event, valid only for that call, and returns nothing. A
/// handler that needs to report a result, or an application problem it detects, keeps it in its own fields. The caller
/// owns the handler and reads it back after [`emit`](EventSource::emit) returns. After each event
/// [`emit`](EventSource::emit) calls [`should_continue`](Self::should_continue); returning `false` stops the run early,
/// for a handler that has read all it needs or has recorded a problem to stop on. [`emit`](EventSource::emit) never
/// calls back for the XML declaration, which a SAX handler models nothing from.
///
pub trait Handler {
  /// A source calls this before any other event.
  ///
  fn start_document(&mut self) {}

  /// A source calls this after the last event, with the document read to its end. It does not call this when a
  /// handler has stopped the run early through [`should_continue`](Self::should_continue).
  ///
  fn end_document(&mut self) {}

  /// A source calls this at the start of an element.
  ///
  fn start_element(&mut self, event: StartElementEvent<'_>) {
    let _ = event;
  }

  /// A source calls this at the end of an element, the implied end of an empty one included.
  ///
  fn end_element(&mut self, event: EndElementEvent<'_>) {
    let _ = event;
  }

  /// A source calls this for a run of character data. One run may arrive as several calls, so a handler that wants a
  /// maximal run coalesces them.
  ///
  fn characters(&mut self, event: CharactersEvent<'_>) {
    let _ = event;
  }

  /// A source calls this for a CDATA section's content, which it keeps separate from ordinary character data.
  ///
  fn cdata(&mut self, event: CdataEvent<'_>) {
    let _ = event;
  }

  /// A source calls this for a comment.
  ///
  fn comment(&mut self, event: CommentEvent<'_>) {
    let _ = event;
  }

  /// A source calls this for a processing instruction.
  ///
  fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) {
    let _ = event;
  }

  /// A source calls this for the document type declaration.
  ///
  fn doctype(&mut self, event: DoctypeEvent<'_>) {
    let _ = event;
  }

  /// A source calls this after each event and stops the run early if it returns `false`, before the document ends.
  /// The default keeps going.
  ///
  fn should_continue(&self) -> bool {
    true
  }
}

/// An abstract source of parser events that generates and dispatch them into a [`Handler`].
///
/// Implementations range from types that act as a [`Reader`] to consume input, to types that traverse a tree
/// constructed by another crate on the fly. A caller runs any of them through the same handler. This is the equivalent
/// of Java's `javax.xml.transform.Source`.
///
/// This is the synchronous form. An async reader emits with `async fn`, which this trait cannot express, so it has its
/// own source type.
///
pub trait EventSource {
  /// Emits every event from this source to `handler` in the order they appear in the document.
  ///
  /// It calls [`start_document`](Handler::start_document) first, an event callback for each event, and
  /// [`end_document`](Handler::end_document) at the end. The handler can stop execution early via
  /// [`should_continue`](Handler::should_continue); in this case, no further calls are made, and the method terminates
  /// successfully.
  ///
  /// This takes a single handler. To feed more than one in a single pass, for example, an application handler beside a
  /// validator, use [`broadcast`](Self::broadcast).
  ///
  /// # Errors
  ///
  /// Returns the parser's error if the document is not well-formed or reading fails. A handler has no error channel;
  /// an application problem is read back from the handler after this returns.
  ///
  fn emit<H: Handler + ?Sized>(&mut self, handler: &mut H) -> Result<()>;

  /// Broadcasts this source's events to several handlers in one pass.
  ///
  /// Add the handlers to the returned [`Broadcast`], then [`run`](Broadcast::run) it. It wires the handlers together for
  /// you, so it is the fluent way to drive several handlers without validation. For a single handler,
  /// [`emit`](Self::emit) is more direct.
  ///
  #[must_use]
  fn broadcast<'h>(self) -> Broadcast<'h, Self>
  where
    Self: Sized,
  {
    Broadcast::new(self)
  }

  /// The parser configuration this source drives with, if it has one.
  ///
  /// A [`Reader`] reports its parser's [`ParserConfig`], so a consumer can pick up defaults such as
  /// [`xml_id`](ParserConfig::xml_id). A source with no parser, for example a tree walk, returns `None`.
  ///
  fn parser_config(&self) -> Option<&ParserConfig> {
    None
  }
}

impl<R: Read> EventSource for Reader<R> {
  fn parser_config(&self) -> Option<&ParserConfig> {
    Some(self.parser().config())
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
          // `base` is a local so the event can borrow the resolved base URI for this one call.
          let base = parser.base_uri();
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
        _ => {}
      }
      if !handler.should_continue() {
        // A handler that has what it needs stops here; the document was not read in full, so no end_document.
        return Ok(());
      }
    }
  }
}

/// A [`Handler`] that forwards each event to every handler it holds, in order. The broadcasting sink behind
/// [`Broadcast`]: a run continues only while every handler's [`should_continue`](Handler::should_continue) is `true`.
struct Dispatch<'h> {
  handlers: Vec<&'h mut dyn Handler>,
}

impl<'h> Dispatch<'h> {
  fn new() -> Self {
    Self { handlers: Vec::new() }
  }

  /// Adds a handler. It receives each event after the handlers already added.
  fn add(&mut self, handler: &'h mut dyn Handler) -> &mut Self {
    self.handlers.push(handler);
    self
  }
}

impl Handler for Dispatch<'_> {
  fn start_document(&mut self) {
    for handler in &mut self.handlers {
      handler.start_document();
    }
  }

  fn end_document(&mut self) {
    for handler in &mut self.handlers {
      handler.end_document();
    }
  }

  fn start_element(&mut self, event: StartElementEvent<'_>) {
    for handler in &mut self.handlers {
      handler.start_element(event.clone());
    }
  }

  fn end_element(&mut self, event: EndElementEvent<'_>) {
    for handler in &mut self.handlers {
      handler.end_element(event.clone());
    }
  }

  fn characters(&mut self, event: CharactersEvent<'_>) {
    for handler in &mut self.handlers {
      handler.characters(event.clone());
    }
  }

  fn cdata(&mut self, event: CdataEvent<'_>) {
    for handler in &mut self.handlers {
      handler.cdata(event.clone());
    }
  }

  fn comment(&mut self, event: CommentEvent<'_>) {
    for handler in &mut self.handlers {
      handler.comment(event.clone());
    }
  }

  fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) {
    for handler in &mut self.handlers {
      handler.processing_instruction(event.clone());
    }
  }

  fn doctype(&mut self, event: DoctypeEvent<'_>) {
    for handler in &mut self.handlers {
      handler.doctype(event.clone());
    }
  }

  fn should_continue(&self) -> bool {
    self.handlers.iter().all(|handler| handler.should_continue())
  }
}

/// A run of several handlers over a source, built up before the run.
///
/// Start it with [`EventSource::broadcast`], add handlers with [`with_handler`](Self::with_handler), then
/// [`run`](Self::run). Every event goes to each handler in the order they were added. A handler keeps its own results,
/// and any problem it detects, in its own fields; the caller owns the handlers and reads them back after the run.
///
/// # Examples
///
/// ```
/// use xenolith_parser::Reader;
/// use xenolith_parser::sax::{CharactersEvent, EventSource, Handler, StartElementEvent};
///
/// #[derive(Default)]
/// struct Elements(usize);
/// impl Handler for Elements {
///   fn start_element(&mut self, _event: StartElementEvent<'_>) {
///     self.0 += 1;
///   }
/// }
///
/// #[derive(Default)]
/// struct TextLength(usize);
/// impl Handler for TextLength {
///   fn characters(&mut self, event: CharactersEvent<'_>) {
///     self.0 += event.text.len();
///   }
/// }
///
/// let mut elements = Elements::default();
/// let mut text_length = TextLength::default();
/// Reader::new("<a>hi<b/></a>".as_bytes())
///     .broadcast()
///     .with_handler(&mut elements)
///     .with_handler(&mut text_length)
///     .run()?;
/// assert_eq!(elements.0, 2);
/// assert_eq!(text_length.0, 2);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
pub struct Broadcast<'h, S: EventSource> {
  source: S,
  handlers: Vec<&'h mut dyn Handler>,
}

impl<S: EventSource> std::fmt::Debug for Broadcast<'_, S> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Broadcast").field("handlers", &self.handlers.len()).finish_non_exhaustive()
  }
}

impl<'h, S: EventSource> Broadcast<'h, S> {
  /// Starts a run over `source` with no handlers yet.
  fn new(source: S) -> Self {
    Self { source, handlers: Vec::new() }
  }

  /// Adds a handler. It receives each event after the handlers already added.
  #[must_use]
  pub fn with_handler(mut self, handler: &'h mut dyn Handler) -> Self {
    self.handlers.push(handler);
    self
  }

  /// Adds handlers, in order.
  #[must_use]
  pub fn with_handlers(mut self, handlers: impl IntoIterator<Item = &'h mut dyn Handler>) -> Self {
    self.handlers.extend(handlers);
    self
  }

  /// Drives the source once, broadcasting each event to every handler.
  ///
  /// # Errors
  ///
  /// Returns the source's error if the input is not well-formed or reading fails.
  pub fn run(mut self) -> Result<()> {
    let mut dispatch = Dispatch::new();
    for handler in self.handlers.drain(..) {
      dispatch.add(handler);
    }
    self.source.emit(&mut dispatch)
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
  fn dispatch_forwards_each_event_to_every_handler_in_order() {
    let mut first = Trace::default();
    let mut second = Trace::default();
    let mut dispatch = Dispatch::new();
    dispatch.add(&mut first);
    dispatch.add(&mut second);
    Reader::new("<a>hi</a>".as_bytes()).emit(&mut dispatch).unwrap();
    drop(dispatch);
    let expected = ["start", "<a>", "t:hi", "</a>", "end"];
    assert_eq!(first.0, expected);
    assert_eq!(second.0, expected, "both handlers see the same stream");
  }

  #[test]
  fn dispatch_stops_when_any_handler_stops() {
    // One handler asks to stop after the first start element; the run ends for both.
    #[derive(Default)]
    struct StopEarly {
      seen: usize,
    }
    impl Handler for StopEarly {
      fn start_element(&mut self, _event: StartElementEvent<'_>) {
        self.seen += 1;
      }
      fn should_continue(&self) -> bool {
        self.seen == 0
      }
    }

    let mut counter = StopEarly::default();
    let mut trace = Trace::default();
    let mut dispatch = Dispatch::new();
    dispatch.add(&mut counter);
    dispatch.add(&mut trace);
    Reader::new("<a><b/><c/></a>".as_bytes()).emit(&mut dispatch).unwrap();
    drop(dispatch);
    assert_eq!(counter.seen, 1);
    // The stop takes effect after the first start element, so the trace ends there, with no end_document.
    assert_eq!(trace.0, ["start", "<a>"]);
  }

  #[test]
  fn dispatch_stops_when_a_handler_records_a_problem() {
    // A handler in the dispatch records an application problem and stops; the run ends for every handler, and the
    // problem is read back afterward.
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
    let mut trace = Trace::default();
    let mut dispatch = Dispatch::new();
    dispatch.add(&mut reject);
    dispatch.add(&mut trace);
    Reader::new("<a><b/></a>".as_bytes()).emit(&mut dispatch).unwrap();
    drop(dispatch);
    assert_eq!(reject.rejected.as_deref(), Some("a"));
    // The run stopped after the first start element, so the trace ends there.
    assert_eq!(trace.0, ["start", "<a>"]);
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
