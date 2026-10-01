//! The application layer: XML read into events or a tree, and a tree written out.

use xenolith::event::{EventConsumer, EventRef, Flow};
use xenolith::io::write::LineBreak;
use xenolith::{Reader, Result, Writer};

/// Records the lexical name of every start element.
#[derive(Default)]
struct Names(Vec<String>);

impl EventConsumer for Names {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    if let EventRef::StartElement(event) = event {
      self.0.push(event.lexical());
    }
    Ok(Flow::Continue(0))
  }
}

#[test]
fn events_reach_the_consumer() {
  let mut names = Names::default();
  Reader::new().events("<a><b/><c/></a>".as_bytes(), &mut names).unwrap();
  assert_eq!(names.0, ["a", "b", "c"]);
}

#[test]
fn a_consumer_that_has_read_enough_ends_the_read_although_the_strict_check_stands_in_front() {
  /// Stops at the first start element.
  #[derive(Default)]
  struct First(Vec<String>);
  impl EventConsumer for First {
    fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
      let EventRef::StartElement(event) = event else { return Ok(Flow::Continue(0)) };
      self.0.push(event.lexical());
      Ok(Flow::Break(0))
    }
  }

  // The rest is never read, so the mismatched end tag after the first element is not found.
  let mut first = First::default();
  Reader::new().events("<a><b/></c>".as_bytes(), &mut first).expect("stopping early is not an error");
  assert_eq!(first.0, ["a"]);
}

#[test]
fn a_strict_read_refuses_each_lexical_violation() {
  for broken in ["<a><!-- x -- y --></a>", "<a x='1' x='2'/>", "<1a/>", "<a><?1pi d?></a>"] {
    assert!(Reader::new().document(broken.as_bytes()).is_err(), "{broken} is not XML and must be refused");
  }
}

#[test]
fn a_strict_read_stops_before_the_consumer_sees_the_violation() {
  // The strict validator stands in front of the consumer, so the consumer never receives the refused start tag.
  let mut names = Names::default();
  let result = Reader::new().events("<a><1b/></a>".as_bytes(), &mut names);
  assert!(result.is_err());
  assert_eq!(names.0, ["a"]);
}

#[test]
fn a_read_that_is_not_strict_hands_on_loose_xml_as_it_was_written() {
  let mut names = Names::default();
  Reader::new().with_strict(false).events("<1a x='1' x='2'><!-- x -- y --></1a>".as_bytes(), &mut names).unwrap();
  assert_eq!(names.0, ["1a"], "nothing is mended");

  let doc = Reader::new().with_strict(false).document("<a><!-- x -- y --></a>".as_bytes()).unwrap();
  let comment = doc.first_child(doc.document_element().unwrap()).unwrap();
  assert_eq!(doc.node_value(comment), Some(" x -- y "));
}

#[test]
fn a_read_that_is_not_strict_still_refuses_what_the_parser_cannot_read() {
  assert!(Reader::new().with_strict(false).document("<a></b>".as_bytes()).is_err());
}

#[test]
fn the_system_id_locates_the_errors() {
  let error = Reader::new().with_system_id("file:///doc.xml").document("<a>".as_bytes()).unwrap_err();
  assert_eq!(error.location().system_id.as_deref(), Some("file:///doc.xml"));
}

#[test]
fn a_tree_read_in_is_written_back_the_same() {
  let xml = "<p:a xmlns:p=\"urn:p\" x=\"1\"><b>t &amp; u</b><!--c--><?pi d?></p:a>";
  let doc = Reader::new().document(xml.as_bytes()).unwrap();
  let out = Writer::new().write(&doc, Vec::new()).unwrap();
  assert_eq!(String::from_utf8(out).unwrap(), xml);
}

#[test]
fn the_reader_passes_its_options_to_the_parts_underneath() {
  use xenolith::io::ParserConfig;

  // The encoding the caller names is the one the bytes are read in, whatever the declaration says.
  let latin1 = [b'<', b'a', b'>', 0xE9, b'<', b'/', b'a', b'>'];
  let doc = Reader::new().with_encoding("ISO-8859-1").document(&latin1[..]).expect("read as Latin-1");
  assert_eq!(doc.text_content(doc.document_element().unwrap()), "\u{e9}");

  // A limit the configuration sets is the parser's, so a document that exceeds it is refused.
  // `#[non_exhaustive]` outside the crate, so the fields are set one at a time rather than in a struct expression.
  let mut config = ParserConfig::default();
  config.limits.document.max_element_depth = Some(2);
  let error = Reader::new().with_config(config).document("<a><b><c/></b></a>".as_bytes()).unwrap_err();
  assert!(error.to_string().contains("depth"), "{error}");

  // One setting decides for the parser and for the tree: with the `xml:id` extension off, the attribute is an
  // ordinary one and the tree marks no ID for it.
  let xml = "<r><a xml:id='x1'/></r>";
  let doc = Reader::new().document(xml.as_bytes()).expect("read");
  assert!(doc.get_element_by_id("x1").is_some());

  let mut config = ParserConfig::default();
  config.extensions.xml_id = false;
  let doc = Reader::new().with_config(config).document(xml.as_bytes()).expect("read");
  assert_eq!(doc.get_element_by_id("x1"), None, "an ordinary attribute now");
}

