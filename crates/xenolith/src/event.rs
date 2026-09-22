//! The vocabulary related to events.
//!
//! A push-style [`EventSource`] generates document [`EventRef`]s, which are received by an [`EventHandler`]. An
//! [`EventCursor`] additionally supports pull-style operations, allowing retrieval of the next [`EventRef`] in the
//! event sequence.
//!
//! Operations such as reading, tree traversal, and writing all generate or consume data based on the same event
//! sequence. This module defines that sequence and enables interoperability through a unified interface. An
//! [`EventSource`] is where handlers are registered, while an [`EventHandler`] is responsible for receiving individual
//! events. These roles are not tied to specific implementations; for instance, an input parser or tree traversal
//! process might act as a source, while a validator, tree builder, or writer might act as a "handler." A source that
//! holds the entire document can also function as an [`EventCursor`], driving the execution of the process. Concrete
//! implementations of sources and handlers reside in the modules that provide them.
//!
//! Handlers return a [`Result`] for each event. Returning an [`Err`] rejects the document, propagating the error to
//! the caller via the source. Conversely, returning `false` from [`should_continue`](EventHandler::should_continue)
//! terminates processing early without an error.
//!
//! [`Dispatch`] forwards a single event to multiple handlers. This allows application-level handlers and validators to
//! receive the same event without requiring the source to be read twice.
//!
//! An [`EventRef`] borrows the source for the single callback that receives it, incurring no overhead. In contrast, an
//! [`Event`] holds the data itself and is used when the data must be retained after the callback completes. These two
//! types are mutually convertible.
//!

pub mod strict;
pub mod validate;

#[cfg(test)]
mod test;

use std::sync::Arc;

use crate::attr::{Attribute, Attributes};
use crate::dtd::model::Dtd;
use crate::error::{Error, Location, Result};
use crate::name::{self, NamePool};

/// The effective handling of `xml:space` is determined by the nearest element within the scope that sets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XmlSpace {
  /// There is no `xml:space` within the scope, or the nearest one specifies `default`.
  #[default]
  Default,
  /// `xml:space="preserve"` exists within the scope.
  Preserve,
}

/// A start element and its surrounding scope, borrowed from the source.
///
/// The element name is passed as its constituent parts (namespace and local name), each borrowed from the source.
/// Regardless of the prefix used in the source text, two elements are considered to be of the same element type if
/// their [`namespace`](Self::namespace) and [`local`](Self::local) match; [`lexical`](Self::lexical), on the other
/// hand, represents the form in which the element was actually written.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StartElementEventRef<'a> {
  /// The prefix used to describe the element (`None` if written without a prefix).
  pub prefix: Option<&'a str>,

  /// The local part of the element name.
  pub local: &'a str,

  /// The namespace to which the element belongs (if written without a prefix and a default namespace is in scope, that
  /// default namespace).
  pub namespace: Option<&'a str>,

  /// The attributes, including namespace declarations, in the order of their appearance in the document.
  pub attributes: Attributes<'a>,

  /// The `xml:space` value in effect for this element.
  pub xml_space: XmlSpace,

  /// The `xml:lang` value in effect for this element (if present).
  pub xml_lang: Option<&'a str>,

  /// The base URI (XML Base) in effect for this element; resolved from `xml:base` and the document's system identifier
  /// (if either is known).
  pub base_uri: Option<&'a str>,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> StartElementEventRef<'a> {
  /// Construct a start-element event for sources that drive an [`EventHandler`], such as parsers or tree walkers. For
  /// sources that do not maintain scope for `xml:space`, `xml:lang`, or the base URI, pass [`XmlSpace::default`] and
  /// `None`.
  #[allow(clippy::too_many_arguments)]
  #[must_use]
  pub fn new(
    prefix: Option<&'a str>,
    local: &'a str,
    namespace: Option<&'a str>,
    attributes: Attributes<'a>,
    xml_space: XmlSpace,
    xml_lang: Option<&'a str>,
    base_uri: Option<&'a str>,
    location: Location,
  ) -> Self {
    Self { prefix, local, namespace, attributes, xml_space, xml_lang, base_uri, location }
  }

  /// The name exactly as written (`prefix:local`, or `local` if there is no prefix).
  ///
  #[must_use]
  pub fn lexical(&self) -> String {
    name::lexical(self.prefix, self.local)
  }
}

/// This is the end element obtained from the source within a callback. As with start elements, the name is passed
/// broken down into its constituent parts.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EndElementEventRef<'a> {
  /// The prefix representing the element, or `None` if written without a prefix.
  pub prefix: Option<&'a str>,

  /// The local part of the element name.
  pub local: &'a str,

  /// The namespace to which the element belongs.
  pub namespace: Option<&'a str>,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> EndElementEventRef<'a> {
  /// Constructs an end-element event for a source that drives an [`EventHandler`].
  #[must_use]
  pub fn new(prefix: Option<&'a str>, local: &'a str, namespace: Option<&'a str>, location: Location) -> Self {
    Self { prefix, local, namespace, location }
  }

  /// The name as specified (`prefix:local`, or `local` if there is no prefix).
  #[must_use]
  pub fn lexical(&self) -> String {
    name::lexical(self.prefix, self.local)
  }
}

