//! Running a stylesheet: the instructions, the built-in rules, and what they build.

use xylograph_core::ErrorKind;
use xylograph_dom::build;
use xylograph_serialize::Serializer;
use xylograph_xdm::DomModel;
use xylograph_xslt::{Stylesheet, transform};

/// Wraps template bodies in an `xsl:stylesheet`.
fn sheet(body: &str) -> String {
  format!("<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>")
}

/// Transforms `xml` with a stylesheet and serializes what comes out.
fn run(body: &str, xml: &str) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  Serializer::new().to_string(result.document(), result.root())
}

/// The message a transformation fails with.
fn error(body: &str, xml: &str) -> String {
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let error = transform(&stylesheet, &model, model.root_node()).expect_err("fails");
  assert_eq!(error.kind(), ErrorKind::Xslt, "{}", error.message());
  error.message().to_owned()
}

#[test]
fn the_built_in_rules_walk_down_to_the_text() {
  // With no rules at all, the built-in ones copy every character of the source.
  assert_eq!(run("", "<a>one<b>two</b></a>"), "onetwo");
}

#[test]
fn a_template_replaces_what_the_built_in_rules_would_have_done() {
  assert_eq!(run("<xsl:template match=\"b\">B</xsl:template>", "<a>one<b>two</b></a>"), "oneB");
  // A rule that produces nothing removes the subtree from the result.
  assert_eq!(run("<xsl:template match=\"b\"/>", "<a>one<b>two</b></a>"), "one");
}

#[test]
fn value_of_takes_the_string_of_an_expression() {
  let body = "<xsl:template match=\"/\"><xsl:value-of select=\"//b\"/></xsl:template>";
  assert_eq!(run(body, "<a><b>two</b></a>"), "two");
  let count = "<xsl:template match=\"/\"><xsl:value-of select=\"count(//b)\"/></xsl:template>";
  assert_eq!(run(count, "<a><b/><b/></a>"), "2");
}

#[test]
fn literal_result_elements_are_copied_with_their_attributes() {
  let body = "<xsl:template match=\"/\"><out k=\"v\"><xsl:value-of select=\"//b\"/></out></xsl:template>";
  assert_eq!(run(body, "<a><b>t</b></a>"), "<out k=\"v\">t</out>");
}

#[test]
fn an_attribute_value_template_is_expanded() {
  let body = "<xsl:template match=\"/\"><out id=\"{//b/@id}\" n=\"{count(//b)}\"/></xsl:template>";
  assert_eq!(run(body, "<a><b id='x'/><b/></a>"), "<out id=\"x\" n=\"2\"/>");
  // A doubled brace is one literal brace.
  let braces = "<xsl:template match=\"/\"><out k=\"{{literal}}\"/></xsl:template>";
  assert_eq!(run(braces, "<a/>"), "<out k=\"{literal}\"/>");
}

#[test]
fn apply_templates_selects_what_to_process() {
  let body = "<xsl:template match=\"/\"><xsl:apply-templates select=\"//b\"/></xsl:template>\
              <xsl:template match=\"b\">[<xsl:value-of select=\".\"/>]</xsl:template>";
  assert_eq!(run(body, "<a><b>1</b><c>skip</c><b>2</b></a>"), "[1][2]");
}

#[test]
fn a_mode_chooses_among_the_rules() {
  let body = "<xsl:template match=\"/\"><xsl:apply-templates select=\"//b\" mode=\"m\"/></xsl:template>\
              <xsl:template match=\"b\">plain</xsl:template>\
              <xsl:template match=\"b\" mode=\"m\">moded</xsl:template>";
  assert_eq!(run(body, "<a><b/></a>"), "moded");
}

#[test]
fn for_each_runs_its_body_over_a_node_set() {
  let body = "<xsl:template match=\"/\"><xsl:for-each select=\"//b\">\
              <xsl:value-of select=\".\"/>,</xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<a><b>1</b><b>2</b></a>"), "1,2,");
}

