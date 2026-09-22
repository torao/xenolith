//! Emitting a built [`Document`] as a stream of events.
//!
//! [`build`](crate::dom::build) converts parser events into a tree. [`DomSource`] traverses the tree and reports them
//! as similar events.
//!

use crate::attr::{AttributeList, AttributeRef, Attributes};
use crate::dtd::model::Dtd;
use crate::error::{Location, Result};
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, Dispatch, DoctypeEventRef, EndElementEventRef, EventCursor,
  EventHandler, EventRef, EventSource, Outcome, ProcessingInstructionEventRef, StartElementEventRef, XmlSpace,
};
use crate::name::NamePool;

use crate::dom::node::NodeData;
use crate::dom::walk::{Visit, Walk};
use crate::dom::{Document, NodeId};

/// An [`EventCursor`] that traverses a built [`Document`], or a subtree thereof, and emits events. It corresponds to
/// Java's `DOMSource`.
///
/// It drives an [`EventHandler`] by walking the tree, so a document already in memory becomes an event source. A
/// consumer of parser events, for example, a writer or a validator, then works on the document without knowing the
/// events came from a tree rather than from a parser. Use it anywhere that takes a source of parser events.
///
/// [`emit`](EventCursor::emit) reports [`StartDocument`](EventRef::StartDocument) first, one event for each node in
/// document order, and [`EndDocument`](EventRef::EndDocument) at the end; [`next`](EventCursor::next) hands them back
/// one at a time instead, notifying the installed handlers as it goes. When the source covers the whole document or a
/// fragment, it emits the children without an enclosing element of their own. A handler that ends the run through
/// [`should_continue`](EventHandler::should_continue) stops the walk, so `EndDocument` does not follow.
///
/// A tree has no source position, so every [`Location`] is [`unknown`](Location::unknown). It also keeps no parsed DTD,
/// which a [`Doctype`](EventRef::Doctype) event carries, so the walk passes over the document type node unless
/// [`with_doctype`](Self::with_doctype) gave it a DTD to report with. The walk does not
/// reconstruct `xml:space` or `xml:lang` scope either, and a start element reports [`XmlSpace::default`] and no
/// language. It does report the base URI the tree recorded for the element.
///
/// # Examples
///
/// ```
/// use xenolith::error::Result;
/// use xenolith::event::{EventRef, EventCursor, EventHandler, EventSource};
/// use xenolith::dom::DomSource;
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::io::StreamSource;
///
/// #[derive(Default)]
/// struct Names(Vec<String>);
/// impl EventHandler for Names {
///   fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
///     if let EventRef::StartElement(event) = event {
///       self.0.push(event.local.to_owned());
///     }
///     Ok(())
///   }
/// }
///
/// // Read a document into a tree through the builder, then walk the tree back out as events.
/// let mut builder = DomBuilder::new();
/// StreamSource::new("<a><b/><c/></a>".as_bytes()).with_handler(&mut builder).emit()?;
/// let doc = builder.into_document();
/// let mut names = Names::default();
/// DomSource::new(&doc).with_handler(&mut names).emit()?;
/// assert_eq!(names.0, ["a", "b", "c"]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct DomSource<'a, 'h> {
  doc: &'a Document,
  node: NodeId,
  /// The walk in progress, made on the first event so that a source can be built and handed on before it runs.
  walk: Option<Walk<'a>>,
  /// The handlers this source was built with, which every event reaches.
  dispatch: Dispatch<'h>,
  /// Where the walk has reached, so `next` knows whether the document has begun or ended.
  step: Step,
  /// The attributes of the element just reported, held here rather than in a local so the event handed out can borrow
  /// them.
  attributes: Option<DomAttributes<'a>>,
  /// The base URI of the element just reported, held here for the same reason.
  base: Option<String>,
  /// The DTD and its name pool to be held by the [`Doctype`](EventRef::Doctype) event. Since the event cannot be
  /// constructed without a DTD to hold, document type nodes are reported only when a DTD exists.
  dtd: Option<(Dtd, NamePool)>,
}

