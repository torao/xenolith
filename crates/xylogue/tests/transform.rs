//! The `javax.xml.transform`-shaped facade.

use xylogue::Result;
use xylogue::dom::build;
use xylogue::transform::{Source, Transformer};
use xylogue::xslt::Loader;

/// A stylesheet writing the names it finds, with a greeting that can be set.
const GREETER: &[u8] = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
    <xsl:output method="text"/>
    <xsl:param name="greeting">Hello</xsl:param>
    <xsl:template match="/"><xsl:for-each select="//name"><xsl:value-of
      select="concat($greeting, ', ', ., '. ')"/></xsl:for-each></xsl:template>
  </xsl:stylesheet>"#;

const PEOPLE: &[u8] = b"<doc><name>Ada</name><name>Alan</name></doc>";

#[test]
fn a_transformer_compiles_once_and_runs_over_many_documents() {
  let transformer = Transformer::compile(Source::bytes(GREETER)).expect("compiles");
  assert_eq!(
    transformer.transform(Source::bytes(PEOPLE)).expect("transforms").text().trim(),
    "Hello, Ada. Hello, Alan."
  );
  let other = b"<doc><name>Grace</name></doc>";
  assert_eq!(transformer.transform(Source::bytes(other)).expect("transforms").text().trim(), "Hello, Grace.");
}

#[test]
fn a_parameter_replaces_the_default_the_stylesheet_gave() {
  let transformer = Transformer::compile(Source::bytes(GREETER)).expect("compiles").with_parameter("greeting", "Hi");
  assert_eq!(transformer.transform(Source::bytes(PEOPLE)).expect("transforms").text().trim(), "Hi, Ada. Hi, Alan.");
}

#[test]
fn a_parameter_set_twice_takes_the_last_value() {
  let transformer = Transformer::compile(Source::bytes(GREETER))
    .expect("compiles")
    .with_parameter("greeting", "first")
    .with_parameter("greeting", "second");
  assert!(transformer.transform(Source::bytes(PEOPLE)).expect("transforms").text().starts_with("second"));
}

#[test]
fn a_top_level_variable_is_not_a_parameter_and_cannot_be_set() {
  // Which is the difference the two declarations exist to draw.
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:output method="text"/>
      <xsl:variable name="fixed">declared</xsl:variable>
      <xsl:template match="/"><xsl:value-of select="$fixed"/></xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source)).expect("compiles").with_parameter("fixed", "supplied");
  assert_eq!(transformer.transform(Source::bytes(b"<a/>")).expect("transforms").text(), "declared");
}

#[test]
fn a_document_already_built_can_be_transformed_without_parsing_it_again() {
  let document = build::parse(PEOPLE).expect("well-formed");
  let transformer = Transformer::compile(Source::bytes(GREETER)).expect("compiles");
  let result = transformer.transform(Source::document(&document)).expect("transforms");
  assert_eq!(result.text().trim(), "Hello, Ada. Hello, Alan.");
}

#[test]
fn the_identity_transformer_writes_the_document_out() {
  let document = build::parse(&b"<r k=\"v\"><a>text</a></r>"[..]).expect("well-formed");
  let result = Transformer::identity().transform(Source::document(&document)).expect("transforms");
  assert_eq!(result.text(), "<r k=\"v\"><a>text</a></r>");
}

#[test]
fn what_xsl_output_asked_for_is_carried_out() {
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:output method="html" omit-xml-declaration="yes"/>
      <xsl:template match="/"><p>one<br/></p></xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source)).expect("compiles");
  assert_eq!(transformer.transform(Source::bytes(b"<a/>")).expect("transforms").text(), "<p>one<br></p>");
}

#[test]
fn the_result_can_be_written_as_bytes() {
  let transformer = Transformer::compile(Source::bytes(GREETER)).expect("compiles");
  let result = transformer.transform(Source::bytes(PEOPLE)).expect("transforms");
  let mut written = Vec::new();
  result.write(&mut written).expect("writes");
  assert_eq!(written, result.bytes());
  assert_eq!(String::from_utf8(written).expect("UTF-8").trim(), "Hello, Ada. Hello, Alan.");
}

#[test]
fn what_a_message_said_comes_back_beside_the_result() {
  // Where JAXP would have called an ErrorListener's warning().
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:template match="/"><xsl:message>looked at it</xsl:message>done</xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source)).expect("compiles");
  let result = transformer.transform(Source::bytes(b"<a/>")).expect("transforms");
  assert_eq!(result.messages(), ["looked at it"]);
}