/// A sequence of character data with references expanded.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CharactersEventRef<'a> {
  /// The text.
  pub text: &'a str,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> CharactersEventRef<'a> {
  /// Constructs a character data event, for a source that drives an [`EventHandler`].
  #[must_use]
  pub fn new(text: &'a str, location: Location) -> Self {
    Self { text, location }
  }
}

/// A CDATA section's content is reported separately from ordinary character data.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CdataEventRef<'a> {
  /// All character strings that enclosed between `<![CDATA[` and `]]>`. Since no reference expansion or trimming is
  /// performed, `<![CDATA[ a<b ]]>`, for example, becomes ` a<b `.
  pub text: &'a str,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> CdataEventRef<'a> {
  /// Constructs a CDATA section event, for a source that drives an [`EventHandler`].
  #[must_use]
  pub fn new(text: &'a str, location: Location) -> Self {
    Self { text, location }
  }
}

/// Comment excluding delimiters.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommentEventRef<'a> {
  /// All characters enclosed between `<!--` and `-->`. Therefore, in the case of `<!-- note -->`, the result is
  /// ` note `, including the spaces before and after.
  pub text: &'a str,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> CommentEventRef<'a> {
  /// Constructs a comment event for a source that drives an [`EventHandler`].
  #[must_use]
  pub fn new(text: &'a str, location: Location) -> Self {
    Self { text, location }
  }
}

/// A processing instruction.
///
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProcessingInstructionEventRef<'a> {
  /// The PI name immediately following `<?`. It spans from that point to the first whitespace character or, if no data
  /// exists, to `?>`.
  pub target: &'a str,

  /// The entire string following the target and the immediately succeeding delimiter whitespace, up to `?>`. Only the
  /// sequence of delimiter whitespace is removed; no other parts are trimmed. If the instruction contains only the
  /// target, the data portion is empty. Thus, for `<?php echo 1; ?>`, the target is `php` and the data is `echo 1; `,
  /// preserving the trailing whitespace.
  pub data: &'a str,

  /// The starting position of the `data` within the source. While `location` indicates the position of `<?`, this
  /// serves as a reference point (anchor) because the actual start of the `data` is obscured by the removal of the
  /// delimiter whitespace. The intention is to let a handler — which parses the `data` as a different language — map
  /// positions found within that data back to the original document by adding them to this reference point.
  pub data_location: Location,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> ProcessingInstructionEventRef<'a> {
  /// Construct a processing instruction event. For sources that don't track separate anchors for `data`, pass
  /// `location` again as the `data location`.
  #[must_use]
  pub fn new(target: &'a str, data: &'a str, data_location: Location, location: Location) -> Self {
    Self { target, data, data_location, location }
  }
}

/// A document type declaration includes the declared root element name, the external identifier, and the parsed DTD.
///
/// Since the entire DTD (including the internal subset, external subset, and all included external parameter entities)
/// has been loaded by the time this event occurs, the `dtd` object gains access to notations, unparsed entities, and
/// declarations for elements, attributes, and entities. In doing so, it uses the `pool` to intern (register and reuse)
/// names and perform lookups.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DoctypeEventRef<'a> {
  /// The root element name specified in the declaration (if specified).
  pub name: Option<&'a str>,

  /// The public identifier (if present).
  pub public_id: Option<&'a str>,

  /// The system identifier (if present).
  pub system_id: Option<&'a str>,

  /// The parsed DTD (both internal and external subsets).
  pub dtd: &'a Dtd,

  /// The name pool (used to intern names for querying the DTD and to resolve the IDs it returns).
  pub pool: &'a NamePool,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl<'a> DoctypeEventRef<'a> {
  /// Constructs a document type declaration event for the source driving the [`EventHandler`].
  #[must_use]
  pub fn new(
    name: Option<&'a str>,
    public_id: Option<&'a str>,
    system_id: Option<&'a str>,
    dtd: &'a Dtd,
    pool: &'a NamePool,
    location: Location,
  ) -> Self {
    Self { name, public_id, system_id, dtd, pool, location }
  }
}

