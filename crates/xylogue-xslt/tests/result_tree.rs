//! The instructions that build result nodes: element, attribute, comment, PI, copy, copy-of.

use xylogue_core::ErrorKind;
use xylogue_dom::build;
use xylogue_serialize::Serializer;
use xylogue_xdm::DomModel;
use xylogue_xslt::{ResultTree, Stylesheet, transform};

/// Wraps top-level content in an `xsl:stylesheet`.
fn sheet(body: &str) -> String {
  format!("<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>")
}

/// Transforms `xml` and serializes what comes out.
fn run(body: &str, xml: &str) -> String {
  serialized(&result(body, xml))
}

fn serialized(result: &ResultTree) -> String {
  Serializer::new().to_string(result.document(), result.root())
}

fn result(body: &str, xml: &str) -> ResultTree {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect("transforms")
}

/// The message a transformation fails with.
fn error(body: &str, xml: &str) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let error = transform(&stylesheet, &model, model.root_node()).expect_err("fails");
  assert_eq!(error.kind(), ErrorKind::Xslt, "{}", error.message());
  error.message().to_owned()
}

#[test]
fn element_builds_an_element_whose_name_is_worked_out() {
  let body = "<xsl:template match=\"/\"><xsl:element name=\"out\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out/>");
  // The name is an attribute value template, so it can come from the source.
  let named = "<xsl:template match=\"/\"><xsl:element name=\"{//a/@n}\">t</xsl:element></xsl:template>";
  assert_eq!(run(named, "<r><a n='chosen'/></r>"), "<chosen>t</chosen>");
}

#[test]
fn element_may_be_put_in_a_namespace() {
  let body = "<xsl:template match=\"/\"><xsl:element name=\"o:out\" namespace=\"urn:o\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<o:out xmlns:o=\"urn:o\"/>");
}

#[test]
fn a_prefix_with_no_namespace_attribute_means_what_the_stylesheet_says() {
  // §7.1.2: with no `namespace`, the prefix of the name means what it means where the
  // instruction was written. The result tree has no declarations of its own to look in.
  let body = "<xsl:template match=\"/\" xmlns:p=\"urn:p\"><xsl:element name=\"p:out\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<p:out xmlns:p=\"urn:p\"/>");

  // §7.1.3 says the same of an attribute.
  let attribute = "<xsl:template match=\"/\" xmlns:p=\"urn:p\"><out><xsl:attribute name=\"p:k\">v</xsl:attribute>\
                   </out></xsl:template>";
  assert_eq!(run(attribute, "<a/>"), "<out p:k=\"v\" xmlns:p=\"urn:p\"/>");
}

#[test]
fn an_unprefixed_name_takes_the_default_namespace_for_an_element_only() {
  // §7.1.2 expands an xsl:element name "including any default namespace declaration"; §7.1.3
  // says of xsl:attribute "not including" it. The difference is Namespaces' own rule that an
  // unprefixed attribute is in no namespace whatever is declared.
  let element = "<xsl:template match=\"/\"><xsl:element name=\"out\" xmlns=\"urn:d\"/></xsl:template>";
  assert_eq!(run(element, "<a/>"), "<out xmlns=\"urn:d\"/>");

  let attribute = "<xsl:template match=\"/\" xmlns=\"urn:d\"><xsl:element name=\"out\">\
                   <xsl:attribute name=\"k\">v</xsl:attribute></xsl:element></xsl:template>";
  assert_eq!(run(attribute, "<a/>"), "<out k=\"v\" xmlns=\"urn:d\"/>");
}

#[test]
fn an_empty_namespace_means_no_namespace() {
  // Not "the namespace whose URI is the empty string": §7.1.2 says the name is in none.
  let body = "<xsl:template match=\"/\" xmlns:p=\"urn:p\"><xsl:element name=\"out\" namespace=\"\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out/>");
}

#[test]
fn a_prefix_the_stylesheet_never_bound_is_refused() {
  // Building the name anyway would put a prefix into the result that means nothing there.
  let body = "<xsl:template match=\"/\"><xsl:element name=\"nowhere:out\"/></xsl:template>";
  assert!(error(body, "<a/>").contains("nowhere"), "{}", error(body, "<a/>"));
}

#[test]
fn attribute_is_added_to_the_element_being_built() {
  let body = "<xsl:template match=\"/\"><out><xsl:attribute name=\"k\">v</xsl:attribute></out></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out k=\"v\"/>");
  // Its value is whatever its content produces, not only literal text.
  let computed = "<xsl:template match=\"/\"><out>\
                  <xsl:attribute name=\"n\"><xsl:value-of select=\"count(//b)\"/></xsl:attribute></out></xsl:template>";
  assert_eq!(run(computed, "<a><b/><b/></a>"), "<out n=\"2\"/>");
}

#[test]
fn an_attribute_with_no_element_open_is_refused() {
  let body = "<xsl:template match=\"/\"><xsl:attribute name=\"k\">v</xsl:attribute></xsl:template>";
  assert!(error(body, "<a/>").contains("no element"), "{}", error(body, "<a/>"));
}

#[test]
fn comments_and_processing_instructions_are_built() {
  let body = "<xsl:template match=\"/\"><out>\
              <xsl:comment>note</xsl:comment>\
              <xsl:processing-instruction name=\"pi\">data</xsl:processing-instruction></out></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out><!--note--><?pi data?></out>");
}

