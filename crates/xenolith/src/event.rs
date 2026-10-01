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
//! the caller via the source. Conversely, returning [`Flow::Break`] terminates processing early without an
//! error. Either way of continuing carries the number of validity errors found while the event was handled, so a
//! limit on them can be applied to a whole pipeline.
//!
//! <a name="transform"></a>
//! A **transform** is a component that relays a pipeline while transforming events. It implements both [`EventHandler`]
//! and [`EventSource`]. For example, `XIncludeTransform` replaces `xi:include` elements with the resources they
//! specify, and transforms that adjust indentation or correct namespaces perform a similar function. This role is
//! identical to that played by `org.xml.sax.XMLFilter` in SAX.
//!
//! The concept is a vocabulary rather than a trait, since a bound of `EventSource + EventHandler` says as much. What a
//! transform is expected to uphold is therefore written here: every event it reports, whether it came from upstream or
//! from the transform itself, goes to the handlers it was given through a [`Dispatch`] of its own; an event it has no
//! reason to change is passed on unchanged; what those handlers return (whether to go on, and how many validity errors
//! they found) is returned upstream, and [`finish`](EventHandler::finish) is relayed to them, since the end of the run
//! is theirs as well; and its own state is reset on [`StartDocument`](EventRef::StartDocument), as for any handler used
//! for more than one document. A transform is not an [`EventCursor`]: it pulls nothing, and is driven by whatever is
//! upstream of it.
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

use std::ops::ControlFlow;
use std::sync::Arc;

use crate::attr::{Attribute, Attributes};
use crate::dtd::model::Dtd;
use crate::error::{Error, Location, Result};
use crate::name::{self, NamePool};
use validate::Validator;

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
  ///
  /// This event reports whitespace characters located before or after the root element (specifically, whitespace
  /// appearing between the XML declaration, document type declaration, comments, processing instructions, and the root
  /// element). Since XML does not permit characters other than whitespace in these positions, and because such
  /// whitespace is not considered part of the document's content, it is excluded from the tree constructed from the
  /// event.
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

impl<'a> EventRef<'a> {
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

