//! Building a DOM from parsed XML.

use std::io::Read;

use xenolith::dom::build::DomBuilder;
use xenolith::dom::{Document, NodeType};
use xenolith::event::{EventCursor, EventHandler, EventSource};
use xenolith::io::StreamSource;

/// Reads `xml` into a tree through the builder.
fn parse(xml: &str) -> Document {
  parse_reader(StreamSource::new(xml.as_bytes()))
}

/// Reads a prepared reader into a tree, so a system identifier or a resolver can be set first.
fn parse_reader<R: Read>(mut source: StreamSource<'_, R>) -> Document {
  // The builder is a local, so it cannot be installed on a source made elsewhere; the run is driven here and each
  // event handed to it as it arrives.
  let mut builder = DomBuilder::new();
  while let Some(event) = source.next().expect("well-formed") {
    builder.handle(&event).expect("well-formed");
  }
  builder.into_document()
}

#[test]
fn builds_elements_text_and_nesting() {
  let doc = parse("<doc><p>Hello</p><p>World</p></doc>");
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(root), "doc");
  let ps: Vec<_> = doc.children(root).collect();
  assert_eq!(ps.len(), 2);
  assert_eq!(doc.node_name(ps[0]), "p");
  assert_eq!(doc.text_content(ps[0]), "Hello");
  assert_eq!(doc.text_content(root), "HelloWorld");
}

#[test]
fn a_long_text_run_coalesces_into_one_text_node() {
  // The reader feeds the parser in chunks, so a long run arrives as several text fragments; the DOM
  // builder must join adjacent character data into a single text node, as the data model requires.
  let body = "x".repeat(30_000);
  let doc = parse(&format!("<a>{body}</a>"));
  let a = doc.document_element().unwrap();
  assert_eq!(doc.children(a).count(), 1, "adjacent text fragments must coalesce into one node");
  assert_eq!(doc.text_content(a), body);
}

#[test]
fn carries_attributes_and_their_values() {
  let doc = parse("<a x='1' y='two'/>");
  let a = doc.document_element().unwrap();
  assert_eq!(doc.attribute(a, "x"), Some("1"));
  assert_eq!(doc.attribute(a, "y"), Some("two"));
}

#[test]
fn resolves_namespaces_onto_names() {
  let doc = parse("<a xmlns='urn:d' xmlns:p='urn:p'><p:b/></a>");
  let a = doc.document_element().unwrap();
  assert_eq!(doc.namespace_uri(a), Some("urn:d"));
  let b = doc.first_child(a).unwrap();
  assert_eq!(doc.namespace_uri(b), Some("urn:p"));
  assert_eq!(doc.prefix(b), Some("p"));
  assert_eq!(doc.local_name(b), Some("b"));
  // The namespace declaration is kept as an attribute in the XMLNS namespace.
  assert_eq!(doc.attribute(a, "xmlns"), Some("urn:d"));
}

#[test]
fn builds_comments_pis_and_cdata() {
  let doc = parse("<doc><!--c--><?pi data?><![CDATA[<raw>]]></doc>");
  let root = doc.document_element().unwrap();
  let kids: Vec<_> = doc.children(root).map(|n| doc.node_type(n)).collect();
  assert_eq!(kids, [NodeType::COMMENT_NODE, NodeType::PROCESSING_INSTRUCTION_NODE, NodeType::CDATA_SECTION_NODE]);
  let cdata = doc.last_child(root).unwrap();
  assert_eq!(doc.node_value(cdata), Some("<raw>"));
}

#[test]
fn a_prolog_comment_becomes_a_child_of_the_document() {
  let doc = parse("<!--intro--><doc/>");
  let first = doc.first_child(doc.document_node()).unwrap();
  assert_eq!(doc.node_type(first), NodeType::COMMENT_NODE);
  assert_eq!(doc.node_value(first), Some("intro"));
}

