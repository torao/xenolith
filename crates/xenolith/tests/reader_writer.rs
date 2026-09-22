//! The application layer: XML read into events or a tree, and a tree written out.

use xenolith::event::{EventHandler, EventRef};
use xenolith::{Reader, Result, Writer};

/// Records the lexical name of every start element.
#[derive(Default)]
struct Names(Vec<String>);

impl EventHandler for Names {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if let EventRef::StartElement(event) = event {
      self.0.push(event.lexical());
    }
    Ok(())
  }
}

#[test]
fn events_reach_the_handler() {
  let mut names = Names::default();
  Reader::new().events("<a><b/><c/></a>".as_bytes(), &mut names).unwrap();
  assert_eq!(names.0, ["a", "b", "c"]);
}

#[test]
fn a_strict_read_refuses_each_lexical_violation() {
  for broken in ["<a><!-- x -- y --></a>", "<a x='1' x='2'/>", "<1a/>", "<a><?1pi d?></a>"] {
    assert!(Reader::new().document(broken.as_bytes()).is_err(), "{broken} is not XML and must be refused");
  }
}

#[test]
fn a_strict_read_stops_before_the_handler_sees_the_violation() {
  // The strict validator stands in front of the handler, so the handler never receives the refused start tag.
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

  // A label the writer does not have is refused by the write, not by the setting.
  let writer = Writer::new().with_encoding("no-such-encoding");
  assert!(writer.write(&doc, Vec::new()).is_err());

  // The document type declaration is written only when it is asked for.
  let out = Writer::new().with_doctype(true).write(&doc, Vec::new()).expect("written");
  assert!(String::from_utf8(out).unwrap().starts_with("<!DOCTYPE a>"));
}
