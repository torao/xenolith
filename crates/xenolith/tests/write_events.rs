//! The writer as a destination for document events.

use xenolith::dom::DomSource;
use xenolith::dom::build::DomBuilder;
use xenolith::error::{Error, Result};
use xenolith::event::{Dispatch, EventCursor, EventHandler, EventRef, EventSource};
use xenolith::io::StreamSource;
use xenolith::io::write::XmlWriter;

/// Reads `xml` into a tree.
fn tree(xml: &str) -> xenolith::dom::Document {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml.as_bytes()).with_handler(&mut builder).emit().expect("well-formed");
  builder.into_document()
}

/// Writes everything `run` drives into a writer, and returns the output as text.
fn written(run: impl FnOnce(&mut XmlWriter<Vec<u8>>) -> Result<()>) -> String {
  let mut writer = XmlWriter::new(Vec::new());
  run(&mut writer).expect("written");
  String::from_utf8(writer.into_inner()).expect("UTF-8")
}

#[test]
fn a_parser_writes_straight_through_the_writer() {
  let xml = "<a x=\"1\"><b>t &amp; u</b><!--c--><?pi d?></a>";
  let out = written(|writer| StreamSource::new(xml.as_bytes()).with_handler(writer).emit());
  assert_eq!(out, xml, "what was read comes back out unchanged");
}

#[test]
fn a_tree_walks_out_through_the_writer() {
  let doc = tree("<a x='1'><b>t</b></a>");
  let out = written(|writer| DomSource::new(&doc).with_handler(writer).emit());
  assert_eq!(out, "<a x=\"1\"><b>t</b></a>");
}

#[test]
fn namespace_declarations_reach_the_output_as_attributes() {
  // The writer leaves namespaces to whoever drives it. A source reports a declaration as an ordinary attribute, so it
  // is written as one and the document reads back with the same bindings.
  let xml = "<p:a xmlns:p=\"urn:p\" xmlns=\"urn:d\"><b/></p:a>";
  let out = written(|writer| StreamSource::new(xml.as_bytes()).with_handler(writer).emit());
  assert_eq!(out, xml);
}

#[test]
fn an_empty_element_collapses_however_it_was_written_before() {
  let xml = "<a><b></b></a>";
  let out = written(|writer| StreamSource::new(xml.as_bytes()).with_handler(writer).emit());
  assert_eq!(out, "<a><b/></a>", "an element with no content collapses, as it does for the incremental calls");
}

#[test]
fn a_doctype_is_written_before_the_root_without_its_internal_subset() {
  // The event carries the DTD as a parsed model, not the markup it was read from, so the declaration is written down
  // to its name and identifiers and the subset is left out.
  let xml = "<!DOCTYPE note [<!ELEMENT note EMPTY>]><note/>";
  let out = written(|writer| StreamSource::new(xml.as_bytes()).with_handler(writer).emit());
  assert_eq!(out, "<!DOCTYPE note><note/>");
}

/// Refuses any element called `bad`, standing for a schema check in front of the writer.
struct Refuse;

impl EventHandler for Refuse {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if let EventRef::StartElement(event) = event
      && event.local == "bad"
    {
      return Err(Error::validity("the schema forbids <bad>"));
    }
    Ok(())
  }
}

#[test]
fn a_check_in_front_of_the_writer_stops_an_event_before_it_is_written() {
  // Fail-fast dispatch with the check first: the writer never sees the event the check refused, so no byte of it is
  // written and the output holds the valid prefix.
  let mut refuse = Refuse;
  let mut writer = XmlWriter::new(Vec::new());
  let error = {
    let mut lane = Dispatch::new().with_handler(&mut refuse).with_handler(&mut writer);
    StreamSource::new("<a><ok/><bad/></a>".as_bytes()).with_handler(&mut lane).emit().unwrap_err()
  };

  assert!(error.to_string().contains("forbids"), "{error}");
  let out = String::from_utf8(writer.into_inner()).expect("UTF-8");
  assert_eq!(out, "<a><ok/>", "the refused element left nothing behind");
}

#[test]
fn an_end_element_with_nothing_open_is_refused_rather_than_a_panic() {
  // The incremental calls panic on this, because there it is a mistake in the calling code. Driven by events, which
  // may come from anywhere, it is an error the caller can handle.
  let mut writer = XmlWriter::new(Vec::new());
  let error = writer.handle(&EventRef::EndDocument).and_then(|()| writer.handle(&event_end())).unwrap_err();
  assert!(error.to_string().contains("no element open"), "{error}");
}

/// An end element event for a name that was never opened.
fn event_end() -> EventRef<'static> {
  use xenolith::error::Location;
  use xenolith::event::EndElementEventRef;

  // A name is text now, so an event can be built without a pool to resolve it against.
  EventRef::EndElement(EndElementEventRef::new(None, "a", None, Location::unknown()))
}
