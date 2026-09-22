//! A built tree walked out as events.
//!
//! These drive the walk through the parser, so they live here rather than beside the code: the parser is a cyclic
//! dev-dependency of this crate, and a unit test inside the library would see two different copies of the event
//! vocabulary, one from the library under test and one from the parser.

use xenolith::dom::build::DomBuilder;
use xenolith::dom::{Document, DomSource};
use xenolith::error::Result;
use xenolith::event::{EventCursor, EventHandler, EventRef, EventSource};
use xenolith::io::StreamSource;

/// Reads `xml` into a tree through the builder.
fn parse(xml: &str) -> Document {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml.as_bytes()).with_handler(&mut builder).emit().unwrap();
  builder.into_document()
}

#[derive(Default)]
struct Trace(Vec<String>);

impl EventHandler for Trace {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    self.0.push(match event {
      EventRef::StartDocument => "start".to_owned(),
      EventRef::EndDocument => "end".to_owned(),
      EventRef::StartElement(event) => {
        let mut line = format!("<{}", event.local);
        for attr in event.attributes.iter() {
          line.push_str(&format!(" {}={}", attr.local, attr.value));
        }
        line.push('>');
        line
      }
      EventRef::EndElement(event) => format!("</{}>", event.local),
      EventRef::Characters(event) => format!("t:{}", event.text),
      EventRef::Cdata(event) => format!("cdata:{}", event.text),
      EventRef::Comment(event) => format!("!:{}", event.text),
      EventRef::ProcessingInstruction(event) => format!("?:{} {}", event.target, event.data),
      EventRef::Doctype(_) => return Ok(()),
    });
    Ok(())
  }
}

fn trace(xml: &str) -> Vec<String> {
  let doc = parse(xml);
  let mut trace = Trace::default();
  DomSource::new(&doc).with_handler(&mut trace).emit().unwrap();
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
  // Record the first element name, then request a stop. The rest of the tree is not visited.
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
  let doc = parse("<a><b/><c/></a>");
  let mut first = First::default();
  DomSource::new(&doc).with_handler(&mut first).emit().unwrap();
  assert_eq!(first.names, ["a"], "only the first start element is seen");
}

#[test]
fn the_document_type_node_is_reported_only_when_a_dtd_is_given() {
  use xenolith::dtd::model::Dtd;
  use xenolith::name::NamePool;

  // No external subset: reading one would need a resolver, and what the tree holds of the declaration is the same.
  let mut builder = DomBuilder::new();
  StreamSource::new("<!DOCTYPE note><note>hi</note>".as_bytes()).with_handler(&mut builder).emit().expect("read");
  let doc = builder.into_document();

  // Without a DTD the node is passed over, as it always was.
  let mut kinds = Kinds::default();
  DomSource::new(&doc).with_handler(&mut kinds).emit().expect("walked");
  assert!(!kinds.0.iter().any(|kind| kind == "doctype"), "{:?}", kinds.0);

  // With one, the walk reports it between the document's start and the root element.
  let mut kinds = Kinds::default();
  DomSource::new(&doc).with_doctype(Dtd::default(), NamePool::new()).with_handler(&mut kinds).emit().expect("walked");
  assert_eq!(kinds.0.first().map(String::as_str), Some("start-document"));
  assert_eq!(kinds.0.get(1).map(String::as_str), Some("doctype"), "{:?}", kinds.0);
}

/// Records the kind of every event, so the order they arrive in can be checked.
#[derive(Default)]
struct Kinds(Vec<String>);

impl EventHandler for Kinds {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    self.0.push(
      match event {
        EventRef::StartDocument => "start-document",
        EventRef::EndDocument => "end-document",
        EventRef::Doctype(_) => "doctype",
        EventRef::StartElement(_) => "start",
        EventRef::EndElement(_) => "end",
        _ => "other",
      }
      .to_owned(),
    );
    Ok(())
  }
}
