//! Template match patterns: what matches, and what priority a pattern gets by default.

use xylogue_core::Error;
use xylogue_dom::build;
use xylogue_xdm::{DomModel, Model, NodeKind};
use xylogue_xpath::{Namespaces, Variables};
use xylogue_xslt::Pattern;

/// Every node of `xml` that matches `pattern`, named so a test can read the answer.
fn matching(xml: &str, pattern: &str) -> String {
  matching_with(xml, pattern, &Namespaces::new())
}

fn matching_with(xml: &str, pattern: &str, namespaces: &Namespaces) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let pattern = Pattern::compile(pattern).expect("compiles");
  let variables = Variables::new();

  let mut matched = Vec::new();
  let mut stack = vec![model.root_node()];
  while let Some(node) = stack.pop() {
    if pattern.matches_with(&model, node, namespaces, &variables).expect("matches") {
      matched.push(describe(&model, node));
    }
    // Attributes are not children, so they have to be visited on their own.
    for attribute in model.attributes(node) {
      if pattern.matches_with(&model, attribute, namespaces, &variables).expect("matches") {
        matched.push(describe(&model, attribute));
      }
    }
    // Pushed in reverse so that popping walks the children in order: the stack yields the
    // nodes in document order, which is the order the answers are compared in.
    let children = model.children(node);
    stack.extend(children.into_iter().rev());
  }
  matched.join(",")
}

/// Names a node, with enough of its content to tell two of the same name apart.
fn describe(model: &DomModel<'_>, node: xylogue_xdm::DomNode) -> String {
  let name = model.qualified_name(node).unwrap_or_default();
  match model.kind(node) {
    NodeKind::Root => "/".to_owned(),
    NodeKind::Attribute => format!("@{name}={}", model.string_value(node)),
    NodeKind::Text => format!("'{}'", model.string_value(node)),
    NodeKind::Comment => "<!---->".to_owned(),
    NodeKind::ProcessingInstruction => format!("?{name}"),
    _ => {
      let value = model.string_value(node);
      if value.is_empty() { name } else { format!("{name}({value})") }
    }
  }
}

/// The default priority of a pattern with a single alternative.
fn priority(pattern: &str) -> f64 {
  let pattern = Pattern::compile(pattern).expect("compiles");
  assert_eq!(pattern.alternatives().len(), 1, "{} has more than one alternative", pattern.source());
  pattern.alternatives()[0].default_priority()
}

/// The message of the error a pattern fails to compile with.
fn error(pattern: &str) -> String {
  let error = Pattern::compile(pattern).expect_err("is refused");
  assert!(matches!(error, Error::Xslt { .. }), "{}", error.message());
  error.message().to_owned()
}

const TREE: &str = "<r><a k='1'><b>x</b></a><c><a><b>y</b></a></c></r>";

#[test]
fn an_unanchored_pattern_matches_at_any_depth() {
  assert_eq!(matching(TREE, "b"), "b(x),b(y)");
  assert_eq!(matching(TREE, "a"), "a(x),a(y)");
  assert_eq!(matching(TREE, "r"), "r(xy)");
}

#[test]
fn a_multi_step_pattern_constrains_the_ancestors() {
  assert_eq!(matching(TREE, "a/b"), "b(x),b(y)", "both b elements have an a parent");
  assert_eq!(matching(TREE, "c/a/b"), "b(y)", "only the second is under c");
  assert_eq!(matching(TREE, "r/a/b"), "b(x)");
  assert_eq!(matching(TREE, "r/b"), "", "no b has an r parent");
}

#[test]
fn a_leading_slash_anchors_at_the_root() {
  assert_eq!(matching(TREE, "/r"), "r(xy)");
  assert_eq!(matching(TREE, "/r/a"), "a(x)", "only the a that is a child of the root element");
  assert_eq!(matching(TREE, "/a"), "", "a is not the document element");
  assert_eq!(matching(TREE, "/"), "/", "the root node itself");
}

#[test]
fn a_double_slash_lets_an_ancestor_stand_in_for_a_parent() {
  assert_eq!(matching(TREE, "//b"), "b(x),b(y)");
  assert_eq!(matching(TREE, "r//b"), "b(x),b(y)", "r is an ancestor of both");
  assert_eq!(matching(TREE, "c//b"), "b(y)");
  assert_eq!(matching(TREE, "/r//b"), "b(x),b(y)");
}

#[test]
fn the_attribute_axis_is_reached_with_an_at_sign() {
  assert_eq!(matching(TREE, "@k"), "@k=1");
  assert_eq!(matching(TREE, "a/@k"), "@k=1");
  assert_eq!(matching(TREE, "c/@k"), "", "the attribute is on a, not c");
  assert_eq!(matching(TREE, "@*"), "@k=1");
  assert_eq!(matching(TREE, "k"), "", "a name on the child axis does not reach an attribute");
}

