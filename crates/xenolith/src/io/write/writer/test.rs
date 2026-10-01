use super::*;

use crate::attr::{AttributeList, AttributeRef, Attributes};
use crate::dtd::model::Dtd;
use crate::error::Location;
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, DoctypeEventRef, EndElementEventRef,
  ProcessingInstructionEventRef, StartElementEventRef, XmlSpace,
};
use crate::name::NamePool;

/// The attributes an event carries, built from pairs the tests write inline.
struct Attrs(Vec<(String, String)>);

impl AttributeList for Attrs {
  fn len(&self) -> usize {
    self.0.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    let (name, value) = self.0.get(index)?;
    Some(AttributeRef {
      prefix: None,
      local: name,
      namespace: None,
      value,
      location: Location::unknown(),
      value_location: Location::unknown(),
    })
  }
}

/// Hands `w` a start element event for `name` with `attributes`, as a source would.
fn start(w: &mut XmlWriter<Vec<u8>>, name: &str, attributes: &[(&str, &str)]) -> Result<()> {
  let list = Attrs(attributes.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect());
  let event = StartElementEventRef::new(
    None,
    name,
    None,
    Attributes::new(&list),
    XmlSpace::default(),
    None,
    None,
    Location::unknown(),
  );
  w.consume(&EventRef::StartElement(event)).map(drop)
}

/// Hands `w` an end element event for `name`, which has to be the innermost element open.
fn end(w: &mut XmlWriter<Vec<u8>>, name: &str) -> Result<()> {
  w.consume(&EventRef::EndElement(EndElementEventRef::new(None, name, None, Location::unknown()))).map(drop)
}

fn text(w: &mut XmlWriter<Vec<u8>>, text: &str) -> Result<()> {
  w.consume(&EventRef::Characters(CharactersEventRef::new(text, Location::unknown()))).map(drop)
}

fn cdata(w: &mut XmlWriter<Vec<u8>>, text: &str) -> Result<()> {
  w.consume(&EventRef::Cdata(CdataEventRef::new(text, Location::unknown()))).map(drop)
}

fn comment(w: &mut XmlWriter<Vec<u8>>, text: &str) -> Result<()> {
  w.consume(&EventRef::Comment(CommentEventRef::new(text, Location::unknown()))).map(drop)
}

fn pi(w: &mut XmlWriter<Vec<u8>>, target: &str, data: &str) -> Result<()> {
  let event = ProcessingInstructionEventRef::new(target, data, Location::unknown(), Location::unknown());
  w.consume(&EventRef::ProcessingInstruction(event)).map(drop)
}

/// Hands `w` a doctype event with an empty DTD, since the writer never writes the internal subset.
fn doctype(
  w: &mut XmlWriter<Vec<u8>>,
  name: Option<&str>,
  public_id: Option<&str>,
  system_id: Option<&str>,
) -> Result<()> {
  let dtd = Dtd::default();
  let pool = NamePool::new();
  let event = DoctypeEventRef::new(name, public_id, system_id, &dtd, &pool, Location::unknown());
  w.consume(&EventRef::Doctype(event)).map(drop)
}

fn written(build: impl FnOnce(&mut XmlWriter<Vec<u8>>) -> Result<()>) -> String {
  written_by(XmlWriter::new(Vec::new()), build)
}

/// As `written`, for a writer that was configured first.
fn written_by(mut w: XmlWriter<Vec<u8>>, build: impl FnOnce(&mut XmlWriter<Vec<u8>>) -> Result<()>) -> String {
  build(&mut w).unwrap();
  String::from_utf8(w.into_inner()).unwrap()
}

#[test]
fn an_element_with_no_content_is_empty() {
  let out = written(|w| {
    start(w, "a", &[])?;
    end(w, "a")
  });
  assert_eq!(out, "<a/>");
}

#[test]
fn nests_elements_and_attributes() {
  let out = written(|w| {
    start(w, "a", &[("x", "1")])?;
    start(w, "b", &[])?;
    text(w, "t")?;
    end(w, "b")?;
    end(w, "a")
  });
  assert_eq!(out, "<a x=\"1\"><b>t</b></a>");
}

#[test]
fn escapes_text_and_attributes() {
  let out = written(|w| {
    start(w, "a", &[("x", "a \"b\" < c")])?;
    text(w, "1 < 2 & 3")?;
    end(w, "a")
  });
  assert_eq!(out, "<a x=\"a &quot;b&quot; &lt; c\">1 &lt; 2 &amp; 3</a>");
}

