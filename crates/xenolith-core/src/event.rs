//! The push event vocabulary: a source emits a document's events, and a [`Handler`] receives them.
//!
//! Reading, walking a tree, and writing all produce or consume the same sequence of events. This module defines the
//! shared shape so they meet on one interface. An [`EventSource`] drives the events; a [`Handler`] receives each one
//! through a callback. Neither is tied to a particular producer: a parser reading input and a walk over a built tree
//! are both [`EventSource`]s, and a validator, a tree builder, and a writer are all [`Handler`]s. The concrete sources
//! live in the crates that produce them, since only this vocabulary is common to all.
//!
//! [`Broadcast`] runs several handlers over one source in a single pass, so an application handler and a validator can
//! see the same events without reading the source twice.
//!
//! A [`Handler`] callback returns nothing. A handler that needs to report a result, or a problem it detects, keeps it
//! in its own fields; the caller owns the handler and reads it back after the run. After each event a source calls
//! [`should_continue`](Handler::should_continue), so a handler that has read all it needs stops the run early.

use crate::attr::Attributes;
use crate::error::{Location, Result};
use crate::model::dtd::Dtd;
use crate::name::{NamePool, QName};

/// The `xml:space` handling in effect, taken from the nearest element in scope that set it.
///
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XmlSpace {
  /// No `xml:space` is in scope, or the nearest one says `default`.
  #[default]
  Default,
  /// `xml:space="preserve"` is in scope.
  Preserve,
}

/// A start element and the scope around it, borrowed from the source for one callback.
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
  /// Builds a start element event, for a source that drives a [`Handler`], for example a parser or a tree walk. A
  /// source with no `xml:space`, `xml:lang`, or base URI scope passes [`XmlSpace::default`] and `None`.
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

/// An end element, borrowed from the source for one callback.
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
  /// Builds an end element event, for a source that drives a [`Handler`].
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
  /// Builds a character data event, for a source that drives a [`Handler`].
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
  /// Builds a CDATA section event, for a source that drives a [`Handler`].
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
  /// Builds a comment event, for a source that drives a [`Handler`].
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
  /// Builds a processing instruction event, for a source that drives a [`Handler`]. A source that tracks no separate
  /// anchor for `data` passes `location` again as `data_location`.
  ///
  #[must_use]
  pub fn new(target: &'a str, data: &'a str, data_location: Location, location: Location) -> Self {
    Self { target, data, data_location, location }
  }
}

/// A document type declaration: the root element name it declares, its external identifiers, and the parsed DTD.
///
/// By this event the whole DTD has been read, its internal subset, external subset, and any external parameter
/// entities included, so `dtd` reaches the notations, unparsed entities, and element, attribute, and entity
/// declarations, interning a name with `pool` to look one up.
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

impl<'a> DoctypeEvent<'a> {
  /// Builds a document type declaration event, for a source that drives a [`Handler`].
  ///
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

/// Receives events from an event source. Every method has a default, so a handler overrides only what it cares about.
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

/// An abstract source of document events that generates and dispatches them into a [`Handler`].
///
/// Implementations range from a parser that consumes input to a walk over a tree another crate built. A caller runs
/// any of them through the same handler. This is the equivalent of Java's `javax.xml.transform.Source`.
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
  /// Returns the source's error if the document is not well-formed or reading fails. A handler has no error channel;
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

  /// Whether this source asks for `xml:id` attributes to be checked as IDs by default.
  ///
  /// A parser reports what its configuration was set to, so a consumer picks the default up without an explicit
  /// request. A source with no such setting, a tree walk for one, returns `false`.
  ///
  fn defaults_xml_id(&self) -> bool {
    false
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
  use super::*;
  use crate::attr::{AttributeList, AttributeRef};
  use crate::name::NamePool;

  /// An attribute list with nothing in it, so a test event can carry no attributes.
  struct NoAttributes;

  impl AttributeList for NoAttributes {
    fn len(&self) -> usize {
      0
    }

    fn get(&self, _index: usize) -> Option<AttributeRef<'_>> {
      None
    }
  }

  /// A tiny source that emits `<a>hi<b/></a>` from held data, so the vocabulary can be tested without a parser.
  struct TinySource {
    pool: NamePool,
  }

  impl EventSource for TinySource {
    fn emit<H: Handler + ?Sized>(&mut self, handler: &mut H) -> Result<()> {
      let a = QName::new(None, None, self.pool.intern("a"));
      let b = QName::new(None, None, self.pool.intern("b"));
      let empty = NoAttributes;
      let attrs = Attributes::new(&empty);
      handler.start_document();
      handler.start_element(StartElementEvent::new(
        a,
        attrs,
        XmlSpace::Default,
        None,
        None,
        &self.pool,
        Location::unknown(),
      ));
      handler.characters(CharactersEvent::new("hi", Location::unknown()));
      handler.start_element(StartElementEvent::new(
        b,
        attrs,
        XmlSpace::Default,
        None,
        None,
        &self.pool,
        Location::unknown(),
      ));
      handler.end_element(EndElementEvent::new(b, &self.pool, Location::unknown()));
      handler.end_element(EndElementEvent::new(a, &self.pool, Location::unknown()));
      handler.end_document();
      Ok(())
    }
  }

  #[derive(Default)]
  struct Counts {
    elements: usize,
    text: usize,
  }

  impl Handler for Counts {
    fn start_element(&mut self, _event: StartElementEvent<'_>) {
      self.elements += 1;
    }

    fn characters(&mut self, event: CharactersEvent<'_>) {
      self.text += event.text.len();
    }
  }

  #[test]
  fn emit_drives_a_handler() {
    let mut source = TinySource { pool: NamePool::new() };
    let mut counts = Counts::default();
    source.emit(&mut counts).unwrap();
    assert_eq!((counts.elements, counts.text), (2, 2));
  }

  #[test]
  fn broadcast_feeds_every_handler() {
    let source = TinySource { pool: NamePool::new() };
    let mut first = Counts::default();
    let mut second = Counts::default();
    source.broadcast().with_handler(&mut first).with_handler(&mut second).run().unwrap();
    assert_eq!(first.elements, 2);
    assert_eq!(second.elements, 2);
  }
}