/// An event emitted by an [`EventSource`]. The value is borrowed from the source for the duration of the handler
/// method call that receives it.
///
/// [`EventSource`] reports the entire document as a sequence of events. This sequence consists of
/// [`StartDocument`](Event::StartDocument), a series of events corresponding to each markup element in document order,
/// and [`EndDocument`](Event::EndDocument). Since the payload of each event references (borrows) the source buffer,
/// the validity of the event is limited to the duration of the method call; handlers requiring the data beyond that
/// scope must make their own copies of the necessary parts.
///
/// The XML declaration `<?xml version="1.0"?>` is not part of the document content, so it is not included in the
/// events.
#[derive(Clone, Debug)]
pub enum EventRef<'a> {
  /// The document reading is about to begin. The event source MUST notify this event before any other event.
  /// The event source must notify this event before any other event, and a single handler instance may initialize its
  /// internal state when it receives this event to be able to reuse across multiple parsing session.
  StartDocument,

  /// The document was read to the end. A source that stopped midway due to a handler request does not report this
  /// event.
  EndDocument,

  /// The start of an element, or an entire empty element.
  StartElement(StartElementEventRef<'a>),

  /// The end of an element, including the implicit end of an empty element.
  EndElement(EndElementEventRef<'a>),

  /// A sequence of character data. A single contiguous block of data (a "run") may be transmitted as multiple adjacent
  /// events; therefore, handlers that wish to treat the string as a complete unit must concatenate them.
  Characters(CharactersEventRef<'a>),

  /// The content of a CDATA section, reported separately from ordinary character data.
  Cdata(CdataEventRef<'a>),

  /// A comment excluding delimiters.
  Comment(CommentEventRef<'a>),

  /// A processing instruction.
  ProcessingInstruction(ProcessingInstructionEventRef<'a>),

  /// The document type declaration and the DTD parsed based on it.
  Doctype(DoctypeEventRef<'a>),
}

impl EventRef<'_> {
  /// The location of this event within the source document. It is `None` for the start and end of the document itself
  /// (where no position is specified).
  #[must_use]
  pub fn location(&self) -> Option<&Location> {
    match self {
      Self::StartDocument | Self::EndDocument => None,
      Self::StartElement(event) => Some(&event.location),
      Self::EndElement(event) => Some(&event.location),
      Self::Characters(event) => Some(&event.location),
      Self::Cdata(event) => Some(&event.location),
      Self::Comment(event) => Some(&event.location),
      Self::ProcessingInstruction(event) => Some(&event.location),
      Self::Doctype(event) => Some(&event.location),
    }
  }
}

/// This event holds its own data, decoupled from the source that emitted it.
///
/// An [`EventRef`] borrows values from the source only for the duration of a specific callback. While this borrowing
/// incurs no cost, the reference becomes invalid once the callback finishes. In contrast, an `Event` owns a copy of
/// the event data, so you can retain it, store it in a collection, or pass it elsewhere even after the source has
/// moved on to subsequent processing. You can create an `Event` from an [`EventRef`] using `Event::from`, and treat
/// it as an [`EventRef`] again using [`as_event_ref`](Self::as_event_ref). This lets you pass a previously stored
/// event to any [`EventHandler`] as needed.
///
/// As with the borrowed form, each event type is defined independently. Each event type can be stored or collected
/// separately from others, and—just like the event system as a whole—conversions can be performed for each specific
/// type.
///
/// # Examples
///
/// ```
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::event::{Event, EventCursor, EventHandler};
/// use xenolith::io::StreamSource;
///
/// let mut source = StreamSource::new("<a x='1'>hi</a>".as_bytes());
/// let mut kept = Vec::new();
/// while let Some(event) = source.next()? {
///   kept.push(Event::from(&event)); // the borrow ends here, and the copy stays
/// }
/// assert!(matches!(&kept[1], Event::StartElement(start) if start.local == "a" && start.attributes.len() == 1));
///
/// // A kept event is lent back in the borrowed form every handler reads.
/// let mut builder = DomBuilder::new();
/// for event in &kept {
///   builder.handle(&event.as_event_ref())?;
/// }
/// let doc = builder.into_document();
/// assert_eq!(doc.text_content(doc.document_element().unwrap()), "hi");
/// # Ok::<(), xenolith::Error>(())
/// ```
#[derive(Clone, Debug)]
pub enum Event {
  /// The document reading is about to begin.
  StartDocument,

  /// The document was read to the end.
  EndDocument,

  /// The start of an element, or an entire empty element.
  StartElement(StartElementEvent),

  /// The end of an element, including the implicit end of an empty element.
  EndElement(EndElementEvent),

  /// A run of character data, with references expanded.
  Characters(CharactersEvent),

  /// The content of a CDATA section, reported apart from ordinary character data.
  Cdata(CdataEvent),

  /// A comment, without its delimiters.
  Comment(CommentEvent),

  /// A processing instruction.
  ProcessingInstruction(ProcessingInstructionEvent),

  /// The document type declaration and the DTD read from it.
  Doctype(DoctypeEvent),
}

impl Event {
  /// Provided in a form that each [`EventHandler`] can reference (borrow).
  ///
  /// Since the provided event is borrowed from `self`, the content held by `self` — including attributes and the DTD
  /// — is read.
  #[must_use]
  pub fn as_event_ref(&self) -> EventRef<'_> {
    match self {
      Self::StartDocument => EventRef::StartDocument,
      Self::EndDocument => EventRef::EndDocument,
      Self::StartElement(event) => EventRef::StartElement(event.as_event_ref()),
      Self::EndElement(event) => EventRef::EndElement(event.as_event_ref()),
      Self::Characters(event) => EventRef::Characters(event.as_event_ref()),
      Self::Cdata(event) => EventRef::Cdata(event.as_event_ref()),
      Self::Comment(event) => EventRef::Comment(event.as_event_ref()),
      Self::ProcessingInstruction(event) => EventRef::ProcessingInstruction(event.as_event_ref()),
      Self::Doctype(event) => EventRef::Doctype(event.as_event_ref()),
    }
  }
}

/// Copy borrowed events from their source, grouped by type.
impl From<&EventRef<'_>> for Event {
  fn from(event: &EventRef<'_>) -> Self {
    match event {
      EventRef::StartDocument => Self::StartDocument,
      EventRef::EndDocument => Self::EndDocument,
      EventRef::StartElement(event) => Self::StartElement(event.into()),
      EventRef::EndElement(event) => Self::EndElement(event.into()),
      EventRef::Characters(event) => Self::Characters(event.into()),
      EventRef::Cdata(event) => Self::Cdata(event.into()),
      EventRef::Comment(event) => Self::Comment(event.into()),
      EventRef::ProcessingInstruction(event) => Self::ProcessingInstruction(event.into()),
      EventRef::Doctype(event) => Self::Doctype(event.into()),
    }
  }
}

/// A start element that holds a value itself.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StartElementEvent {
  /// The prefix the element was written with, or `None` when it was written without one.
  pub prefix: Option<String>,

  /// The local part of the element's name.
  pub local: String,

  /// The namespace the element is in, which is the default namespace when it was written with no prefix and one is in
  /// scope.
  pub namespace: Option<String>,

  /// The attributes, in document order, namespace declarations included.
  pub attributes: Vec<Attribute>,

  /// The `xml:space` in effect inside this element.
  pub xml_space: XmlSpace,

  /// The `xml:lang` in effect inside this element, if any.
  pub xml_lang: Option<String>,

  /// The base URI in effect at this element (XML Base), if one is known.
  pub base_uri: Option<String>,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl StartElementEvent {
  /// Construct a start element event that holds its own data.
  #[allow(clippy::too_many_arguments)]
  #[must_use]
  pub fn new(
    prefix: Option<String>,
    local: String,
    namespace: Option<String>,
    attributes: Vec<Attribute>,
    xml_space: XmlSpace,
    xml_lang: Option<String>,
    base_uri: Option<String>,
    location: Location,
  ) -> Self {
    Self { prefix, local, namespace, attributes, xml_space, xml_lang, base_uri, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> StartElementEventRef<'_> {
    StartElementEventRef::new(
      self.prefix.as_deref(),
      &self.local,
      self.namespace.as_deref(),
      Attributes::new(&self.attributes),
      self.xml_space,
      self.xml_lang.as_deref(),
      self.base_uri.as_deref(),
      self.location.clone(),
    )
  }
}

impl From<&StartElementEventRef<'_>> for StartElementEvent {
  fn from(event: &StartElementEventRef<'_>) -> Self {
    Self::new(
      event.prefix.map(ToOwned::to_owned),
      event.local.to_owned(),
      event.namespace.map(ToOwned::to_owned),
      event.attributes.iter().map(Attribute::from).collect(),
      event.xml_space,
      event.xml_lang.map(ToOwned::to_owned),
      event.base_uri.map(ToOwned::to_owned),
      event.location.clone(),
    )
  }
}

/// An end element that holds a value itself.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EndElementEvent {
  /// The prefix the element was written with, or `None` when it was written without one.
  pub prefix: Option<String>,

  /// The local part of the element's name.
  pub local: String,

  /// The namespace the element is in.
  pub namespace: Option<String>,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl EndElementEvent {
  /// Construct an end element event that holds its own data.
  #[must_use]
  pub fn new(prefix: Option<String>, local: String, namespace: Option<String>, location: Location) -> Self {
    Self { prefix, local, namespace, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> EndElementEventRef<'_> {
    EndElementEventRef::new(self.prefix.as_deref(), &self.local, self.namespace.as_deref(), self.location.clone())
  }
}

impl From<&EndElementEventRef<'_>> for EndElementEvent {
  fn from(event: &EndElementEventRef<'_>) -> Self {
    Self::new(
      event.prefix.map(ToOwned::to_owned),
      event.local.to_owned(),
      event.namespace.map(ToOwned::to_owned),
      event.location.clone(),
    )
  }
}

/// A sequence of character data produced by expanding a reference as a character string.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CharactersEvent {
  /// The text.
  pub text: String,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl CharactersEvent {
  /// Construct a character data event that holds the text itself.
  #[must_use]
  pub fn new(text: String, location: Location) -> Self {
    Self { text, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> CharactersEventRef<'_> {
    CharactersEventRef::new(&self.text, self.location.clone())
  }
}

impl From<&CharactersEventRef<'_>> for CharactersEvent {
  fn from(event: &CharactersEventRef<'_>) -> Self {
    Self::new(event.text.to_owned(), event.location.clone())
  }
}

/// The content of a CDATA section.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CdataEvent {
  /// The section's text: everything between `<![CDATA[` and `]]>`, with no reference expanded and nothing trimmed.
  pub text: String,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl CdataEvent {
  /// Construct a CDATA section event that holds the text itself.
  #[must_use]
  pub fn new(text: String, location: Location) -> Self {
    Self { text, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> CdataEventRef<'_> {
    CdataEventRef::new(&self.text, self.location.clone())
  }
}

impl From<&CdataEventRef<'_>> for CdataEvent {
  fn from(event: &CdataEventRef<'_>) -> Self {
    Self::new(event.text.to_owned(), event.location.clone())
  }
}

/// A comment without its delimiters.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommentEvent {
  /// The comment's text: everything between `<!--` and `-->`, verbatim.
  pub text: String,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl CommentEvent {
  /// Construct a comment event that holds the text itself.
  #[must_use]
  pub fn new(text: String, location: Location) -> Self {
    Self { text, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> CommentEventRef<'_> {
    CommentEventRef::new(&self.text, self.location.clone())
  }
}

impl From<&CommentEventRef<'_>> for CommentEvent {
  fn from(event: &CommentEventRef<'_>) -> Self {
    Self::new(event.text.to_owned(), event.location.clone())
  }
}

/// A processing instruction.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProcessingInstructionEvent {
  /// The target: the name right after `<?`.
  pub target: String,

  /// Everything after the target and the whitespace separating it, up to `?>`.
  pub data: String,

  /// Where `data` begins in the source.
  pub data_location: Location,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl ProcessingInstructionEvent {
  /// Construct a processing instruction event that holds its data itself.
  #[must_use]
  pub fn new(target: String, data: String, data_location: Location, location: Location) -> Self {
    Self { target, data, data_location, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> ProcessingInstructionEventRef<'_> {
    ProcessingInstructionEventRef::new(&self.target, &self.data, self.data_location.clone(), self.location.clone())
  }
}

impl From<&ProcessingInstructionEventRef<'_>> for ProcessingInstructionEvent {
  fn from(event: &ProcessingInstructionEventRef<'_>) -> Self {
    Self::new(event.target.to_owned(), event.data.to_owned(), event.data_location.clone(), event.location.clone())
  }
}

/// A document type declaration and the DTD read from it.
///
/// When the event is cloned, it shares the DTD rather than copying it. This is because the pool that interns
/// declarations and names is held via an [`Arc`]. Since this pool is a [`fork`](NamePool::fork) of the original pool,
/// continued interning in the original pool does not affect the cloned pool.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DoctypeEvent {
  /// The root element name the declaration gave, if it gave one.
  pub name: Option<String>,

  /// The public identifier, if any.
  pub public_id: Option<String>,

  /// The system identifier, if any.
  pub system_id: Option<String>,

  /// The parsed DTD, both its internal and external subsets.
  pub dtd: Arc<Dtd>,

  /// The pool the DTD's names are interned in, for interning a name to query `dtd` and resolving the ids it returns.
  pub pool: Arc<NamePool>,

  /// The source location where this event begins, for diagnostic purposes.
  pub location: Location,
}

impl DoctypeEvent {
  /// Construct a document type declaration event that holds the DTD itself.
  #[must_use]
  pub fn new(
    name: Option<String>,
    public_id: Option<String>,
    system_id: Option<String>,
    dtd: Arc<Dtd>,
    pool: Arc<NamePool>,
    location: Location,
  ) -> Self {
    Self { name, public_id, system_id, dtd, pool, location }
  }

  /// Creates an event that borrows from `self`.
  #[must_use]
  pub fn as_event_ref(&self) -> DoctypeEventRef<'_> {
    DoctypeEventRef::new(
      self.name.as_deref(),
      self.public_id.as_deref(),
      self.system_id.as_deref(),
      &self.dtd,
      &self.pool,
      self.location.clone(),
    )
  }
}

/// Since the DTD is replicated once and its pool is forked, the copy is independent of the source.
impl From<&DoctypeEventRef<'_>> for DoctypeEvent {
  fn from(event: &DoctypeEventRef<'_>) -> Self {
    Self::new(
      event.name.map(ToOwned::to_owned),
      event.public_id.map(ToOwned::to_owned),
      event.system_id.map(ToOwned::to_owned),
      Arc::new(event.dtd.clone()),
      Arc::new(event.pool.fork()),
      event.location.clone(),
    )
  }
}