#[test]
fn writes_the_prolog_and_leaves() {
  let out = written_by(XmlWriter::new(Vec::new()).with_declaration(true).with_standalone(Some(true)), |w| {
    let _ = w.consume(&EventRef::StartDocument)?;
    comment(w, "hi")?;
    start(w, "a", &[])?;
    pi(w, "pi", "d")?;
    cdata(w, "<raw>]]>x")?;
    end(w, "a")
  });
  assert_eq!(
    out,
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><!--hi--><a><?pi d?><![CDATA[<raw>]]>]]&gt;<![CDATA[x]]></a>"
  );
}

#[test]
fn a_doctype_takes_the_shape_its_identifiers_allow() {
  let both = written(|w| doctype(w, Some("html"), Some("-//W3C//DTD XHTML 1.0 Strict//EN"), Some("x.dtd")));
  assert_eq!(both, "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x.dtd\">");

  let system = written(|w| doctype(w, Some("note"), None, Some("note.dtd")));
  assert_eq!(system, "<!DOCTYPE note SYSTEM \"note.dtd\">");

  let bare = written(|w| doctype(w, Some("note"), None, None));
  assert_eq!(bare, "<!DOCTYPE note>");

  // `PUBLIC` without a system identifier is not a form XML has, so the public one is left out rather than written
  // into something that will not parse.
  let lone_public = written(|w| doctype(w, Some("note"), Some("-//X//EN"), None));
  assert_eq!(lone_public, "<!DOCTYPE note>");

  // A declaration with no name is not a form XML has either, so the event is skipped.
  let nameless = written(|w| doctype(w, None, None, Some("note.dtd")));
  assert_eq!(nameless, "");
}

#[test]
fn a_doctype_sits_between_the_declaration_and_the_root() {
  let out = written_by(XmlWriter::new(Vec::new()).with_declaration(true), |w| {
    let _ = w.consume(&EventRef::StartDocument)?;
    doctype(w, Some("note"), None, Some("note.dtd"))?;
    start(w, "note", &[])?;
    end(w, "note")
  });
  assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE note SYSTEM \"note.dtd\"><note/>");
}

#[test]
fn whitespace_outside_the_root_is_written_as_given() {
  let out = written_by(XmlWriter::new(Vec::new()).with_declaration(true), |w| {
    let _ = w.consume(&EventRef::StartDocument)?;
    text(w, "\n")?;
    doctype(w, Some("a"), None, None)?;
    text(w, "\n")?;
    start(w, "a", &[])?;
    end(w, "a")?;
    text(w, "\n")
  });
  assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE a>\n<a/>\n");
}

#[test]
fn anything_but_whitespace_outside_the_root_is_refused_by_the_strict_validator_ahead() {
  use crate::event::Dispatcher;
  use crate::event::strict::StrictXmlConstraints;

  // The writer writes what it is given. That XML allows only whitespace, comments and processing instructions outside
  // the root element (`Misc`) is the strict validator's rule, applied when it stands ahead of the writer.
  let loose = written(|w| {
    text(w, " x ")?;
    cdata(w, "y")?;
    start(w, "a", &[])?;
    end(w, "a")
  });
  assert_eq!(loose, " x <![CDATA[y]]><a/>");

  let text = EventRef::Characters(CharactersEventRef::new(" x ", Location::unknown()));
  let cdata = EventRef::Cdata(CdataEventRef::new("y", Location::unknown()));
  for event in [text, cdata] {
    let mut strict = StrictXmlConstraints::new();
    let mut w = XmlWriter::new(Vec::new());
    {
      let mut lane = Dispatcher::new().add_consumer(&mut strict).add_consumer(&mut w);
      let _ = lane.consume(&EventRef::StartDocument).unwrap();
      let error = lane.consume(&event).expect_err("not allowed outside the root element");
      assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
    }
    assert!(w.into_inner().is_empty(), "the validator stopped it before it was written");
  }
}

#[test]
fn a_document_read_and_written_keeps_its_layout_outside_the_root() {
  use crate::event::{EventCursor, EventProducer};
  use crate::io::StreamSource;

  let xml = "<?xml version='1.0'?>\n<!DOCTYPE a>\n<!-- note -->\n<a>x</a>\n";
  let mut w = XmlWriter::new(Vec::new()).with_declaration(true);
  StreamSource::new(xml.as_bytes()).add_consumer(&mut w).emit().unwrap();
  let out = String::from_utf8(w.into_inner()).unwrap();
  assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE a>\n<!-- note -->\n<a>x</a>\n");
}

