//! `xsl:output` and how a result is written (XSLT 1.0 §16).

use xenolith_dom::build;
use xenolith_xdm::DomModel;
use xenolith_xslt::{OutputMethod, Stylesheet, transform};

/// Transforms `<a/>` with the given stylesheet body and writes the result as §16 asks.
fn written(body: &str) -> String {
  written_over(body, "<a/>")
}

fn written_over(body: &str, xml: &str) -> String {
  let source = format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>"
  );
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect("transforms").serialize()
}

/// The result of a stylesheet, for asking about its output settings.
fn settings(body: &str) -> xenolith_xslt::Output {
  let source = format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">{body}</xsl:stylesheet>"
  );
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  stylesheet.output().clone()
}

// --- The XML method (§16.1) ----------------------------------------------------------------------

#[test]
fn a_result_whose_root_is_html_is_written_as_html_without_being_told() {
  // §16: with no method stated, a result whose document element is `html` in no namespace is
  // written by the HTML method. It cannot be settled until the tree exists, which is the point —
  // a stylesheet that writes HTML without saying so still gets `<br>` rather than `<br/>`.
  let body = "<xsl:template match='/'><html><br/></html></xsl:template>";
  assert_eq!(written(body), "<html><br></html>");

  // "In any combination of upper and lower case", and no XML declaration either way.
  let shouted = "<xsl:template match='/'><HTML><BR/></HTML></xsl:template>";
  assert_eq!(written(shouted), "<HTML><BR></HTML>");

  // In a namespace it is not that `html`, so the default stays XML — declaration and all.
  let namespaced = "<xsl:template match='/'><html xmlns='urn:x'><br/></html></xsl:template>";
  let written_namespaced = written(namespaced);
  assert!(written_namespaced.starts_with("<?xml"), "{written_namespaced}");
  assert!(written_namespaced.contains("<br/>"), "{written_namespaced}");

  // And what the stylesheet did say is never second-guessed.
  let stated = "<xsl:output method='xml'/><xsl:template match='/'><html><br/></html></xsl:template>";
  assert_eq!(written(stated), "<?xml version=\"1.0\" encoding=\"UTF-8\"?><html><br/></html>");
}

#[test]
fn the_xml_method_writes_a_declaration_by_default() {
  let body = "<xsl:template match='/'><out/></xsl:template>";
  assert_eq!(written(body), "<?xml version=\"1.0\" encoding=\"UTF-8\"?><out/>");
}

#[test]
fn the_declaration_can_be_left_out() {
  let body = "<xsl:output omit-xml-declaration='yes'/><xsl:template match='/'><out/></xsl:template>";
  assert_eq!(written(body), "<out/>");
}

#[test]
fn the_declaration_carries_what_it_was_told() {
  let body = "<xsl:output version='1.1' encoding='UTF-8' standalone='yes'/>\
              <xsl:template match='/'><out/></xsl:template>";
  assert_eq!(written(body), "<?xml version=\"1.1\" encoding=\"UTF-8\" standalone=\"yes\"?><out/>");
}

#[test]
fn a_doctype_is_written_before_the_first_element() {
  let body = "<xsl:output omit-xml-declaration='yes' doctype-system='out.dtd'/>\
              <xsl:template match='/'><out/></xsl:template>";
  assert_eq!(written(body), "<!DOCTYPE out SYSTEM \"out.dtd\"><out/>");

  let public = "<xsl:output omit-xml-declaration='yes' doctype-public='-//X//DTD//EN' doctype-system='out.dtd'/>\
                <xsl:template match='/'><out/></xsl:template>";
  assert_eq!(written(public), "<!DOCTYPE out PUBLIC \"-//X//DTD//EN\" \"out.dtd\"><out/>");
}

#[test]
fn cdata_section_elements_wrap_their_text() {
  let body = "<xsl:output omit-xml-declaration='yes' cdata-section-elements='code'/>\
              <xsl:template match='/'><out><code>a &lt; b</code><plain>a &lt; b</plain></out></xsl:template>";
  assert_eq!(written(body), "<out><code><![CDATA[a < b]]></code><plain>a &lt; b</plain></out>");
}

#[test]
fn indent_puts_elements_on_lines_of_their_own() {
  let body = "<xsl:output omit-xml-declaration='yes' indent='yes'/>\
              <xsl:template match='/'><out><a/><b/></out></xsl:template>";
  assert_eq!(written(body), "<out>\n  <a/>\n  <b/>\n</out>");
}

#[test]
fn indent_leaves_text_alone() {
  // Adding whitespace beside text would change what the text says, so it is not added.
  let body = "<xsl:output omit-xml-declaration='yes' indent='yes'/>\
              <xsl:template match='/'><out>text<a/></out></xsl:template>";
  assert_eq!(written(body), "<out>text<a/></out>");
}

// --- The HTML method (§16.2) ---------------------------------------------------------------------

#[test]
fn the_html_method_leaves_an_empty_element_open() {
  let body = "<xsl:output method='html'/><xsl:template match='/'><p>one<br/>two</p></xsl:template>";
  assert_eq!(written(body), "<p>one<br>two</p>");
}

#[test]
fn the_html_method_writes_no_declaration() {
  let body = "<xsl:output method='html'/><xsl:template match='/'><p/></xsl:template>";
  assert_eq!(written(body), "<p></p>", "and an empty element that is not on the list gets an end tag");
}

