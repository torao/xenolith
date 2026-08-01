//! `exsl:document`: a result other than the principal one.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use xylogue_core::ErrorKind;
use xylogue_dom::build;
use xylogue_xdm::DomModel;
use xylogue_xslt::{ResultSink, Stylesheet, Transform, transform};

/// A sink that keeps what it was given, so a test can look at it.
#[derive(Default)]
struct Collected(BTreeMap<String, String>);

impl ResultSink for Collected {
  fn write(&mut self, href: &str, bytes: &[u8]) -> xylogue_core::Result<()> {
    let text = String::from_utf8(bytes.to_vec()).expect("what was written to be UTF-8");
    self.0.insert(href.to_owned(), text);
    Ok(())
  }
}

/// A sink that refuses, to check that a refusal stops the transformation.
struct Refuses;

impl ResultSink for Refuses {
  fn write(&mut self, _href: &str, _bytes: &[u8]) -> xylogue_core::Result<()> {
    Err(xylogue_core::Error::new(ErrorKind::Io, "this sink writes nothing".to_owned()))
  }
}

/// Wraps a body in a stylesheet that has bound `exsl` as an extension element prefix.
fn sheet(body: &str) -> String {
  format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
       xmlns:exsl=\"http://exslt.org/common\" extension-element-prefixes=\"exsl\">{body}</xsl:stylesheet>"
  )
}

/// Runs a stylesheet with a collecting sink, giving the principal result and the secondary ones.
fn run(body: &str, xml: &str) -> (String, BTreeMap<String, String>) {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///dir/s.xsl").expect("compiles");
  let document = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&document);
  let sink = Rc::new(RefCell::new(Collected::default()));
  let result = Transform::new()
    .with_results(Rc::clone(&sink) as Rc<RefCell<dyn ResultSink>>)
    .run(&stylesheet, &model, model.root_node())
    .expect("transforms");
  let written = sink.borrow().0.clone();
  (result.serialize(), written)
}

/// The message a transformation fails with.
fn error(body: &str, xml: &str, sink: Option<Rc<RefCell<dyn ResultSink>>>) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///dir/s.xsl").expect("compiles");
  let document = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&document);
  let mut run = Transform::new();
  if let Some(sink) = sink {
    run = run.with_results(sink);
  }
  run.run(&stylesheet, &model, model.root_node()).expect_err("fails").message().to_owned()
}

#[test]
fn a_secondary_result_goes_to_the_sink_and_not_into_the_principal_one() {
  let body = "<xsl:template match=\"/\">principal\
              <exsl:document href=\"out.xml\"><side><xsl:value-of select=\"//a\"/></side></exsl:document>\
              </xsl:template>";
  let (principal, written) = run(body, "<r><a>text</a></r>");

  // Nothing the element built appears in the main output — that is the point of it.
  assert_eq!(principal, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>principal");
  assert_eq!(written.len(), 1);
  // The href is resolved against the stylesheet's base URI, so a sink is given something
  // absolute rather than something that depends on the working directory.
  let (href, content) = written.iter().next().expect("one file");
  assert_eq!(href, "file:///dir/out.xml");
  assert_eq!(content, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><side>text</side>");
}

#[test]
fn the_href_is_an_attribute_value_template() {
  let body = "<xsl:template match=\"/\"><xsl:for-each select=\"//a\">\
              <exsl:document href=\"{@n}.xml\"><p><xsl:value-of select=\".\"/></p></exsl:document>\
              </xsl:for-each></xsl:template>";
  let (_, written) = run(body, "<r><a n='one'>1</a><a n='two'>2</a></r>");

  assert_eq!(written.keys().collect::<Vec<_>>(), ["file:///dir/one.xml", "file:///dir/two.xml"]);
  assert!(written["file:///dir/one.xml"].ends_with("<p>1</p>"), "{written:?}");
}

#[test]
fn it_takes_the_output_attributes_xsl_output_takes() {
  let body = "<xsl:output encoding=\"UTF-8\" indent=\"no\"/>\
              <xsl:template match=\"/\">\
              <exsl:document href=\"t.txt\" method=\"text\" omit-xml-declaration=\"yes\">\
              <p>text <b>and more</b></p></exsl:document></xsl:template>";
  let (_, written) = run(body, "<r/>");

  // The text method writes the characters and no markup, and this said so for itself.
  assert_eq!(written["file:///dir/t.txt"], "text and more");
}

#[test]
fn what_the_stylesheet_asked_for_is_inherited_unless_it_says_otherwise() {
  // The principal xsl:output says omit the declaration; the secondary result does not override
  // it, so it inherits — a stylesheet should not have to repeat itself per file.
  let body = "<xsl:output omit-xml-declaration=\"yes\"/>\
              <xsl:template match=\"/\"><exsl:document href=\"a.xml\"><a/></exsl:document></xsl:template>";
  let (_, written) = run(body, "<r/>");
  assert_eq!(written["file:///dir/a.xml"], "<a/>");
}

#[test]
fn with_no_sink_it_is_refused_by_name() {
  // Writing to a path a stylesheet chose, with no one having said it may, is not a default this
  // library takes — and neither is silence, which would look like the file had been written.
  let body = "<xsl:template match=\"/\"><exsl:document href=\"out.xml\"><a/></exsl:document></xsl:template>";
  let message = error(body, "<r/>", None);
  assert!(message.contains("out.xml"), "{message}");
  assert!(message.contains("with_results"), "{message}");
}

#[test]
fn a_sink_that_refuses_stops_the_transformation() {
  let body = "<xsl:template match=\"/\"><exsl:document href=\"out.xml\"><a/></exsl:document></xsl:template>";
  let sink: Rc<RefCell<dyn ResultSink>> = Rc::new(RefCell::new(Refuses));
  assert!(error(body, "<r/>", Some(sink)).contains("writes nothing"));
}

#[test]
fn a_stylesheet_can_ask_whether_it_is_there() {
  // §15: ask before relying on it. The answer has to be about this implementation, so it says
  // yes for the one extension element there is and no for one there is not.
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"element-available('exsl:document')\"/>\
              <xsl:text>,</xsl:text>\
              <xsl:value-of select=\"element-available('exsl:invented')\"/></xsl:template>";
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///dir/s.xsl").expect("compiles");
  let document = build::parse("<r/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&document);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  assert_eq!(result.text(), "true,false");
}

#[test]
fn without_the_extension_prefix_it_is_a_literal_result_element() {
  // §14: an element is an extension element only where its prefix is declared as one. Without
  // that declaration this is ordinary output, and treating it as an instruction would change
  // what a stylesheet that never asked for EXSLT does.
  let body = "<xsl:template match=\"/\"><exsl:document href=\"out.xml\"><a/></exsl:document></xsl:template>";
  let plain = format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
       xmlns:exsl=\"http://exslt.org/common\">{body}</xsl:stylesheet>"
  );
  let stylesheet = Stylesheet::compile(plain.as_bytes(), "file:///dir/s.xsl").expect("compiles");
  let document = build::parse("<r/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&document);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  assert!(result.serialize().contains("document"), "{}", result.serialize());
}
