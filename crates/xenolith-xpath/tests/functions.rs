//! The core function library (XPath 1.0 §4), evaluated against a document.

use xenolith_core::Error;
use xenolith_dom::build;
use xenolith_xdm::{DomModel, Model};
use xenolith_xpath::{Value, XPath};

/// An expression's result over `xml`, converted to a string the way XPath would. The prefix `p`
/// is bound, since the sample document uses it.
fn value(xml: &str, expression: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().with_namespace("p", "urn:p").compile(expression).expect("parses");
  let value = query.evaluate(&model, model.root_node()).expect("evaluates");
  value.string(&model)
}

/// The string-values of the nodes an expression selects, in document order.
fn text(xml: &str, expression: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().with_namespace("p", "urn:p").compile(expression).expect("parses");
  match query.evaluate(&model, model.root_node()).expect("evaluates") {
    Value::NodeSet(nodes) => nodes.iter().map(|node| model.string_value(*node)).collect::<Vec<_>>().join(","),
    other => panic!("expected a node-set, got {other:?}"),
  }
}

/// The message of the error an expression fails with.
fn error(xml: &str, expression: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().compile(expression).expect("parses");
  let error = query.evaluate(&model, model.root_node()).expect_err("fails");
  assert!(matches!(error, Error::XPath { .. }));
  error.message().to_owned()
}

const DOC: &str = "<r xmlns:p='urn:p'><a>one</a><p:b>two</p:b><n>1</n><n>2</n><n>3</n></r>";

#[test]
fn node_set_functions_report_names_and_counts() {
  assert_eq!(value(DOC, "count(/r/*)"), "5");
  assert_eq!(value(DOC, "local-name(/r/p:*)"), "b");
  assert_eq!(value(DOC, "namespace-uri(/r/p:*)"), "urn:p");
  assert_eq!(value(DOC, "name(/r/p:*)"), "p:b", "name keeps the prefix as written");
  assert_eq!(value(DOC, "name(/r/a)"), "a");
  // An empty node-set has no name at all.
  assert_eq!(value(DOC, "name(/r/nosuch)"), "");
  assert_eq!(value(DOC, "local-name(/r/nosuch)"), "");
  // With no argument the functions describe the context node, which a predicate supplies: a
  // function call is not a step, so `a/name()` is not the way to ask.
  assert_eq!(value(DOC, "count(/r/*[name() = 'p:b'])"), "1");
  assert_eq!(value(DOC, "count(/r/*[local-name() = 'b'])"), "1");
  assert_eq!(value(DOC, "count(/r/n)"), "3");
}

#[test]
fn id_selects_the_elements_a_dtd_typed_as_ids() {
  let xml = "<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT i EMPTY><!ATTLIST i k ID #IMPLIED>]>\
             <r><i k='a'/><i k='b'/></r>";
  assert_eq!(value(xml, "name(id('a'))"), "i");
  assert_eq!(value(xml, "count(id('a b'))"), "2", "the argument is a whitespace-separated list");
  assert_eq!(value(xml, "count(id('nosuch'))"), "0");
  // Without ID typing there is nothing for id() to find.
  assert_eq!(value("<r><i k='a'/></r>", "count(id('a'))"), "0");
}

#[test]
fn string_functions_work_on_characters() {
  assert_eq!(value(DOC, "string(42)"), "42");
  assert_eq!(value(DOC, "string(/r/a)"), "one");
  assert_eq!(value(DOC, "concat('a', 'b', 'c')"), "abc");
  assert_eq!(value(DOC, "starts-with('abcd', 'ab')"), "true");
  assert_eq!(value(DOC, "contains('abcd', 'bc')"), "true");
  assert_eq!(value(DOC, "substring-before('1999/04', '/')"), "1999");
  assert_eq!(value(DOC, "substring-after('1999/04', '/')"), "04");
  assert_eq!(value(DOC, "substring-before('abc', 'x')"), "", "no match gives the empty string");
  assert_eq!(value(DOC, "substring('12345', 2)"), "2345");
  assert_eq!(value(DOC, "substring('12345', 1.5, 2.6)"), "234", "both bounds are rounded");
  assert_eq!(value(DOC, "string-length('hello')"), "5");
  assert_eq!(value(DOC, "normalize-space('  a  b  ')"), "a b");
  assert_eq!(value(DOC, "translate('bar', 'abc', 'ABC')"), "BAr");
  assert_eq!(value(DOC, "translate('--aaa--', 'abc-', 'ABC')"), "AAA");
  // With no argument they read the context node, which a predicate supplies.
  assert_eq!(value(DOC, "count(/r/*[string-length() = 3])"), "2");
  assert_eq!(value(DOC, "count(/r/*[string() = 'one'])"), "1");
}

