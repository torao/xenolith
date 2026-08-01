//! `xsl:number` (XSLT 1.0 §7.7).

use xylogue_dom::build;
use xylogue_xdm::DomModel;
use xylogue_xslt::{Stylesheet, transform};

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

/// Numbers every `item` in the document with the given `xsl:number`, separated by spaces.
fn number_items(attributes: &str, xml: &str) -> String {
  let body = format!(
    "<xsl:template match='/'><xsl:for-each select='//item'>\
       <xsl:number {attributes}/><xsl:text> </xsl:text></xsl:for-each></xsl:template>"
  );
  run(&body, xml).trim_end().to_owned()
}

#[test]
fn value_says_the_number_outright() {
  assert_eq!(run("<xsl:template match='/'><xsl:number value='42'/></xsl:template>", "<a/>"), "42");
  // An expression, not just a literal — and it is rounded, as §7.7 asks.
  assert_eq!(run("<xsl:template match='/'><xsl:number value='count(//a) + 0.6'/></xsl:template>", "<r><a/></r>"), "2");
}

#[test]
fn a_number_that_comes_to_nothing_writes_nothing() {
  // level="multiple" with nothing the count pattern matches among the ancestors: there is no
  // number, so the format's punctuation must not be written either. A stray ". " in the result
  // would say a number had been worked out.
  let body = "<xsl:template match='title'><xsl:number level='multiple' count='chapter' format='1.1. '/>\
              <xsl:value-of select='.'/></xsl:template>";
  assert_eq!(run(body, "<doc><title>Preface</title></doc>"), "Preface");
  // With one, both the number and the punctuation around it are written.
  assert_eq!(run(body, "<doc><chapter><title>First</title></chapter></doc>"), "1. First");
}

#[test]
fn a_format_says_how_the_number_is_written() {
  let numbered = |format: &str| {
    run(&format!("<xsl:template match='/'><xsl:number value='4' format='{format}'/></xsl:template>"), "<a/>")
  };
  assert_eq!(numbered("1"), "4");
  assert_eq!(numbered("01"), "04");
  assert_eq!(numbered("a"), "d");
  assert_eq!(numbered("A"), "D");
  assert_eq!(numbered("i"), "iv");
  assert_eq!(numbered("I"), "IV");
}

#[test]
fn without_a_value_the_number_comes_from_where_the_node_sits() {
  assert_eq!(number_items("", "<r><item/><item/><item/></r>"), "1 2 3");
}

#[test]
fn only_nodes_of_the_same_name_are_counted_by_default() {
  // §7.7 with no `count`: nodes of the same kind and expanded name as the current one.
  assert_eq!(number_items("", "<r><item/><other/><item/><other/><item/></r>"), "1 2 3");
}

#[test]
fn a_count_pattern_chooses_what_is_counted() {
  let xml = "<r><item/><other/><item/></r>";
  assert_eq!(number_items("count='item|other'", xml), "1 3");
}

#[test]
fn level_single_counts_within_one_parent() {
  // The default level: each `item` is numbered among its own siblings, so the count restarts.
  let xml = "<r><part><item/><item/></part><part><item/><item/></part></r>";
  assert_eq!(number_items("", xml), "1 2 1 2");
}

#[test]
fn level_multiple_numbers_every_level() {
  let xml = "<r><part><item/><item/></part><part><item/></part></r>";
  assert_eq!(number_items("level='multiple' count='part|item'", xml), "1.1 1.2 2.1");
}

#[test]
fn level_multiple_writes_the_levels_outermost_first() {
  let xml = "<r><a><b><c/></b></a></r>";
  let body = "<xsl:template match='/'><xsl:for-each select='//c'>\
              <xsl:number level='multiple' count='a|b|c' format='1.1.1'/></xsl:for-each></xsl:template>";
  assert_eq!(run(body, xml), "1.1.1");
}

#[test]
fn level_any_counts_across_the_whole_document() {
  // Every `item` before this one, wherever it sits, so the count never restarts.
  let xml = "<r><part><item/><item/></part><part><item/><item/></part></r>";
  assert_eq!(number_items("level='any'", xml), "1 2 3 4");
}

#[test]
fn from_restarts_the_count() {
  let xml = "<r><part><item/><item/></part><part><item/><item/></part></r>";
  // With level='any' and from='part', counting begins again at each part.
  assert_eq!(number_items("level='any' from='part'", xml), "1 2 1 2");
}

#[test]
fn from_bounds_how_far_up_single_looks() {
  let xml = "<r><part><item/></part></r>";
  // The nearest ancestor matching `from` is `part`, and `r` is above it, so `r` is not searched.
  let body = "<xsl:template match='/'><xsl:for-each select='//item'>\
              <xsl:number level='multiple' count='r|part|item' from='part'/></xsl:for-each></xsl:template>";
  assert_eq!(run(body, xml), "1");
}

#[test]
fn a_node_with_nothing_to_count_gives_nothing() {
  // No ancestor-or-self matches the count pattern, so §7.7 gives an empty list.
  let body = "<xsl:template match='/'><xsl:for-each select='//item'>\
              [<xsl:number count='nosuch'/>]</xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<r><item/></r>"), "[]");
}

#[test]
fn the_format_repeats_its_last_token_for_a_deeper_tree() {
  let xml = "<r><a><a><a/></a></a></r>";
  let body = "<xsl:template match='/'><xsl:for-each select='//a[not(a)]'>\
              <xsl:number level='multiple' count='a' format='1.'/></xsl:for-each></xsl:template>";
  assert_eq!(run(body, xml), "1.1.1.");
}

#[test]
fn grouping_separates_the_digits() {
  let body = "<xsl:template match='/'>\
              <xsl:number value='1234567' grouping-separator=',' grouping-size='3'/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "1,234,567");
}

#[test]
fn letter_value_settles_what_a_lone_i_means() {
  let numbered = |letter_value: &str| {
    run(
      &format!(
        "<xsl:template match='/'><xsl:number value='4' format='i' letter-value='{letter_value}'/></xsl:template>"
      ),
      "<a/>",
    )
  };
  assert_eq!(numbered("traditional"), "iv");
  assert_eq!(numbered("alphabetic"), "d");
}

#[test]
fn the_attributes_of_a_number_are_attribute_value_templates() {
  let body = "<xsl:template match='/'><xsl:number value='4' format='{//@f}'/></xsl:template>";
  assert_eq!(run(body, "<r f='I'/>"), "IV");
}

#[test]
fn numbering_works_on_nodes_that_are_not_elements() {
  // §7.7 counts by node kind, so text nodes are numbered among text nodes.
  let body = "<xsl:template match='/'><xsl:for-each select='//text()'>\
              <xsl:number/><xsl:text> </xsl:text></xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<r>one<a/>two<a/>three</r>").trim_end(), "1 2 3");
}

#[test]
fn what_a_number_cannot_make_sense_of_is_reported() {
  let level = "<xsl:template match='/'><xsl:number level='sideways'/></xsl:template>";
  assert!(error(level, "<a/>").contains("single, multiple or any"), "{}", error(level, "<a/>"));

  let letter = "<xsl:template match='/'><xsl:number value='1' letter-value='roman'/></xsl:template>";
  assert!(error(letter, "<a/>").contains("alphabetic"), "{}", error(letter, "<a/>"));
}

#[test]
fn element_available_says_number_is_there() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"element-available('xsl:number')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "true");
}