#[test]
fn position_and_last_report_where_a_node_is_in_the_list() {
  // The separator is written with xsl:text: a space between instructions is whitespace-only
  // text, which §3.4 strips out of a stylesheet.
  let body = "<xsl:template match=\"/\"><xsl:for-each select=\"//b\">\
              <xsl:value-of select=\"position()\"/>of<xsl:value-of select=\"last()\"/><xsl:text> </xsl:text>\
              </xsl:for-each></xsl:template>";
  assert_eq!(run(body, "<a><b/><b/><b/></a>"), "1of3 2of3 3of3 ");
  // The same holds for the nodes a template rule is applied to.
  let applied = "<xsl:template match=\"/\"><xsl:apply-templates select=\"//b\"/></xsl:template>\
                 <xsl:template match=\"b\"><xsl:value-of select=\"position()\"/></xsl:template>";
  assert_eq!(run(applied, "<a><b/><b/></a>"), "12");
}

#[test]
fn if_and_choose_pick_a_branch() {
  let body = "<xsl:template match=\"b\"><xsl:if test=\". > 1\">big</xsl:if></xsl:template>";
  assert_eq!(run(body, "<a><b>1</b><b>2</b></a>"), "big");

  let choose = "<xsl:template match=\"b\"><xsl:choose>\
                <xsl:when test=\". = 1\">one</xsl:when>\
                <xsl:when test=\". = 2\">two</xsl:when>\
                <xsl:otherwise>other</xsl:otherwise></xsl:choose></xsl:template>";
  assert_eq!(run(choose, "<a><b>1</b><b>2</b><b>9</b></a>"), "onetwoother");
}

#[test]
fn a_variable_is_visible_to_what_follows_it() {
  let body = "<xsl:template match=\"/\"><xsl:variable name=\"n\" select=\"count(//b)\"/>\
              <xsl:value-of select=\"$n\"/></xsl:template>";
  assert_eq!(run(body, "<a><b/><b/></a>"), "2");
}

#[test]
fn a_variable_without_a_select_takes_its_content() {
  let body = "<xsl:template match=\"/\"><xsl:variable name=\"v\">text</xsl:variable>\
              <xsl:value-of select=\"$v\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "text");
}

#[test]
fn a_global_variable_is_in_scope_everywhere() {
  let body = "<xsl:variable name=\"g\" select=\"'global'\"/>\
              <xsl:template match=\"/\"><xsl:value-of select=\"$g\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "global");
}

#[test]
fn call_template_runs_a_named_template() {
  let body = "<xsl:template match=\"/\"><xsl:call-template name=\"greet\"/></xsl:template>\
              <xsl:template name=\"greet\">hello</xsl:template>";
  assert_eq!(run(body, "<a/>"), "hello");
}

#[test]
fn a_parameter_takes_the_value_the_caller_gives_it() {
  let body = "<xsl:template match=\"/\">\
                <xsl:call-template name=\"say\"><xsl:with-param name=\"what\" select=\"'given'\"/></xsl:call-template>\
                <xsl:call-template name=\"say\"/>\
              </xsl:template>\
              <xsl:template name=\"say\"><xsl:param name=\"what\" select=\"'default'\"/>\
              [<xsl:value-of select=\"$what\"/>]</xsl:template>";
  assert_eq!(run(body, "<a/>"), "[given][default]");
}

#[test]
fn a_parameter_reaches_a_rule_through_apply_templates() {
  let body = "<xsl:template match=\"/\">\
                <xsl:apply-templates select=\"//b\"><xsl:with-param name=\"p\" select=\"'x'\"/></xsl:apply-templates>\
              </xsl:template>\
              <xsl:template match=\"b\"><xsl:param name=\"p\" select=\"'-'\"/><xsl:value-of select=\"$p\"/></xsl:template>";
  assert_eq!(run(body, "<a><b/><b/></a>"), "xx");
}

#[test]
fn xsl_text_keeps_the_whitespace_the_stylesheet_would_otherwise_lose() {
  // The indentation between instructions is stripped; what xsl:text holds is not.
  let body = "<xsl:template match=\"/\">
                <xsl:text>  kept  </xsl:text>
              </xsl:template>";
  assert_eq!(run(body, "<a/>"), "  kept  ");
}

