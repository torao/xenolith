//! Emitting a built [`Document`] as a stream of parser events.
//!
//! [`build`](crate::build) turns a parser's events into a tree; [`DomSource`] goes the other way.

use xenolith_core::attr::{AttributeList, AttributeRef, Attributes};
use xenolith_core::error::{Location, Result};
use xenolith_core::name::{NameId, NamePool, QName};
use xenolith_parser::XmlSpace;
use xenolith_parser::sax::{
  CdataEvent, CharactersEvent, CommentEvent, EndElementEvent, EventSource, Handler, ProcessingInstructionEvent,
  StartElementEvent,
};

use crate::node::NodeData;
use crate::{Document, NodeId};

/// A built [`Document`], or a subtree of it, as an [`EventSource`], the counterpart of Java's `DOMSource`.
///
/// It drives a [`Handler`] by walking the tree, so a document already in memory becomes an event source. Anything that
/// consumes parser events, a serializer or a validator wrapped as a handler, then works on a tree without knowing it did
/// not come from a parser. Pass it wherever a source of parser events is taken.
///
/// [`emit`](EventSource::emit) calls [`start_document`](Handler::start_document) first, an event for each node in
/// document order, and [`end_document`](Handler::end_document) at the end. When the source covers the whole document or a
/// fragment, its children are emitted without an enclosing element. A handler that stops the run through
/// [`should_continue`](Handler::should_continue) ends it before the final call.
///
/// The events carry no source position, since a tree has none, so every [`Location`] is [`unknown`](Location::unknown).
/// A tree keeps no document type declaration in a form a [`doctype`](Handler::doctype) callback could receive, so that
/// node is skipped. `xml:space` and `xml:lang` scope is not reconstructed; a start element reports
/// [`XmlSpace::default`] and no language.
///
/// # Examples
///
/// ```
/// use xenolith_dom::{DomSource, build};
/// use xenolith_parser::sax::{EventSource, Handler, StartElementEvent};
///
/// #[derive(Default)]
/// struct Names(Vec<String>);
/// impl Handler for Names {
///   fn start_element(&mut self, event: StartElementEvent<'_>) {
///     self.0.push(event.pool.resolve(event.name.local()).to_owned());
///   }
/// }
///
/// let doc = build::parse("<a><b/><c/></a>".as_bytes())?;
/// let mut names = Names::default();
/// DomSource::new(&doc).emit(&mut names)?;
/// assert_eq!(names.0, ["a", "b", "c"]);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
pub struct DomSource<'a> {
  doc: &'a Document,
  node: NodeId,
}

impl std::fmt::Debug for DomSource<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("DomSource").field("node", &self.node).finish_non_exhaustive()
  }
}

impl<'a> DomSource<'a> {
  /// A source over the whole document.
  #[must_use]
  pub fn new(doc: &'a Document) -> Self {
    Self { doc, node: doc.root() }
  }

  /// A source over the subtree rooted at `node`.
  #[must_use]
  pub fn at(doc: &'a Document, node: NodeId) -> Self {
    Self { doc, node }
  }
}

impl EventSource for DomSource<'_> {
  fn emit<H: Handler + ?Sized>(&mut self, handler: &mut H) -> Result<()> {
    handler.start_document();
    if walk(self.doc, self.node, handler) {
      // The whole subtree was emitted, so the document was read in full.
      handler.end_document();
    }
    Ok(())
  }
}

/// A step in the walk: enter a node, or leave an element after its children.
enum Step {
  Enter(NodeId),
  Leave(NodeId),
}

/// Walks the subtree rooted at `node`, returning `false` if a handler stopped the run early.
fn walk<H: Handler + ?Sized>(doc: &Document, node: NodeId, handler: &mut H) -> bool {
  let mut stack = vec![Step::Enter(node)];
  while let Some(step) = stack.pop() {
    match step {
      Step::Enter(id) => {
        if !enter(doc, id, handler, &mut stack) {
          return false;
        }
      }
      Step::Leave(id) => {
        let name = element_name(doc, id).expect("only an element is scheduled to leave");
        handler.end_element(EndElementEvent::new(name, doc.pool(), Location::unknown()));
        if !handler.should_continue() {
          return false;
        }
      }
    }
  }
  true
}

