//! `http://exslt.org/regular-expressions`, run through the XSLT engine.

#![cfg(feature = "regexp")]

use std::rc::Rc;

use xylogue_core::error::Result;
use xylogue_dom::build;
use xylogue_xdm::{Documents, DomModel};
use xylogue_xpath::Functions;
use xylogue_xslt::{DocumentSource, Stylesheet, Transform, TreeSpace};

/// The namespace declarations these stylesheets need.
const PREFIXES: &str = "xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
                        xmlns:regexp=\"http://exslt.org/regular-expressions\"";

/// Evaluates one expression, with somewhere for a built tree to go.
fn value_of(expression: &str) -> String {
  evaluate(expression).expect("transforms")
}

fn evaluate(expression: &str) -> Result<String> {
  let body = format!("<xsl:template match='/'><xsl:value-of select=\"{expression}\"/></xsl:template>");
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl")?;
  let doc = build::parse("<a/>".as_bytes())?;
  let documents = Documents::new();
  let model = DomModel::with_documents(&doc, &documents);
  let space: Rc<dyn DocumentSource<_>> = Rc::new(TreeSpace::new(&documents));
  let functions = xylogue_exslt::register_with(Functions::new(), &space);
  let result = Transform::new().run_with_documents(&stylesheet, &model, model.root_node(), functions, space)?;
  Ok(result.text())
}

#[test]
fn test_says_whether_the_pattern_is_there() {
  assert_eq!(value_of("regexp:test('abc123', '[0-9]+')"), "true");
  assert_eq!(value_of("regexp:test('abc', '[0-9]+')"), "false");
}

#[test]
fn the_i_flag_matches_without_regard_to_case() {
  assert_eq!(value_of("regexp:test('ABC', 'abc')"), "false");
  assert_eq!(value_of("regexp:test('ABC', 'abc', 'i')"), "true");
}

#[test]
fn the_m_flag_anchors_at_every_line() {
  assert_eq!(value_of("regexp:test('a&#10;b', '^b$')"), "false");
  assert_eq!(value_of("regexp:test('a&#10;b', '^b$', 'm')"), "true");
}

#[test]
fn replace_changes_the_first_match_or_every_one() {
  assert_eq!(value_of("regexp:replace('a1b2c3', '[0-9]', '', '-')"), "a-b2c3");
  assert_eq!(value_of("regexp:replace('a1b2c3', '[0-9]', 'g', '-')"), "a-b-c-");
}

#[test]
fn a_dollar_in_the_replacement_is_a_dollar_sign() {
  // EXSLT says nothing about `$1` naming a captured group, and libxslt does not read one that
  // way either, so the replacement goes in as it stands.
  assert_eq!(value_of("regexp:replace('ab', '(a)', 'g', '$1')"), "$1b");
}

#[test]
fn match_without_g_gives_the_match_and_then_its_groups() {
  assert_eq!(value_of("count(regexp:match('2026-07-29', '([0-9]{4})-([0-9]{2})-([0-9]{2})'))"), "4");
  assert_eq!(value_of("regexp:match('2026-07-29', '([0-9]{4})-([0-9]{2})-([0-9]{2})')[1]"), "2026-07-29");
  assert_eq!(value_of("regexp:match('2026-07-29', '([0-9]{4})-([0-9]{2})-([0-9]{2})')[2]"), "2026");
  assert_eq!(value_of("regexp:match('2026-07-29', '([0-9]{4})-([0-9]{2})-([0-9]{2})')[4]"), "29");
}

#[test]
fn match_with_g_gives_one_piece_per_match() {
  assert_eq!(value_of("count(regexp:match('a1b22c333', '[0-9]+', 'g'))"), "3");
  assert_eq!(value_of("regexp:match('a1b22c333', '[0-9]+', 'g')[2]"), "22");
}

#[test]
fn match_names_each_piece_a_match() {
  assert_eq!(value_of("name(regexp:match('abc', 'b')[1])"), "match");
}

#[test]
fn a_pattern_that_finds_nothing_gives_nothing() {
  assert_eq!(value_of("count(regexp:match('abc', '[0-9]'))"), "0");
  assert_eq!(value_of("count(regexp:match('abc', '[0-9]', 'g'))"), "0");
}

#[test]
fn what_it_builds_is_a_node_set_and_behaves_as_one() {
  assert_eq!(value_of("count(regexp:match('a1b22', '[0-9]+', 'g')[. != '1'])"), "1");
}

#[test]
fn a_pattern_that_cannot_be_used_is_reported_rather_than_treated_as_no_match() {
  let error = evaluate("regexp:test('a', '(unclosed')").expect_err("not a pattern");
  assert!(error.message().contains("cannot be used"), "{}", error.message());

  // A backreference is a real difference from a backtracking engine, and being told is the
  // point — a silent false would look like the text simply not matching.
  let backreference = evaluate(r"regexp:test('aa', '(a)\1')").expect_err("not supported");
  assert!(backreference.message().contains("cannot be used"), "{}", backreference.message());
}

#[test]
fn a_flag_nobody_defined_is_refused() {
  let error = evaluate("regexp:test('a', 'a', 'x')").expect_err("no such flag");
  assert!(error.message().contains("g, i or m"), "{}", error.message());
}

#[test]
fn function_available_says_what_this_build_has() {
  assert_eq!(value_of("function-available('regexp:test')"), "true");
  assert_eq!(value_of("function-available('regexp:match')"), "true");
  assert_eq!(value_of("function-available('regexp:replace')"), "true");
}