#[test]
fn string_functions_count_characters_not_bytes() {
  // Three characters, nine bytes in UTF-8.
  let xml = "<r>\u{65e5}\u{672c}\u{8a9e}</r>";
  assert_eq!(value(xml, "string-length(/r)"), "3");
  assert_eq!(value(xml, "substring(/r, 2, 1)"), "\u{672c}");
  assert_eq!(value(xml, "substring-after(/r, '\u{65e5}')"), "\u{672c}\u{8a9e}");
}

#[test]
fn boolean_functions_convert_and_test_language() {
  assert_eq!(value(DOC, "boolean(1)"), "true");
  assert_eq!(value(DOC, "boolean('')"), "false");
  assert_eq!(value(DOC, "boolean(/r/nosuch)"), "false");
  assert_eq!(value(DOC, "not(boolean(0))"), "true");
  assert_eq!(value(DOC, "true()"), "true");
  assert_eq!(value(DOC, "false()"), "false");

  // lang() reads the context node, so it is asked inside a predicate.
  let xml = "<r xml:lang='en'><a/><b xml:lang='fr'><c/></b></r>";
  assert_eq!(value(xml, "count(/r/a[lang('en')])"), "1", "the language is inherited");
  assert_eq!(value(xml, "count(/r/a[lang('EN')])"), "1", "and compared without regard to case");
  assert_eq!(value(xml, "count(/r/b/c[lang('en')])"), "0", "the nearest xml:lang settles it");
  assert_eq!(value(xml, "count(/r/b/c[lang('fr')])"), "1");
  assert_eq!(value("<r><a/></r>", "count(/r/a[lang('en')])"), "0", "with no xml:lang in scope");

  let sub = "<r xml:lang='en-GB'><a/></r>";
  assert_eq!(value(sub, "count(/r/a[lang('en')])"), "1", "a sublanguage answers to its language");
  assert_eq!(value(sub, "count(/r/a[lang('en-US')])"), "0");
}

#[test]
fn number_functions_follow_the_xpath_rounding_rules() {
  assert_eq!(value(DOC, "number('42')"), "42");
  assert_eq!(value(DOC, "number('x')"), "NaN");
  assert_eq!(value(DOC, "number(true())"), "1");
  assert_eq!(value(DOC, "sum(/r/n)"), "6");
  assert_eq!(value(DOC, "sum(/r/nosuch)"), "0", "an empty node-set sums to zero");
  assert_eq!(value(DOC, "floor(1.9)"), "1");
  assert_eq!(value(DOC, "floor(-1.1)"), "-2");
  assert_eq!(value(DOC, "ceiling(1.1)"), "2");
  assert_eq!(value(DOC, "ceiling(-1.9)"), "-1");
  assert_eq!(value(DOC, "round(1.5)"), "2");
  assert_eq!(value(DOC, "round(-1.5)"), "-1", "a half goes towards positive infinity");
  assert_eq!(value(DOC, "round(0.5)"), "1");
  // With no argument, number() reads the context node.
  assert_eq!(value(DOC, "count(/r/n[number() > 1])"), "2");
}

#[test]
fn functions_compose_in_predicates() {
  assert_eq!(text(DOC, "/r/n[number() > 1]"), "2,3");
  assert_eq!(text(DOC, "/r/*[starts-with(name(), 'p:')]"), "two");
  assert_eq!(text(DOC, "/r/n[position() = last() - 1]"), "2");
  assert_eq!(value(DOC, "sum(/r/n[. > 1])"), "5");
  assert_eq!(value(DOC, "count(/r/*[string-length() = 3])"), "2", "one and two are both three long");
}

#[test]
fn a_call_that_cannot_be_made_says_why() {
  assert!(error(DOC, "nosuch()").contains("no function named"), "{}", error(DOC, "nosuch()"));
  assert!(error(DOC, "concat('a')").contains("at least 2 arguments"), "{}", error(DOC, "concat('a')"));
  assert!(error(DOC, "substring('a')").contains("2 or 3 arguments"), "{}", error(DOC, "substring('a')"));
  assert!(error(DOC, "floor(1, 2)").contains("takes 1 argument"), "{}", error(DOC, "floor(1, 2)"));
  assert!(error(DOC, "sum(1)").contains("needs a node-set"), "{}", error(DOC, "sum(1)"));
  assert!(error(DOC, "name(1)").contains("needs a node-set"), "{}", error(DOC, "name(1)"));
}