/// Receives the events generated by a source, with a single call for each event.
///
/// A handler processes the sequence of events obtained from parsing a document — whether by building a tree,
/// generating a byte stream, making a decision, or passing the data to a subsequent process. Since handlers receive
/// the same [`EventRef`] sequence regardless of how the events were generated, the same handler can be used both with
/// a parser reading input data and with a process traversing an in-memory tree.
///
/// A handler can stop the processing of an event source in two ways. Returning an [`Err`] signifies that the document
/// has been **refused**, and the error propagates back to the caller via the source. Conversely, returning `false`
/// from [`should_continue`](Self::should_continue) indicates that the handler has obtained all necessary information;
/// this results in **early termination** without an [`EndDocument`](EventRef::EndDocument) event, and the caller
/// reports a successful completion.
///
/// [`finish`](Self::finish) notifies all handlers of the execution of the same [`Outcome`] when the execution
/// concludes. Therefore, even if another handler terminates the execution, each handler can implement
/// [`finish`](Self::finish) to release resources held for document processing.
///
/// Note that the sequence of events notified does not necessarily conform to the structure of a valid XML document.
/// For example, the events might include mismatched tags, multiple root elements, or names that are not valid
/// `QName`s. By placing a validator, such as [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator) or
/// [`ValidatorSet`](crate::event::validate::ValidatorSet), before the handler on the [`EventSource`], you can ensure
/// that the sequence of events received by the handler has passed the corresponding validation.
///
/// # Examples
///
/// ```
/// use xenolith::event::{EventRef, EventHandler};
/// use xenolith::error::Result;
///
/// /// Counts elements, and stops as soon as it has seen enough.
/// #[derive(Default)]
/// struct FirstFew {
///   seen: usize,
/// }
///
/// impl EventHandler for FirstFew {
///   fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
///     if matches!(event, EventRef::StartElement(_)) {
///       self.seen += 1;
///     }
///     Ok(())
///   }
///
///   fn should_continue(&self) -> bool {
///     self.seen < 3
///   }
/// }
/// ```
pub trait EventHandler {
  /// Receives one event.
  ///
  /// A single handler instance can handle different, independent parsing operations. In this case, you must initialize
  /// the internal state within the [`StartDocument`](EventRef::StartDocument) event.
  ///
  /// # Errors
  ///
  /// If the handler refuses to accept the document for any reason — such as a schema violation, a structure the
  /// handler cannot represent, or a failure during writing — the source stops processing and passes the error to the
  /// caller.
  ///
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()>;

