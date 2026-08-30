//! Evaluating XPath 1.0 expressions: axes, node tests, predicates, operators and conversions.

use xenolith_core::Error;
use xenolith_dom::build;
use xenolith_xdm::{DomModel, DomNode, Model, NodeKind};
use xenolith_xpath::{Value, Variables, XPath};

/// Evaluates `expression` over `xml` and hands the result to `render`. The prefix `p` and the
/// variable `$want` are bound, since the cases below use them.
fn with<T>(xml: &str, expression: &str, render: impl FnOnce(&DomModel<'_>, Value<DomNode>) -> T) -> T {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().with_namespace("p", "urn:p").compile(expression).expect("parses");
  let variables = Variables::new().with("want", Value::String("2".into()));
  let value = query.evaluate_with(&model, model.root_node(), &variables).expect("evaluates");
  render(&model, value)
}

/// The names of the nodes an expression selects, in document order.
fn names(xml: &str, expression: &str) -> String {
  with(xml, expression, |model, value| match value {
    Value::NodeSet(nodes) => nodes.iter().map(|node| name_of(model, *node)).collect::<Vec<_>>().join(","),
    other => panic!("expected a node-set, got {other:?}"),
  })
}

/// The string-values of the nodes an expression selects, in document order.
fn text(xml: &str, expression: &str) -> String {
  with(xml, expression, |model, value| match value {
    Value::NodeSet(nodes) => nodes.iter().map(|node| model.string_value(*node)).collect::<Vec<_>>().join(","),
    other => panic!("expected a node-set, got {other:?}"),
  })
}

/// An expression's result, converted to a string the way XPath would.
fn value(xml: &str, expression: &str) -> String {
  with(xml, expression, |model, value| value.string(model))
}

/// The message of the error an expression fails with, with nothing bound.
fn error(xml: &str, expression: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().compile(expression).expect("parses");
  let error = query.evaluate(&model, model.root_node()).expect_err("fails");
  assert!(matches!(error, Error::XPath { .. }));
  error.message().to_owned()
}

fn name_of(model: &DomModel<'_>, node: DomNode) -> String {
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
}

/// `<a>` sits between a sibling before and after it, and has two children of its own.
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

#[test]
fn attribute_and_namespace_axes_reach_what_is_not_a_child() {
  let xml = "<r xmlns:p='urn:p' k='v' j='w'><a/></r>";
  assert_eq!(names(xml, "/r/@*"), "@k,@j");
  assert_eq!(names(xml, "/r/attribute::k"), "@k");
  assert_eq!(value(xml, "/r/@k"), "v");
  // The declaration is a namespace node, not an attribute, and `xml` is always in scope.
  assert_eq!(value(xml, "count(/r/namespace::*)"), "2");
  assert_eq!(value(xml, "count(/r/@*)"), "2");
  assert_eq!(names(xml, "/r/@*/parent::*"), "r", "an attribute's parent is its element");
}

#[test]
fn the_xml_prefix_needs_no_binding() {
  // Namespaces in XML §3 binds `xml` by definition and forbids binding it to anything else, so
  // an expression may use it without the caller having said anything. Nothing here binds it.
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
fn a_name_test_resolves_its_prefix_through_the_environment() {
  let xml = "<r xmlns:q='urn:p'><q:a/><a/></r>";
  // The expression's prefix is `p`, bound to urn:p; the document's is `q`. Only the URI matters.
  assert_eq!(names(xml, "/r/p:a"), "a");
  assert_eq!(names(xml, "/r/p:*"), "a");
  assert_eq!(value(xml, "count(/r/*)"), "2");
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
  // Without a node-set, a boolean operand makes it a boolean comparison.
  assert_eq!(value(xml, "1 = true()"), "true");
  assert_eq!(value(xml, "'a' = 'a'"), "true");
  // A relational comparison converts to numbers, so 10 > 9 — though "10" sorts before "9".
  assert_eq!(value(xml, "'10' > '9'"), "true");
  assert_eq!(value(xml, "'10' = '9'"), "false", "an equality comparison of two strings compares strings");
}

#[test]
fn a_union_merges_node_sets_into_document_order() {
  assert_eq!(names(TREE, "/r/y | /r/x"), "x,y");
  assert_eq!(names(TREE, "/r/a | /r/a"), "a", "a node-set holds a node once");
  assert_eq!(value(TREE, "count(//b | //c | //y)"), "3");
}

#[test]
fn a_variable_carries_a_value_into_the_expression() {
  let xml = "<r><n>1</n><n>2</n></r>";
  assert_eq!(text(xml, "//n[. = $want]"), "2");
  assert_eq!(value(xml, "$want"), "2");
}

#[test]
fn a_path_may_continue_from_an_expression() {
  assert_eq!(names(TREE, "(/r/a)/b"), "b");
  assert_eq!(names(TREE, "(/r/a | /r/x)/*"), "x1,b,c");
}

#[test]
fn errors_say_what_the_context_could_not_supply() {
  assert!(error(TREE, "$nosuch").contains("not bound"), "{}", error(TREE, "$nosuch"));
  assert!(error(TREE, "//q:a").contains("prefix \"q\" is not bound"), "{}", error(TREE, "//q:a"));
  assert!(error(TREE, "nosuch()").contains("no function named"), "{}", error(TREE, "nosuch()"));
  // A prefixed name is an extension function, so the complaint is about the prefix first.
  assert!(error(TREE, "ext:f()").contains("prefix \"ext\""), "{}", error(TREE, "ext:f()"));
  assert!(error(TREE, "1 | 2").contains("union joins node-sets"), "{}", error(TREE, "1 | 2"));
  assert!(error(TREE, "(1)/a").contains("continue from a node-set"), "{}", error(TREE, "(1)/a"));
  assert!(error(TREE, "count(1)").contains("needs a node-set"), "{}", error(TREE, "count(1)"));
  assert!(error(TREE, "position(1)").contains("takes 0 arguments"), "{}", error(TREE, "position(1)"));
}