/// Where a [`DomSource`] has reached: the document's own start and end are events of their own, so the walk remembers
/// which side of the tree it is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
  /// Nothing reported yet; the next event is [`StartDocument`](EventRef::StartDocument).
  Before,
  /// The tree is being walked.
  Walking,
  /// The walk reached its end and [`EndDocument`](EventRef::EndDocument) was reported; the next call tells the
  /// handlers the run completed.
  Ended,
  /// Every handler has finished early; the next call tells the handlers the run stopped.
  Stopped,
  /// The run is over, and the handlers have been told how it ended.
  Done,
}

impl std::fmt::Debug for DomSource<'_, '_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("DomSource").field("node", &self.node).finish_non_exhaustive()
  }
}

impl<'a> DomSource<'a, '_> {
  /// Creates a source over the whole document.
  #[must_use]
  pub fn new(doc: &'a Document) -> Self {
    Self::at(doc, doc.document_node())
  }

  /// Creates a source over the subtree rooted at `node`.
  ///
  /// # Panics
  ///
  /// If `node` was made by another document.
  ///
  #[must_use]
  pub fn at(doc: &'a Document, node: NodeId) -> Self {
    assert!(doc.owns(node), "the node was made by another document, and a node id is only valid with its own");
    Self {
      doc,
      node,
      walk: None,
      dispatch: Dispatch::new(),
      step: Step::Before,
      attributes: None,
      base: None,
      dtd: None,
    }
  }

  /// Emits the document type declaration node as a [`Doctype`](EventRef::Doctype) event containing the `dtd` and the
  /// `NamePool` in which its name is interned.
  ///
  /// If these are not specified, the emission of the doctype node event is skipped. The document retains only the name
  /// and identifiers of the document type declaration as string values, but does not hold the full DTD. The
  /// application can obtain these two arguments using [`DtdReader::read`](crate::dtd::DtdReader::read).
  ///
  /// Alternatively, it can specify [`Dtd::default`](crate::dtd::model::Dtd::default) and an empty name pool to output
  /// only the declaration part. However, if the event passes through the
  /// [DTD validation](crate::event::validate::ValidatorSet::validating_dtd), the document structure is evaluated
  /// against an empty DTD; this results in an error because all elements are deemed "undeclared." Conversely, if this
  /// option is not used, the validation considers that there is no DTD to validate against.
  #[must_use]
  pub fn with_doctype(mut self, dtd: Dtd, pool: NamePool) -> Self {
    self.dtd = Some((dtd, pool));
    self
  }
}

impl<'a, 'h> EventSource<'h> for DomSource<'a, 'h> {
  fn with_handler(mut self, handler: &'h mut dyn EventHandler) -> Self {
    self.dispatch = self.dispatch.with_handler(handler);
    self
  }
}

