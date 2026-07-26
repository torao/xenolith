//! Compiling a stylesheet: what it declares, and which rule wins for a node.

use std::collections::HashMap;

use xylograph_core::{Error, ErrorKind};
use xylograph_dom::build;
use xylograph_xdm::{DomModel, Model};
use xylograph_xslt::{Loader, Stylesheet};

/// Wraps top-level content in an `xsl:stylesheet`.
fn sheet(body: &str) -> String {
  format!("<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>")
}

/// Compiles a single-document stylesheet.
fn compile(body: &str) -> Stylesheet {
  Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles")
}

/// The message a stylesheet fails to compile with.
fn error(body: &str) -> String {
  let error = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect_err("is refused");
  error.message().to_owned()
}

/// A loader over a map of URI to stylesheet text.
struct Modules(HashMap<String, String>);

impl Modules {
  fn new(entries: &[(&str, &str)]) -> Self {
    Self(entries.iter().map(|(uri, text)| ((*uri).to_owned(), (*text).to_owned())).collect())
  }
}

impl Loader for Modules {
  fn load(&mut self, uri: &str) -> Result<Vec<u8>, Error> {
    self
      .0
      .get(uri)
      .map(|text| text.as_bytes().to_vec())
      .ok_or_else(|| Error::new(ErrorKind::Io, format!("no module at {uri}")))
  }
}

/// The priority and precedence of the rule chosen for the first `b` element of `xml`.
fn chosen(stylesheet: &Stylesheet, xml: &str, mode: Option<&str>) -> Option<(f64, i32)> {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let root = model.children(model.root_node())[0];
  let node = model.children(root).into_iter().find(|&n| model.qualified_name(n).as_deref() == Some("b"))?;
  let template = stylesheet.template_for(&model, node, mode).expect("matches")?;
  Some((template.priority(), template.precedence()))
}

#[test]
fn a_stylesheet_declares_templates_and_variables() {
  let stylesheet = compile(
    "<xsl:template match=\"a\"/>\
     <xsl:template name=\"helper\"/>\
     <xsl:variable name=\"v\" select=\"1\"/>\
     <xsl:param name=\"p\"/>",
  );
  assert_eq!(stylesheet.templates().len(), 2);
  assert_eq!(stylesheet.variables().len(), 2);
  assert_eq!(stylesheet.variables()[0].name(), "v");
  assert_eq!(stylesheet.variables()[0].select(), Some("1"));
  assert!(!stylesheet.variables()[0].is_parameter());
  assert!(stylesheet.variables()[1].is_parameter());
  assert_eq!(stylesheet.variables()[1].select(), None, "without a select the content is the value");
  assert!(stylesheet.template_named("helper").is_some());
  assert!(stylesheet.template_named("nosuch").is_none());
}

#[test]
fn each_alternative_of_a_pattern_becomes_its_own_rule() {
  // XSLT treats `a|b` as two rules, so each gets its own default priority.
  let stylesheet = compile("<xsl:template match=\"a|b/c\"/>");
  assert_eq!(stylesheet.templates().len(), 2);
  let priorities: Vec<f64> = stylesheet.templates().iter().map(|t| t.priority()).collect();
  assert_eq!(priorities, [0.0, 0.5]);
}

#[test]
fn a_stated_priority_replaces_the_default() {
  let stylesheet = compile("<xsl:template match=\"a\" priority=\"3\"/><xsl:template match=\"b\"/>");
  assert_eq!(stylesheet.templates()[0].priority(), 3.0);
  assert_eq!(stylesheet.templates()[1].priority(), 0.0, "the default for a bare name");
}

#[test]
fn the_more_specific_pattern_wins_by_its_default_priority() {
  let stylesheet = compile("<xsl:template match=\"b\"/><xsl:template match=\"a/b\"/>");
  // `b` has priority 0, `a/b` has 0.5.
  assert_eq!(chosen(&stylesheet, "<a><b/></a>", None), Some((0.5, 0)));
}

