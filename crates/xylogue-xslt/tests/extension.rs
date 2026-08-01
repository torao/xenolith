//! Extension functions, and the output method a stylesheet asks for.

use xylogue_core::ErrorKind;
use xylogue_dom::build;
use xylogue_xdm::DomModel;
use xylogue_xpath::{Context, Functions, Value};
use xylogue_xslt::{OutputMethod, Stylesheet, Transform};

/// Wraps template bodies in an `xsl:stylesheet` that binds `my` to `urn:my`.
fn sheet(body: &str) -> String {
  format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
     xmlns:my=\"urn:my\">{body}</xsl:stylesheet>"
  )
}

/// Runs a stylesheet with a couple of extension functions registered, and takes the text.
fn run(body: &str, xml: &str) -> Result<String, xylogue_core::Error> {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl")?;
  let doc = build::parse(xml.as_bytes())?;
  let model = DomModel::new(&doc);

  let functions = Functions::new()
    .with("urn:my", "shout", |arguments: Vec<Value<_>>, context: &Context<'_, _>| {
      Ok(Value::String(arguments[0].string(context.model).to_uppercase()))
    })
    // One that reaches for the context rather than only its arguments.
    .with("urn:my", "depth", |_: Vec<Value<_>>, context: &Context<'_, _>| Ok(Value::Number(context.size as f64)));

  let result = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions)?;
  Ok(result.text())
}

/// The message a transformation fails with.
fn error(body: &str, xml: &str) -> String {
  let error = run(body, xml).expect_err("fails");
  error.message().to_owned()
}

#[test]
fn a_registered_function_can_be_called_by_the_prefix_the_stylesheet_chose() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"my:shout('quiet')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>").unwrap(), "QUIET");
}

#[test]
fn an_extension_function_sees_the_context_it_was_called_in() {
  let body = "<xsl:template match=\"/\"><xsl:for-each select=\"//b\">\
              <xsl:value-of select=\"my:depth()\"/></xsl:for-each></xsl:template>";
  // Three nodes are being processed, so the context size is 3 each time.
  assert_eq!(run(body, "<a><b/><b/><b/></a>").unwrap(), "333");
}

#[test]
fn an_extension_function_may_take_what_an_expression_produced() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"my:shout(//b)\"/></xsl:template>";
  assert_eq!(run(body, "<a><b>text</b></a>").unwrap(), "TEXT");
}

#[test]
fn a_function_that_is_not_registered_says_what_is() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"my:nosuch()\"/></xsl:template>";
  let message = error(body, "<a/>");
  assert!(message.contains("{urn:my}nosuch"), "{message}");
  assert!(message.contains("{urn:my}shout"), "the message lists what is registered: {message}");
}

#[test]
fn a_function_whose_prefix_is_not_bound_says_so() {
  let text = "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\
              <xsl:template match=\"/\"><xsl:value-of select=\"other:f()\"/></xsl:template></xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(text.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let functions = Functions::new();
  let error = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect_err("fails");
  assert_eq!(error.kind(), ErrorKind::XPath);
  assert!(error.message().contains("not bound"), "{}", error.message());
}

#[test]
fn a_core_function_is_not_reached_by_a_prefix() {
  // `my:count` is not the core `count`, whatever the prefix is bound to.
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"my:count(//b)\"/></xsl:template>";
  assert!(error(body, "<a><b/></a>").contains("{urn:my}count"));
}

#[test]
fn the_output_method_is_read_from_the_stylesheet() {
  let text = Stylesheet::compile(sheet("<xsl:output method=\"text\"/>").as_bytes(), "file:///s.xsl").unwrap();
  assert_eq!(text.output_method(), OutputMethod::Text);

  let html = Stylesheet::compile(sheet("<xsl:output method=\"html\"/>").as_bytes(), "file:///s.xsl").unwrap();
  assert_eq!(html.output_method(), OutputMethod::Html);

  let silent = Stylesheet::compile(sheet("").as_bytes(), "file:///s.xsl").unwrap();
  assert_eq!(silent.output_method(), OutputMethod::Xml, "xml is what a stylesheet that says nothing gets");
}

#[test]
fn an_output_method_that_cannot_be_written_is_refused() {
  let error =
    Stylesheet::compile(sheet("<xsl:output method=\"pdf\"/>").as_bytes(), "file:///s.xsl").expect_err("is refused");
  assert_eq!(error.kind(), ErrorKind::Xslt);
  assert!(error.message().contains("pdf"), "{}", error.message());
}

#[test]
fn the_text_method_writes_the_character_data_and_nothing_else() {
  let body = "<xsl:output method=\"text\"/>\
              <xsl:template match=\"/\"><wrapper><xsl:value-of select=\"//b\"/></wrapper></xsl:template>";
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").unwrap();
  assert_eq!(stylesheet.output_method(), OutputMethod::Text);

  let doc = build::parse("<a><b>content</b></a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let result = Transform::new().run(&stylesheet, &model, model.root_node()).unwrap();
  // The element the stylesheet built is not written; only what it contains.
  assert_eq!(result.text(), "content");
}
