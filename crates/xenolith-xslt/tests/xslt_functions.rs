//! The functions XSLT adds to XPath (XSLT 1.0 §12.4, §15).

use xenolith_core::Error;
use xenolith_dom::build;
use xenolith_xdm::DomModel;
use xenolith_xpath::{Context, Functions, Value};
use xenolith_xslt::{Stylesheet, Transform, transform};

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

/// Evaluates one expression at the root and gives its string.
fn value_of(expression: &str, xml: &str) -> String {
  run(&format!("<xsl:template match=\"/\"><xsl:value-of select=\"{expression}\"/></xsl:template>"), xml)
}

#[test]
fn current_is_the_node_the_instruction_is_on_not_the_one_a_predicate_is_testing() {
  // Inside a predicate the context node moves along the candidates, while the current node
  // stays where xsl:for-each left it. That difference is the whole reason current() exists.
  let body = "<xsl:template match=\"/\"><xsl:for-each select=\"//a\">\
                <xsl:value-of select=\"count(//a[@id = current()/@id])\"/></xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<r><a id='1'/><a id='2'/></r>"), "11");

  // With `.` instead, the predicate compares each candidate with itself, so every one matches.
  let context_node = "<xsl:template match=\"/\"><xsl:for-each select=\"//a\">\
                      <xsl:value-of select=\"count(//a[@id = ./@id])\"/></xsl:for-each></xsl:template>";
  assert_eq!(run(context_node, "<r><a id='1'/><a id='2'/></r>"), "22");
}

#[test]
fn current_follows_apply_templates_as_well() {
  let body = "<xsl:template match=\"/\"><xsl:apply-templates select=\"//a\"/></xsl:template>\
              <xsl:template match=\"a\"><xsl:value-of select=\"current()/@id\"/></xsl:template>";
  assert_eq!(run(body, "<r><a id='x'/><a id='y'/></r>"), "xy");
}

#[test]
fn current_takes_no_arguments() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"current(1)\"/></xsl:template>";
  let message = error(body, "<a/>");
  assert!(message.contains("current()"), "{message}");
}

#[test]
fn generate_id_is_the_same_for_a_node_and_different_for_another() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"generate-id(//a[1])\"/>|\
              <xsl:value-of select=\"generate-id(//a[1])\"/>|\
              <xsl:value-of select=\"generate-id(//a[2])\"/></xsl:template>";
  let text = run(body, "<r><a/><a/></r>");
  let ids: Vec<&str> = text.split('|').collect();
  assert_eq!(ids[0], ids[1], "the same node asked twice gives the same identifier");
  assert_ne!(ids[0], ids[2], "different nodes give different identifiers");

  // §12.4 asks for an identifier that is alphanumeric and begins with a letter, so that it can
  // be used where a name is wanted.
  for id in &ids {
    assert!(id.starts_with(|c: char| c.is_ascii_alphabetic()), "{id} should start with a letter");
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric()), "{id} should be alphanumeric");
  }
}

#[test]
fn generate_id_of_nothing_is_the_empty_string() {
  assert_eq!(value_of("generate-id(//nothing)", "<r><a/></r>"), "");
  // With no argument at all it is the context node, which at the root is the root.
  assert!(!value_of("generate-id()", "<r><a/></r>").is_empty());
}

#[test]
fn system_property_answers_the_three_the_specification_names() {
  // A number rather than a string, so that `>= 1.0` is a sensible thing for a stylesheet to ask.
  assert_eq!(value_of("system-property('xsl:version')", "<a/>"), "1");
  assert_eq!(value_of("system-property('xsl:version') &gt;= 1.0", "<a/>"), "true");
  assert_eq!(value_of("system-property('xsl:vendor')", "<a/>"), "xenolith");
  assert!(value_of("system-property('xsl:vendor-url')", "<a/>").starts_with("https://"));
}

#[test]
fn an_unknown_system_property_is_empty_rather_than_an_error() {
  // §12.4: a property the processor does not know gives the empty string.
  assert_eq!(value_of("system-property('xsl:nosuch')", "<a/>"), "");
  assert_eq!(value_of("system-property('nosuch')", "<a/>"), "");
}