  /// Indicates whether the handler requires further events. This check is performed each time an event is passed; once
  /// `false` is returned, no further events will be passed to it during that sequence execution.
  ///
  /// This serves as a way to terminate processing early without an error. A handler that has read all the necessary
  /// information signals this here. When none of the downstream handlers require further events, the source stops, and
  /// processing concludes successfully. The default behavior is for processing to continue.
  ///
  fn should_continue(&self) -> bool {
    true
  }

  /// Receives the final outcome of the run.
  ///
  /// A source calls this method exactly once at the end of each execution. [`Outcome::Completed`] is passed if all
  /// handlers have accepted [`EndDocument`](EventRef::EndDocument). [`Outcome::Stopped`] is passed if all handlers
  /// have terminated early via [`should_continue`](Self::should_continue). If the execution terminates because either
  /// the input or a handler produces an error, [`Outcome::Failed`] is passed along with that error. All handlers
  /// involved in the execution — including those that terminated early or caused an error — are notified of the same
  /// outcome. Handlers holding document - related resources (such as files being written to) can choose here whether
  /// to release them or retain them. The default implementation performs no action.
  ///
  /// If the [`Dispatch`] holding the handler is dropped and execution is interrupted, [`Outcome::Abandoned`] is
  /// signaled. This occurs, for example, if a pull-style event source stops reading before returning `None`. Since
  /// this method is called even during panic unwinding, the handler's `finish` method must not panic. No notification
  /// is sent to handlers that never received a [`StartDocument`](EventRef::StartDocument) event.
  ///
  fn finish(&mut self, outcome: Outcome<'_>) {
    let _ = outcome;
  }
}