#[test]
fn builds_the_document_type_node() {
  let doc = parse("<!DOCTYPE greeting [<!ELEMENT greeting (#PCDATA)>]><greeting>hi</greeting>");
  let doctype = doc.doctype().unwrap();
  assert_eq!(doc.node_type(doctype), NodeType::DOCUMENT_TYPE_NODE);
  assert_eq!(doc.node_name(doctype), "greeting");
}

#[test]
fn xml_id_is_marked_so_get_element_by_id_finds_it() {
  let doc = parse("<r><a xml:id='x1'/><b xml:id='x2'/></r>");
  let a = doc.get_element_by_id("x1").unwrap();
  assert_eq!(doc.node_name(a), "a");
  assert_eq!(doc.get_element_by_id("x2").map(|n| doc.node_name(n)).as_deref(), Some("b"));
  assert_eq!(doc.get_element_by_id("nope"), None);
}

#[test]
fn xml_id_turned_off_leaves_an_ordinary_attribute_unless_the_dtd_declares_it() {
  let build = |xml: &str| {
    let mut builder = DomBuilder::new().with_xml_id(false);
    StreamSource::new(xml.as_bytes()).with_handler(&mut builder).emit().unwrap();
    builder.into_document()
  };

  let doc = build("<r><a xml:id='x1'/></r>");
  assert_eq!(doc.get_element_by_id("x1"), None, "an ordinary attribute");
  let a = doc.document_element().and_then(|r| doc.first_child(r)).unwrap();
  assert!(doc.has_attribute(a, "xml:id"), "still present");

  let doc = build("<!DOCTYPE r [<!ATTLIST a xml:id ID #IMPLIED>]><r><a xml:id='x1'/></r>");
  assert!(doc.get_element_by_id("x1").is_some(), "an ID because the DTD declares it one");
}

#[test]
fn records_base_uris_from_the_system_id_and_xml_base() {
  use xenolith::io::StreamSource;
  let xml = "<a><b xml:base='../c/'><d/></b></a>";
  let doc = parse_reader(StreamSource::with_system_id(xml.as_bytes(), "file:///a/b/doc.xml"));
  let a = doc.document_element().unwrap();
  assert_eq!(doc.base_uri(a).as_deref(), Some("file:///a/b/doc.xml"));
  let b = doc.first_child(a).unwrap();
  assert_eq!(doc.base_uri(b).as_deref(), Some("file:///a/c/"), "xml:base is resolved against the document URI");
  let d = doc.first_child(b).unwrap();
  assert_eq!(doc.base_uri(d).as_deref(), Some("file:///a/c/"), "a child with no xml:base inherits");
}

#[test]
fn base_uri_is_none_without_a_system_id_or_xml_base() {
  let doc = parse("<a><b/></a>");
  assert_eq!(doc.base_uri(doc.document_element().unwrap()), None);
}

#[test]
fn captures_the_doctype_public_and_system_ids() {
  use xenolith::io::StreamSource;
  use xenolith::io::resolve::{EntityRequest, UriResolver};

  // A resolver that serves the external subset as empty — enough for the DOCTYPE to be read.
  struct Empty;
  impl UriResolver for Empty {
    fn resolve(&mut self, _request: &EntityRequest) -> Result<Option<Box<dyn std::io::Read>>, xenolith::Error> {
      Ok(Some(Box::new(std::io::empty())))
    }
  }

  let xml = "<!DOCTYPE a PUBLIC \"pub-id\" \"a.dtd\"><a/>";
  let doc = parse_reader(StreamSource::new(xml.as_bytes()).with_resolver(Empty));
  let doctype = doc.doctype().unwrap();
  assert_eq!(doc.node_name(doctype), "a");
  assert_eq!(doc.public_id(doctype), Some("pub-id"));
  assert_eq!(doc.system_id(doctype), Some("a.dtd"));
}