#[test]
fn whitespace_only_text_in_the_stylesheet_is_stripped() {
  let body = "<xsl:template match=\"/\">
                <out/>
              </xsl:template>";
  assert_eq!(run(body, "<a/>"), "<out/>", "the newlines and indentation do not reach the result");
}

#[test]
fn a_namespaced_literal_element_keeps_its_namespace() {
  let text = "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
              xmlns:o=\"urn:o\"><xsl:template match=\"/\"><o:out/></xsl:template></xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(text.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  let out = Serializer::new().to_string(result.document(), result.root());
  // The serializer supplies the declaration; the stylesheet's own xmlns:xsl does not come along.
  assert_eq!(out, "<o:out xmlns:o=\"urn:o\"/>");
}

#[test]
fn recursion_without_end_is_stopped() {
  // The point of the default limit is that it is reached before the stack is, so this must come
  // back as an error rather than take the process down with it.
  //
  // The stack this runs on is stated rather than inherited. A test harness gives a lone test the
  // main thread and everything else a spawned one, and those have different amounts of stack —
  // so a test that took whatever it was given would pass or fail by how it happened to be run.
  // Two mebibytes is what Rust gives a spawned thread by default, and that is the case the
  // limit is chosen for; `the_depth_guard_is_reached_before_the_stack_is` measures the margin.
  let recursion = std::thread::Builder::new()
    .stack_size(2 * 1024 * 1024)
    .spawn(|| {
      let body = "<xsl:template match=\"/\"><xsl:call-template name=\"loop\"/></xsl:template>\
                  <xsl:template name=\"loop\"><xsl:call-template name=\"loop\"/></xsl:template>";
      error(body, "<a/>")
    })
    .expect("spawns");
  let message = recursion.join().expect("the guard stops it before the stack does");
  assert!(message.contains("templates deep"), "{message}");
}

#[test]
fn the_depth_a_transformation_may_reach_can_be_set() {
  use xylograph_xslt::Transform;

  // Ten nested calls: allowed at the default, refused at a limit of five.
  let body = "<xsl:template match=\"/\"><xsl:call-template name=\"down\">\
                <xsl:with-param name=\"n\" select=\"10\"/></xsl:call-template></xsl:template>\
              <xsl:template name=\"down\"><xsl:param name=\"n\"/>\
                <xsl:if test=\"$n > 0\">.<xsl:call-template name=\"down\">\
                  <xsl:with-param name=\"n\" select=\"$n - 1\"/></xsl:call-template></xsl:if></xsl:template>";
  let stylesheet = Stylesheet::compile(sheet(body).as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);

  let deep = Transform::new().run(&stylesheet, &model, model.root_node()).expect("transforms");
  assert_eq!(deep.text(), "..........");

  let shallow = Transform::new().with_max_depth(5).run(&stylesheet, &model, model.root_node());
  assert!(shallow.unwrap_err().message().contains("templates deep"));
}

#[test]
fn an_instruction_that_is_not_implemented_says_so_rather_than_being_skipped() {
  let body = "<xsl:template match=\"/\"><xsl:number/></xsl:template>";
  let message = error(body, "<a/>");
  assert!(message.contains("xsl:number"), "{message}");
  assert!(message.contains("ROADMAP"), "{message}");
}

#[test]
fn what_an_instruction_cannot_do_without_is_reported() {
  assert!(error("<xsl:template match=\"/\"><xsl:value-of/></xsl:template>", "<a/>").contains("needs a select"));
  assert!(error("<xsl:template match=\"/\"><xsl:if>x</xsl:if></xsl:template>", "<a/>").contains("needs a test"));
  let call = "<xsl:template match=\"/\"><xsl:call-template name=\"nosuch\"/></xsl:template>";
  assert!(error(call, "<a/>").contains("no template is named"));
  let for_each = "<xsl:template match=\"/\"><xsl:for-each select=\"1\">x</xsl:for-each></xsl:template>";
  assert!(error(for_each, "<a/>").contains("selects a node-set"));
}