#[test]
fn what_would_have_been_a_fatal_error_is_the_error_of_the_call() {
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:template match="/"><xsl:message terminate="yes">no good</xsl:message></xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source)).expect("compiles");
  let error = transformer.transform(Source::bytes(b"<a/>")).expect_err("stopped");
  assert!(error.message().contains("no good"), "{}", error.message());
}

// --- Resolving what a stylesheet names -----------------------------------------------------------

/// A resolver serving one module and one document.
struct Shelf;

impl Loader for Shelf {
  fn load(&mut self, uri: &str) -> Result<Vec<u8>> {
    if uri.ends_with("common.xsl") {
      return Ok(
        br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
          <xsl:template match="shared">[shared]</xsl:template>
        </xsl:stylesheet>"#
          .to_vec(),
      );
    }
    Ok(b"<extra><n>fetched</n></extra>".to_vec())
  }
}

#[test]
fn a_stylesheet_built_from_several_modules_is_compiled_through_the_resolver() {
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:output method="text"/>
      <xsl:include href="common.xsl"/>
      <xsl:template match="/"><xsl:apply-templates select="//shared"/></xsl:template>
    </xsl:stylesheet>"#;
  let transformer =
    Transformer::compile_with(Source::bytes(source).with_system_id("file:///dir/s.xsl"), &mut Shelf).expect("compiles");
  assert_eq!(transformer.transform(Source::bytes(b"<r><shared/></r>")).expect("transforms").text(), "[shared]");
}

#[test]
fn a_module_named_with_no_resolver_is_reported() {
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:include href="common.xsl"/>
    </xsl:stylesheet>"#;
  let error = Transformer::compile(Source::bytes(source)).expect_err("no resolver");
  assert!(error.message().contains("no loader was given"), "{}", error.message());
}

#[test]
fn document_fetches_through_the_resolver() {
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:output method="text"/>
      <xsl:template match="/"><xsl:value-of select="document('other.xml')//n"/></xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source).with_system_id("file:///dir/s.xsl"))
    .expect("compiles")
    .with_resolver(|| Box::new(Shelf));
  assert_eq!(transformer.transform(Source::bytes(b"<a/>")).expect("transforms").text(), "fetched");
}

#[test]
fn without_a_resolver_document_finds_nothing() {
  // Fetching is I/O on the caller's behalf, so it is not done unless asked for.
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:output method="text"/>
      <xsl:template match="/"><xsl:value-of select="count(document('other.xml'))"/></xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source)).expect("compiles");
  assert_eq!(transformer.transform(Source::bytes(b"<a/>")).expect("transforms").text(), "0");
}

#[cfg(feature = "exslt")]
#[test]
fn the_exslt_functions_are_there_without_being_asked_for() {
  let source = br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
                    xmlns:math="http://exslt.org/math" xmlns:exsl="http://exslt.org/common">
      <xsl:output method="text"/>
      <xsl:template match="/">
        <xsl:variable name="frag"><i>1</i><i>2</i></xsl:variable>
        <xsl:value-of select="concat(math:max(//n), '/', count(exsl:node-set($frag)/i))"/>
      </xsl:template>
    </xsl:stylesheet>"#;
  let transformer = Transformer::compile(Source::bytes(source)).expect("compiles");
  let result = transformer.transform(Source::bytes(b"<r><n>3</n><n>9</n></r>")).expect("transforms");
  assert_eq!(result.text().trim(), "9/2", "and a fragment has somewhere to become a tree");
}

#[test]
fn a_stylesheet_that_is_not_one_is_refused_with_a_reason() {
  let error = Transformer::compile(Source::bytes(b"<not-a-stylesheet/>")).expect_err("not a stylesheet");
  assert!(error.message().contains("xsl:stylesheet"), "{}", error.message());
}

#[test]
fn a_document_is_compiled_from_bytes_rather_than_from_a_tree() {
  // The modules a stylesheet names are fetched while it is compiled, and each needs its own
  // document, so this says so rather than half-working.
  let document = build::parse(&b"<a/>"[..]).expect("well-formed");
  let error = Transformer::compile(Source::document(&document)).expect_err("wrong kind of source");
  assert!(error.message().contains("Source::bytes"), "{}", error.message());
}