#[test]
fn an_end_with_no_element_open_is_refused() {
  let mut w = XmlWriter::new(Vec::new());
  let error = end(&mut w, "a").expect_err("nothing is open to end");
  assert!(error.to_string().contains("no element open"), "{error}");
}

#[test]
fn an_end_that_names_another_element_is_refused() {
  // The writer would otherwise close `a` while the events said `b`, putting out a document they do not describe.
  let mut w = XmlWriter::new(Vec::new());
  start(&mut w, "a", &[]).unwrap();
  let error = end(&mut w, "b").expect_err("the innermost element open is `a`");
  assert!(error.to_string().contains("expected </a>"), "{error}");
  assert!(error.to_string().contains("found </b>"), "{error}");
}

#[test]
fn a_doctype_inside_an_element_is_refused() {
  let mut w = XmlWriter::new(Vec::new());
  start(&mut w, "note", &[]).unwrap();
  let error = doctype(&mut w, Some("note"), None, None).expect_err("a doctype may not appear inside an element");
  assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
}

#[test]
fn what_is_not_escaped_reaches_the_output_as_it_stands() {
  // `XmlWriter` says a comment's text and a processing instruction's data are written as they were given. That
  // is a footgun worth pinning: both of these produce output that will not parse, and the writer allows it.
  let out = written(|w| {
    comment(w, "a--b")?;
    pi(w, "pi", "closes ?> early")
  });
  assert_eq!(out, "<!--a--b--><?pi closes ?> early?>");
}

#[test]
fn tracks_the_open_elements() {
  let mut w = XmlWriter::new(Vec::new());
  assert_eq!(w.open.len(), 0);
  start(&mut w, "a", &[]).unwrap();
  start(&mut w, "b", &[]).unwrap();
  assert_eq!(w.open.len(), 2);
  end(&mut w, "b").unwrap();
  assert_eq!(w.open.len(), 1);
}

#[test]
fn a_document_that_begins_drops_what_an_earlier_one_left_open() {
  let mut w = XmlWriter::new(Vec::new());
  start(&mut w, "a", &[]).unwrap();
  start(&mut w, "b", &[]).unwrap();
  assert_eq!(w.open.len(), 2);

  let _ = w.consume(&EventRef::StartDocument).unwrap();
  assert_eq!(w.open.len(), 0, "a document's beginning is a new run");
  assert!(!w.pending, "the start tag the earlier run left open is not this run's to close");

  // So the new run has nothing to end, and says so rather than closing an element of the run before it.
  let error = end(&mut w, "b").expect_err("nothing is open in this document");
  assert!(error.to_string().contains("no element open"), "{error}");
}

#[test]
fn the_declaration_is_written_when_the_document_starts() {
  let out = written_by(XmlWriter::new(Vec::new()).with_declaration(true).with_standalone(Some(false)), |w| {
    let _ = w.consume(&EventRef::StartDocument)?;
    start(w, "a", &[])?;
    end(w, "a")
  });
  assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?><a/>");
}

#[test]
fn what_follows_the_declaration_is_the_line_break_set_for_it() {
  let write = |line_break: Option<LineBreak>| {
    written_by(XmlWriter::new(Vec::new()).with_declaration(true).with_declaration_line_break(line_break), |w| {
      let _ = w.consume(&EventRef::StartDocument)?;
      start(w, "a", &[])?;
      end(w, "a")
    })
  };
  let declaration = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
  for (line_break, between) in
    [(LineBreak::Cr, "\r"), (LineBreak::CrLf, "\r\n"), (LineBreak::Lf, "\n"), (LineBreak::Space, " ")]
  {
    assert_eq!(line_break.as_str(), between);
    assert_eq!(write(Some(line_break)), format!("{declaration}{between}<a/>"), "{line_break:?}");
  }
  // `None` writes nothing, and it is also what the writer does unless asked, whatever the enum's own default is.
  assert_eq!(write(None), format!("{declaration}<a/>"));
  assert_eq!(LineBreak::default(), LineBreak::Lf);
  let unset = written_by(XmlWriter::new(Vec::new()).with_declaration(true), |w| {
    let _ = w.consume(&EventRef::StartDocument)?;
    start(w, "a", &[])?;
    end(w, "a")
  });
  assert_eq!(unset, format!("{declaration}<a/>"));

  // Without a declaration there is nothing for it to follow, so nothing is written.
  let out = written_by(XmlWriter::new(Vec::new()).with_declaration_line_break(Some(LineBreak::CrLf)), |w| {
    let _ = w.consume(&EventRef::StartDocument)?;
    start(w, "a", &[])?;
    end(w, "a")
  });
  assert_eq!(out, "<a/>");
}
