//! `format-number()` and `xsl:decimal-format` (XSLT 1.0 §12.3).

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

/// The message a compilation fails with.
fn compile_error(body: &str) -> String {
  Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect_err("fails").message().to_owned()
}

/// Evaluates one expression at the root and gives its string.
fn value_of(expression: &str) -> String {
  run(&format!("<xsl:template match='/'><xsl:value-of select=\"{expression}\"/></xsl:template>"), "<a/>")
}

#[test]
fn format_number_writes_a_number_against_a_pattern() {
  assert_eq!(value_of("format-number(1234.5, '#,##0.00')"), "1,234.50");
  assert_eq!(value_of("format-number(0.25, '0%')"), "25%");
  assert_eq!(value_of("format-number(5, '000')"), "005");
  assert_eq!(value_of("format-number(1.5, '0.###')"), "1.5");
}

#[test]
fn a_negative_number_takes_the_second_half_of_the_pattern() {
  assert_eq!(value_of("format-number(-1234.5, '#,##0.00;(#,##0.00)')"), "(1,234.50)");
  assert_eq!(value_of("format-number(-1234.5, '#,##0.00')"), "-1,234.50");
}

#[test]
fn the_first_argument_is_converted_to_a_number() {
  // It is an expression of any type, put through number() as §12.3 says.
  assert_eq!(value_of("format-number('42', '0')"), "42");
  assert_eq!(value_of("format-number(true(), '0')"), "1");
  assert_eq!(value_of("format-number('oops', '0')"), "NaN");
}

#[test]
fn nan_and_infinity_use_their_own_symbols() {
  assert_eq!(value_of("format-number(1 div 0, '0.00')"), "Infinity");
  assert_eq!(value_of("format-number(-1 div 0, '0.00')"), "-Infinity");
  assert_eq!(value_of("format-number(0 div 0, '0.00')"), "NaN");
}

#[test]
fn a_named_decimal_format_renames_the_characters() {
  let body = "<xsl:decimal-format name='european' decimal-separator=',' grouping-separator='.'/>\
              <xsl:template match='/'>\
                <xsl:value-of select=\"format-number(1234.5, '#.##0,00', 'european')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "1.234,50");
}

#[test]
fn an_unnamed_decimal_format_replaces_the_default() {
  let body = "<xsl:decimal-format decimal-separator=',' grouping-separator=' ' minus-sign='\u{2212}'/>\
              <xsl:template match='/'>\
                <xsl:value-of select=\"format-number(-1234.5, '# ##0,00')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "\u{2212}1 234,50");
}

#[test]
fn the_infinity_and_nan_strings_can_be_renamed() {
  let body = "<xsl:decimal-format infinity='forever' NaN='not a number'/>\
              <xsl:template match='/'><xsl:value-of select=\"format-number(1 div 0, '0')\"/>|\
              <xsl:value-of select=\"format-number(0 div 0, '0')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "forever|not a number");
}

#[test]
fn a_decimal_format_name_may_be_in_a_namespace() {
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:d='urn:d'>\
                  <xsl:decimal-format name='d:euro' decimal-separator=','/>\
                  <xsl:template match='/'>\
                    <xsl:value-of select=\"format-number(1.5, '0,0', 'd:euro')\"/>\
                  </xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  assert_eq!(transform(&stylesheet, &model, model.root_node()).expect("transforms").text(), "1,5");
}

#[test]
fn the_digit_and_pattern_characters_can_be_renamed_too() {
  // The pattern is written in the characters the decimal-format names, not the defaults.
  let body = "<xsl:decimal-format name='odd' digit='?' zero-digit='@' pattern-separator='|'/>\
              <xsl:template match='/'>\
                <xsl:value-of select=\"format-number(-5, '?@@|[?@@]', 'odd')\"/></xsl:template>";
  // `?@@` is `#00`, so 5 is written with two integer digits; `@` starts the digit run, so the
  // digits 0 and 5 are `@` and `E`.
  assert_eq!(run(body, "<a/>"), "[@E]");
}

#[test]
fn a_decimal_format_that_was_never_declared_is_reported() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"format-number(1, '0', 'nosuch')\"/></xsl:template>";
  assert!(error(body, "<a/>").contains("nosuch"), "{}", error(body, "<a/>"));
}

#[test]
fn two_declarations_of_one_name_must_agree() {
  // §12.3 makes a contradiction an error rather than letting the last one win.
  let clashing = "<xsl:decimal-format name='a' decimal-separator=','/>\
                  <xsl:decimal-format name='a' decimal-separator=';'/>";
  assert!(compile_error(clashing).contains("do not agree"), "{}", compile_error(clashing));

  // Two that say the same thing are no contradiction at all.
  let agreeing = "<xsl:decimal-format name='a' decimal-separator=','/>\
                  <xsl:decimal-format name='a' decimal-separator=','/>\
                  <xsl:template match='/'><xsl:value-of select=\"format-number(1.5, '0,0', 'a')\"/></xsl:template>";
  assert_eq!(run(agreeing, "<a/>"), "1,5");
}

#[test]
fn an_attribute_that_should_be_one_character_is_checked() {
  let body = "<xsl:decimal-format decimal-separator='..'/>";
  assert!(compile_error(body).contains("one character"), "{}", compile_error(body));
}

#[test]
fn a_pattern_with_too_many_halves_is_reported() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"format-number(1, '0;0;0')\"/></xsl:template>";
  assert!(error(body, "<a/>").contains("more than"), "{}", error(body, "<a/>"));
}

#[test]
fn function_available_now_says_format_number_is_there() {
  assert_eq!(value_of("function-available('format-number')"), "true");
}

#[test]
fn rounding_matches_what_java_does() {
  // DecimalFormat rounds half to even, and §12.3 defers to DecimalFormat.
  assert_eq!(value_of("format-number(0.5, '0')"), "0");
  assert_eq!(value_of("format-number(1.5, '0')"), "2");
  assert_eq!(value_of("format-number(2.5, '0')"), "2");
}
