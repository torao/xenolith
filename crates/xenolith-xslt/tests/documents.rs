//! `document()` and result tree fragments as trees (XSLT 1.0 §12.1, §11.1).

use std::collections::HashMap;
use std::rc::Rc;

use xenolith_core::error::{Error, Result};
use xenolith_dom::build;
use xenolith_serialize::Serializer;
use xenolith_xdm::{Documents, DomModel};
use xenolith_xpath::Functions;
use xenolith_xslt::{LoadedDocuments, Loader, Stylesheet, Transform, transform};

/// Wraps top-level content in an `xsl:stylesheet`.
fn sheet(body: &str) -> String {
  format!("<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>")
}

/// A loader serving a fixed set of documents by URI.
struct Shelf(HashMap<String, String>);

impl Shelf {
  fn new(entries: &[(&str, &str)]) -> Self {
    Self(entries.iter().map(|(uri, xml)| ((*uri).to_owned(), (*xml).to_owned())).collect())
  }
}

impl Loader for Shelf {
  fn load(&mut self, uri: &str) -> Result<Vec<u8>> {
    match self.0.get(uri) {
      Some(xml) => Ok(xml.as_bytes().to_vec()),
      None => Err(Error::xslt(format!("nothing is shelved at {uri:?}"))),
    }
  }
}

/// Transforms `xml`, with `shelf` available to `document()`.
fn run_with_shelf(body: &str, xml: &str, shelf: &[(&str, &str)]) -> Result<String> {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///dir/s.xsl")?;
  let source = build::parse(xml.as_bytes())?;
  let documents = Documents::new();
  let model = DomModel::with_documents(&source, &documents);
  let available = Rc::new(LoadedDocuments::new(&documents, Shelf::new(shelf)));
  let result =
    Transform::new().run_with_documents(&stylesheet, &model, model.root_node(), Functions::new(), available)?;
  Ok(result.text())
}

/// Transforms `xml` with no document source at all, and serializes the result.
fn run(body: &str, xml: &str) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  Serializer::new().to_string(result.document(), result.root())
}

// --- document() ---------------------------------------------------------------------------------

#[test]
fn document_brings_in_another_tree() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"document('other.xml')//name\"/></xsl:template>";
  let shelf = [("file:///dir/other.xml", "<r><name>Ada</name></r>")];
  assert_eq!(run_with_shelf(body, "<a/>", &shelf).expect("transforms"), "Ada");
}

#[test]
fn a_relative_uri_is_resolved_against_the_stylesheet() {
  // The stylesheet is at file:///dir/s.xsl, so `sub/x.xml` is below that directory.
  let body = "<xsl:template match='/'><xsl:value-of select=\"document('sub/x.xml')/r\"/></xsl:template>";
  let shelf = [("file:///dir/sub/x.xml", "<r>found</r>")];
  assert_eq!(run_with_shelf(body, "<a/>", &shelf).expect("transforms"), "found");
}

#[test]
fn the_nodes_of_a_fetched_document_can_be_walked_and_matched() {
  let body = "<xsl:template match='/'><xsl:apply-templates select=\"document('other.xml')//item\"/></xsl:template>\
              <xsl:template match='item'>[<xsl:value-of select='.'/>]</xsl:template>";
  let shelf = [("file:///dir/other.xml", "<r><item>one</item><item>two</item></r>")];
  assert_eq!(run_with_shelf(body, "<a/>", &shelf).expect("transforms"), "[one][two]");
}

#[test]
fn a_fetched_document_has_a_root_of_its_own() {
  // `/` inside a template applied to a fetched node means that document's root, not the source's.
  let body = "<xsl:template match='/'><xsl:apply-templates select=\"document('other.xml')/r\"/></xsl:template>\
              <xsl:template match='r'><xsl:value-of select='name(/*)'/></xsl:template>";
  let shelf = [("file:///dir/other.xml", "<r/>")];
  assert_eq!(run_with_shelf(body, "<source/>", &shelf).expect("transforms"), "r");
}

#[test]
fn one_uri_gives_one_tree_however_often_it_is_asked_for() {
  // §12.1: two calls naming the same URI give the same node, so the union of them is one node.
  let body = "<xsl:template match='/'>\
              <xsl:value-of select=\"count(document('a.xml') | document('a.xml'))\"/></xsl:template>";
  let shelf = [("file:///dir/a.xml", "<r/>")];
  assert_eq!(run_with_shelf(body, "<a/>", &shelf).expect("transforms"), "1");
}