/// The completion status of the event source processing reported to [`EventHandler::finish`].
#[derive(Clone, Copy, Debug)]
pub enum Outcome<'a> {
  /// The document has been fully loaded, and all handlers have accepted [`EndDocument`](EventRef::EndDocument).
  Completed,

  /// Processing stopped because a handler requested early termination via
  /// [`should_continue`](EventHandler::should_continue). This is not an error; note that one or more handlers may not
  /// have received [`EndDocument`](EventRef::EndDocument).
  Stopped,

  /// The processing terminated due to an input error or because a handler rejected an event by the error.
  Failed(&'a Error),

  /// Processing was interrupted before the source could report a completion status, either because the event source
  /// itself was dropped during processing or because the dispatch process managing the registered handlers was
  /// interrupted.
  Abandoned,
}

/// An abstract event source that declares the capability to generate document events and pass them to registered
/// handlers.
///
/// This corresponds to Java's `javax.xml.transform.Source`; implementations include parsers that consume input,
/// traversals of pre-built trees, and writers generating XML output. While they share the common feature of event
/// handler registration, they differ in the mechanisms used to drive pull-style and push-style events. A source that
/// owns the entire document can pull events and operate autonomously as an [`EventCursor`]. In contrast, a push-style
/// source — where data is supplied by the caller — lacks such an independent execution process.
///
/// Handlers are **borrowed** rather than owned; the caller retains the handler and reads the results it produced after
/// processing completes. The lifetime `'h` ensures that the source cannot outlive the handler.
///
/// This is a synchronous interface. Since asynchronous readers emit events using `async fn` — a pattern that cannot be
/// expressed by this trait — a separate source type is provided for asynchronous use.
///
pub trait EventSource<'h> {
  /// Installs a handler. The handler receives events after any already installed handlers.
  fn with_handler(self, handler: &'h mut dyn EventHandler) -> Self
  where
    Self: Sized;
}