#[test]
fn element_available_answers_for_this_implementation() {
  assert_eq!(value_of("element-available('xsl:if')", "<a/>"), "true");
  assert_eq!(value_of("element-available('xsl:copy-of')", "<a/>"), "true");
  // Not an XSLT 1.0 element at all, so the honest answer is false — which is what lets a
  // stylesheet pick another route rather than fail part-way through.
  assert_eq!(value_of("element-available('xsl:perform-magic')", "<a/>"), "false");
  // A top-level declaration is not an instruction.
  assert_eq!(value_of("element-available('xsl:template')", "<a/>"), "false");
  // An element outside the XSLT namespace is nobody's instruction here.
  assert_eq!(value_of("element-available('xsl:nosuch')", "<a/>"), "false");
}

#[test]
fn function_available_asks_the_registry_rather_than_a_list() {
  assert_eq!(value_of("function-available('substring-before')", "<a/>"), "true");
  assert_eq!(value_of("function-available('current')", "<a/>"), "true");
  assert_eq!(value_of("function-available('generate-id')", "<a/>"), "true");
  assert_eq!(value_of("function-available('key')", "<a/>"), "true");
  assert_eq!(value_of("function-available('format-number')", "<a/>"), "true");
  // A name nobody has registered, which is the only way this comes back false: the registry is
  // what is asked, so the answer follows what a call would actually do.
  assert_eq!(value_of("function-available('nosuch')", "<a/>"), "false");
  assert_eq!(value_of("function-available('unparsed-entity-uri')", "<a/>"), "false");
}

#[test]
fn function_available_sees_an_extension_the_caller_registered() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"function-available('my:shout')\"/>\
              <xsl:value-of select=\"function-available('my:whisper')\"/></xsl:template>";
  let source = format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
     xmlns:my=\"urn:my\">{body}</xsl:stylesheet>"
  );
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let functions = Functions::new().with("urn:my", "shout", |arguments: Vec<Value<_>>, context: &Context<'_, _>| {
    Ok(Value::String(arguments[0].string(context.model).to_uppercase()))
  });

  let result = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect("transforms");
  assert_eq!(result.text(), "truefalse");
}

#[test]
fn an_unbound_prefix_in_one_of_these_names_is_reported() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"element-available('nope:thing')\"/></xsl:template>";
  let message = error(body, "<a/>");
  assert!(message.contains("not bound"), "{message}");
}

#[test]
fn a_registered_function_cannot_shadow_one_of_xpaths_own() {
  // The core library is consulted first, so registering `count` in no namespace changes nothing.
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"count(//a)\"/></xsl:template>";
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<r><a/><a/></r>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let functions = Functions::new().with("", "count", |_: Vec<Value<_>>, _: &Context<'_, _>| Ok(Value::Number(99.0)));

  let result = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect("transforms");
  assert_eq!(result.text(), "2", "XPath's count() wins over one registered under the same name");
}

#[test]
fn every_instruction_named_as_available_is_one_that_runs() {
  // `element-available()` answers from a list written beside the dispatch, and a list beside a
  // match drifts from it unless something checks. Each name here is put in a template and run:
  // most complain about a missing attribute, which is fine — what must not happen is the
  // instruction being unknown.
  let named = [
    "apply-templates",
    "attribute",
    "call-template",
    "choose",
    "comment",
    "copy",
    "copy-of",
    "element",
    "for-each",
    "if",
    "message",
    "param",
    "processing-instruction",
    "text",
    "value-of",
    "variable",
  ];

  for instruction in named {
    let available = value_of(&format!("element-available('xsl:{instruction}')"), "<a/>");
    assert_eq!(available, "true", "xsl:{instruction} should be reported as available");

    let body = format!("<xsl:template match=\"/\"><xsl:{instruction}/></xsl:template>");
    let stylesheet = Stylesheet::compile(sheet(&body).as_bytes(), "file:///s.xsl").expect("compiles");
    let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
    let model = DomModel::new(&doc);
    if let Err(error) = transform(&stylesheet, &model, model.root_node()) {
      assert!(matches!(error, Error::Xslt { .. }));
      assert!(!error.message().contains("not implemented"), "xsl:{instruction}: {}", error.message());
    }
  }
}