#[test]
fn the_html_method_does_not_escape_a_script() {
  // An HTML parser will not unescape it, so escaping it would change what it means.
  let body = "<xsl:output method='html'/>\
              <xsl:template match='/'><script>if (a &lt; b) go()</script></xsl:template>";
  assert_eq!(written(body), "<script>if (a < b) go()</script>");
}

#[test]
fn the_html_method_still_escapes_ordinary_text() {
  let body = "<xsl:output method='html'/><xsl:template match='/'><p>a &lt; b</p></xsl:template>";
  assert_eq!(written(body), "<p>a &lt; b</p>");
}

#[test]
fn the_html_method_writes_a_doctype_when_asked() {
  let body = "<xsl:output method='html' doctype-public='-//W3C//DTD HTML 4.01//EN'/>\
              <xsl:template match='/'><html/></xsl:template>";
  assert_eq!(written(body), "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\"><html></html>");
}

// --- The text method (§16.3) ---------------------------------------------------------------------

#[test]
fn the_text_method_writes_characters_and_no_markup() {
  let body = "<xsl:output method='text'/><xsl:template match='/'><out>one<a>two</a></out></xsl:template>";
  assert_eq!(written(body), "onetwo");
}

// --- Disabling output escaping (§16.4) -----------------------------------------------------------

#[test]
fn disable_output_escaping_writes_the_text_as_it_stands() {
  let body = "<xsl:output omit-xml-declaration='yes'/>\
              <xsl:template match='/'><out><xsl:text disable-output-escaping='yes'>&lt;b&gt;</xsl:text></out>\
              </xsl:template>";
  assert_eq!(written(body), "<out><b></out>");
}

#[test]
fn value_of_may_disable_escaping_too() {
  let body = "<xsl:output omit-xml-declaration='yes'/>\
              <xsl:template match='/'><out><xsl:value-of select='//raw' disable-output-escaping='yes'/></out>\
              </xsl:template>";
  assert_eq!(written_over(body, "<r><raw>&lt;i/&gt;</raw></r>"), "<out><i/></out>");
}

#[test]
fn escaping_is_only_disabled_where_it_was_asked_for() {
  // The mark is on the node, so the same text arriving another way is escaped as it should be.
  let body = "<xsl:output omit-xml-declaration='yes'/>\
              <xsl:template match='/'><out><xsl:value-of select='//raw'/></out></xsl:template>";
  assert_eq!(written_over(body, "<r><raw>&lt;i/&gt;</raw></r>"), "<out>&lt;i/&gt;</out>");
}

// --- Merging the declarations (§16) --------------------------------------------------------------

#[test]
fn several_output_declarations_are_merged_attribute_by_attribute() {
  let body = "<xsl:output method='html'/><xsl:output indent='yes'/><xsl:output encoding='UTF-8'/>";
  let output = settings(body);
  assert_eq!(output.method(), OutputMethod::Html);
  assert!(output.indent());
  assert_eq!(output.encoding(), Some("UTF-8"));
}

#[test]
fn cdata_section_elements_are_the_union_of_every_declaration() {
  let body = "<xsl:output omit-xml-declaration='yes' cdata-section-elements='a'/>\
              <xsl:output cdata-section-elements='b'/>\
              <xsl:template match='/'><out><a>1</a><b>2</b><c>3</c></out></xsl:template>";
  assert_eq!(written(body), "<out><a><![CDATA[1]]></a><b><![CDATA[2]]></b><c>3</c></out>");
}

#[test]
fn an_unknown_output_method_is_reported() {
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform'>\
                <xsl:output method='postscript'/></xsl:stylesheet>";
  let error = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect_err("fails");
  assert!(error.message().contains("postscript"), "{}", error.message());
}

// --- Encoding ------------------------------------------------------------------------------------

#[test]
fn utf8_needs_nothing() {
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform'>\
                <xsl:output omit-xml-declaration='yes'/>\
                <xsl:template match='/'><out>\u{65e5}</out></xsl:template></xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = transform(&stylesheet, &model, model.root_node()).expect("transforms");
  assert_eq!(result.to_bytes().expect("writes"), "<out>\u{65e5}</out>".as_bytes());
}

/// A result asking to be written in Shift_JIS, whose one character is a Japanese one.
fn shift_jis_result() -> xenolith_xslt::ResultTree {
  let source = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform'>\
                <xsl:output omit-xml-declaration='yes' encoding='Shift_JIS'/>\
                <xsl:template match='/'><out>\u{65e5}</out></xsl:template></xsl:stylesheet>";
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  transform(&stylesheet, &model, model.root_node()).expect("transforms")
}

#[cfg(feature = "encodings")]
#[test]
fn another_encoding_is_written_when_the_feature_is_there() {
  // Shift_JIS holds the character in two bytes where UTF-8 takes three.
  let bytes = shift_jis_result().to_bytes().expect("writes");
  assert!(bytes.windows(2).any(|pair| pair == [0x93, 0xfa]), "{bytes:?}");
  assert!(!bytes.windows(3).any(|triple| triple == "\u{65e5}".as_bytes()), "not UTF-8 bytes: {bytes:?}");
}

#[cfg(not(feature = "encodings"))]
#[test]
fn another_encoding_is_refused_by_name_without_the_feature() {
  // Never bytes in one encoding under a declaration naming another; an error saying which
  // feature would provide it.
  let error = shift_jis_result().to_bytes().expect_err("cannot be written");
  assert!(error.message().contains("encodings"), "{}", error.message());
  assert!(error.message().contains("Shift_JIS"), "{}", error.message());
}