/// A source that holds the entire document and allows events to be retrieved (pulled) sequentially, one by one.
///
/// The next event can be retrieved using the [`next`](Self::next) method; this also triggers the execution of
/// registered handlers before returning the event. This design integrates "pull" and "push" processing models into
/// [`next`](Self::next) while ensuring that any pulled event has already passed the validation implemented by the
/// handlers.
///
/// Calling [`emit`](Self::emit) instead of [`next`](Self::next) pulls all events and invokes the event handlers until
/// the event sequence concludes or a handler interrupts the process.
///
/// Since the events borrow (reference) data from the source, they are accessible only during the handler invocation.
/// Callers requiring the events beyond this point must make their own copies of the necessary data.
///
pub trait EventCursor<'h>: EventSource<'h> {
  /// Proceeds to the next event, passes it to the registered handler, and returns the result.
  ///
  /// It first reports [`StartDocument`](EventRef::StartDocument), then reports events for each markup element one by
  /// one, and finally reports [`EndDocument`](EventRef::EndDocument). It returns `None` when document processing is
  /// complete or when all handlers have finished processing as indicated by
  /// [`should_continue`](EventHandler::should_continue) (in the latter case, `EndDocument` is not reported).
  ///
  /// It reports [`StartDocument`](EventRef::StartDocument) first, one event for each piece of markup, and
  /// [`EndDocument`](EventRef::EndDocument) at the end. It returns `None` once the document is finished, or once every
  /// handler has finished through [`should_continue`](EventHandler::should_continue), in which case no `EndDocument`
  /// was reported.
  ///
  /// For registered handlers, the [`finish`](EventHandler::finish) method is called only the first time `None` or an
  /// error is returned. Since execution is considered finished after an error occurs, subsequent calls to `next` will
  /// return `None`. If the event source is dropped before that, the handler is notified that execution has been
  /// abandoned.
  ///
  /// # Errors
  ///
  /// Returns a source error if the document is not well-formed or fails to load, or returns the reason for rejection
  /// (an error) if a handler refuses to accept the event.
  ///
  fn next(&mut self) -> Result<Option<EventRef<'_>>>;

  /// Processes the source to completion and passes all generated events to the registered handlers.
  ///
  /// This is equivalent to repeatedly calling [`next`](Self::next) until document processing is complete; however, it
  /// discards the events and only runs the handlers.
  ///
  /// # Errors
  ///
  /// Same as [`next`](Self::next).
  ///
  fn emit(&mut self) -> Result<()> {
    while self.next()?.is_some() {}
    Ok(())
  }

  /// Returns an iterator that yields the remaining events one by one. Each event is copied into an owned [`Event`].
  ///
  /// Since each element is identical to what [`next`](Self::next) returns, registered handlers are invoked as
  /// iteration proceeds. Iteration terminates when `next` returns `None` or immediately after an error is returned.
  /// Because the cursor is only borrowed, it remains usable after the iterator is dropped.
  fn events(&mut self) -> Events<'_, Self>
  where
    Self: Sized,
  {
    Events { cursor: self, done: false }
  }
}

/// An iterator that treats the events of an [`EventCursor`], returned by [`EventCursor::events`], as owned [`Event`]s.
#[derive(Debug)]
pub struct Events<'c, C> {
  /// The cursor the events are pulled from.
  cursor: &'c mut C,

  /// Whether the cursor has reported its end or an error, after which nothing more is pulled.
  done: bool,
}

impl<'h, C: EventCursor<'h>> Iterator for Events<'_, C> {
  type Item = Result<Event>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.done {
      return None;
    }
    match self.cursor.next() {
      Ok(Some(event)) => Some(Ok(Event::from(&event))),
      Ok(None) => {
        self.done = true;
        None
      }
      Err(error) => {
        self.done = true;
        Some(Err(error))
      }
    }
  }
}

impl<'h, C: EventCursor<'h>> std::iter::FusedIterator for Events<'_, C> {}