#[test]
fn among_equals_the_last_declaration_wins() {
  let stylesheet = compile("<xsl:template match=\"b\" priority=\"1\"/><xsl:template match=\"*\" priority=\"1\"/>");
  // Both match with priority 1; the specification calls this an error and lets an
  // implementation recover by taking the last, which is what happens.
  let doc = build::parse("<a><b/></a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let root = model.children(model.root_node())[0];
  let b = model.children(root)[0];
  let template = stylesheet.template_for(&model, b, None).unwrap().unwrap();
  assert_eq!(template.pattern().unwrap().source(), "*");
}

#[test]
fn a_mode_keeps_rules_apart() {
  let stylesheet = compile("<xsl:template match=\"b\"/><xsl:template match=\"b\" mode=\"m\" priority=\"9\"/>");
  assert_eq!(chosen(&stylesheet, "<a><b/></a>", None).map(|(p, _)| p), Some(0.0));
  assert_eq!(chosen(&stylesheet, "<a><b/></a>", Some("m")).map(|(p, _)| p), Some(9.0));
  assert_eq!(chosen(&stylesheet, "<a><b/></a>", Some("other")), None, "no rule in that mode");
}

#[test]
fn no_matching_rule_is_not_an_error() {
  let stylesheet = compile("<xsl:template match=\"nosuch\"/>");
  assert_eq!(chosen(&stylesheet, "<a><b/></a>", None), None);
}

#[test]
fn a_prefix_in_a_pattern_means_what_the_stylesheet_declares() {
  let text = "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
              xmlns:d=\"urn:d\"><xsl:template match=\"d:b\"/></xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(text.as_bytes(), "file:///s.xsl").expect("compiles");

  // The document uses a different prefix for the same namespace; only the namespace matters.
  let doc = build::parse("<a xmlns:q='urn:d'><q:b/></a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let root = model.children(model.root_node())[0];
  let b = model.children(root)[0];
  assert!(stylesheet.template_for(&model, b, None).unwrap().is_some());
}

#[test]
fn include_brings_in_rules_at_the_same_precedence() {
  let mut loader = Modules::new(&[("file:///part.xsl", &sheet("<xsl:template match=\"b\" priority=\"5\"/>"))]);
  let stylesheet = Stylesheet::compile_with(
    sheet("<xsl:include href=\"part.xsl\"/><xsl:template match=\"a\"/>").as_bytes(),
    "file:///s.xsl",
    &mut loader,
  )
  .expect("compiles");

  assert_eq!(stylesheet.templates().len(), 2);
  let precedences: Vec<i32> = stylesheet.templates().iter().map(|t| t.precedence()).collect();
  assert_eq!(precedences, [0, 0], "an include shares the precedence of the module that includes it");
}

#[test]
fn import_brings_in_rules_at_a_lower_precedence() {
  let mut loader = Modules::new(&[("file:///base.xsl", &sheet("<xsl:template match=\"b\" priority=\"9\"/>"))]);
  let stylesheet = Stylesheet::compile_with(
    sheet("<xsl:import href=\"base.xsl\"/><xsl:template match=\"b\" priority=\"1\"/>").as_bytes(),
    "file:///s.xsl",
    &mut loader,
  )
  .expect("compiles");

  // The imported rule has the higher priority but the lower precedence, and precedence is
  // looked at first — so the importing stylesheet's rule wins.
  assert_eq!(chosen(&stylesheet, "<a><b/></a>", None), Some((1.0, 1)));
}

#[test]
fn a_later_import_outranks_an_earlier_one() {
  let mut loader = Modules::new(&[
    ("file:///first.xsl", &sheet("<xsl:template match=\"b\"/>")),
    ("file:///second.xsl", &sheet("<xsl:template match=\"b\"/>")),
  ]);
  let stylesheet = Stylesheet::compile_with(
    sheet("<xsl:import href=\"first.xsl\"/><xsl:import href=\"second.xsl\"/>").as_bytes(),
    "file:///s.xsl",
    &mut loader,
  )
  .expect("compiles");

  let precedences: Vec<i32> = stylesheet.templates().iter().map(|t| t.precedence()).collect();
  assert_eq!(precedences.len(), 2);
  assert!(precedences[1] > precedences[0], "the second import outranks the first: {precedences:?}");
}

#[test]
fn a_module_reached_twice_is_refused() {
  let mut loader = Modules::new(&[("file:///part.xsl", &sheet(""))]);
  let error = Stylesheet::compile_with(
    sheet("<xsl:include href=\"part.xsl\"/><xsl:include href=\"part.xsl\"/>").as_bytes(),
    "file:///s.xsl",
    &mut loader,
  )
  .expect_err("is refused");
  assert!(error.message().contains("more than once"), "{}", error.message());
}

#[test]
fn a_module_that_cannot_be_loaded_says_which_entry_point_would_load_it() {
  let error =
    Stylesheet::compile(sheet("<xsl:include href=\"part.xsl\"/>").as_bytes(), "file:///s.xsl").expect_err("is refused");
  assert!(error.message().contains("compile_with"), "{}", error.message());
}

#[test]
fn what_is_not_a_stylesheet_is_refused_with_a_reason() {
  let not_a_stylesheet = Stylesheet::compile(b"<html/>", "file:///s.xsl").expect_err("is refused");
  assert_eq!(not_a_stylesheet.kind(), ErrorKind::Xslt);
  assert!(not_a_stylesheet.message().contains("xsl:stylesheet"), "{}", not_a_stylesheet.message());

  let no_version =
    Stylesheet::compile(b"<xsl:stylesheet xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\"/>", "file:///s.xsl")
      .expect_err("is refused");
  assert!(no_version.message().contains("version"), "{}", no_version.message());

  assert!(error("<xsl:template/>").contains("match or a name"));
  assert!(error("<xsl:variable select=\"1\"/>").contains("needs a name"));
  assert!(error("<xsl:template match=\"a\" priority=\"high\"/>").contains("must be a number"));
}

#[test]
fn top_level_elements_a_later_phase_reads_are_passed_over() {
  // xsl:output and xsl:key are not read yet; they must not stop the stylesheet compiling.
  let stylesheet = compile("<xsl:output method=\"text\"/><xsl:template match=\"a\"/>");
  assert_eq!(stylesheet.templates().len(), 1);
}
