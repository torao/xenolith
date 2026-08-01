//! Parsing XPath 1.0 expressions: abbreviations, axes, node tests, predicates, precedence.
//!
//! Each case checks the tree by printing it back, which writes the unabbreviated form with
//! binary expressions parenthesized — so both what was recognized and how it was grouped show.

use xylogue_core::ErrorKind;
use xylogue_xpath::parse;

/// The parsed expression, written back out.
fn tree(expression: &str) -> String {
  parse(expression).expect("parses").to_string()
}

/// The message of the error `expression` fails with.
fn error(expression: &str) -> String {
  let error = parse(expression).expect_err("fails");
  assert_eq!(error.kind(), ErrorKind::XPath);
  error.message().to_owned()
}

#[test]
fn a_step_with_no_axis_is_on_the_child_axis() {
  assert_eq!(tree("a"), "child::a");
  assert_eq!(tree("a/b/c"), "child::a/child::b/child::c");
}

#[test]
fn absolute_paths_begin_at_the_root() {
  assert_eq!(tree("/"), "/");
  assert_eq!(tree("/a"), "/child::a");
  assert_eq!(tree("/a/b"), "/child::a/child::b");
}

#[test]
fn the_abbreviations_expand() {
  assert_eq!(tree("."), "self::node()");
  assert_eq!(tree(".."), "parent::node()");
  assert_eq!(tree("@x"), "attribute::x");
  assert_eq!(tree("//a"), "/descendant-or-self::node()/child::a");
  assert_eq!(tree("a//b"), "child::a/descendant-or-self::node()/child::b");
  assert_eq!(tree("../@x"), "parent::node()/attribute::x");
}

#[test]
fn every_axis_is_recognized() {
  for axis in [
    "ancestor",
    "ancestor-or-self",
    "attribute",
    "child",
    "descendant",
    "descendant-or-self",
    "following",
    "following-sibling",
    "namespace",
    "parent",
    "preceding",
    "preceding-sibling",
    "self",
  ] {
    assert_eq!(tree(&format!("{axis}::a")), format!("{axis}::a"));
  }
}

#[test]
fn node_tests_cover_names_wildcards_and_kinds() {
  assert_eq!(tree("*"), "child::*");
  assert_eq!(tree("p:a"), "child::p:a");
  assert_eq!(tree("p:*"), "child::p:*");
  assert_eq!(tree("node()"), "child::node()");
  assert_eq!(tree("text()"), "child::text()");
  assert_eq!(tree("comment()"), "child::comment()");
  assert_eq!(tree("processing-instruction()"), "child::processing-instruction()");
  assert_eq!(tree("processing-instruction('php')"), "child::processing-instruction('php')");
}

#[test]
fn predicates_follow_a_step_in_order() {
  assert_eq!(tree("a[1]"), "child::a[1]");
  assert_eq!(tree("a[@x][b]"), "child::a[attribute::x][child::b]");
  assert_eq!(tree("a[b/c='x']"), "child::a[(child::b/child::c = 'x')]");
}

#[test]
fn operators_group_by_precedence() {
  assert_eq!(tree("1 + 2 * 3"), "(1 + (2 * 3))", "multiplication binds tighter than addition");
  assert_eq!(tree("1 * 2 + 3"), "((1 * 2) + 3)");
  assert_eq!(tree("1 - 2 - 3"), "((1 - 2) - 3)", "the additive operators are left-associative");
  assert_eq!(tree("a or b and c"), "(child::a or (child::b and child::c))", "and binds tighter than or");
  assert_eq!(tree("1 = 2 or 3 != 4"), "((1 = 2) or (3 != 4))");
  assert_eq!(tree("1 < 2 = 3"), "((1 < 2) = 3)", "comparison binds tighter than equality");
  assert_eq!(tree("1 div 2 mod 3"), "((1 div 2) mod 3)");
  assert_eq!(tree("-1 + 2"), "(-1 + 2)");
  assert_eq!(tree("a | b | c"), "((child::a | child::b) | child::c)");
}

#[test]
fn parentheses_override_precedence() {
  assert_eq!(tree("(1 + 2) * 3"), "((1 + 2) * 3)");
}

#[test]
fn primary_expressions_are_literals_numbers_variables_and_calls() {
  assert_eq!(tree("'text'"), "'text'");
  assert_eq!(tree("\"it's\""), "\"it's\"", "a value with an apostrophe is written in double quotes");
  assert_eq!(tree("42"), "42");
  assert_eq!(tree("1.5"), "1.5");
  assert_eq!(tree(".5"), "0.5");
  assert_eq!(tree("$x"), "$x");
  assert_eq!(tree("$p:x"), "$p:x");
  assert_eq!(tree("true()"), "true()");
  assert_eq!(tree("substring('abc', 1, 2)"), "substring('abc', 1, 2)");
  assert_eq!(tree("p:f(a)"), "p:f(child::a)");
}

#[test]
fn a_filter_expression_may_carry_predicates_and_a_path() {
  assert_eq!(tree("(a | b)[1]"), "(child::a | child::b)[1]");
  assert_eq!(tree("$x/a"), "$x/child::a");
  assert_eq!(tree("id('a')//b"), "id('a')/descendant-or-self::node()/child::b");
  // A parenthesised root prints as `/`, and the separator that follows would make `//` — the
  // abbreviation for something else entirely. The parentheses stay so that it does not.
  assert_eq!(tree("(/)//b"), "(/)/descendant-or-self::node()/child::b");
  assert_eq!(tree("(/)/b"), "(/)/child::b");
}

#[test]
fn operator_names_are_names_where_a_name_belongs() {
  assert_eq!(tree("div"), "child::div", "a bare name, not the operator");
  assert_eq!(tree("child::div"), "child::div");
  assert_eq!(tree("1 div 2"), "(1 div 2)");
  assert_eq!(tree("a/mod"), "child::a/child::mod");
}

#[test]
fn a_star_is_a_wildcard_in_a_step_and_multiplication_after_an_operand() {
  assert_eq!(tree("a/*"), "child::a/child::*");
  assert_eq!(tree("2 * 3"), "(2 * 3)");
  assert_eq!(tree("a[* = 1]"), "child::a[(child::* = 1)]");
}

#[test]
fn errors_name_what_was_found_and_where() {
  assert!(error("a/").contains("expected a name or a node test"), "{}", error("a/"));
  assert!(error("a b").contains("after a complete expression"), "{}", error("a b"));
  assert!(error("(1").contains("expected \")\""), "{}", error("(1"));
  assert!(error("a[1").contains("expected \"]\""), "{}", error("a[1"));
  assert!(error("").contains("end of the expression"), "{}", error(""));
  assert!(error("nosuch::a").contains("thirteen XPath axes"), "{}", error("nosuch::a"));
  assert!(error("'unclosed").contains("never closed"), "{}", error("'unclosed"));
  // The position points into the expression, so a long one can be found.
  assert!(error("a/b/c/(").contains("position 6"), "{}", error("a/b/c/("));
}
