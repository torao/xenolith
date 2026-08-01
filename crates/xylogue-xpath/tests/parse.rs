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
  // A negation embedded in anything keeps its parentheses. It only has to where a union is the
  // operator — unary minus binds looser than `|` and would otherwise swallow it — but this
  // printer parenthesises every composite so that the parse it settled on is plain to see, and
  // one rule that always holds is worth more here than a shorter line.
  assert_eq!(tree("-1 + 2"), "((-1) + 2)");
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
  // Parentheses around a location path that a path continues from say nothing: `(P)/Q` walks Q
  // from each node P yields, which is what `P/Q` does. So the steps are spliced in and the two
  // spellings become one tree, rather than two that print to each other's text.
  assert_eq!(tree("(/)//b"), "/descendant-or-self::node()/child::b");
  assert_eq!(tree("(/)/b"), "/child::b");
  assert_eq!(tree("(/..)/b"), "/parent::node()/child::b");
  assert_eq!(tree("(a/b)/c"), "child::a/child::b/child::c");
  // A filter is not spliced: there the predicate applies to the whole node-set, and
  // `(a)[1]/b` asks something `a[1]/b` does not.
  assert_eq!(tree("(a)[1]/b"), "(child::a)[1]/child::b");
}

#[test]
fn the_root_survives_being_printed_beside_anything() {
  // The root is the one expression whose text ends with an operator, and XPath's lexer reads
  // `*`, `mod`, `div`, `and` and `or` as name tests when an operator precedes them — so `(/) * b`
  // printed bare comes back as the path `/child::*` and a stray name. Every operator is tried
  // rather than the few that were known to break, and both sides of each, so an operator added
  // later is covered by a test nobody has to remember to extend.
  let operators = ["*", "mod", "div", "and", "or", "|", "+", "-", "=", "!=", "<", "<=", ">", ">="];
  let mut shapes: Vec<String> = Vec::new();
  for operator in operators {
    shapes.push(format!("(/) {operator} 1"));
    shapes.push(format!("1 {operator} (/)"));
    shapes.push(format!("(/) {operator} b"));
  }
  // And the other places one expression is printed inside another.
  for shape in ["(/)[1]", "-(/)", "(/)/b", "(/)//b", "count((/))", "((/))*b"] {
    shapes.push(shape.to_owned());
  }

  round_trips(&shapes);
}

#[test]
fn printing_gives_back_the_same_tree_and_not_merely_the_same_text() {
  // Where the printed form has to keep parentheses to mean what the tree means. Each of these
  // was found by the fuzzer, one round after another, because the property being checked was
  // that printing was *stable* rather than that it was *faithful* — a printer can be perfectly
  // consistent about printing something that means a different thing.
  let mut shapes: Vec<String> = Vec::new();

  // Unary minus binds looser than union (§3.5), so `-a | b` is `-(a | b)`: a negation used as
  // an operand of a union has to keep its parentheses, and a negative number is a negation too.
  for left in ["-a", "-/a", "-1", "- -a"] {
    shapes.push(format!("({left}) | b"));
    shapes.push(format!("b | ({left})"));
  }

  // A predicate after a path binds to the path's last step. `(//a)[1]` is the first of all the
  // `a`s and `//a[1]` is the first under each parent — different node-sets, not a nicety.
  for inner in ["a", "//a", "a/b", "a|b", "$x", "id('a')", "1", "-a", "/"] {
    shapes.push(format!("({inner})[1]"));
    shapes.push(format!("({inner})[1][2]"));
  }

  round_trips(&shapes);
}

/// Every expression prints to text that parses back to the very same tree.
fn round_trips(shapes: &[String]) {
  for shape in shapes {
    let expression = parse(shape).unwrap_or_else(|error| panic!("{shape:?} does not parse: {}", error.message()));
    let printed = expression.to_string();
    let again = parse(&printed)
      .unwrap_or_else(|error| panic!("{shape:?} printed as {printed:?}, which will not parse: {}", error.message()));
    assert_eq!(again, expression, "{shape:?} printed as {printed:?}, which parses to a different tree");
  }
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
