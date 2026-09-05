//! What a stylesheet says about processing: whitespace, namespace aliases, and being read by a
//! processor older than it was written for (XSLT 1.0 §3.4, §7.1.1, §2.5, §15).

use xenolith_dom::build;
use xenolith_serialize::Serializer;
use xenolith_xdm::DomModel;
use xenolith_xslt::{Stylesheet, transform};

/// Wraps top-level content in an `xsl:stylesheet` of a given version.
fn sheet_version(version: &str, body: &str) -> String {
  format!(
    "<xsl:stylesheet version=\"{version}\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>"
  )
}

/// Transforms `xml` and takes the text of the result.
fn run(body: &str, xml: &str) -> String {
  run_version("1.0", body, xml)
}

fn run_version(version: &str, body: &str, xml: &str) -> String {
  let source = sheet_version(version, body);
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect("transforms").text()
}

/// Transforms `xml` and serializes the result as markup.
fn markup(body: &str, xml: &str) -> String {
  let source = sheet_version("1.0", body);
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  Serializer::new().to_string(result.document(), result.root())
}

/// The message a transformation fails with.
fn error(body: &str, xml: &str) -> String {
  let source = sheet_version("1.0", body);
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect_err("fails").message().to_owned()
}

// --- xsl:strip-space and xsl:preserve-space (§3.4) -----------------------------------------------

/// Source with whitespace between its elements, which is what strip-space is about.
const SPACED: &str = "<r>\n  <a>one</a>\n  <b>two</b>\n</r>";

#[test]
fn source_whitespace_is_kept_unless_the_stylesheet_asks() {
  // The default: XSLT strips stylesheet whitespace, never the source's. The `a` and `b` have no
  // rule of their own, so the built-in one walks into them and their text is bracketed too.
  let body = "<xsl:template match='/'><xsl:apply-templates select='//r/node()'/></xsl:template>\
              <xsl:template match='text()'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(body, SPACED), "[\n  ][one][\n  ][two][\n]");
}

#[test]
fn strip_space_drops_whitespace_only_text_under_the_elements_it_names() {
  let body = "<xsl:strip-space elements='r'/>\
              <xsl:template match='/'><xsl:apply-templates select='//r/node()'/></xsl:template>\
              <xsl:template match='text()'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(body, SPACED), "[one][two]", "only the whitespace between the elements goes");
}

#[test]
fn text_that_is_not_only_whitespace_is_never_stripped() {
  let body = "<xsl:strip-space elements='*'/>\
              <xsl:template match='/'><xsl:apply-templates select='//r/node()'/></xsl:template>\
              <xsl:template match='text()'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(body, "<r>  kept  <a/></r>"), "[  kept  ]");
}

#[test]
fn a_star_names_every_element() {
  let body = "<xsl:strip-space elements='*'/>\
              <xsl:template match='/'><xsl:value-of select='.'/></xsl:template>";
  assert_eq!(run(body, SPACED), "\n  one\n  two\n", "value-of is XPath's, and reads the source as it is");

  // What is *processed* is what strip-space changes.
  let walked = "<xsl:strip-space elements='*'/>\
                <xsl:template match='/'><xsl:apply-templates select='//r/node()'/></xsl:template>\
                <xsl:template match='text()'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(walked, SPACED), "[one][two]");
}

#[test]
fn preserve_space_wins_where_it_is_more_specific() {
  // §3.4 settles a conflict the way §5.5 does: the more specific name test decides. Only the
  // text nodes are selected here, so each survivor is counted once.
  let body = "<xsl:strip-space elements='*'/><xsl:preserve-space elements='keep'/>\
              <xsl:template match='/'><xsl:apply-templates select='//text()'/></xsl:template>\
              <xsl:template match='text()'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(body, "<r> <keep> </keep> </r>"), "[ ]", "the two under r go, the one under keep stays");
}

#[test]
fn xml_space_preserve_overrules_the_stylesheet() {
  let body = "<xsl:strip-space elements='*'/>\
              <xsl:template match='/'><xsl:apply-templates select='//r/node()'/></xsl:template>\
              <xsl:template match='text()'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(body, "<r xml:space='preserve'> <a/> </r>"), "[ ][ ]");
  // And the nearest declaration decides, so `default` below `preserve` strips again.
  assert_eq!(run(body, "<r xml:space='preserve'> <a xml:space='default'> </a> </r>"), "[ ][ ]");
}