#[test]
fn the_writer_passes_its_options_to_the_parts_underneath() {
  let doc = Reader::new().document("<!DOCTYPE a><a>caf\u{e9}</a>".as_bytes()).expect("read");

  // The declaration carries the encoding and the standalone the writer was given.
  let out = Writer::new()
    .with_xml_declaration(true)
    .with_standalone(Some(true))
    .with_encoding("ISO-8859-1")
    .write(&doc, Vec::new())
    .expect("written");
  assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\" standalone=\"yes\"?>"), "{out:?}");
  // `é` is one byte in this encoding, so the text is not UTF-8.
  assert!(out.ends_with(&[b'<', b'a', b'>', b'c', b'a', b'f', 0xE9, b'<', b'/', b'a', b'>']), "{out:?}");

  // The declared name can differ from the encoding the bytes are in.
  let out =
    Writer::new().with_xml_declaration(true).with_declared_encoding("latin1").write(&doc, Vec::new()).expect("written");
  assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"latin1\"?>"), "{out:?}");

  // Nothing follows the declaration unless a line break is asked for, and then the one asked for does.
  let out = Writer::new().with_xml_declaration(true).write(&doc, Vec::new()).expect("written");
  assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><a>"), "{out:?}");
  let out = Writer::new()
    .with_xml_declaration(true)
    .with_declaration_line_break(Some(LineBreak::CrLf))
    .write(&doc, Vec::new())
    .expect("written");
  assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<a>"), "{out:?}");

  // A label the writer does not have is refused by the write, not by the setting.
  let writer = Writer::new().with_encoding("no-such-encoding");
  assert!(writer.write(&doc, Vec::new()).is_err());

  // The document type declaration is written only when it is asked for.
  let out = Writer::new().with_doctype(true).write(&doc, Vec::new()).expect("written");
  assert!(String::from_utf8(out).unwrap().starts_with("<!DOCTYPE a>"));
}

#[test]
fn a_read_can_assemble_a_document_from_its_parts() {
  use std::io::Read;
  use xenolith::io::resolve::{EntityRequest, UriResolver};

  /// A resolver over a map, standing in for a filesystem or a catalogue.
  struct Map(Vec<(&'static str, &'static str)>);
  impl UriResolver for Map {
    fn resolve(&self, request: &EntityRequest) -> Result<Option<Box<dyn Read>>> {
      let uri = request.resolved_uri().unwrap_or_default();
      let found = self.0.iter().find(|(name, _)| *name == uri).map(|(_, body)| body.as_bytes());
      Ok(found.map(|body| Box::new(body) as Box<dyn Read>))
    }
  }

  let xml = "<doc xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='part.xml'/></doc>";
  let map = Map(vec![("file:///doc/part.xml", "<p>from the part</p>")]);

  // Off by default: the element is what the tree holds, and nothing was fetched.
  let doc = Reader::new().with_system_id("file:///doc/main.xml").document(xml.as_bytes()).expect("read");
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(doc.first_child(root).unwrap()), "xi:include");

  // On, with a resolver to fetch through: the tree holds the assembled document.
  let doc = Reader::new()
    .with_system_id("file:///doc/main.xml")
    .with_xinclude(true)
    .with_resolver(&map)
    .document(xml.as_bytes())
    .expect("read and assembled");
  let root = doc.document_element().unwrap();
  let included = doc.first_child(root).expect("the included element");
  assert_eq!(doc.node_name(included), "p");
  assert_eq!(doc.text_content(included), "from the part");
  // The base URI fixup says where it came from, so a reference inside it still resolves.
  assert_eq!(doc.attribute(included, "xml:base"), Some("file:///doc/part.xml"));
}

#[test]
fn an_inclusion_with_nothing_to_fetch_through_is_refused() {
  let xml = "<doc xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='part.xml'/></doc>";
  let error = Reader::new()
    .with_system_id("file:///doc/main.xml")
    .with_xinclude(true)
    .document(xml.as_bytes())
    .expect_err("nothing may be fetched without a resolver");
  assert!(error.message().contains("with_resolver"), "{error}");
}
