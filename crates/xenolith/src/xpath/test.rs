use super::*;
use crate::dom::build::DomBuilder;
use crate::event::{EventCursor, EventProducer};
use crate::io::StreamSource;
use crate::xpath::ast::{BinaryOp, Path, PathStart, Step};
use crate::xpath::model::{DomNode, Model, NodeKind};

// --- Parsing ------------------------------------------------------------------------------------
//
// Each case checks the tree by printing it back in full (`{:#}`), which writes the unabbreviated form with binary
// expressions parenthesized — so both what was recognized and how it was grouped show.

/// The parsed expression, written back out in full.
fn tree(expression: &str) -> String {
  format!("{:#}", XPath::parse(expression, Location::new()).expect("parses").expr)
}

/// The parsed expression, written back out with the abbreviations.
fn abbreviated(expression: &str) -> String {
  XPath::parse(expression, Location::new()).expect("parses").to_string()
}

/// The error returned when `expression` fails to parse.
fn parse_error(expression: &str) -> Error {
  let error = XPath::parse(expression, Location::new()).expect_err("fails");
  assert!(matches!(error, Error::XPath { .. }), "{error:?}");
  error
}

/// The message returned when `expression` fails to parse.
fn parse_message(expression: &str) -> String {
  parse_error(expression).message().to_owned()
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
  assert_eq!(tree("xml:a"), "child::xml:a");
  assert_eq!(tree("xml:*"), "child::xml:*");
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
  // A negation embedded in anything keeps its parentheses. It only has to where a union is the operator — unary minus
  // binds looser than `|` and would otherwise swallow it — but this printer parenthesises every composite so that the
  // parse it settled on is plain to see.
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
  assert_eq!(tree("$xml:x"), "$xml:x");
  assert_eq!(tree("true()"), "true()");
  assert_eq!(tree("substring('abc', 1, 2)"), "substring('abc', 1, 2)");
  // A prefixed call does not parse while no extension function is available, but a tree that holds one prints it.
  let call =
    Expr::Function { prefix: Some("p".to_owned()), local: "f".to_owned(), arguments: vec![Expr::Number(1.0)], at: 0 };
  assert_eq!(format!("{call:#}"), "p:f(1)");
}

#[test]
fn a_filter_expression_may_carry_predicates_and_a_path() {
  assert_eq!(tree("(a | b)[1]"), "(child::a | child::b)[1]");
  assert_eq!(tree("$x/a"), "$x/child::a");
  assert_eq!(tree("id('a')//b"), "id('a')/descendant-or-self::node()/child::b");
  // Parentheses around a location path that a path continues from say nothing: `(P)/Q` walks Q from each node P
  // yields, which is what `P/Q` does. So the steps are spliced in and the two spellings become one tree.
  assert_eq!(tree("(/)//b"), "/descendant-or-self::node()/child::b");
  assert_eq!(tree("(/)/b"), "/child::b");
  assert_eq!(tree("(/..)/b"), "/parent::node()/child::b");
  assert_eq!(tree("(a/b)/c"), "child::a/child::b/child::c");
  // A filter is not spliced: there the predicate applies to the whole node-set, and `(a)[1]/b` asks something
  // `a[1]/b` does not.
  assert_eq!(tree("(a)[1]/b"), "(child::a)[1]/child::b");
}

#[test]
fn the_root_survives_being_printed_beside_anything() {
  // The root is the one expression whose text ends with an operator, and XPath's lexer reads `*`, `mod`, `div`, `and`
  // and `or` as name tests when an operator precedes them — so `(/) * b` printed bare comes back as the path
  // `/child::*` and a stray name. Every operator is tried, and both sides of each.
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
  let mut shapes: Vec<String> = Vec::new();
  // Unary minus binds looser than union (§3.5), so `-a | b` is `-(a | b)`: a negation used as an operand of a union
  // has to keep its parentheses, and a negative number is a negation too.
  for left in ["-a", "-/a", "-1", "- -a"] {
    shapes.push(format!("({left}) | b"));
    shapes.push(format!("b | ({left})"));
  }
  // A predicate after a path binds to the path's last step. `(//a)[1]` is the first of all the `a`s and `//a[1]` is
  // the first under each parent — different node-sets, not a nicety.
  for inner in ["a", "//a", "a/b", "a|b", "$x", "id('a')", "1", "-a", "/"] {
    shapes.push(format!("({inner})[1]"));
    shapes.push(format!("({inner})[1][2]"));
  }
  round_trips(&shapes);
}

/// Every expression prints, in full and abbreviated, to text that parses back to the very same tree.
fn round_trips(shapes: &[String]) {
  for shape in shapes {
    let expression = XPath::parse(shape, Location::new())
      .unwrap_or_else(|error| panic!("{shape:?} does not parse: {}", error.message()))
      .expr;
    for printed in [format!("{expression:#}"), format!("{expression}")] {
      let again = XPath::parse(&printed, Location::new())
        .unwrap_or_else(|error| panic!("{shape:?} printed as {printed:?}, which will not parse: {}", error.message()))
        .expr;
      assert_eq!(
        without_positions(&again),
        without_positions(&expression),
        "{shape:?} printed as {printed:?}, which parses to a different tree"
      );
    }
  }
}