  /// The start of an element with the given name, or `None` for any other event.
  #[must_use]
  pub fn start_element_named(&self, namespace: Option<&str>, local: &str) -> Option<&StartElementEventRef<'a>> {
    match self {
      EventRef::StartElement(start) if start.namespace == namespace && start.local == local => Some(start),
      _ => None,
    }
  }

  /// The end of an element with the given name, or `None` for any other event.
  #[must_use]
  pub fn end_element_named(&self, namespace: Option<&str>, local: &str) -> Option<&EndElementEventRef<'a>> {
    match self {
      EventRef::EndElement(end) if end.namespace == namespace && end.local == local => Some(end),
      _ => None,
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

  /// References the attribute with the specified name.
  pub fn attribute(&self, namespace: Option<&str>, local: &str) -> Option<&Attribute> {
    self.attributes.iter().find(|attr| attr.namespace.as_deref() == namespace && attr.local == local)
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

/// Return value of [`EventHandler::handle`]: Returns [`Continue`](ControlFlow::Continue) to proceed to the next event,
/// or [`Break`](ControlFlow::Break) if no further processing is required for the current execution. In either case,
/// the return value includes the number of validation errors detected during the validation of that event.
pub type Flow = ControlFlow<usize, usize>;

/// Receives the events generated by a source, with a single call for each event.
///
/// A handler processes the sequence of events obtained from parsing a document — whether by building a tree,
/// generating a byte stream, making a decision, or passing the data to a subsequent process. Since handlers receive
/// the same [`EventRef`] sequence regardless of how the events were generated, the same handler can be used both with
/// a parser reading input data and with a process traversing an in-memory tree.
///
/// A handler can stop the processing of an event source in two ways. Returning [`Err`] signifies that the document has
/// been **rejected**, and the error propagates back to the caller through the source. Conversely, returning
/// [`Flow::Break`] indicates that the handler has obtained all the information it requires. Once no further events
/// are passed during that execution, and assuming no other handlers require additional events, the process
/// **terminates early** without triggering an [`EndDocument`](EventRef::EndDocument) event, reporting successful
/// completion to the caller.
///
/// Both [`Flow::Continue`] and [`Flow::Break`] carry a count of validity errors detected during event processing. This
/// count is tallied either by the handler itself (if it acts as a validator) or by the downstream handler (if it
/// forwards events). Handlers that neither validate nor forward events return `0`. As these counts propagate upstream,
/// [`Dispatch`] can halt the entire pipeline if the number of detected errors exceeds the allowed limit (configured
/// via [`with_max_errors`](Dispatch::with_max_errors)). The errors themselves remain with the validator that detected
/// them.
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
/// use xenolith::event::{EventRef, EventHandler, Flow};
/// use xenolith::error::Result;
///
/// /// Counts elements, and stops as soon as it has seen enough.
/// #[derive(Default)]
/// struct FirstFew {
///   seen: usize,
/// }
///
/// impl EventHandler for FirstFew {
///   fn handle(&mut self, event: &EventRef<'_>) -> Result<Flow> {
///     if matches!(event, EventRef::StartElement(_)) {
///       self.seen += 1;
///     }
///     // This handler validates nothing, so it reports no validity errors either way.
///     Ok(if self.seen < 3 { Flow::Continue(0) } else { Flow::Break(0) })
///   }
/// }
/// ```
pub trait EventHandler {
  /// Consumes a single event and returns whether the handler requires further events, along with the number of
  /// validation errors detected during processing.
  ///
  /// A return value of [`Flow::Continue`] requests the next event, while [`Flow::Break`] indicates that the handler
  /// requires no further events for the current execution. In either case, the value returned represents the number
  /// of validation errors detected while processing the event, including those found by the handler itself and by any
  /// handlers to which the event propagated. For a handler that does neither, the value is `0`.
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
  fn handle(&mut self, event: &EventRef<'_>) -> Result<Flow>;

  /// Receives the final outcome of the run.
  ///
  /// A source calls this method exactly once at the end of each execution. [`Outcome::Completed`] is passed if all
  /// handlers have accepted [`EndDocument`](EventRef::EndDocument). [`Outcome::Stopped`] is passed if the handlers
  /// have terminated early by returning [`Flow::Break`]. If the execution terminates because either the input or a
  /// handler produces an error, [`Outcome::Failed`] is passed along with that error. All handlers involved in the
  /// execution — including those that terminated early or caused an error — are notified of the same outcome. Handlers
  /// holding document - related resources (such as files being written to) can choose here whether to release them or
  /// retain them. The default implementation performs no action.
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

  /// Processing stopped because a handler returned [`Flow::Break`] to request early termination. This is not an error.
  /// However, please note that one or more handlers may not have received [`EndDocument`](EventRef::EndDocument).
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
  /// complete or when the handlers have finished processing by returning [`Flow::Break`] (in the latter case,
  /// `EndDocument` is not reported).
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
/// A dispatch treats two types of members differently. **Consumers** ([`with_handler`](Self::with_handler)) are
/// handlers that consume events and can autonomously terminate processing by returning [`Flow::Break`]. Once a
/// consumer terminates, it receives no further events during the current run, though processing continues for other
/// members. In contrast, **validators** ([`with_validator`](Self::with_validator)) only validate events and do not
/// determine when execution ends. When all consumers have terminated, the dispatch returns [`Flow::Break`], allowing
/// the source (event provider) to stop processing early. Since a dispatch containing no consumers never terminates
/// autonomously, a source with only validators registered will read the entire document and report all issues found.
///
/// The dispatcher aggregates validity errors reported by each member for each event, returns the total to the upstream
/// component, and maintains a running total for the entire execution. If [`with_max_errors`](Self::with_max_errors) is
/// configured, the execution is rejected (halted) once the total error count exceeds the limit, preventing the event
/// from being passed to the next member. The errors themselves are retained within the validator that detected them
/// and can be retrieved from each validator after execution concludes.
///
/// When the [`StartDocument`](EventRef::StartDocument) event occurs, the completion status and error totals of the
/// members are reset, and a new execution begins. A dispatcher composed of members that reset their internal state in
/// response to this event can be reused across multiple document processing runs.
///
/// The [`finish`](EventHandler::finish) method is called for all members, including those that terminated early or had
/// their execution interrupted by an error; consequently, all members observe the same execution result. If the
/// `Dispatch` instance is dropped during execution (after [`StartDocument`](EventRef::StartDocument) but before
/// notification of completion), registered members are notified of [`Outcome::Abandoned`].
///
/// Since members are borrowed rather than having their ownership transferred, the caller retains possession of them
/// and can read the results they produced after execution completes.
///
/// # Examples
///
/// ```
/// use xenolith::error::Result;
/// use xenolith::event::{Dispatch, EventRef, EventHandler, Flow};
///
/// #[derive(Default)]
/// struct Count(usize);
/// impl EventHandler for Count {
///   fn handle(&mut self, event: &EventRef<'_>) -> Result<Flow> {
///     if matches!(event, EventRef::StartElement(_)) {
///       self.0 += 1;
///     }
///     Ok(Flow::Continue(0))
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
  members: Vec<Member<'h>>,
  /// Whether a run has begun and its members have not yet been told how it ended.
  running: bool,
  /// The number of validity errors after which the run is refused, or `None` for no limit.
  max_errors: Option<usize>,
  /// The number of validity errors the members have reported in the current run.
  validity_error_count: usize,
}

/// One of the members of [`Dispatch`] and what known to that [`Dispatch`] in the current execution.
struct Member<'h> {
  handler: &'h mut dyn EventHandler,
  /// Whether this member only checks the events, and so has no say in when the run ends.
  validator: bool,
  /// Whether it returned [`Flow::Break`] in the current run. Cleared at each `StartDocument`.
  finished: bool,
}

impl std::fmt::Debug for Dispatch<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Dispatch")
      .field("members", &self.members.len())
      .field("max_errors", &self.max_errors)
      .field("validity_error_count", &self.validity_error_count)
      .finish_non_exhaustive()
  }
}

