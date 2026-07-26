//! `xsl:key` and `key()` (XSLT 1.0 §12.2).

use xylograph_dom::build;
use xylograph_xdm::DomModel;
use xylograph_xslt::{Stylesheet, transform};

/// Wraps top-level content in an `xsl:stylesheet`.
fn sheet(body: &str) -> String {
  format!("<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>")
}

/// Transforms `xml` and takes the text of the result.
fn run(body: &str, xml: &str) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect("transforms").text()
}

/// The message a compilation fails with.
fn compile_error(body: &str) -> String {
  Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect_err("fails").message().to_owned()
}

/// A small catalogue used by most of these.
const CATALOGUE: &str = "<catalogue>\
    <item code='a' kind='fruit'>apple</item>\
    <item code='b' kind='fruit'>banana</item>\
    <item code='c' kind='veg'>carrot</item>\
  </catalogue>";

#[test]
fn a_key_finds_the_nodes_a_value_leads_to() {
  let body = "<xsl:key name='by-code' match='item' use='@code'/>\
              <xsl:template match='/'><xsl:value-of select=\"key('by-code', 'b')\"/></xsl:template>";
  assert_eq!(run(body, CATALOGUE), "banana");
}

#[test]
fn several_nodes_may_share_a_value() {
  let body = "<xsl:key name='by-kind' match='item' use='@kind'/>\
              <xsl:template match='/'><xsl:for-each select=\"key('by-kind', 'fruit')\">\
                <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>";
  assert_eq!(run(body, CATALOGUE), "apple;banana;");
}

#[test]
fn a_value_that_leads_nowhere_finds_nothing() {
  let body = "<xsl:key name='by-code' match='item' use='@code'/>\
              <xsl:template match='/'><xsl:value-of select=\"count(key('by-code', 'zz'))\"/></xsl:template>";
  assert_eq!(run(body, CATALOGUE), "0");
}

#[test]
fn a_key_may_cover_nodes_that_are_not_elements() {
  // §12.2 puts no restriction on the kind of node a key's match may select.
  let body = "<xsl:key name='by-self' match='@code' use='.'/>\
              <xsl:template match='/'><xsl:value-of select=\"count(key('by-self', 'a'))\"/>\
              <xsl:value-of select=\"name(key('by-self', 'a'))\"/></xsl:template>";
  assert_eq!(run(body, CATALOGUE), "1code");
}

#[test]
fn a_use_that_gives_a_node_set_files_the_node_under_each_value() {
  // Each of the values in the node-set leads to the node, so one node has several ways in.
  let body = "<xsl:key name='by-tag' match='item' use='tag'/>\
              <xsl:template match='/'><xsl:value-of select=\"key('by-tag', 'red')/@id\"/>\
              <xsl:value-of select=\"key('by-tag', 'round')/@id\"/></xsl:template>";
  let xml = "<r><item id='1'><tag>red</tag><tag>round</tag></item></r>";
  assert_eq!(run(body, xml), "11");
}

#[test]
fn a_node_set_argument_makes_the_lookup_a_join() {
  // §12.2: with a node-set, the result is the union over each of its nodes' string values.
  let body = "<xsl:key name='by-code' match='item' use='@code'/>\
              <xsl:template match='/'><xsl:for-each select=\"key('by-code', //want)\">\
                <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>";
  let xml = "<r><catalogue><item code='a'>apple</item><item code='b'>banana</item>\
             <item code='c'>carrot</item></catalogue><want>a</want><want>c</want></r>";
  assert_eq!(run(body, xml), "apple;carrot;");
}

#[test]
fn a_key_result_is_in_document_order_and_holds_each_node_once() {
  let body = "<xsl:key name='by-kind' match='item' use='@kind'/>\
              <xsl:template match='/'><xsl:for-each select=\"key('by-kind', //want)\">\
                <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>";
  // Both values reach the same fruit items, and the wants are in the reverse of document order.
  let xml = "<r><catalogue><item kind='fruit'>apple</item><item kind='veg'>carrot</item>\
             <item kind='fruit'>banana</item></catalogue><want>veg</want><want>fruit</want></r>";
  assert_eq!(run(body, xml), "apple;carrot;banana;");
}

#[test]
fn every_declaration_of_a_name_contributes() {
  // §12.2 has the declarations of a name add their entries together rather than one winning.
  let body = "<xsl:key name='both' match='item' use='@code'/>\
              <xsl:key name='both' match='item' use='@kind'/>\
              <xsl:template match='/'><xsl:value-of select=\"count(key('both', 'fruit'))\"/>\
              <xsl:value-of select=\"count(key('both', 'c'))\"/></xsl:template>";
  assert_eq!(run(body, CATALOGUE), "21");
}

#[test]
fn a_key_name_may_be_in_a_namespace() {
  // The prefix is resolved where each is written, so the two need not agree on the spelling.
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:k='urn:k'>\
                  <xsl:key name='k:by-code' match='item' use='@code'/>\
                  <xsl:template match='/'>\
                    <xsl:for-each select='.' xmlns:other='urn:k'>\
                      <xsl:value-of select=\"key('other:by-code', 'b')\"/>\
                    </xsl:for-each>\
                  </xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(CATALOGUE.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  assert_eq!(result.text(), "banana");
}

#[test]
fn a_pattern_may_be_anchored_at_a_key() {
  // §5.2 lets a pattern begin with key(); until the tables existed this matched nothing at all.
  let body = "<xsl:key name='by-code' match='item' use='@code'/>\
              <xsl:template match=\"key('by-code', 'b')\">[<xsl:value-of select='.'/>]</xsl:template>\
              <xsl:template match='item'><xsl:value-of select='.'/></xsl:template>\
              <xsl:template match='/'><xsl:apply-templates select='//item'/></xsl:template>";
  assert_eq!(run(body, CATALOGUE), "apple[banana]carrot");
}

#[test]
fn a_pattern_anchored_at_a_key_may_have_steps_below_it() {
  let body = "<xsl:key name='by-code' match='item' use='@code'/>\
              <xsl:template match=\"key('by-code', 'b')/name\">[<xsl:value-of select='.'/>]</xsl:template>\
              <xsl:template match='name'><xsl:value-of select='.'/></xsl:template>\
              <xsl:template match='/'><xsl:apply-templates select='//name'/></xsl:template>";
  let xml = "<r><item code='a'><name>apple</name></item><item code='b'><name>banana</name></item></r>";
  assert_eq!(run(body, xml), "apple[banana]");
}

#[test]
fn a_key_may_be_used_before_it_is_declared_in_the_stylesheet() {
  // The tables are built before the walk starts, so declaration order does not matter.
  let body = "<xsl:template match='/'><xsl:value-of select=\"key('by-code', 'a')\"/></xsl:template>\
              <xsl:key name='by-code' match='item' use='@code'/>";
  assert_eq!(run(body, CATALOGUE), "apple");
}

#[test]
fn function_available_now_says_key_is_there() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"function-available('key')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "true");
}

#[test]
fn what_a_key_declaration_cannot_do_without_is_reported() {
  assert!(compile_error("<xsl:key match='item' use='@code'/>").contains("needs a name"));
  assert!(compile_error("<xsl:key name='k' use='@code'/>").contains("needs a match"));
  assert!(compile_error("<xsl:key name='k' match='item'/>").contains("needs a use"));
  assert!(compile_error("<xsl:key name='no:such' match='item' use='@code'/>").contains("not bound"));
}