#[test]
fn strip_space_applies_to_the_children_a_built_in_rule_walks() {
  let body = "<xsl:strip-space elements='*'/>\
              <xsl:template match='a'>[<xsl:value-of select='.'/>]</xsl:template>";
  // The built-in rules walk from the root; the whitespace between elements produces nothing.
  assert_eq!(run(body, SPACED), "[one]two");
}

#[test]
fn what_a_space_declaration_cannot_do_without_is_reported() {
  let missing = "<xsl:strip-space/>";
  let error = Stylesheet::compile(sheet_version("1.0", missing).as_bytes(), "file:///s.xsl").expect_err("fails");
  assert!(error.message().contains("needs an elements"), "{}", error.message());
}

// --- xsl:namespace-alias (§7.1.1) ----------------------------------------------------------------

#[test]
fn a_namespace_alias_sends_one_namespace_into_the_result_as_another() {
  // The reason it exists: writing a stylesheet that produces a stylesheet.
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:out='urn:placeholder'>\
                  <xsl:namespace-alias stylesheet-prefix='out' result-prefix='xsl'/>\
                  <xsl:template match='/'><out:template match='x'/></xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  let written = Serializer::new().to_string(result.document(), result.root());
  assert!(written.contains("http://www.w3.org/1999/XSL/Transform"), "{written}");
  assert!(written.contains(":template"), "{written}");
  assert!(!written.contains("urn:placeholder"), "the stylesheet's own namespace does not reach the result: {written}");
}

#[test]
fn a_namespace_alias_may_name_the_default_namespace() {
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:out='urn:placeholder' xmlns='urn:wanted'>\
                  <xsl:namespace-alias stylesheet-prefix='out' result-prefix='#default'/>\
                  <xsl:template match='/'><out:thing/></xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  let written = Serializer::new().to_string(result.document(), result.root());
  assert!(written.contains("urn:wanted"), "{written}");
  assert!(!written.contains("urn:placeholder"), "{written}");
}

#[test]
fn without_an_alias_a_literal_element_keeps_its_namespace() {
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:o='urn:o'>\
                  <xsl:template match='/'><o:thing/></xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  let written = Serializer::new().to_string(result.document(), result.root());
  assert!(written.contains("urn:o"), "{written}");
}

#[test]
fn a_namespace_declaration_of_the_stylesheet_is_never_copied_to_the_result() {
  // Which is what `exclude-result-prefixes` is for elsewhere: this engine copies an element's
  // name and namespace, never its declarations, and the serializer writes only what the result
  // needs. So there is nothing for the attribute to exclude.
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:unused='urn:unused'>\
                  <xsl:template match='/'><out/></xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  assert_eq!(Serializer::new().to_string(result.document(), result.root()), "<out/>");
}

// --- Forwards-compatible processing (§2.5) and xsl:fallback (§15) --------------------------------

#[test]
fn an_element_this_does_not_know_is_an_error_in_a_one_point_zero_stylesheet() {
  let body = "<xsl:template match='/'><xsl:perform-magic/></xsl:template>";
  assert!(error(body, "<a/>").contains("perform-magic"), "{}", error(body, "<a/>"));
}

#[test]
fn a_later_version_makes_an_unknown_element_wait_until_it_is_reached() {
  // §2.5: the stylesheet says it was written for a later XSLT, so an element this does not know
  // is only a problem if it is actually run — and then only without a fallback.
  let body = "<xsl:template match='/'>\
                <xsl:perform-magic><xsl:fallback>no magic here</xsl:fallback></xsl:perform-magic>\
              </xsl:template>";
  assert_eq!(run_version("2.0", body, "<a/>"), "no magic here");
}

#[test]
fn an_unknown_element_that_is_never_reached_costs_nothing() {
  let body = "<xsl:template match='/'>ran\
                <xsl:if test='false()'><xsl:perform-magic/></xsl:if>\
              </xsl:template>";
  assert_eq!(run_version("2.0", body, "<a/>"), "ran");
}

#[test]
fn an_unknown_element_with_no_fallback_is_still_reported() {
  let body = "<xsl:template match='/'><xsl:perform-magic/></xsl:template>";
  let source = sheet_version("2.0", body);
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let error = transform(&stylesheet, &model, model.root_node()).expect_err("fails");
  assert!(error.message().contains("perform-magic"), "{}", error.message());
}

