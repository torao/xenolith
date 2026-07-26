//! `xsl:sort` (XSLT 1.0 §10).

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

/// The message a transformation fails with.
fn error(body: &str, xml: &str) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect_err("fails").message().to_owned()
}

/// `xsl:for-each` over `//i`, sorted as the given `xsl:sort` elements say.
fn for_each(sorts: &str) -> String {
  format!(
    "<xsl:template match='/'><xsl:for-each select='//i'>{sorts}\
     <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>"
  )
}

#[test]
fn without_a_sort_the_nodes_stay_in_document_order() {
  assert_eq!(run(&for_each(""), "<r><i>c</i><i>a</i><i>b</i></r>"), "c;a;b;");
}

#[test]
fn a_sort_orders_by_its_key() {
  let body = for_each("<xsl:sort select='.'/>");
  assert_eq!(run(&body, "<r><i>c</i><i>a</i><i>b</i></r>"), "a;b;c;");
}

#[test]
fn a_sort_without_a_select_takes_the_node_itself() {
  let body = for_each("<xsl:sort/>");
  assert_eq!(run(&body, "<r><i>c</i><i>a</i><i>b</i></r>"), "a;b;c;");
}

#[test]
fn descending_turns_the_order_round() {
  let body = for_each("<xsl:sort select='.' order='descending'/>");
  assert_eq!(run(&body, "<r><i>c</i><i>a</i><i>b</i></r>"), "c;b;a;");
}

#[test]
fn a_text_sort_is_not_a_number_sort() {
  // The whole point of data-type: as text, "10" comes before "9".
  let text = for_each("<xsl:sort select='.'/>");
  assert_eq!(run(&text, "<r><i>10</i><i>9</i><i>100</i></r>"), "10;100;9;");

  let number = for_each("<xsl:sort select='.' data-type='number'/>");
  assert_eq!(run(&number, "<r><i>10</i><i>9</i><i>100</i></r>"), "9;10;100;");
}

#[test]
fn a_key_that_is_not_a_number_sorts_before_the_numbers() {
  // §10: NaN comes first, which is what a key that cannot be read as a number becomes.
  let body = for_each("<xsl:sort select='.' data-type='number'/>");
  assert_eq!(run(&body, "<r><i>2</i><i>oops</i><i>1</i></r>"), "oops;1;2;");
}

#[test]
fn several_sorts_run_major_to_minor() {
  let body = "<xsl:template match='/'><xsl:for-each select='//i'>\
                <xsl:sort select='@group'/><xsl:sort select='.'/>\
                <xsl:value-of select='@group'/><xsl:value-of select='.'/>;\
              </xsl:for-each></xsl:template>";
  let xml = "<r><i group='b'>2</i><i group='a'>2</i><i group='b'>1</i><i group='a'>1</i></r>";
  assert_eq!(run(body, xml), "a1;a2;b1;b2;");
}

#[test]
fn the_sort_is_stable() {
  // §10 requires it: keys that compare equal keep the order they were selected in.
  let body = "<xsl:template match='/'><xsl:for-each select='//i'>\
                <xsl:sort select='@group'/>\
                <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>";
  let xml = "<r><i group='a'>first</i><i group='b'>x</i><i group='a'>second</i><i group='a'>third</i></r>";
  assert_eq!(run(body, xml), "first;second;third;x;");
}

#[test]
fn a_sort_key_is_evaluated_against_the_list_as_selected() {
  // position() in a sort key is the node's place before anything moves, not after.
  let body = for_each("<xsl:sort select='position()' data-type='number' order='descending'/>");
  assert_eq!(run(&body, "<r><i>a</i><i>b</i><i>c</i></r>"), "c;b;a;");
}

#[test]
fn apply_templates_sorts_too() {
  let body = "<xsl:template match='/'><xsl:apply-templates select='//i'>\
                <xsl:sort select='.'/></xsl:apply-templates></xsl:template>\
              <xsl:template match='i'><xsl:value-of select='.'/>;</xsl:template>";
  assert_eq!(run(body, "<r><i>c</i><i>a</i><i>b</i></r>"), "a;b;c;");
}

#[test]
fn position_inside_the_body_follows_the_sorted_order() {
  // §10: after sorting, the current node list is the sorted one, so position() counts in it.
  let body = "<xsl:template match='/'><xsl:for-each select='//i'>\
                <xsl:sort select='.'/>\
                <xsl:value-of select='position()'/><xsl:value-of select='.'/>;\
              </xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<r><i>c</i><i>a</i><i>b</i></r>"), "1a;2b;3c;");
}

#[test]
fn case_order_decides_between_keys_that_are_otherwise_equal() {
  let upper = for_each("<xsl:sort select='.' case-order='upper-first'/>");
  assert_eq!(run(&upper, "<r><i>a</i><i>A</i></r>"), "A;a;");

  let lower = for_each("<xsl:sort select='.' case-order='lower-first'/>");
  assert_eq!(run(&lower, "<r><i>A</i><i>a</i></r>"), "a;A;");
}

#[test]
fn the_attributes_of_a_sort_are_attribute_value_templates() {
  let body = "<xsl:template match='/'><xsl:for-each select='//i'>\
                <xsl:sort select='.' order='{//@how}'/>\
                <xsl:value-of select='.'/>;</xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<r how='descending'><i>a</i><i>c</i><i>b</i></r>"), "c;b;a;");
}

#[test]
fn what_a_sort_cannot_make_sense_of_is_reported() {
  let data_type = for_each("<xsl:sort select='.' data-type='colour'/>");
  assert!(error(&data_type, "<r><i>a</i><i>b</i></r>").contains("data-type"), "unknown data-type");

  let order = for_each("<xsl:sort select='.' order='sideways'/>");
  assert!(error(&order, "<r><i>a</i><i>b</i></r>").contains("ascending"), "unknown order");

  let case = for_each("<xsl:sort select='.' case-order='middle-first'/>");
  assert!(error(&case, "<r><i>a</i><i>b</i></r>").contains("upper-first"), "unknown case-order");
}

#[test]
fn a_language_that_cannot_be_read_still_sorts() {
  // §10 does not make an unknown language an error; the collation falls back instead.
  let body = for_each("<xsl:sort select='.' lang='not a tag at all'/>");
  assert_eq!(run(&body, "<r><i>c</i><i>a</i><i>b</i></r>"), "a;b;c;");
}

#[test]
fn element_available_says_sort_is_there() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"element-available('xsl:sort')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "true");
}

#[cfg(feature = "icu")]
#[test]
fn a_language_puts_its_own_letters_where_it_expects_them() {
  // The reason for the ICU dependency: in Swedish, ä comes after z; in German it sorts beside a.
  // Code-point order would put it after z in both, so this is the difference the data makes.
  let swedish = for_each("<xsl:sort select='.' lang='sv'/>");
  assert_eq!(run(&swedish, "<r><i>\u{e4}</i><i>z</i><i>a</i></r>"), "a;z;\u{e4};");

  let german = for_each("<xsl:sort select='.' lang='de'/>");
  assert_eq!(run(&german, "<r><i>\u{e4}</i><i>z</i><i>a</i></r>"), "a;\u{e4};z;");
}