/// Handles entering one node, scheduling its children, and returns `false` if a handler stopped the run.
fn enter<H: Handler + ?Sized>(doc: &Document, id: NodeId, handler: &mut H, stack: &mut Vec<Step>) -> bool {
  match doc.node_data(id) {
    NodeData::Element(element) => {
      let attributes = DomAttributes { doc, attributes: &element.attributes };
      let event = StartElementEvent::new(
        element.name,
        Attributes::new(&attributes),
        XmlSpace::default(),
        None,
        doc.pool(),
        Location::unknown(),
      );
      handler.start_element(event);
      if !handler.should_continue() {
        return false;
      }
      // Leave after the children, and push the children so the first is processed first.
      stack.push(Step::Leave(id));
      push_children(doc, id, stack);
    }
    NodeData::Text(text) => {
      handler.characters(CharactersEvent::new(text, Location::unknown()));
      if !handler.should_continue() {
        return false;
      }
    }
    NodeData::CdataSection(text) => {
      handler.cdata(CdataEvent::new(text, Location::unknown()));
      if !handler.should_continue() {
        return false;
      }
    }
    NodeData::Comment(text) => {
      handler.comment(CommentEvent::new(text, Location::unknown()));
      if !handler.should_continue() {
        return false;
      }
    }
    NodeData::ProcessingInstruction { target, data } => {
      let target = doc.pool().resolve(*target);
      handler.processing_instruction(ProcessingInstructionEvent::new(
        target,
        data,
        Location::unknown(),
        Location::unknown(),
      ));
      if !handler.should_continue() {
        return false;
      }
    }
    // The document and a fragment are containers with no event of their own; emit their children in order.
    NodeData::Document | NodeData::DocumentFragment => push_children(doc, id, stack),
    // A tree keeps no DTD, so there is nothing to hand a doctype callback. An attribute is not a child.
    NodeData::DocumentType { .. } | NodeData::Attribute(_) => {}
  }
  true
}

/// Pushes a node's children so that, popped from the stack, they are visited in document order.
fn push_children(doc: &Document, id: NodeId, stack: &mut Vec<Step>) {
  let children: Vec<NodeId> = doc.children(id).collect();
  for child in children.into_iter().rev() {
    stack.push(Step::Enter(child));
  }
}

/// The name of a node when it is an element.
fn element_name(doc: &Document, id: NodeId) -> Option<QName> {
  match doc.node_data(id) {
    NodeData::Element(element) => Some(element.name),
    _ => None,
  }
}

/// The attributes of a DOM element, presented as an [`AttributeList`] so a [`Handler`] receives them the way the parser
/// delivers them.
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
    Some(AttributeRef {
      name: attr.name,
      value: &attr.value,
      declares_namespace: declares_namespace(attr.name, self.doc.pool()),
    })
  }
}

/// Whether an attribute is a namespace declaration (`xmlns` or `xmlns:p`).
fn declares_namespace(name: QName, pool: &NamePool) -> bool {
  name.namespace() == Some(NameId::XMLNS_NS) || (name.prefix.is_none() && pool.resolve(name.local()) == "xmlns")
}

#[cfg(test)]
mod tests {
  use xenolith_parser::sax::{
    CdataEvent, CharactersEvent, CommentEvent, EndElementEvent, EventSource, Handler, ProcessingInstructionEvent,
    StartElementEvent,
  };

  use super::*;
  use crate::build;

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
      let mut line = format!("<{}", event.pool.resolve(event.name.local()));
      for attr in event.attributes.iter() {
        line.push_str(&format!(" {}={}", event.pool.resolve(attr.name.local()), attr.value));
      }
      line.push('>');
      self.0.push(line);
    }
    fn end_element(&mut self, event: EndElementEvent<'_>) {
      self.0.push(format!("</{}>", event.pool.resolve(event.name.local())));
    }
    fn characters(&mut self, event: CharactersEvent<'_>) {
      self.0.push(format!("t:{}", event.text));
    }
    fn cdata(&mut self, event: CdataEvent<'_>) {
      self.0.push(format!("cdata:{}", event.text));
    }
    fn comment(&mut self, event: CommentEvent<'_>) {
      self.0.push(format!("!:{}", event.text));
    }
    fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) {
      self.0.push(format!("?:{} {}", event.target, event.data));
    }
  }

  fn trace(xml: &str) -> Vec<String> {
    let doc = build::parse(xml.as_bytes()).unwrap();
    let mut trace = Trace::default();
    DomSource::new(&doc).emit(&mut trace).unwrap();
    trace.0
  }

  #[test]
  fn emits_a_document_in_order() {
    let events = trace("<a>hi<b/><!--c--><?p d?></a>");
    assert_eq!(events, ["start", "<a>", "t:hi", "<b>", "</b>", "!:c", "?:p d", "</a>", "end"]);
  }

  #[test]
  fn emits_attributes_with_the_start_element() {
    let events = trace("<a x='1' y='2'/>");
    assert_eq!(events, ["start", "<a x=1 y=2>", "</a>", "end"]);
  }

  #[test]
  fn emits_cdata_apart_from_text() {
    let events = trace("<a><![CDATA[<raw>]]></a>");
    assert_eq!(events, ["start", "<a>", "cdata:<raw>", "</a>", "end"]);
  }

  #[test]
  fn emits_nested_elements_with_matching_ends() {
    let events = trace("<a><b><c/></b></a>");
    assert_eq!(events, ["start", "<a>", "<b>", "<c>", "</c>", "</b>", "</a>", "end"]);
  }

  #[test]
  fn a_handler_stops_the_emission_early() {
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
    let doc = build::parse("<a><b/><c/></a>".as_bytes()).unwrap();
    let mut first = First::default();
    DomSource::new(&doc).emit(&mut first).unwrap();
    assert_eq!(first.names, ["a"], "only the first start element is seen");
  }
}