#[test]
fn a_node_set_argument_names_one_uri_per_node() {
  let body = "<xsl:template match='/'><xsl:for-each select=\"document(//want)\">\
              <xsl:value-of select='/r'/>;</xsl:for-each></xsl:template>";
  let shelf = [("file:///dir/one.xml", "<r>first</r>"), ("file:///dir/two.xml", "<r>second</r>")];
  let xml = "<a><want>one.xml</want><want>two.xml</want></a>";
  assert_eq!(run_with_shelf(body, xml, &shelf).expect("transforms"), "first;second;");
}

#[test]
fn two_documents_are_ordered_one_after_the_other() {
  // XPath 1.0 §5 leaves the order between documents to the implementation but asks that it be
  // consistent; a whole document sits before or after another rather than interleaving.
  let body = "<xsl:template match='/'><xsl:for-each select=\"document(//want)//item\">\
              <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>";
  let shelf = [
    ("file:///dir/one.xml", "<r><item>1a</item><item>1b</item></r>"),
    ("file:///dir/two.xml", "<r><item>2a</item></r>"),
  ];
  let xml = "<a><want>one.xml</want><want>two.xml</want></a>";
  assert_eq!(run_with_shelf(body, xml, &shelf).expect("transforms"), "1a;1b;2a;");
}

#[test]
fn a_document_that_cannot_be_served_is_reported() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"document('missing.xml')\"/></xsl:template>";
  let error = run_with_shelf(body, "<a/>", &[]).expect_err("nothing is shelved");
  assert!(error.message().contains("missing.xml"), "{}", error.message());
}

#[test]
fn without_a_document_source_document_finds_nothing() {
  // The default: fetching is I/O on the caller's behalf, so it is not done unless asked for.
  let body = "<xsl:template match='/'><xsl:value-of select=\"count(document('other.xml'))\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "0");
}

#[test]
fn function_available_says_document_is_there() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"function-available('document')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "true");
}

// --- Result tree fragments ----------------------------------------------------------------------

#[test]
fn copy_of_a_fragment_copies_the_tree_and_not_the_text() {
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><b k='v'>text</b></xsl:variable>\
                <out><xsl:copy-of select='$frag'/></out>\
              </xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out><b k=\"v\">text</b></out>");
}

#[test]
fn a_fragment_is_still_its_string_everywhere_else() {
  // §11.1 allows exactly two uses; anywhere but xsl:copy-of, a fragment is its string value.
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><b>text</b></xsl:variable>\
                <xsl:value-of select='$frag'/>|<xsl:value-of select='string-length($frag)'/>\
              </xsl:template>";
  assert_eq!(run(body, "<a/>"), "text|4");
}

#[test]
fn a_fragment_with_several_nodes_is_copied_whole() {
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><b/>between<c/></xsl:variable>\
                <out><xsl:copy-of select='$frag'/></out>\
              </xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out><b/>between<c/></out>");
}

#[test]
fn a_fragment_may_be_copied_more_than_once() {
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><b/></xsl:variable>\
                <out><xsl:copy-of select='$frag'/><xsl:copy-of select='$frag'/></out>\
              </xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out><b/><b/></out>");
}

#[test]
fn a_fragment_reaches_a_template_through_a_parameter() {
  let body = "<xsl:template match='/'>\
                <xsl:call-template name='wrap'>\
                  <xsl:with-param name='content'><b>inner</b></xsl:with-param>\
                </xsl:call-template>\
              </xsl:template>\
              <xsl:template name='wrap'><xsl:param name='content'/>\
                <out><xsl:copy-of select='$content'/></out></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out><b>inner</b></out>");
}

#[test]
fn a_variable_with_a_select_is_not_a_fragment() {
  // Its value is a node-set of the source, which xsl:copy-of copies from the source model.
  let body = "<xsl:template match='/'>\
                <xsl:variable name='chosen' select='//b'/>\
                <out><xsl:copy-of select='$chosen'/></out>\
              </xsl:template>";
  assert_eq!(run(body, "<a><b k='v'>text</b></a>"), "<out><b k=\"v\">text</b></out>");
}