#[test]
fn copy_takes_the_node_but_not_what_is_under_it() {
  // The element is copied without its attributes or children; the body supplies the content.
  let body = "<xsl:template match=\"a\"><xsl:copy><xsl:apply-templates/></xsl:copy></xsl:template>\
              <xsl:template match=\"b\"><xsl:copy/></xsl:template>";
  assert_eq!(run(body, "<a k='dropped'><b>text</b></a>"), "<a><b/></a>");
}

#[test]
fn copy_of_takes_the_node_and_everything_below_it() {
  let body = "<xsl:template match=\"/\"><xsl:copy-of select=\"//a\"/></xsl:template>";
  assert_eq!(run(body, "<r><a k='kept'><b>text</b></a></r>"), "<a k=\"kept\"><b>text</b></a>");
}

#[test]
fn copy_of_something_that_is_not_a_node_set_gives_its_string() {
  let body = "<xsl:template match=\"/\"><xsl:copy-of select=\"count(//b)\"/></xsl:template>";
  assert_eq!(run(body, "<a><b/><b/></a>"), "2");
}

#[test]
fn copy_of_a_node_set_copies_every_node_in_it() {
  let body = "<xsl:template match=\"/\"><out><xsl:copy-of select=\"//b\"/></out></xsl:template>";
  assert_eq!(run(body, "<a><b>1</b><b>2</b></a>"), "<out><b>1</b><b>2</b></out>");
}

#[test]
fn copy_of_keeps_a_namespace() {
  let body = "<xsl:template match=\"/\"><xsl:copy-of select=\"//*[local-name() = 'b']\"/></xsl:template>";
  assert_eq!(run(body, "<a xmlns:p='urn:p'><p:b/></a>"), "<p:b xmlns:p=\"urn:p\"/>");
}

#[test]
fn an_attribute_set_adds_its_attributes() {
  let body = "<xsl:attribute-set name=\"common\">\
                <xsl:attribute name=\"class\">plain</xsl:attribute>\
                <xsl:attribute name=\"lang\">en</xsl:attribute>\
              </xsl:attribute-set>\
              <xsl:template match=\"/\"><out xsl:use-attribute-sets=\"common\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out class=\"plain\" lang=\"en\"/>");
}

#[test]
fn an_attribute_written_on_the_element_beats_the_set() {
  let body = "<xsl:attribute-set name=\"common\">\
                <xsl:attribute name=\"class\">from-set</xsl:attribute>\
              </xsl:attribute-set>\
              <xsl:template match=\"/\"><out xsl:use-attribute-sets=\"common\" class=\"own\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out class=\"own\"/>");
}

#[test]
fn xsl_element_may_use_an_attribute_set_too() {
  let body = "<xsl:attribute-set name=\"common\"><xsl:attribute name=\"k\">v</xsl:attribute></xsl:attribute-set>\
              <xsl:template match=\"/\"><xsl:element name=\"out\" use-attribute-sets=\"common\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out k=\"v\"/>");
}

#[test]
fn an_attribute_set_may_use_another_one() {
  let body = "<xsl:attribute-set name=\"base\"><xsl:attribute name=\"a\">base</xsl:attribute>\
                <xsl:attribute name=\"b\">base</xsl:attribute></xsl:attribute-set>\
              <xsl:attribute-set name=\"derived\" use-attribute-sets=\"base\">\
                <xsl:attribute name=\"b\">derived</xsl:attribute></xsl:attribute-set>\
              <xsl:template match=\"/\"><out xsl:use-attribute-sets=\"derived\"/></xsl:template>";
  // The used set goes on first, so the using set's own attribute is what is left standing.
  assert_eq!(run(body, "<a/>"), "<out a=\"base\" b=\"derived\"/>");
}

#[test]
fn an_attribute_set_that_uses_itself_is_refused() {
  let body = "<xsl:attribute-set name=\"one\" use-attribute-sets=\"two\"/>\
              <xsl:attribute-set name=\"two\" use-attribute-sets=\"one\"/>\
              <xsl:template match=\"/\"><out xsl:use-attribute-sets=\"one\"/></xsl:template>";
  assert!(error(body, "<a/>").contains("uses itself"), "{}", error(body, "<a/>"));
}

#[test]
fn an_attribute_set_that_does_not_exist_is_refused() {
  let body = "<xsl:template match=\"/\"><out xsl:use-attribute-sets=\"nosuch\"/></xsl:template>";
  assert!(error(body, "<a/>").contains("no attribute set"), "{}", error(body, "<a/>"));
}

#[test]
fn a_message_is_kept_beside_the_result() {
  let body = "<xsl:template match=\"b\"><xsl:message>saw <xsl:value-of select=\".\"/></xsl:message></xsl:template>";
  let result = result(body, "<a><b>1</b><b>2</b></a>");
  assert_eq!(result.messages(), ["saw 1", "saw 2"]);
  assert_eq!(serialized(&result), "", "a message is not part of the result");
}

#[test]
fn a_message_that_terminates_stops_the_transformation() {
  let body = "<xsl:template match=\"/\"><xsl:message terminate=\"yes\">no good</xsl:message></xsl:template>";
  assert!(error(body, "<a/>").contains("no good"), "{}", error(body, "<a/>"));
}

#[test]
fn the_identity_transformation_gives_the_document_back() {
  // The classic: copy every node, and let apply-templates walk into what is under it.
  let body = "<xsl:template match=\"@*|node()\">\
                <xsl:copy><xsl:apply-templates select=\"@*|node()\"/></xsl:copy>\
              </xsl:template>";
  let source = "<r k=\"v\"><a>one</a><!--c--><b/></r>";
  assert_eq!(run(body, source), source);
}