#[test]
fn the_builder_runs_beside_another_handler_in_one_pass() {
  // The DOM builder is an EventHandler, so a source can drive it and another handler together in a single read. Here a
  // counting handler runs alongside it through a `Dispatch`; a validator would take the same place.
  use xenolith::dom::build::DomBuilder;
  use xenolith::error::Result;
  use xenolith::event::{Dispatch, EventCursor, EventHandler, EventRef, EventSource};
  use xenolith::io::StreamSource;

  #[derive(Default)]
  struct CountElements(usize);
  impl EventHandler for CountElements {
    fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
      if matches!(event, EventRef::StartElement(_)) {
        self.0 += 1;
      }
      Ok(())
    }
  }

  let mut builder = DomBuilder::new();
  let mut counter = CountElements::default();
  {
    let mut both = Dispatch::new().with_handler(&mut builder).with_handler(&mut counter);
    StreamSource::new("<doc><a/><b/></doc>".as_bytes()).with_handler(&mut both).emit().expect("well-formed");
  }

  assert_eq!(counter.0, 3, "the counter saw every element in the same pass");
  let doc = builder.into_document();
  assert_eq!(doc.node_name(doc.document_element().unwrap()), "doc");
  assert_eq!(doc.children(doc.document_element().unwrap()).count(), 2);
}

#[test]
fn a_dtd_id_attribute_is_marked() {
  let xml = "<!DOCTYPE r [<!ELEMENT r (item)><!ELEMENT item EMPTY><!ATTLIST item key ID #IMPLIED>]>\
             <r><item key='k1'/></r>";
  let doc = parse(xml);
  let item = doc.get_element_by_id("k1").unwrap();
  assert_eq!(doc.node_name(item), "item");
  // A non-ID attribute of the same value is not found.
  assert_eq!(doc.get_element_by_id("r"), None);
}

#[test]
fn an_end_element_the_events_do_not_open_is_refused() {
  use xenolith::error::Location;
  use xenolith::event::{EndElementEventRef, EventRef};

  fn end<'a>(name: &'a str) -> EventRef<'a> {
    EventRef::EndElement(EndElementEventRef::new(None, name, None, Location::unknown()))
  }

  // Nothing open: the document sits under the elements and is not one to end, so this closes nothing.
  let mut builder = DomBuilder::new();
  let error = builder.handle(&end("a")).expect_err("no element is open");
  assert!(error.to_string().contains("no element open"), "{error}");

  // Open, but named something else: closing the open one would build a tree the events do not describe.
  let mut builder = DomBuilder::new();
  let mut source = StreamSource::new("<a>".as_bytes());
  let start = source.next().expect("read").expect("start document");
  builder.handle(&start).expect("start document");
  let start = source.next().expect("read").expect("a start element");
  builder.handle(&start).expect("the root opens");
  let error = builder.handle(&end("b")).expect_err("the innermost element open is `a`");
  assert!(error.to_string().contains("expected </a>"), "{error}");
}

#[test]
fn a_document_that_begins_drops_what_an_earlier_one_left_open() {
  use xenolith::error::Location;
  use xenolith::event::{EndElementEventRef, EventRef};

  // The first run is abandoned with `a` still open. The second begins, so what it builds goes under the document
  // rather than inside `a`, and the end element of the run before it no longer closes anything.
  let mut builder = DomBuilder::new();
  let mut source = StreamSource::new("<a><b/>".as_bytes());
  // The read ends in an error, since `a` is never closed; what reached the builder before that is the point here.
  while let Ok(Some(event)) = source.next() {
    builder.handle(&event).expect("well-formed so far");
  }

  builder.handle(&EventRef::StartDocument).expect("a new run");
  let end = EventRef::EndElement(EndElementEventRef::new(None, "a", None, Location::unknown()));
  let error = builder.handle(&end).expect_err("nothing is open in this run");
  assert!(error.to_string().contains("no element open"), "{error}");
}

#[test]
fn a_second_document_through_one_builder_is_the_one_that_comes_out() {
  // A run builds a document, so the second run starts from an empty one: its root is not a second root, and what the
  // first run built is gone. A caller that wants both takes the first with `into_document` before the second begins.
  let mut builder = DomBuilder::new();
  {
    let mut source = StreamSource::new("<a/>".as_bytes()).with_handler(&mut builder);
    source.emit().expect("the first document is built");
  }
  {
    let mut source = StreamSource::new("<b><c/></b>".as_bytes()).with_handler(&mut builder);
    source.emit().expect("the second document is built, not refused as a second root");
  }
  let doc = builder.into_document();
  assert_eq!(doc.node_name(doc.document_element().unwrap()), "b");
  assert_eq!(doc.children(doc.document_element().unwrap()).count(), 1);
}