impl<'h> Dispatch<'h> {
  /// Creates a dispatch with no members, which forwards nothing.
  #[must_use]
  pub fn new() -> Self {
    Self { members: Vec::new(), running: false, max_errors: None, validity_error_count: 0 }
  }

  /// Adds a consumer. It receives each event after the members already added.
  #[must_use]
  pub fn with_handler(mut self, handler: &'h mut dyn EventHandler) -> Self {
    self.add(handler);
    self
  }

  /// Adds consumers, in order.
  #[must_use]
  pub fn with_handlers(mut self, handlers: impl IntoIterator<Item = &'h mut dyn EventHandler>) -> Self {
    for handler in handlers {
      self.add(handler);
    }
    self
  }

  /// Adds a consumer to a dispatch already built, for a caller assembling one a piece at a time.
  pub fn add(&mut self, handler: &'h mut dyn EventHandler) -> &mut Self {
    self.members.push(Member { handler, validator: false, finished: false });
    self
  }

  /// Adds a validator. It receives each event after the members already added, and the errors it reports count toward
  /// the limit, but it has no say in when the run ends.
  #[must_use]
  pub fn with_validator(mut self, validator: &'h mut dyn Validator) -> Self {
    self.add_validator(validator);
    self
  }

  /// Adds a validator to a dispatch already built, for a caller assembling one a piece at a time.
  pub fn add_validator(&mut self, validator: &'h mut dyn Validator) -> &mut Self {
    self.members.push(Member { handler: validator.as_event_handler(), validator: true, finished: false });
    self
  }

  /// Sets the number of validity errors members can report before execution is rejected. The default `None` signifies
  /// no limit, while `Some(0)` causes execution to be rejected upon the first error.
  ///
  /// This count includes reports from consumers in subsequent stages of the pipeline; therefore, setting a limit on
  /// the dispatch process closest to the source applies that limit to the entire pipeline.
  #[must_use]
  pub fn with_max_errors(mut self, max: Option<usize>) -> Self {
    self.max_errors = max;
    self
  }

  /// How many members it forwards to.
  #[must_use]
  pub fn len(&self) -> usize {
    self.members.len()
  }

  /// Whether it forwards to none.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.members.is_empty()
  }

  /// Indicates whether all consumers have finished their current execution and the source driving this dispatch can be
  /// stopped. For dispatches with no consumers, this value is always `false`.
  ///
  /// This corresponds to the value reported as [`Flow::Break`] by [`handle`](EventHandler::handle). In transformation
  /// processes that drive their own sources—such as loading a separate document into the pipeline—this value must be
  /// used to propagate the termination signal downstream, as the source itself does not inherit the dispatch's return
  /// value.
  #[must_use]
  pub fn is_stopped(&self) -> bool {
    let mut consumers = self.members.iter().filter(|member| !member.validator).peekable();
    consumers.peek().is_some() && consumers.all(|member| member.finished)
  }

  /// Ends the run with `error`: tells every member that the run failed, and hands the error back to be returned.
  pub(crate) fn fail(&mut self, error: Error) -> Error {
    self.finish(Outcome::Failed(&error));
    error
  }
}

impl EventHandler for Dispatch<'_> {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    // A new document is a new run, and every member takes part in it again.
    if matches!(event, EventRef::StartDocument) {
      for member in &mut self.members {
        member.finished = false;
      }
      self.running = true;
      self.validity_error_count = 0;
    }
    let mut event_validity_error_count: usize = 0;
    for member in &mut self.members {
      if member.finished {
        continue;
      }
      let member_validity_error_count = match member.handler.handle(event)? {
        Flow::Continue(count) => count,
        Flow::Break(count) => {
          member.finished = true;
          count
        }
      };
      // The counts come from handlers an application may have written, so a wrong one saturates rather than overflows,
      // and still exceeds the limit.
      event_validity_error_count = event_validity_error_count.saturating_add(member_validity_error_count);
      self.validity_error_count = self.validity_error_count.saturating_add(member_validity_error_count);
      // Checked before the next member, so a consumer behind a validator never sees an event past the limit.
      if let Some(max) = self.max_errors.filter(|&max| self.validity_error_count > max) {
        let error = Error::validity(format!("more than {max} validity errors were found"));
        return Err(match event.location() {
          Some(at) => error.at(at.clone()),
          None => error,
        });
      }
    }
    Ok(if self.is_stopped() {
      Flow::Break(event_validity_error_count)
    } else {
      Flow::Continue(event_validity_error_count)
    })
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    if self.running {
      self.running = false;
      // Every member, finished or not: one that stopped early may still hold something to release.
      for member in &mut self.members {
        member.handler.finish(outcome);
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