/// `expr` with every position in the expression cleared, so that trees parsed from different text can be compared.
fn without_positions(expr: &Expr) -> Expr {
  let all = |exprs: &[Expr]| exprs.iter().map(without_positions).collect();
  match expr {
    Expr::Binary { op, left, right, .. } => Expr::Binary {
      op: *op,
      left: Box::new(without_positions(left)),
      right: Box::new(without_positions(right)),
      at: 0,
    },
    Expr::Negate(inner) => Expr::Negate(Box::new(without_positions(inner))),
    Expr::Path(path) => Expr::Path(Path {
      start: match &path.start {
        PathStart::Expr(start) => PathStart::Expr(Box::new(without_positions(start))),
        other => other.clone(),
      },
      steps: path
        .steps
        .iter()
        .map(|step| Step { axis: step.axis, node_test: step.node_test.clone(), predicates: all(&step.predicates) })
        .collect(),
      at: 0,
    }),
    Expr::Filter { expr, predicates, .. } => {
      Expr::Filter { expr: Box::new(without_positions(expr)), predicates: all(predicates), at: 0 }
    }
    Expr::Variable { prefix, local, .. } => Expr::Variable { prefix: prefix.clone(), local: local.clone(), at: 0 },
    Expr::Function { prefix, local, arguments, .. } => {
      Expr::Function { prefix: prefix.clone(), local: local.clone(), arguments: all(arguments), at: 0 }
    }
    Expr::Literal(_) | Expr::Number(_) => expr.clone(),
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
fn parse_errors_name_what_was_found() {
  assert!(parse_message("a/").contains("expected a name or a node test"), "{}", parse_message("a/"));
  assert!(parse_message("a b").contains("after a complete expression"), "{}", parse_message("a b"));
  assert!(parse_message("(1").contains("expected \")\""), "{}", parse_message("(1"));
  assert!(parse_message("a[1").contains("expected \"]\""), "{}", parse_message("a[1"));
  assert!(parse_message("").contains("end of the expression"), "{}", parse_message(""));
  assert!(parse_message("nosuch::a").contains("thirteen XPath axes"), "{}", parse_message("nosuch::a"));
  assert!(parse_message("'unclosed").contains("never closed"), "{}", parse_message("'unclosed"));
}

#[test]
fn a_parse_error_is_located_from_where_the_expression_starts() {
  // The `(` at byte 6 is the seventh character on the line.
  let error = parse_error("a/b/c/(");
  assert_eq!((error.location().line, error.location().column), (1, 7));
  // Counted from the location given, as an expression in an attribute value is.
  let start = Location { line: 3, column: 10, offset: 40, ..Location::new() };
  let error = XPath::parse("a/\u{65e5}/(", start).expect_err("fails");
  assert_eq!((error.location().line, error.location().column), (3, 14), "characters are counted, not bytes");
}

#[test]
fn from_str_parses_as_parse_does() {
  let parsed: XPath = "a//b".parse().expect("parses");
  assert_eq!(format!("{:#}", parsed.expr), tree("a//b"));
  let error = "a/".parse::<XPath>().expect_err("fails");
  assert_eq!((error.location().line, error.location().column), (1, 3));
}

#[test]
fn display_writes_the_abbreviations_and_alternate_display_writes_in_full() {
  let xpath = XPath::parse("/descendant-or-self::node()/child::a[attribute::x = 1] | parent::node()", Location::new())
    .expect("parses");
  assert_eq!(xpath.to_string(), "//a[@x = 1] | ..");
  assert_eq!(format!("{xpath:#}"), "(/descendant-or-self::node()/child::a[(attribute::x = 1)] | parent::node())");
}

#[test]
fn every_abbreviation_is_used_where_it_applies() {
  assert_eq!(abbreviated("child::a/attribute::x"), "a/@x");
  assert_eq!(abbreviated("self::node()/parent::node()"), "./..");
  assert_eq!(abbreviated("child::a/descendant-or-self::node()/child::b"), "a//b");
  assert_eq!(abbreviated("$x/descendant-or-self::node()/child::b"), "$x//b");
  assert_eq!(abbreviated("ancestor::a/following-sibling::*"), "ancestor::a/following-sibling::*", "no short form");
  // Where an abbreviation would mean something else, the step keeps its axis.
  assert_eq!(abbreviated("self::node()[1]"), "self::node()[1]", "`.` cannot carry a predicate");
  assert_eq!(abbreviated("parent::node()[1]"), "parent::node()[1]", "nor can `..`");
  assert_eq!(
    abbreviated("child::a/descendant-or-self::node()"),
    "a/descendant-or-self::node()",
    "`//` needs a step after it"
  );
  assert_eq!(abbreviated("descendant-or-self::node()/child::a"), "descendant-or-self::node()/a", "`//a` is absolute");
  assert_eq!(abbreviated("descendant-or-self::node()[1]/child::a"), "descendant-or-self::node()[1]/a");
  assert_eq!(
    abbreviated("a/descendant-or-self::node()/descendant-or-self::node()/b"),
    "a//descendant-or-self::node()/b",
    "`///` does not parse"
  );
}

#[test]
fn abbreviated_parentheses_are_only_those_the_tree_needs() {
  assert_eq!(abbreviated("(1 + (2 * 3))"), "1 + 2 * 3");
  assert_eq!(abbreviated("((1 + 2) * 3)"), "(1 + 2) * 3");
  assert_eq!(abbreviated("((1 - 2) - 3)"), "1 - 2 - 3");
  assert_eq!(abbreviated("1 - (2 - 3)"), "1 - (2 - 3)", "the operators are left-associative");
  assert_eq!(abbreviated("a | (b | c)"), "a | (b | c)");
  assert_eq!(abbreviated("-(a | b)"), "-a | b", "a minus binds more loosely than a union");
  assert_eq!(abbreviated("(-a) | b"), "(-a) | b");
  assert_eq!(abbreviated("2 * -3"), "2 * -3");
  assert_eq!(abbreviated("(/) * b"), "(/) * b", "`/ *` would read `*` as a name test");
  assert_eq!(abbreviated("(a | b)/c"), "(a | b)/c");
  assert_eq!(abbreviated("(//a)[1]"), "(//a)[1]");
  assert_eq!(abbreviated("a or b and c"), "a or b and c");
  assert_eq!(abbreviated("(a or b) and c"), "(a or b) and c");
}

#[test]
fn a_string_with_both_quotes_is_written_as_a_concat_of_the_same_value() {
  let doc = parse_document("<r/>");
  for (value, written) in [
    ("it's \"x\"", "concat(\"it's \", '\"x\"')"),
    ("'\"", "concat(\"'\", '\"')"),
    ("a'b\"c'd", "concat(\"a'b\", '\"c', \"'d\")"),
  ] {
    let literal = Expr::Literal(value.to_owned());
    assert_eq!(literal.to_string(), written);
    assert_eq!(format!("{literal:#}"), written);
    // It parses to a call, not a literal, but to one that yields the same string.
    let again = XPath::parse(written, Location::new()).expect("parses");
    assert_eq!(again.evaluate(&doc, doc.document_node()).expect("evaluates"), XPathResult::String(value.to_owned()));
  }
  // A string with one kind of quote is still a literal, in the other kind.
  assert_eq!(Expr::Literal("it's".to_owned()).to_string(), "\"it's\"");
  assert_eq!(Expr::Literal("say \"hi\"".to_owned()).to_string(), "'say \"hi\"'");
}

#[test]
fn a_number_no_literal_can_write_is_written_as_something_of_the_same_value() {
  // A literal too long for a double reads as infinity, and is written back as digits that read as infinity again.
  let huge = format!("1{}", "0".repeat(400));
  assert_eq!(XPath::parse(&huge, Location::new()).expect("parses").expr, Expr::Number(f64::INFINITY));
  round_trips(&[huge, format!("2 * 1{}", "0".repeat(400))]);
  assert_eq!(Expr::Number(f64::INFINITY).to_string(), format!("18{}", "0".repeat(307)));

  // No parse yields these; they are written as expressions of the same value.
  for (value, written) in [(f64::NAN, "(0 div 0)"), (f64::NEG_INFINITY, "(-1 div 0)"), (-0.0, "-0")] {
    assert_eq!(Expr::Number(value).to_string(), written);
    assert_eq!(format!("{:#}", Expr::Number(value)), written);
  }
  let union = Expr::Binary {
    op: BinaryOp::Union,
    left: Box::new(Expr::Number(f64::NEG_INFINITY)),
    right: Box::new(Expr::Number(-1.0)),
    at: 0,
  };
  assert_eq!(union.to_string(), "(-1 div 0) | (-1)", "a negative number binds as a negation does");
}

#[test]
fn the_abbreviated_forms_round_trip() {
  let shapes = [
    "child::a/descendant-or-self::node()/descendant-or-self::node()/child::b",
    "descendant-or-self::node()/child::a",
    "child::a/descendant-or-self::node()",
    "self::node()[1]/parent::node()[2]",
    "//.",
    "/descendant-or-self::node()/self::node()",
    "attribute::*[1] | namespace::*",
    "child::div/child::mod * child::and",
    "-(-a)",
    "1 - (2 - (3 - 4))",
    "(a = b) = (c = d)",
    "concat(-(a | b), (/))",
  ];
  round_trips(&shapes.map(ToOwned::to_owned));
}

// --- Evaluating ---------------------------------------------------------------------------------

/// Reads `xml` into a tree through the parser and the builder.
fn parse_document(xml: &str) -> Document {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml.as_bytes()).add_consumer(&mut builder).emit().expect("well-formed");
  builder.into_document()
}

/// Evaluates `expression` over `xml` from the document node.
fn evaluate(xml: &str, expression: &str) -> (Document, Result<XPathResult>) {
  let doc = parse_document(xml);
  let xpath = XPath::parse(expression, Location::new()).expect("parses");
  let value = xpath.evaluate(&doc, doc.document_node());
  (doc, value)
}

/// The data-model node represented by a node in a node-set.
fn model_node(model: &DomModel<'_>, doc: &Document, node: &Node) -> DomNode {
  match node {
    Node::Dom(id) => model.node(*id),
    Node::Namespace { element, prefix } => {
      DomNode::Namespace { element: *element, prefix: prefix.as_deref().and_then(|prefix| doc.pool().get(prefix)) }
    }
  }
}

/// The nodes in a node-set, rendered with `render` and joined by commas.
fn nodes(xml: &str, expression: &str, render: impl Fn(&DomModel<'_>, DomNode) -> String) -> String {
  let (doc, value) = evaluate(xml, expression);
  let model = DomModel::new(&doc);
  match value.expect("evaluates") {
    XPathResult::NodeSet(nodes) => {
      nodes.iter().map(|node| render(&model, model_node(&model, &doc, node))).collect::<Vec<_>>().join(",")
    }
    other => panic!("expected a node-set, got {other:?}"),
  }
}

/// The names of the nodes an expression selects, in document order.
fn names(xml: &str, expression: &str) -> String {
  nodes(xml, expression, |model, node| {
    let local = model.expanded_name(node).map(|name| name.local).unwrap_or_default();
    match model.kind(node) {
      NodeKind::Root => "/".to_owned(),
      NodeKind::Attribute => format!("@{local}"),
      NodeKind::Namespace => format!("ns:{local}"),
      NodeKind::Text => format!("'{}'", model.string_value(node)),
      NodeKind::Comment => "<!---->".to_owned(),
      NodeKind::ProcessingInstruction => format!("?{local}"),
      NodeKind::Element => local,
    }
  })
}

/// The string-values of the nodes an expression selects, in document order.
fn text(xml: &str, expression: &str) -> String {
  nodes(xml, expression, |model, node| model.string_value(node))
}

/// An expression result converted to a string using XPath rules.
fn value(xml: &str, expression: &str) -> String {
  let (doc, result) = evaluate(xml, expression);
  result.expect("evaluates").string(&doc).expect("a node of the document")
}

/// The message returned when an expression fails to evaluate.
fn error(xml: &str, expression: &str) -> String {
  let error = evaluate(xml, expression).1.expect_err("fails");
  assert!(matches!(error, Error::XPath { .. }), "{error:?}");
  error.message().to_owned()
}

/// `<a>` has a preceding sibling, a following sibling, and two children.
const TREE: &str = "<r><x><x1/></x><a><b/><c/></a><y/></r>";

#[test]
fn the_forward_axes_walk_in_document_order() {
  assert_eq!(names(TREE, "/r/a/child::*"), "b,c");
  assert_eq!(names(TREE, "/r/a/descendant::*"), "b,c");
  assert_eq!(names(TREE, "/r/a/descendant-or-self::*"), "a,b,c");
  assert_eq!(names(TREE, "/r/a/following-sibling::*"), "y");
  assert_eq!(names(TREE, "/r/a/self::*"), "a");
  assert_eq!(names(TREE, "/r/a/following::*"), "y", "following excludes the node's own descendants");
}

#[test]
fn the_reverse_axes_number_positions_from_the_node_outwards() {
  assert_eq!(names(TREE, "/r/a/parent::*"), "r");
  assert_eq!(names(TREE, "/r/a/preceding-sibling::*"), "x");
  assert_eq!(
    names(TREE, "/r/a/preceding::*"),
    "x,x1",
    "preceding excludes ancestors, and is in document order as a set"
  );
  // The predicate counts along the axis, so [1] is the nearest ancestor, not the outermost.
  assert_eq!(names(TREE, "/r/a/b/ancestor::*[1]"), "a");
  assert_eq!(names(TREE, "/r/a/b/ancestor::*[2]"), "r");
  assert_eq!(names(TREE, "/r/a/b/ancestor-or-self::*[1]"), "b");
  assert_eq!(names(TREE, "/r/a/preceding::*[1]"), "x1", "the nearest preceding node comes first");
}

/// Two nested sibling levels around `e`, with children at each level and an attribute and namespace on `e`.
const LEVELS: &str = "<r xmlns:p='urn:p'><x><x1/></x><e k='v'><a><a1/></a><b/></e><y><y1/></y></r>";

#[test]
fn following_and_preceding_reach_across_levels_in_axis_order() {
  assert_eq!(names(LEVELS, "//x1/following::*"), "e,a,a1,b,y,y1");
  assert_eq!(names(LEVELS, "//b/following::*"), "y,y1");
  assert_eq!(names(LEVELS, "//a1/preceding::*"), "x,x1", "the ancestors a, e and r are left out");
  // Positions count along the axis: nearest first for preceding, each subtree from its last node to its first.
  for (position, name) in [(1, "a1"), (2, "a"), (3, "x1"), (4, "x")] {
    assert_eq!(names(LEVELS, &format!("//b/preceding::*[{position}]")), name);
  }
  for (position, name) in [(1, "y"), (2, "y1")] {
    assert_eq!(names(LEVELS, &format!("//b/following::*[{position}]")), name);
  }
}

#[test]
fn what_follows_an_attribute_or_a_namespace_node_begins_with_its_element_s_children() {
  // Both come before the children of their element in document order, without being their ancestors.
  assert_eq!(names(LEVELS, "//e/@k/following::*"), "a,a1,b,y,y1");
  assert_eq!(names(LEVELS, "//e/@k/following::*[1]"), "a");
  assert_eq!(names(LEVELS, "//e/namespace::p/following::*"), "a,a1,b,y,y1");
  // Their element is an ancestor, so it does not precede them.
  assert_eq!(names(LEVELS, "//e/@k/preceding::*"), "x,x1");
  assert_eq!(names(LEVELS, "//e/@k/preceding::*[1]"), "x1");
}

#[test]
fn attribute_and_namespace_axes_reach_what_is_not_a_child() {
  let xml = "<r xmlns:p='urn:p' k='v' j='w'><a/></r>";
  assert_eq!(names(xml, "/r/@*"), "@k,@j");
  assert_eq!(names(xml, "/r/attribute::k"), "@k");
  assert_eq!(value(xml, "/r/@k"), "v");
  // The declaration is a namespace node, not an attribute, and `xml` is always in scope.
  assert_eq!(value(xml, "count(/r/namespace::*)"), "2");
  assert_eq!(names(xml, "/r/namespace::*"), "ns:p,ns:xml");
  assert_eq!(value(xml, "count(/r/@*)"), "2");
  assert_eq!(names(xml, "/r/@*/parent::*"), "r", "an attribute's parent is its element");
}

#[test]
fn a_namespace_node_is_its_element_and_the_prefix_it_binds() {
  // `/*`, since `r` is in the default namespace and `/r` names an element in none.
  let (doc, value) = evaluate("<r xmlns='urn:d' xmlns:p='urn:p'/>", "/*/namespace::*");
  let element = doc.document_element().expect("the document element");
  let expected = [None, Some("p"), Some("xml")]
    .map(|prefix| Node::Namespace { element, prefix: prefix.map(ToOwned::to_owned) })
    .to_vec();
  assert_eq!(value.expect("evaluates"), XPathResult::NodeSet(expected));
}

#[test]
fn the_xml_prefix_needs_no_binding() {
  // Namespaces in XML §3 binds `xml` by definition and forbids binding it to anything else, so an expression may use
  // it without the caller having said anything.
  let xml = "<r xml:lang='en'><a xml:space='preserve'/></r>";
  assert_eq!(value(xml, "/r/@xml:lang"), "en");
  assert_eq!(value(xml, "count(//@xml:space)"), "1");
  // It is the XML namespace it stands for, not merely a prefix that happens to match.
  assert_eq!(value(xml, "count(//@*[namespace-uri() = 'http://www.w3.org/XML/1998/namespace'])"), "2");
}

#[test]
fn node_tests_select_by_kind_and_by_name() {
  let xml = "<r>t1<a/><!--c--><?pi d?>t2</r>";
  assert_eq!(names(xml, "/r/node()"), "'t1',a,<!---->,?pi,'t2'");
  assert_eq!(names(xml, "/r/text()"), "'t1','t2'");
  assert_eq!(names(xml, "/r/comment()"), "<!---->");
  assert_eq!(names(xml, "/r/processing-instruction()"), "?pi");
  assert_eq!(names(xml, "/r/processing-instruction('pi')"), "?pi");
  assert_eq!(names(xml, "/r/processing-instruction('other')"), "");
  assert_eq!(names(xml, "/r/*"), "a", "a name test selects only elements");
}

#[test]
fn a_text_node_is_the_first_dom_node_of_its_run() {
  let (doc, value) = evaluate("<r>one<![CDATA[two]]></r>", "/r/text()");
  let r = doc.document_element().expect("the document element");
  let first = doc.first_child(r).expect("the text");
  assert_eq!(value.expect("evaluates"), XPathResult::NodeSet(vec![Node::Dom(first)]));
}

#[test]
fn a_numeric_predicate_tests_the_position() {
  let xml = "<r><g><i>1</i><i>2</i></g><g><i>3</i><i>4</i></g></r>";
  assert_eq!(text(xml, "//i[2]"), "2,4", "the second i of each parent");
  assert_eq!(text(xml, "(//i)[2]"), "2", "the second of the whole set");
  assert_eq!(text(xml, "//i[last()]"), "2,4");
  assert_eq!(text(xml, "//i[position() = 1]"), "1,3");
  assert_eq!(text(xml, "//i[. = '3']"), "3");
  assert_eq!(text(xml, "//g[1]/i[1]"), "1", "predicates chain along the path");
  assert_eq!(value(xml, "count(//i)"), "4");
}

#[test]
fn predicates_apply_in_order() {
  let xml = "<r><i k='y'>1</i><i>2</i><i k='y'>3</i></r>";
  // The position is counted among what the previous predicate left.
  assert_eq!(text(xml, "//i[@k][2]"), "3");
  assert_eq!(text(xml, "//i[2][@k]"), "", "the second i has no k, so nothing survives");
}

#[test]
fn arithmetic_follows_ieee_754() {
  assert_eq!(value(TREE, "1 + 2 * 3"), "7");
  assert_eq!(value(TREE, "1 div 2"), "0.5");
  assert_eq!(value(TREE, "1 div 0"), "Infinity");
  assert_eq!(value(TREE, "-1 div 0"), "-Infinity");
  assert_eq!(value(TREE, "0 div 0"), "NaN");
  assert_eq!(value(TREE, "5 mod 3"), "2");
  assert_eq!(value(TREE, "-5 mod 3"), "-2", "mod truncates towards zero");
  assert_eq!(value(TREE, "-(2 + 3)"), "-5");
}

#[test]
fn booleans_convert_and_short_circuit() {
  assert_eq!(value(TREE, "true() and false()"), "false");
  assert_eq!(value(TREE, "true() or false()"), "true");
  assert_eq!(value(TREE, "not(1 = 1)"), "false");
  // The right side is never evaluated, so its unbound variable goes unnoticed.
  assert_eq!(value(TREE, "false() and $nosuch"), "false");
  assert_eq!(value(TREE, "true() or $nosuch"), "true");
}

#[test]
fn comparisons_convert_by_the_types_they_are_given() {
  let xml = "<r><n>1</n><n>2</n></r>";
  // A node-set compares by its members: true if any node makes it true.
  assert_eq!(value(xml, "/r/n = 2"), "true");
  assert_eq!(value(xml, "/r/n = 3"), "false");
  assert_eq!(value(xml, "/r/n > 1"), "true");
  assert_eq!(value(xml, "/r/n = '2'"), "true");
  assert_eq!(value(xml, "/r/n != 1"), "true", "the other node makes it true");
  // Against a boolean, an equality test asks only whether the node-set is empty.
  assert_eq!(value(xml, "/r/n = true()"), "true");
  assert_eq!(value(xml, "/r/nosuch = false()"), "true");
  // So does a relational test, which then compares the two booleans as numbers, 1 or 0, on either side.
  assert_eq!(value(xml, "/r/n > false()"), "true");
  assert_eq!(value(xml, "/r/n < true()"), "false");
  assert_eq!(value(xml, "/r/nosuch < true()"), "true");
  assert_eq!(value(xml, "false() < /r/n"), "true");
  assert_eq!(value(xml, "true() >= /r/n"), "true");
  // Against a number, each node's value is a number, and an infinity is one too.
  assert_eq!(value(xml, "/r/n < 1 div 0"), "true");
  assert_eq!(value(xml, "/r/n > -1 div 0"), "true");
  assert_eq!(value(xml, "/r/n != 1 div 0"), "true");
  assert_eq!(value(xml, "/r/n = 1 div 0"), "false");
  assert_eq!(value(xml, "1 div 0 > /r/n"), "true");
  assert_eq!(value("<r><n>Infinity</n></r>", "/r/n = 1 div 0"), "false", "the string Infinity is not a number");
  // Without a node-set, a boolean operand makes it a boolean comparison.
  assert_eq!(value(xml, "1 = true()"), "true");
  assert_eq!(value(xml, "'a' = 'a'"), "true");
  // A relational comparison converts to numbers, so 10 > 9 — though "10" sorts before "9".
  assert_eq!(value(xml, "'10' > '9'"), "true");
  assert_eq!(value(xml, "'10' = '9'"), "false", "an equality comparison of two strings compares strings");
}

#[test]
fn two_node_sets_compare_as_if_every_pair_were_tried() {
  // Strings, numbers, a value that is NaN as a number, the two zeros, an infinity, and padding a number reads past.
  let values = ["1", "2", "a", "", "-0", "0", "Infinity", " 2 "];
  let mut sets: Vec<Vec<&str>> = vec![Vec::new()];
  sets.extend(values.iter().map(|&value| vec![value]));
  for (i, &first) in values.iter().enumerate() {
    sets.extend(values[i + 1..].iter().map(|&second| vec![first, second]));
  }
  // What §3.4 says, pair by pair.
  let every_pair = |op: &str, a: &[&str], b: &[&str]| {
    a.iter().any(|x| {
      b.iter().any(|y| {
        let (p, q) = (object::string_to_number(x), object::string_to_number(y));
        match op {
          "=" => x == y,
          "!=" => x != y,
          "<" => p < q,
          "<=" => p <= q,
          ">" => p > q,
          ">=" => p >= q,
          _ => unreachable!("one of the six"),
        }
      })
    })
  };
  for a in &sets {
    for b in &sets {
      let elements =
        |name: &str, set: &[&str]| set.iter().map(|value| format!("<{name}>{value}</{name}>")).collect::<String>();
      let doc = parse_document(&format!("<r>{}{}</r>", elements("a", a), elements("b", b)));
      for op in ["=", "!=", "<", "<=", ">", ">="] {
        let xpath = XPath::parse(&format!("/r/a {op} /r/b"), Location::new()).expect("parses");
        let got = xpath.evaluate(&doc, doc.document_node()).expect("evaluates");
        assert_eq!(got, XPathResult::Boolean(every_pair(op, a, b)), "{a:?} {op} {b:?}");
      }
    }
  }
}

#[test]
fn large_node_sets_compare_without_trying_every_pair() {
  // Ten thousand nodes a side: every pair would be a hundred million string-values.
  let side =
    |name: &str, from: usize| (from..from + 10_000).map(|n| format!("<{name}>{n}</{name}>")).collect::<String>();
  let doc = parse_document(&format!("<r>{}{}</r>", side("a", 0), side("b", 10_000)));
  let compare = |expression: &str| {
    let xpath = XPath::parse(expression, Location::new()).expect("parses");
    xpath.evaluate(&doc, doc.document_node()).expect("evaluates")
  };
  assert_eq!(compare("/r/a = /r/b"), XPathResult::Boolean(false));
  assert_eq!(compare("/r/a != /r/b"), XPathResult::Boolean(true));
  assert_eq!(compare("/r/a < /r/b"), XPathResult::Boolean(true));
  assert_eq!(compare("/r/a > /r/b"), XPathResult::Boolean(false));
}

#[test]
fn a_union_merges_node_sets_into_document_order() {
  assert_eq!(names(TREE, "/r/y | /r/x"), "x,y");
  assert_eq!(names(TREE, "/r/a | /r/a"), "a", "a node-set holds a node once");
  assert_eq!(value(TREE, "count(//b | //c | //y)"), "3");
}

#[test]
fn a_path_may_continue_from_an_expression() {
  assert_eq!(names(TREE, "(/r/a)/b"), "b");
  assert_eq!(names(TREE, "(/r/a | /r/x)/*"), "x1,b,c");
}

#[test]
fn a_relative_path_starts_at_the_context_node() {
  let doc = parse_document(TREE);
  let r = doc.document_element().expect("the document element");
  let a = doc.children(r).nth(1).expect("<a>");
  let xpath = XPath::parse("*", Location::new()).expect("parses");
  let XPathResult::NodeSet(children) = xpath.evaluate(&doc, a).expect("evaluates") else { panic!("a node-set") };
  assert_eq!(children, doc.children(a).map(Node::Dom).collect::<Vec<_>>());
  let xpath = XPath::parse("count(/r/*)", Location::new()).expect("parses");
  assert_eq!(
    xpath.evaluate(&doc, a).expect("evaluates"),
    XPathResult::Number(3.0),
    "an absolute path starts at the root"
  );
}

#[test]
fn a_context_node_of_another_document_is_refused() {
  let doc = parse_document(TREE);
  let other = parse_document(TREE);
  let start = Location { line: 2, column: 5, ..Location::new() };
  let xpath = XPath::parse("*", start).expect("parses");
  let error = xpath.evaluate(&doc, other.document_node()).expect_err("another document's node");
  assert!(matches!(error, Error::XPath { .. }), "{error:?}");
  assert_eq!((error.location().line, error.location().column), (2, 5));
}

#[test]
fn a_context_node_outside_the_xpath_tree_is_refused() {
  let mut doc = parse_document("<!DOCTYPE r><r xmlns:p='urn:p' a='1'/>");
  let r = doc.document_element().expect("the document element");
  let detached = doc.create_element("d").expect("an element");
  doc.set_attribute(detached, "x", "1").expect("an attribute");
  let fragment = doc.create_document_fragment().expect("created");
  let in_fragment = doc.create_element("f").expect("an element");
  doc.append_child(fragment, in_fragment).expect("appended");
  let refused = [
    ("a document type", doc.doctype().expect("a document type")),
    ("a namespace declaration", doc.get_attribute_node(r, "xmlns:p").expect("the declaration")),
    ("a node not inserted", detached),
    ("an attribute of a node not inserted", doc.get_attribute_node(detached, "x").expect("the attribute")),
    ("a fragment", fragment),
    ("a node in a fragment", in_fragment),
  ];
  let xpath = XPath::parse("self::node()", Location::new()).expect("parses");
  for (what, node) in refused {
    let error = xpath.evaluate(&doc, node).expect_err(what);
    assert!(error.message().contains("not a node in the XPath tree"), "{what}: {error}");
  }
  assert!(xpath.evaluate(&doc, doc.get_attribute_node(r, "a").expect("the attribute")).is_ok());
  // Once inserted, the node is in the tree.
  doc.append_child(r, detached).expect("appended");
  assert!(xpath.evaluate(&doc, detached).is_ok());
  assert!(xpath.evaluate(&doc, doc.get_attribute_node(detached, "x").expect("the attribute")).is_ok());
}

#[test]
fn a_node_a_function_returns_from_outside_the_tree_is_refused() {
  let mut doc = parse_document(TREE);
  let detached = doc.create_element("d").expect("an element");
  let context = XPathContext::new()
    .with_namespace("ex", "urn:example")
    .and_then(|context| {
      context.with_function("urn:example", "detached", 0, Some(0), move |_, _| {
        Ok(XPathResult::NodeSet(vec![Node::Dom(detached)]))
      })
    })
    .expect("defines");
  let xpath = context.parse("ex:detached()", Location::new()).expect("parses");
  let error = xpath.evaluate(&doc, doc.document_node()).expect_err("a node outside the tree");
  assert!(error.message().contains("not in the XPath tree"), "{error}");
}

#[test]
fn evaluation_errors_say_what_the_context_could_not_supply() {
  assert!(error(TREE, "$nosuch").contains("not bound"), "{}", error(TREE, "$nosuch"));
  assert!(error(TREE, "1 | 2").contains("union joins node-sets"), "{}", error(TREE, "1 | 2"));
  assert!(error(TREE, "(1)/a").contains("continue from a node-set"), "{}", error(TREE, "(1)/a"));
  assert!(error(TREE, "count(1)").contains("needs a node-set"), "{}", error(TREE, "count(1)"));
}

#[test]
fn an_evaluation_error_is_located_where_the_expression_starts() {
  let doc = parse_document(TREE);
  let start = Location { line: 4, column: 2, ..Location::new() };
  let error = XPath::parse("$nosuch", start).expect("parses").evaluate(&doc, doc.document_node()).expect_err("fails");
  assert_eq!((error.location().line, error.location().column), (4, 2));
}

// --- The core function library (§4) -----------------------------------------------------------

const DOC: &str = "<r xmlns:p='urn:p'><a>one</a><p:b>two</p:b><n>1</n><n>2</n><n>3</n></r>";

#[test]
fn node_set_functions_report_names_and_counts() {
  assert_eq!(value(DOC, "count(/r/*)"), "5");
  assert_eq!(value(DOC, "local-name(/r/*[2])"), "b");
  assert_eq!(value(DOC, "namespace-uri(/r/*[2])"), "urn:p");
  assert_eq!(value(DOC, "name(/r/*[2])"), "p:b", "name keeps the prefix as written");
  assert_eq!(value(DOC, "name(/r/a)"), "a");
  // An empty node-set has no name at all.
  assert_eq!(value(DOC, "name(/r/nosuch)"), "");
  assert_eq!(value(DOC, "local-name(/r/nosuch)"), "");
  // With no argument the functions describe the context node, which a predicate supplies.
  assert_eq!(value(DOC, "count(/r/*[name() = 'p:b'])"), "1");
  assert_eq!(value(DOC, "count(/r/*[local-name() = 'b'])"), "1");
  assert_eq!(value(DOC, "count(/r/n)"), "3");
}

#[test]
fn id_selects_the_elements_a_dtd_typed_as_ids() {
  let xml = "<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT i EMPTY><!ATTLIST i k ID #IMPLIED>]><r><i k='a'/><i k='b'/></r>";
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
fn a_call_that_cannot_be_made_is_refused_when_it_is_parsed() {
  // The name and the number of arguments are settled by the expression alone, so no document is needed to refuse them.
  assert!(parse_message("nosuch()").contains("no function named"), "{}", parse_message("nosuch()"));
  assert!(parse_message("concat('a')").contains("at least 2 arguments"), "{}", parse_message("concat('a')"));
  assert!(parse_message("substring('a')").contains("2 or 3 arguments"), "{}", parse_message("substring('a')"));
  assert!(parse_message("floor(1, 2)").contains("takes 1 argument"), "{}", parse_message("floor(1, 2)"));
  assert!(parse_message("position(1)").contains("takes 0 arguments"), "{}", parse_message("position(1)"));
  // A prefixed name is an extension function, so the complaint is about the prefix first.
  assert!(parse_message("ext:f()").contains("prefix \"ext\""), "{}", parse_message("ext:f()"));
  assert!(parse_message("xml:f()").contains("no extension function"), "{}", parse_message("xml:f()"));
  // The arguments are not evaluated first: an unbound variable in one does not hide the wrong number of them.
  assert!(parse_message("floor(1, $x)").contains("takes 1 argument"), "{}", parse_message("floor(1, $x)"));
  // Located at the function's name.
  let error = parse_error("1 + nosuch()");
  assert_eq!((error.location().line, error.location().column), (1, 5));
}

#[test]
fn an_argument_of_the_wrong_type_is_refused_when_it_is_evaluated() {
  assert!(error(DOC, "sum(1)").contains("needs a node-set"), "{}", error(DOC, "sum(1)"));
  assert!(error(DOC, "name(1)").contains("needs a node-set"), "{}", error(DOC, "name(1)"));
}

#[test]
fn every_function_the_parser_accepts_can_be_called() {
  // Each with as few arguments as it takes, all of them the root, which every function can take.
  for (name, min, _) in functions::SIGNATURES {
    let arguments = vec!["/"; *min].join(", ");
    let expression = format!("{name}({arguments})");
    let (_, result) = evaluate(DOC, &expression);
    assert!(result.is_ok(), "{expression}: {result:?}");
  }
}

#[test]
fn round_does_not_lose_the_largest_double_below_one_half_to_the_addition() {
  assert_eq!(value(DOC, "round(0.49999999999999994)"), "0");
  assert_eq!(value(DOC, "1 div round(-0.49999999999999994)"), "-Infinity", "and rounds to negative zero below zero");
  assert_eq!(value(DOC, "round(2.5)"), "3");
  assert_eq!(value(DOC, "round(-2.5)"), "-2");
}

#[test]
fn id_finds_the_first_element_with_each_id() {
  let xml = "<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT i EMPTY><!ATTLIST i k ID #IMPLIED n CDATA #IMPLIED>]>\
             <r><i k='a' n='1'/><i k='b' n='2'/><i k='c' n='3'/></r>";
  assert_eq!(text(xml, "id('c a b a')/@n"), "1,2,3", "each found once, in document order");
  assert_eq!(text(xml, "id(//i[2]/@k)/@n"), "2", "a node-set argument gives its string-values");
}

// --- XPathContext --------------------------------------------------------------------------------

/// An `XPathContext` that binds `ex` to `urn:example`.
fn example() -> XPathContext {
  XPathContext::new().with_namespace("ex", "urn:example").expect("binds")
}

/// Evaluates `expression`, parsed in `context`, over `xml` from the document node.
fn evaluate_in(context: &XPathContext, xml: &str, expression: &str) -> (Document, Result<XPathResult>) {
  let doc = parse_document(xml);
  let xpath = context.parse(expression, Location::new()).expect("parses");
  let value = xpath.evaluate(&doc, doc.document_node());
  (doc, value)
}

#[test]
fn a_bound_prefix_selects_by_its_namespace_not_by_the_document_s_prefix() {
  let xml = "<r xmlns:a='urn:example'><a:x/><x/><a:y/></r>";
  let (doc, result) = evaluate_in(&example(), xml, "count(//ex:x) + 10 * count(/r/ex:*)");
  assert_eq!(result.expect("evaluates").string(&doc).expect("a string"), "21");
  let error = example().parse("//a:x", Location::new()).expect_err("the document's prefix is not bound");
  assert!(error.message().contains("prefix \"a\" is not bound"), "{error}");
}

#[test]
fn an_application_defined_function_is_called_with_its_arguments_and_the_context() {
  let context = example()
    .with_function("urn:example", "upper", 1, Some(1), |arguments, call| {
      Ok(XPathResult::String(arguments[0].string(call.document())?.to_uppercase()))
    })
    .expect("defines")
    .with_function("urn:example", "where", 0, Some(0), |_, call| {
      Ok(XPathResult::String(format!("{} of {}", call.position(), call.size())))
    })
    .expect("defines")
    .with_function("urn:example", "here", 0, Some(0), |_, call| Ok(XPathResult::NodeSet(vec![call.node().clone()])))
    .expect("defines");
  let xml = "<r><a>one</a><b>two</b></r>";
  let (doc, result) = evaluate_in(&context, xml, "ex:upper(/r/b)");
  assert_eq!(result.expect("evaluates").string(&doc).expect("a string"), "TWO");
  let (doc, result) = evaluate_in(&context, xml, "/r/*[ex:where() = '2 of 2']");
  assert_eq!(result.expect("evaluates").string(&doc).expect("a string"), "two");
  let (doc, result) = evaluate_in(&context, xml, "/r/*[name(ex:here()) = 'b']");
  assert_eq!(result.expect("evaluates").string(&doc).expect("a string"), "two");
  let (doc, result) = evaluate_in(&context, xml, "count(ex:here()/r/*)");
  assert_eq!(result.expect("evaluates").string(&doc).expect("a string"), "2", "a node-set it returns is walked on");
}

#[test]
fn a_node_set_a_function_returns_is_put_in_document_order_without_duplicates() {
  let context = example()
    .with_function("urn:example", "shuffled", 0, Some(0), |_, call| {
      let doc = call.document();
      let r = doc.first_child(doc.document_node()).expect("the document element");
      let mut children: Vec<_> = doc.children(r).map(Node::Dom).collect();
      children.reverse();
      children.extend(children.clone());
      Ok(XPathResult::NodeSet(children))
    })
    .expect("defines");
  let (doc, result) = evaluate_in(&context, "<r><a/><b/><c/></r>", "ex:shuffled()");
  let r = doc.first_child(doc.document_node()).expect("the document element");
  assert_eq!(result.expect("evaluates"), XPathResult::NodeSet(doc.children(r).map(Node::Dom).collect()));
}

#[test]
fn a_node_a_function_returns_from_another_document_is_refused_at_the_call() {
  let other = parse_document("<o/>");
  let stranger = Node::Dom(other.document_node());
  let context = example()
    .with_function("urn:example", "stranger", 0, Some(0), move |_, _| Ok(XPathResult::NodeSet(vec![stranger.clone()])))
    .expect("defines");
  let (_, result) = evaluate_in(&context, TREE, "1 + count(ex:stranger())");
  let error = result.expect_err("another document's node");
  assert!(matches!(error, Error::XPath { .. }), "{error:?}");
  assert!(error.message().contains("not in the XPath tree"), "{error}");
  assert_eq!((error.location().line, error.location().column), (1, 11), "at the function's name");
}

#[test]
fn an_error_a_function_returns_is_located_at_the_call() {
  let context =
    example().with_function("urn:example", "fail", 0, None, |_, _| Err(Error::xpath("refused"))).expect("defines");
  let (_, result) = evaluate_in(&context, TREE, "1 + ex:fail(1, 2, 3)");
  let error = result.expect_err("fails");
  assert_eq!(error.message(), "refused");
  assert_eq!((error.location().line, error.location().column), (1, 5));
}

#[test]
fn a_call_to_an_application_defined_function_is_checked_when_it_is_parsed() {
  let context =
    example().with_function("urn:example", "f", 1, Some(3), |_, _| Ok(XPathResult::Boolean(true))).expect("defines");
  let message = |expression| context.parse(expression, Location::new()).expect_err(expression).message().to_owned();
  assert!(message("ex:f()").contains("\"ex:f()\" takes 1 to 3 arguments"), "{}", message("ex:f()"));
  assert!(message("ex:f(1, 2, 3, 4)").contains("1 to 3 arguments"), "{}", message("ex:f(1, 2, 3, 4)"));
  assert!(message("ex:g()").contains("no extension function {urn:example}g"), "{}", message("ex:g()"));
  assert!(context.parse("ex:f(1, 2, 3)", Location::new()).is_ok());
}

#[test]
fn a_parsed_expression_keeps_the_context_it_was_parsed_in() {
  let first = example().with_function("urn:example", "f", 0, Some(0), |_, _| Ok(XPathResult::Number(1.0))).expect("1");
  let xpath = first.parse("ex:f()", Location::new()).expect("parses");
  // Redefining the function in a copy changes neither the original context nor what was parsed in it.
  let second = first.clone().with_function("urn:example", "f", 0, Some(0), |_, _| Ok(XPathResult::Number(2.0)));
  let second = second.expect("2").parse("ex:f()", Location::new()).expect("parses");
  let doc = parse_document(TREE);
  assert_eq!(xpath.evaluate(&doc, doc.document_node()).expect("evaluates"), XPathResult::Number(1.0));
  assert_eq!(second.evaluate(&doc, doc.document_node()).expect("evaluates"), XPathResult::Number(2.0));
  let third = first.parse("ex:f()", Location::new()).expect("parses");
  assert_eq!(third.evaluate(&doc, doc.document_node()).expect("evaluates"), XPathResult::Number(1.0));
}

#[test]
fn a_binding_or_a_definition_that_cannot_be_used_is_refused() {
  let bind = |prefix, uri| XPathContext::new().with_namespace(prefix, uri).map(|_| ());
  assert!(matches!(bind("1p", "urn:a"), Err(Error::Name { .. })));
  assert!(matches!(bind("p:q", "urn:a"), Err(Error::Name { .. })));
  assert!(matches!(bind("xmlns", "urn:a"), Err(Error::Namespace { .. })));
  assert!(matches!(bind("xml", "urn:a"), Err(Error::Namespace { .. })));
  assert!(matches!(bind("p", ""), Err(Error::Namespace { .. })));
  assert!(matches!(bind("p", XML_NS_URI), Err(Error::Namespace { .. })));
  assert!(matches!(bind("p", XMLNS_NS_URI), Err(Error::Namespace { .. })));
  assert!(bind("xml", XML_NS_URI).is_ok());
  let define = |namespace, local, min, max| {
    XPathContext::new().with_function(namespace, local, min, max, |_, _| Ok(XPathResult::Boolean(true))).map(|_| ())
  };
  assert!(matches!(define("urn:a", "a:b", 0, None), Err(Error::Name { .. })));
  assert!(matches!(define("", "f", 0, None), Err(Error::Namespace { .. })));
  assert!(matches!(define("urn:a", "f", 2, Some(1)), Err(Error::Internal { .. })));
  assert!(define("urn:a", "f", 2, Some(2)).is_ok());
}

#[test]
fn a_parsed_expression_can_be_shared_between_threads() {
  const fn shareable<T: Send + Sync>() {}
  shareable::<XPath>();
  shareable::<XPathContext>();
}

// --- Reading a result ---------------------------------------------------------------------------

#[test]
fn node_is_the_first_node_of_a_node_set() {
  let (doc, result) = evaluate(TREE, "/r/*");
  let r = doc.document_element().expect("the document element");
  let x = doc.first_child(r).expect("<x>");
  assert_eq!(result.expect("evaluates").node(), Some(&Node::Dom(x)));
  assert_eq!(evaluate(TREE, "/r/nosuch").1.expect("evaluates").node(), None, "an empty node-set");
  assert_eq!(evaluate(TREE, "count(/r/*)").1.expect("evaluates").node(), None, "not a node-set");
}

#[test]
fn boolean_converts_as_the_boolean_function_does() {
  assert!(XPathResult::NodeSet(vec![Node::Dom(parse_document(TREE).document_node())]).boolean());
  assert!(!XPathResult::NodeSet(Vec::new()).boolean());
  assert!(XPathResult::Number(-1.0).boolean());
  assert!(!XPathResult::Number(0.0).boolean());
  assert!(!XPathResult::Number(f64::NAN).boolean());
  assert!(XPathResult::String("false".to_owned()).boolean(), "a string is true when it is not empty");
  assert!(!XPathResult::String(String::new()).boolean());
}

#[test]
fn string_converts_as_the_string_function_does() {
  let doc = parse_document("<r xmlns:p='urn:p'>one<![CDATA[two]]><a k='v'/></r>");
  assert_eq!(XPathResult::Boolean(true).string(&doc).expect("a string"), "true");
  assert_eq!(XPathResult::Number(1.5).string(&doc).expect("a string"), "1.5");
  assert_eq!(XPathResult::Number(f64::INFINITY).string(&doc).expect("a string"), "Infinity");
  assert_eq!(XPathResult::NodeSet(Vec::new()).string(&doc).expect("a string"), "");
  // A node-set is the string-value of its first node: a text node reads its whole run.
  let r = doc.document_element().expect("the document element");
  let first = doc.first_child(r).expect("the text");
  assert_eq!(XPathResult::NodeSet(vec![Node::Dom(first)]).string(&doc).expect("a string"), "onetwo");
  assert_eq!(XPathResult::NodeSet(vec![Node::Dom(r)]).string(&doc).expect("a string"), "onetwo");
  // A namespace node is the namespace it binds.
  let namespace = Node::Namespace { element: r, prefix: Some("p".to_owned()) };
  assert_eq!(XPathResult::NodeSet(vec![namespace]).string(&doc).expect("a string"), "urn:p");
}

#[test]
fn string_refuses_a_node_of_another_document() {
  let doc = parse_document(TREE);
  let other = parse_document(TREE);
  let result = XPathResult::NodeSet(vec![Node::Dom(other.document_node())]);
  let error = result.string(&doc).expect_err("another document's node");
  assert!(matches!(error, Error::XPath { .. }), "{error:?}");
}

#[test]
fn string_refuses_a_node_outside_the_xpath_tree_as_evaluate_does() {
  let mut doc = parse_document("<!DOCTYPE r><r xmlns:p='urn:p'><s xmlns:q='urn:q'/></r>");
  let r = doc.document_element().expect("the document element");
  let detached = doc.create_text_node("loose").expect("created");
  let refused = [
    ("a document type", Node::Dom(doc.doctype().expect("a document type"))),
    ("a namespace declaration", Node::Dom(doc.get_attribute_node(r, "xmlns:p").expect("the declaration"))),
    ("a node not inserted", Node::Dom(detached)),
    ("a prefix bound nowhere", Node::Namespace { element: r, prefix: Some("nosuch".to_owned()) }),
    ("a prefix bound only below", Node::Namespace { element: r, prefix: Some("q".to_owned()) }),
  ];
  for (what, node) in refused {
    let error = XPathResult::NodeSet(vec![node]).string(&doc).expect_err(what);
    assert!(error.message().contains("not a node in the XPath tree"), "{what}: {error}");
  }
}

// --- Variables ----------------------------------------------------------------------------------

/// Evaluates `expression`, parsed in `context`, over `doc` from the document node with `variables`.
fn evaluate_vars(
  context: &XPathContext,
  doc: &Document,
  expression: &str,
  variables: &XPathVariables,
) -> Result<String> {
  let xpath = context.parse(expression, Location::new()).expect("parses");
  xpath.evaluate_with(doc, doc.document_node(), variables)?.string(doc)
}

#[test]
fn a_variable_passed_to_the_evaluation_is_referred_to_by_its_expanded_name() {
  let doc = parse_document("<r><a>1</a><b>2</b></r>");
  let mut variables = XPathVariables::new();
  variables.set(None, "x", XPathResult::Number(40.0)).expect("set");
  variables.set(Some("urn:example"), "x", XPathResult::Number(2.0)).expect("set");
  assert_eq!(evaluate_vars(&example(), &doc, "$x + $ex:x", &variables).expect("evaluates"), "42");
  // A node-set value is put into document order without duplicates, and can be walked on.
  let r = doc.document_element().expect("the document element");
  let mut children: Vec<_> = doc.children(r).map(Node::Dom).collect();
  children.reverse();
  children.extend(children.clone());
  variables.set(None, "n", XPathResult::NodeSet(children)).expect("set");
  assert_eq!(evaluate_vars(&example(), &doc, "concat(count($n), $n, $n[2])", &variables).expect("evaluates"), "212");
  assert_eq!(evaluate_vars(&example(), &doc, "count($n/self::b)", &variables).expect("evaluates"), "1");
  assert_eq!(variables.remove(None, "x"), Some(XPathResult::Number(40.0)));
  assert_eq!(variables.remove(None, "x"), None);
}

#[test]
fn a_variable_that_is_not_bound_is_an_error_only_where_it_is_evaluated() {
  let doc = parse_document(TREE);
  let variables = XPathVariables::new();
  assert_eq!(evaluate_vars(&example(), &doc, "false() and $nosuch", &variables).expect("not evaluated"), "false");
  let error = evaluate_vars(&example(), &doc, "1 + $nosuch", &variables).expect_err("unbound");
  assert!(error.message().contains("\"$nosuch\" is not bound"), "{error}");
  assert_eq!((error.location().line, error.location().column), (1, 5));
}

#[test]
fn a_node_a_variable_holds_from_outside_the_tree_is_refused_where_it_is_referred_to() {
  let mut doc = parse_document(TREE);
  let detached = doc.create_element("d").expect("an element");
  let mut variables = XPathVariables::new();
  variables.set(None, "loose", XPathResult::NodeSet(vec![Node::Dom(detached)])).expect("set");
  assert_eq!(evaluate_vars(&example(), &doc, "false() and $loose", &variables).expect("not referred to"), "false");
  let error = evaluate_vars(&example(), &doc, "1 + count($loose)", &variables).expect_err("outside the tree");
  assert!(error.message().contains("\"$loose\" holds a node that is not in the XPath tree"), "{error}");
  assert_eq!((error.location().line, error.location().column), (1, 11));
}

#[test]
fn a_defined_variable_is_evaluated_from_the_document_node() {
  let doc = parse_document("<r><a><b/></a></r>");
  let top = XPath::parse("name(*)", Location::new()).expect("parses");
  let context = example().with_variable(None, "top", top).expect("defines");
  let variables = XPathVariables::new();
  assert_eq!(evaluate_vars(&context, &doc, "count(//b[$top = 'r'])", &variables).expect("evaluates"), "1");
}

#[test]
fn a_definition_sees_the_definitions_before_it_and_the_values_passed() {
  let doc = parse_document(TREE);
  let context = example();
  let first = context.parse("10", Location::new()).expect("parses");
  let context = context.with_variable(Some("urn:example"), "first", first).expect("defines");
  let second = context.parse("$ex:first + $given", Location::new()).expect("parses");
  let context = context.with_variable(Some("urn:example"), "second", second).expect("defines");
  let mut variables = XPathVariables::new();
  variables.set(None, "given", XPathResult::Number(5.0)).expect("set");
  assert_eq!(evaluate_vars(&context, &doc, "$ex:second", &variables).expect("evaluates"), "15");
  // A definition cannot see itself or what is defined after it, so definitions never go round in a circle.
  let own = context.parse("$ex:own", Location::new()).expect("parses");
  let context = context.with_variable(Some("urn:example"), "own", own).expect("defines");
  let error = evaluate_vars(&context, &doc, "$ex:own", &variables).expect_err("not bound in its own definition");
  assert!(error.message().contains("\"$ex:own\" is not bound"), "{error}");
}

#[test]
fn a_value_passed_hides_a_definition_of_the_same_name() {
  let doc = parse_document(TREE);
  let defined = XPath::parse("'defined'", Location::new()).expect("parses");
  let context = example().with_variable(None, "v", defined).expect("defines");
  let mut variables = XPathVariables::new();
  assert_eq!(evaluate_vars(&context, &doc, "$v", &variables).expect("evaluates"), "defined");
  variables.set(None, "v", XPathResult::String("passed".to_owned())).expect("set");
  assert_eq!(evaluate_vars(&context, &doc, "$v", &variables).expect("evaluates"), "passed");
}

#[test]
fn a_definition_is_evaluated_once_in_each_evaluation() {
  use std::sync::atomic::{AtomicUsize, Ordering};
  let calls = Arc::new(AtomicUsize::new(0));
  let counted = Arc::clone(&calls);
  let context = example()
    .with_function("urn:example", "tick", 0, Some(0), move |_, _| {
      Ok(XPathResult::Number(counted.fetch_add(1, Ordering::SeqCst) as f64))
    })
    .expect("defines");
  let tick = context.parse("ex:tick()", Location::new()).expect("parses");
  let context = context.with_variable(None, "t", tick).expect("defines");
  let doc = parse_document(TREE);
  let xpath = context.parse("concat($t, $t, count(//*[$t = 0]))", Location::new()).expect("parses");
  let variables = XPathVariables::new();
  let value = xpath.evaluate_with(&doc, doc.document_node(), &variables).expect("evaluates").string(&doc);
  assert_eq!(value.expect("a string"), "007", "the same value at every reference");
  assert_eq!(calls.load(Ordering::SeqCst), 1);
  let value = xpath.evaluate_with(&doc, doc.document_node(), &variables).expect("evaluates").string(&doc);
  assert_eq!(value.expect("a string"), "110", "evaluated again in the next evaluation");
  assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn an_error_in_a_definition_is_located_in_the_definition() {
  let doc = parse_document(TREE);
  let start = Location { line: 9, column: 3, ..Location::new() };
  let broken = XPath::parse("1 + $nosuch", start).expect("parses");
  let context = example().with_variable(None, "broken", broken).expect("defines");
  let error = evaluate_vars(&context, &doc, "$broken", &XPathVariables::new()).expect_err("fails");
  assert_eq!((error.location().line, error.location().column), (9, 7));
}

#[test]
fn a_variable_name_that_cannot_be_used_is_refused() {
  let mut variables = XPathVariables::new();
  assert!(matches!(variables.set(None, "a:b", XPathResult::Boolean(true)), Err(Error::Name { .. })));
  assert!(matches!(variables.set(Some(""), "a", XPathResult::Boolean(true)), Err(Error::Namespace { .. })));
  let definition = || XPath::parse("1", Location::new()).expect("parses");
  assert!(matches!(XPathContext::new().with_variable(None, "1a", definition()), Err(Error::Name { .. })));
  assert!(matches!(XPathContext::new().with_variable(Some(""), "a", definition()), Err(Error::Namespace { .. })));
}

// --- Limits -------------------------------------------------------------------------------------

/// Expressions of each kind of nesting, `depth` levels deep counting the expression itself.
fn nested(depth: usize) -> [(&'static str, String); 6] {
  let inner = depth - 1;
  [
    ("parentheses", format!("{}1{}", "(".repeat(inner), ")".repeat(inner))),
    ("predicates", format!("{}1{}", "e[".repeat(inner), "]".repeat(inner))),
    ("calls", format!("{}1{}", "boolean(".repeat(inner), ")".repeat(inner))),
    ("unary minus", format!("{}1", "-".repeat(inner))),
    ("operators", format!("1{}", " + 1".repeat(inner))),
    ("unions", format!("e{}", " | e".repeat(inner))),
  ]
}

#[test]
fn every_kind_of_nesting_counts_toward_the_depth() {
  let context = XPathContext::new().with_limits(Limits { max_depth: Some(10), ..Limits::default() });
  for (kind, expression) in nested(10) {
    assert!(context.parse(&expression, Location::new()).is_ok(), "{kind} 10 levels deep: {expression}");
  }
  for (kind, expression) in nested(11) {
    let error = context.parse(&expression, Location::new()).expect_err(kind);
    assert!(matches!(error, Error::Limit { .. }), "{kind}: {error:?}");
    assert!(error.message().contains("deeper than 10"), "{kind}: {error}");
  }
  // The depth is how deep the expression nests, not how long it is.
  assert!(context.parse(&format!("concat({})", vec!["1"; 100].join(", ")), Location::new()).is_ok());
  assert!(context.parse(&format!("{}a", "a/".repeat(100)), Location::new()).is_ok());
}

#[test]
fn unlimited_takes_any_depth() {
  let context = XPathContext::new().with_limits(Limits::unlimited());
  for (kind, expression) in nested(65) {
    assert!(XPath::parse(&expression, Location::new()).is_err(), "{kind} beyond the default");
    assert!(context.parse(&expression, Location::new()).is_ok(), "{kind}");
  }
  assert_eq!(Limits::default().max_depth, Some(64));
}

#[test]
fn a_depth_error_is_located_where_the_limit_is_passed() {
  let context = XPathContext::new().with_limits(Limits { max_depth: Some(3), ..Limits::default() });
  // The third `(` opens the fourth level, which the error points at: the `1` that begins it.
  let error = context.parse("((( 1 )))", Location::new()).expect_err("too deep");
  assert_eq!((error.location().line, error.location().column), (1, 5));
  // A chain of operators is measured once it is parsed, and located at the start of the expression.
  let start = Location { line: 5, column: 7, ..Location::new() };
  let error = context.parse("1 + 1 + 1 + 1", start).expect_err("too deep");
  assert_eq!((error.location().line, error.location().column), (5, 7));
}

/// `context` with `$v0` defined as `1` and each `$vN` after it, up to `$v(length - 1)`, as `step` with `{}` replaced
/// by a reference to the one before it.
fn chained(mut context: XPathContext, length: usize, step: &str) -> XPathContext {
  for n in 0..length {
    let expression = if n == 0 { "1".to_owned() } else { step.replace("{}", &format!("$v{}", n - 1)) };
    let definition = context.parse(&expression, Location::new()).expect("parses");
    context = context.with_variable(None, &format!("v{n}"), definition).expect("defines");
  }
  context
}

#[test]
fn a_chain_of_definitions_counts_toward_the_depth() {
  // `$v0` is one level deep and each definition after it one more, so a reference to `$v8` is ten levels deep.
  let context = chained(XPathContext::new().with_limits(Limits { max_depth: Some(10), ..Limits::default() }), 9, "{}");
  assert!(context.parse("$v8", Location::new()).is_ok());
  // So is a definition that would make the chain too deep, which is refused before it can be defined.
  let error = context.parse("-$v8", Location::new()).expect_err("eleven levels deep");
  assert!(matches!(error, Error::Limit { .. }), "{error:?}");
  assert!(error.message().contains("deeper than 10"), "{error}");
  // A value passed to the evaluation only hides a definition, so the depth counted is the most it can take.
  let mut variables = XPathVariables::new();
  variables.set(None, "v8", XPathResult::Number(1.0)).expect("set");
  let doc = parse_document(TREE);
  let xpath = context.parse("$v8", Location::new()).expect("parses");
  assert_eq!(xpath.evaluate_with(&doc, doc.document_node(), &variables).expect("evaluates"), XPathResult::Number(1.0));
}

#[test]
fn a_parsed_expression_keeps_only_the_definitions_it_refers_to() {
  let context = chained(XPathContext::new(), 30, "{}");
  // Were the whole table kept, every definition would share it, and each later definition would copy it.
  assert_eq!(Arc::strong_count(&context.variables), 1, "no definition shares the table");
  let xpath = context.parse("$v10 + $v20", Location::new()).expect("parses");
  assert_eq!(xpath.context.variable_count(), 2);
  let doc = parse_document(TREE);
  assert_eq!(
    xpath.evaluate(&doc, doc.document_node()).expect("evaluates through the chains"),
    XPathResult::Number(2.0)
  );
}

#[test]
fn the_number_of_definitions_is_limited() {
  let context = XPathContext::new().with_limits(Limits { max_variables: Some(2), ..Limits::default() });
  let one = || XPath::parse("1", Location::new()).expect("parses");
  let context = context.with_variable(None, "a", one()).expect("one");
  let context = context.with_variable(Some("urn:example"), "a", one()).expect("two, in another namespace");
  let context = context.with_variable(None, "a", one()).expect("redefining a name does not count again");
  let error = context.with_variable(None, "b", one()).expect_err("a third name");
  assert!(matches!(error, Error::Limit { .. }), "{error:?}");
  assert!(error.message().contains("more than 2 variables"), "{error}");
  assert!(XPathContext::new().with_limits(Limits::unlimited()).with_variable(None, "a", one()).is_ok());
  assert_eq!(Limits::default().max_variables, Some(2048));
}

#[test]
fn expressions_at_the_default_depth_fit_in_a_one_mib_stack() {
  // 1 MiB is the stack of the main thread on Windows, the smallest a caller is likely to run on. Every kind of nesting
  // at the default depth is parsed, evaluated against a tree deep enough for each predicate to be evaluated, printed
  // back and dropped there. If the default outgrows the stack, this aborts the test run instead of failing quietly.
  let handle = std::thread::Builder::new()
    .stack_size(1024 * 1024)
    .spawn(|| {
      let mut doc = Document::new();
      let mut parent = doc.document_node();
      for _ in 0..DEFAULT_MAX_DEPTH {
        let element = doc.create_element("e").expect("a name");
        doc.append_child(parent, element).expect("a child");
        parent = element;
      }
      for (kind, expression) in nested(DEFAULT_MAX_DEPTH) {
        let xpath = XPath::parse(&expression, Location::new()).expect(kind);
        let _ = xpath.evaluate(&doc, doc.document_node());
        let _ = (xpath.to_string(), format!("{xpath:#}"));
        drop(xpath);
      }
      // A chain of definitions as deep as the default allows: one level for each, or two with a predicate each.
      for (kind, step, length) in
        [("a chain of definitions", "{}", 63), ("predicates through definitions", "e[{}]", 32)]
      {
        let context = chained(XPathContext::new(), length, step);
        let xpath = context.parse(&format!("$v{}", length - 1), Location::new()).expect(kind);
        xpath.evaluate(&doc, doc.document_node()).expect(kind);
      }
    })
    .expect("a thread");
  handle.join().expect("no stack overflow");
}

#[test]
fn a_document_tree_of_any_depth_is_walked_without_recursion() {
  // A tree built by hand has no depth limit, so the axes and document order walk it with a stack of their own. This is
  // over twice as deep as recursion survived on a 1 MiB stack in a debug build (about 1,300 levels).
  const DEPTH: usize = 3_000;
  let handle = std::thread::Builder::new()
    .stack_size(1024 * 1024)
    .spawn(|| {
      let mut doc = Document::new();
      let mut parent = doc.document_node();
      for _ in 0..DEPTH {
        let element = doc.create_element("e").expect("a name");
        doc.append_child(parent, element).expect("a child");
        parent = element;
      }
      let text = doc.create_text_node("leaf").expect("created");
      doc.append_child(parent, text).expect("a child");
      let value = |expression: &str| {
        let xpath = XPath::parse(expression, Location::new()).expect("parses");
        xpath.evaluate(&doc, doc.document_node()).expect("evaluates")
      };
      assert_eq!(value("count(//*)"), XPathResult::Number(DEPTH as f64));
      assert_eq!(value("count(/descendant::e[last()]/preceding::node() | //e)"), XPathResult::Number(DEPTH as f64));
      assert_eq!(value("count(/e/following::node())"), XPathResult::Number(0.0));
      assert_eq!(value("string(/)"), XPathResult::String("leaf".to_owned()));
    })
    .expect("a thread");
  handle.join().expect("no stack overflow");
}

#[test]
fn an_unbound_prefix_is_refused_where_it_is_written() {
  for (expression, column) in [("//q:a", 3), ("/r/q:*", 4), ("1 + $q:x", 5)] {
    let error = parse_error(expression);
    assert!(error.message().contains("prefix \"q\" is not bound"), "{expression}: {error}");
    assert_eq!(error.location().column, column, "{expression}");
  }
}

#[test]
fn an_evaluation_error_is_located_at_what_raised_it() {
  let doc = parse_document(TREE);
  // The expression is written from line 3, column 10, as it would be in an attribute value.
  let start = Location { line: 3, column: 10, offset: 40, ..Location::new() };
  for (expression, offset, raised) in [
    ("1 + sum(1)", 4, "sum"),
    ("1 = $nosuch", 4, "$"),
    ("/r | 2", 3, "|"),
    ("1 + ('a')/b", 4, "the path"),
    ("1 + ('a')[1]", 4, "the filter"),
  ] {
    let xpath = XPath::parse(expression, start.clone()).expect(expression);
    let error = xpath.evaluate(&doc, doc.document_node()).expect_err(expression);
    assert_eq!((error.location().line, error.location().column), (3, 10 + offset), "{expression}: at {raised}");
  }
}