#[test]
fn a_fallback_reached_on_its_own_does_nothing() {
  // §15: it is only meaningful inside an element the processor did not understand.
  let body = "<xsl:template match='/'>before<xsl:fallback>unused</xsl:fallback>after</xsl:template>";
  assert_eq!(run(body, "<a/>"), "beforeafter");
}

#[test]
fn element_available_says_fallback_is_there() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"element-available('xsl:fallback')\"/>\
              <xsl:value-of select=\"element-available('xsl:perform-magic')\"/></xsl:template>";
  assert_eq!(run(body, "<a/>"), "truefalse");
}

#[test]
fn a_version_that_is_not_a_number_is_read_as_one_point_zero() {
  // Being forgiving is for a *later* XSLT; something unreadable is not one, so its unknown
  // elements stay errors rather than being quietly skipped.
  let body = "<xsl:template match='/'><xsl:perform-magic/></xsl:template>";
  let source = sheet_version("tomorrow", body);
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  assert!(transform(&stylesheet, &model, model.root_node()).is_err());
}

// --- Extension elements (§14) --------------------------------------------------------------------

/// A stylesheet declaring `ext` as an extension-element prefix.
fn with_extensions(body: &str) -> String {
  format!(
    "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
     xmlns:ext='urn:ext' extension-element-prefixes='ext'>{body}</xsl:stylesheet>"
  )
}

/// Transforms with the extension prefix declared, giving the result as markup.
fn extension_markup(body: &str) -> Result<String, String> {
  let source = with_extensions(body);
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  match transform(&stylesheet, &model, model.root_node()) {
    Ok(result) => Ok(Serializer::new().to_string(result.document(), result.root())),
    Err(error) => Err(error.message().to_owned()),
  }
}

#[test]
fn an_element_of_an_extension_namespace_never_reaches_the_result() {
  // Before extension elements were told apart from literal ones this was copied into the
  // output, where it looked like something the stylesheet meant to produce.
  let body = "<xsl:template match='/'><out><ext:magic/></out></xsl:template>";
  let error = extension_markup(body).expect_err("no extension element is implemented");
  assert!(error.contains("extension element"), "{error}");
  assert!(error.contains("magic"), "{error}");
}

#[test]
fn an_extension_element_uses_its_fallback() {
  let body = "<xsl:template match='/'><out><ext:magic><xsl:fallback>plain</xsl:fallback></ext:magic></out>\
              </xsl:template>";
  assert_eq!(extension_markup(body).expect("falls back"), "<out>plain</out>");
}

#[test]
fn an_element_of_a_namespace_that_was_not_declared_is_a_literal_result_element() {
  // Only the prefixes `extension-element-prefixes` lists are extension elements; everything
  // else in a namespace is ordinary output.
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:o='urn:o'><xsl:template match='/'><o:thing/></xsl:template></xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  let written = Serializer::new().to_string(result.document(), result.root());
  assert!(written.contains("urn:o"), "{written}");
}

#[test]
fn the_declaration_reaches_only_the_element_it_is_on_and_below() {
  // Declared on one literal result element, in the XSLT namespace because an unprefixed
  // attribute there would be part of the result.
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform' \
                xmlns:ext='urn:ext'>\
                  <xsl:template match='/'>\
                    <outside><ext:magic/></outside>\
                    <inside xsl:extension-element-prefixes='ext'><ext:magic/></inside>\
                  </xsl:template>\
                </xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  // The one outside the declaration is a literal result element and comes out; the one inside
  // is an extension element and stops the transformation.
  let error = transform(&stylesheet, &model, model.root_node()).expect_err("the inner one is an extension element");
  assert!(error.message().contains("extension element"), "{}", error.message());
}

#[test]
fn element_available_is_false_for_an_extension_element() {
  // Which is the answer that lets a stylesheet choose another route before it commits.
  let body = "<xsl:template match='/'><xsl:value-of select=\"element-available('ext:magic')\"/></xsl:template>";
  assert_eq!(extension_markup(body).expect("transforms"), "false");
}

#[test]
fn markup_still_comes_out_as_it_did() {
  // A guard that none of the above changed the ordinary case.
  let body = "<xsl:template match='/'><out><xsl:value-of select='//a'/></out></xsl:template>";
  assert_eq!(markup(body, "<r><a>text</a></r>"), "<out>text</out>");
}