/// An [`EventHandler`] that forwards each event to several handlers, in the order they were added.
///
/// One source, one pass, several consumers: a validator checking the document while a builder makes a tree of it. It
/// is itself an [`EventHandler`], so it goes wherever one does, nesting included.
///
/// It is **fail-fast**. The first handler to refuse an event stops the forwarding there, and that error is what the
/// dispatch returns, so the handlers after it never see an event one of their predecessors rejected.
///
/// A handler that has read all it needs finishes on its own, and the others are not cut short by it. The dispatch asks
/// each handler for [`should_continue`](EventHandler::should_continue) after handing it an event, and one that answers
/// `false` is given nothing more in that run, while the rest go on to the end. The dispatch itself answers `false`
/// once every handler has finished, which is what lets a source stop early. A dispatch with no handlers never finishes
/// on its own, so a source with nothing installed still reads the whole document and reports what is wrong with it.
///
/// Which handlers have finished belongs to one run, and [`StartDocument`](EventRef::StartDocument) begins a new one:
/// that event reaches every handler again. A dispatch whose handlers reset themselves at it can be handed the next
/// document.
///
/// [`finish`](EventHandler::finish) is invoked for all handlers, including those that finished early or whose
/// execution was terminated due to an error; consequently, all handlers observe the same outcome. If the `Dispatch`
/// instance is dropped during execution (after [`StartDocument`](EventRef::StartDocument) but before the termination
/// of execution is signaled), [`Outcome::Abandoned`] is reported to the registered handlers.
///
/// The handlers are borrowed, not owned, so the caller keeps them and reads back what they made once the run is over.
///
/// # Examples
///
/// ```
/// use xenolith::error::Result;
/// use xenolith::event::{Dispatch, EventRef, EventHandler};
///
/// #[derive(Default)]
/// struct Count(usize);
/// impl EventHandler for Count {
///   fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
///     if matches!(event, EventRef::StartElement(_)) {
///       self.0 += 1;
///     }
///     Ok(())
///   }
/// }
///
/// let mut first = Count::default();
/// let mut second = Count::default();
/// let mut both = Dispatch::new().with_handler(&mut first).with_handler(&mut second);
/// both.handle(&EventRef::StartDocument)?;
/// # Ok::<(), xenolith::Error>(())
/// ```
#[derive(Default)]
pub struct Dispatch<'h> {
  handlers: Vec<&'h mut dyn EventHandler>,
  /// Which handlers have finished the current run, by the index they were added at. Cleared at each `StartDocument`.
  finished: Vec<bool>,
  /// Whether a run has begun and its handlers have not yet been told how it ended.
  running: bool,
}

impl std::fmt::Debug for Dispatch<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Dispatch").field("handlers", &self.handlers.len()).finish_non_exhaustive()
  }
}

impl<'h> Dispatch<'h> {
  /// Creates a dispatch with no handlers, which forwards nothing.
  #[must_use]
  pub fn new() -> Self {
    Self { handlers: Vec::new(), finished: Vec::new(), running: false }
  }

  /// Adds a handler. It receives each event after the handlers already added.
  #[must_use]
  pub fn with_handler(mut self, handler: &'h mut dyn EventHandler) -> Self {
    self.add(handler);
    self
  }

  /// Adds handlers, in order.
  #[must_use]
  pub fn with_handlers(mut self, handlers: impl IntoIterator<Item = &'h mut dyn EventHandler>) -> Self {
    self.handlers.extend(handlers);
    self.finished.resize(self.handlers.len(), false);
    self
  }

  /// Adds a handler to a dispatch already built, for a caller assembling one a piece at a time.
  pub fn add(&mut self, handler: &'h mut dyn EventHandler) -> &mut Self {
    self.handlers.push(handler);
    self.finished.push(false);
    self
  }

  /// How many handlers it forwards to.
  #[must_use]
  pub fn len(&self) -> usize {
    self.handlers.len()
  }

  /// Whether it forwards to none.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.handlers.is_empty()
  }

  /// Ends the run with `error`: tells every handler that the run failed, and hands the error back to be returned.
  pub(crate) fn fail(&mut self, error: Error) -> Error {
    self.finish(Outcome::Failed(&error));
    error
  }
}

impl EventHandler for Dispatch<'_> {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    // A new document is a new run, and every handler takes part in it again.
    if matches!(event, EventRef::StartDocument) {
      self.finished.fill(false);
      self.running = true;
    }
    for (handler, finished) in self.handlers.iter_mut().zip(&mut self.finished) {
      if *finished {
        continue;
      }
      handler.handle(event)?;
      // Asked after the event rather than before it, so a handler that finished the previous run is still handed this
      // run's `StartDocument`, which is where it resets.
      *finished = !handler.should_continue();
    }
    Ok(())
  }

  fn should_continue(&self) -> bool {
    // With no handlers there is no one to finish, and the run goes on for the document's own sake.
    self.handlers.is_empty() || self.finished.contains(&false)
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    if self.running {
      self.running = false;
      // Every handler, finished or not: one that stopped early may still hold something to release.
      for handler in &mut self.handlers {
        handler.finish(outcome);
      }
    }
  }
}

impl Drop for Dispatch<'_> {
  fn drop(&mut self) {
    // Dropped before anyone said how the run ended, so the run was given up part way.
    self.finish(Outcome::Abandoned);
  }
}