#[test]
fn node_tests_select_by_kind() {
  let xml = "<r>t<a/><!--c--><?pi d?></r>";
  assert_eq!(matching(xml, "text()"), "'t'");
  assert_eq!(matching(xml, "comment()"), "<!---->");
  assert_eq!(matching(xml, "processing-instruction()"), "?pi");
  assert_eq!(matching(xml, "processing-instruction('pi')"), "?pi");
  assert_eq!(matching(xml, "processing-instruction('other')"), "");
  assert_eq!(matching(xml, "*"), "r(t),a", "a name test selects only elements");
  // node() reaches everything that can be a child, but neither the root nor an attribute.
  assert_eq!(matching(xml, "r/node()"), "'t',a,<!---->,?pi");
}

#[test]
fn a_predicate_asks_about_the_node_among_its_siblings() {
  let xml = "<r><i>1</i><i>2</i><i>3</i></r>";
  assert_eq!(matching(xml, "i[1]"), "i(1)");
  assert_eq!(matching(xml, "i[2]"), "i(2)");
  assert_eq!(matching(xml, "i[last()]"), "i(3)");
  assert_eq!(matching(xml, "i[. > 1]"), "i(2),i(3)");
  assert_eq!(matching(xml, "r/i[1]"), "i(1)");
}

#[test]
fn alternatives_are_separated_by_a_bar() {
  assert_eq!(matching(TREE, "b|c"), "b(x),c(y),b(y)", "in document order, whichever alternative matched");
  let pattern = Pattern::compile("a|b|c").expect("compiles");
  assert_eq!(pattern.alternatives().len(), 3, "each alternative is its own template rule");
}

#[test]
fn a_prefix_in_a_pattern_is_resolved_through_the_bindings() {
  let xml = "<r xmlns:d='urn:d'><d:a/><a/></r>";
  let namespaces = Namespaces::new().with("p", "urn:d");
  assert_eq!(matching_with(xml, "p:a", &namespaces), "d:a");
  assert_eq!(matching_with(xml, "p:*", &namespaces), "d:a");
  assert_eq!(matching_with(xml, "a", &namespaces), "a", "an unprefixed name is in no namespace");
}

#[test]
fn id_anchors_a_pattern_to_a_known_element() {
  let xml = "<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT s ANY><!ATTLIST s k ID #IMPLIED>]>\
             <r><s k='one'><b/></s><s k='two'><b/></s></r>";
  assert_eq!(matching(xml, "id('one')"), "s");
  assert_eq!(matching(xml, "id('one')/b"), "b");
  assert_eq!(matching(xml, "id('nosuch')"), "");
}

#[test]
fn default_priorities_follow_how_specific_the_pattern_is() {
  // A bare name, or a processing instruction with a target: 0.
  assert_eq!(priority("a"), 0.0);
  assert_eq!(priority("@k"), 0.0);
  assert_eq!(priority("p:a"), 0.0);
  assert_eq!(priority("processing-instruction('pi')"), 0.0);
  // A namespace wildcard: -0.25.
  assert_eq!(priority("p:*"), -0.25);
  // Anything less specific than a name: -0.5.
  assert_eq!(priority("*"), -0.5);
  assert_eq!(priority("@*"), -0.5);
  assert_eq!(priority("node()"), -0.5);
  assert_eq!(priority("text()"), -0.5);
  assert_eq!(priority("comment()"), -0.5);
  assert_eq!(priority("processing-instruction()"), -0.5);
  // Saying anything more than that: 0.5.
  assert_eq!(priority("a/b"), 0.5);
  assert_eq!(priority("/a"), 0.5);
  assert_eq!(priority("//a"), 0.5);
  assert_eq!(priority("a[1]"), 0.5);
  assert_eq!(priority("id('x')"), 0.5);
}

#[test]
fn what_is_not_a_pattern_is_refused_with_a_reason() {
  assert!(error("following-sibling::a").contains("child and attribute axes"), "{}", error("following-sibling::a"));
  assert!(error("../a").contains("child and attribute axes"), "{}", error("../a"));
  assert!(error("ancestor::a").contains("child and attribute axes"), "{}", error("ancestor::a"));
  assert!(error("count(a)").contains("id() and key()"), "{}", error("count(a)"));
  assert!(error("$x/a").contains("only id() and key()"), "{}", error("$x/a"));
  assert!(error("1 + 2").contains("a path"), "{}", error("1 + 2"));
  // `a//` never reaches the pattern checks — the path grammar wants a step after `//` — but the
  // step that `//` stands for can be written out, and then it does.
  assert!(matches!(Pattern::compile("a//").unwrap_err(), Error::XPath { .. }));
  let dangling = "a/descendant-or-self::node()";
  assert!(error(dangling).contains("may not end"), "{}", error(dangling));
  // Not a path at all: the XPath parser refuses it first, so the kind is XPath.
  assert!(matches!(Pattern::compile("a[").unwrap_err(), Error::XPath { .. }));
}

#[test]
fn key_is_accepted_but_matches_nothing_until_the_key_tables_exist() {
  let pattern = Pattern::compile("key('k', 'v')").expect("compiles");
  assert_eq!(pattern.alternatives().len(), 1);
  assert_eq!(matching(TREE, "key('k', 'v')"), "", "no key tables yet, so nothing matches");
}