impl<'a, 'h> EventCursor<'h> for DomSource<'a, 'h> {
  fn next(&mut self) -> Result<Option<EventRef<'_>>> {
    match self.step {
      Step::Done => return Ok(None),
      Step::Ended | Step::Stopped => {
        let outcome = if self.step == Step::Ended { Outcome::Completed } else { Outcome::Stopped };
        self.step = Step::Done;
        self.dispatch.finish(outcome);
        return Ok(None);
      }
      Step::Before => {
        self.step = Step::Walking;
        self.walk = Some(self.doc.walk(self.node));
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
      Step::Walking => {}
    }

    // Walk on until a node that has an event of its own. The document and a fragment are containers with no event, a
    // document type node has one only when `with_doctype` gave it a DTD to carry, an attribute is not a child, and
    // only an element closes.
    let doctype_reported = self.dtd.is_some();
    let doc = self.doc;
    let walk = self.walk.as_mut().expect("the walk is made when the document starts");
    let found = loop {
      let Some((visit, node)) = walk.next() else { break None };
      let data = doc.node_data(node);
      let reportable = match visit {
        Visit::Enter => match data {
          NodeData::Document | NodeData::DocumentFragment | NodeData::Attribute(_) => false,
          NodeData::DocumentType { .. } => doctype_reported,
          _ => true,
        },
        Visit::Leave => matches!(data, NodeData::Element(_)),
      };
      if reportable {
        break Some((visit, node));
      }
    };
    let Some((visit, node)) = found else {
      // The walk covered the whole subtree, so the document was read in full.
      self.step = Step::Ended;
      let event = EventRef::EndDocument;
      if let Err(error) = self.dispatch.handle(&event) {
        self.step = Step::Done;
        return Err(self.dispatch.fail(error));
      }
      return Ok(Some(event));
    };

    // Into fields rather than locals, so the event handed back can borrow them.
    if let (Visit::Enter, NodeData::Element(element)) = (visit, doc.node_data(node)) {
      self.attributes = Some(DomAttributes { doc, attributes: &element.attributes });
      self.base = doc.base_uri(node);
    }
    let event = match (visit, doc.node_data(node)) {
      (Visit::Enter, NodeData::Element(element)) => EventRef::StartElement(StartElementEventRef::new(
        element.name.prefix.map(|prefix| doc.pool().resolve(prefix)),
        doc.pool().resolve(element.name.local()),
        element.name.namespace().map(|namespace| doc.pool().resolve(namespace)),
        Attributes::new(self.attributes.as_ref().expect("just recorded")),
        XmlSpace::default(),
        None,
        self.base.as_deref(),
        Location::unknown(),
      )),
      (Visit::Leave, NodeData::Element(element)) => EventRef::EndElement(EndElementEventRef::new(
        element.name.prefix.map(|prefix| doc.pool().resolve(prefix)),
        doc.pool().resolve(element.name.local()),
        element.name.namespace().map(|namespace| doc.pool().resolve(namespace)),
        Location::unknown(),
      )),
      (_, NodeData::Text(text)) => EventRef::Characters(CharactersEventRef::new(text, Location::unknown())),
      (_, NodeData::CdataSection(text)) => EventRef::Cdata(CdataEventRef::new(text, Location::unknown())),
      (_, NodeData::Comment(text)) => EventRef::Comment(CommentEventRef::new(text, Location::unknown())),
      (_, NodeData::ProcessingInstruction { target, data }) => EventRef::ProcessingInstruction(
        ProcessingInstructionEventRef::new(doc.pool().resolve(*target), data, Location::unknown(), Location::unknown()),
      ),
      (_, NodeData::DocumentType { name, public_id, system_id }) => {
        let (dtd, pool) = self.dtd.as_ref().expect("the node is reported only when a DTD was given");
        EventRef::Doctype(DoctypeEventRef::new(
          Some(doc.pool().resolve(*name)),
          public_id.as_deref(),
          system_id.as_deref(),
          dtd,
          pool,
          Location::unknown(),
        ))
      }
      _ => unreachable!("a node with no event of its own was skipped above"),
    };
    if let Err(error) = self.dispatch.handle(&event) {
      self.step = Step::Done;
      return Err(self.dispatch.fail(error));
    }
    if !self.dispatch.should_continue() {
      self.step = Step::Stopped;
    }
    Ok(Some(event))
  }
}

/// The attributes of a DOM element, presented as an [`AttributeList`] so a handler receives them the way the parser
/// delivers them.
///
struct DomAttributes<'a> {
  doc: &'a Document,
  attributes: &'a [NodeId],
}

impl AttributeList for DomAttributes<'_> {
  fn len(&self) -> usize {
    self.attributes.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    let id = *self.attributes.get(index)?;
    let NodeData::Attribute(attr) = self.doc.node_data(id) else { return None };
    let pool = self.doc.pool();
    Some(AttributeRef {
      prefix: attr.name.prefix.map(|prefix| pool.resolve(prefix)),
      local: pool.resolve(attr.name.local()),
      namespace: attr.name.namespace().map(|namespace| pool.resolve(namespace)),
      value: &attr.value,
      // A tree keeps no positions, as the events of this source carry none either.
      location: Location::unknown(),
      value_location: Location::unknown(),
    })
  }
}