#[test]
fn the_first_document_is_built_in_the_one_the_builder_was_made_with() {
  use xenolith::event::EventRef;

  // The document a run begins with is the one already there when nothing has been handled, so the first
  // `StartDocument` puts nothing back. Several in a row are the same run beginning, and leave the tree alone.
  let mut builder = DomBuilder::new();
  builder.handle(&EventRef::StartDocument).expect("the run begins");
  builder.handle(&EventRef::StartDocument).expect("still nothing handled");

  let mut source = StreamSource::new("<a/>".as_bytes());
  while let Some(event) = source.next().expect("well-formed") {
    builder.handle(&event).expect("well-formed");
  }
  let doc = builder.into_document();
  assert_eq!(doc.node_name(doc.document_element().unwrap()), "a");
}

#[test]
fn a_dom_exception_reaches_the_caller_as_the_error_that_stopped_the_run() {
  use xenolith::Error;
  use xenolith::dom::ExceptionCode;
  use xenolith::event::EventRef;

  // A root element, then a second one without a document beginning in between, so the DOM is asked for something a
  // document cannot hold.
  let mut builder = DomBuilder::new();
  let mut first = StreamSource::new("<a/>".as_bytes());
  while let Some(event) = first.next().expect("well-formed") {
    builder.handle(&event).expect("well-formed");
  }

  let mut second = StreamSource::new("<b/>".as_bytes());
  let mut refused = None;
  while let Ok(Some(event)) = second.next() {
    // Its `StartDocument` is left out: with it, the builder would begin a run of its own and build `b` cleanly.
    if matches!(event, EventRef::StartDocument) {
      continue;
    }
    if let Err(error) = builder.handle(&event) {
      refused = Some(error);
      break;
    }
  }

  let error = refused.expect("a document may not have a second root element");
  let Error::Dom { exception, .. } = &error else { panic!("the DOM's own exception is carried: {error}") };
  assert_eq!(exception.code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  assert!(error.to_string().contains("HIERARCHY_REQUEST_ERR"), "{error}");
  // The tree comes back as it stands: the run is what reported the exception, and it holds the root it did place.
  let doc = builder.into_document();
  assert_eq!(doc.node_name(doc.document_element().unwrap()), "a");
}

#[test]
fn a_refusal_is_reported_where_the_event_that_caused_it_was() {
  use xenolith::error::Location;
  use xenolith::event::{EndElementEventRef, EventRef};

  // The DOM is handed nodes, not positions, so the location has to come from the event being handled.
  let mut builder = DomBuilder::new();
  let mut first = StreamSource::with_system_id("<a/>".as_bytes(), "file:///doc.xml");
  while let Some(event) = first.next().expect("well-formed") {
    builder.handle(&event).expect("well-formed");
  }

  let mut second = StreamSource::with_system_id(
    "
  <b/>"
      .as_bytes(),
    "file:///doc.xml",
  );
  let mut refused = None;
  while let Ok(Some(event)) = second.next() {
    if matches!(event, EventRef::StartDocument) {
      continue;
    }
    if let Err(error) = builder.handle(&event) {
      refused = Some(error);
      break;
    }
  }
  let error = refused.expect("a document may not have a second root element");
  assert_eq!(error.location().to_string(), "file:///doc.xml:2:3", "{error}");

  // The builder's own refusals are located the same way.
  let at = Location { line: 9, column: 4, ..Location::unknown() }.with_system_id("file:///doc.xml");
  let end = EventRef::EndElement(EndElementEventRef::new(None, "zz", None, at));
  let error = DomBuilder::new().handle(&end).expect_err("nothing is open to end");
  assert_eq!(error.location().to_string(), "file:///doc.xml:9:4", "{error}");
}
