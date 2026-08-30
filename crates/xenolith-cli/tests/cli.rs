//! What the `xenolith` binary does when it is actually run.
//!
//! These drive the compiled executable the way a shell would — arguments in, standard input
//! piped, standard output and the exit status read back — so what they check is the contract the
//! command line makes, not the shape of the code behind it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// A directory of this test's own, removed when it is done.
struct Sandbox {
  path: PathBuf,
}

impl Sandbox {
  fn new(name: &str) -> Self {
    let path = std::env::temp_dir().join(format!("xenolith-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("a directory to work in");
    Self { path }
  }

  /// Writes a file into the sandbox and gives its path.
  fn file(&self, name: &str, content: &str) -> PathBuf {
    let path = self.path.join(name);
    fs::write(&path, content).expect("to write the file");
    path
  }
}

impl Drop for Sandbox {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.path);
  }
}

/// Runs the binary with `arguments`, feeding `stdin` to it.
fn xenolith(arguments: &[&str], stdin: &str) -> Output {
  let mut child = Command::new(env!("CARGO_BIN_EXE_xenolith"))
    .args(arguments)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("to start xenolith");
  child.stdin.as_mut().expect("a pipe to write to").write_all(stdin.as_bytes()).expect("to write to xenolith");
  child.wait_with_output().expect("to wait for xenolith")
}

fn stdout(output: &Output) -> String {
  String::from_utf8(output.stdout.clone()).expect("what it wrote to be UTF-8")
}

fn stderr(output: &Output) -> String {
  String::from_utf8(output.stderr.clone()).expect("what it said to be UTF-8")
}

const STYLESHEET: &str = r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="text"/>
  <xsl:param name="greeting">Hello</xsl:param>
  <xsl:template match="/">
    <xsl:value-of select="concat($greeting, ', ', /doc/name)"/>
  </xsl:template>
</xsl:stylesheet>
"#;

const DOCUMENT: &str = "<doc><name>world</name></doc>";

#[test]
fn a_stylesheet_runs_over_a_document_on_standard_input() {
  let sandbox = Sandbox::new("transform");
  let stylesheet = sandbox.file("s.xsl", STYLESHEET);

  let output = xenolith(&["transform", stylesheet.to_str().unwrap()], DOCUMENT);
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "Hello, world");
}

#[test]
fn a_parameter_reaches_the_stylesheet() {
  let sandbox = Sandbox::new("param");
  let stylesheet = sandbox.file("s.xsl", STYLESHEET);
  let input = sandbox.file("d.xml", DOCUMENT);

  let output =
    xenolith(&["transform", "--param", "greeting=Good day", stylesheet.to_str().unwrap(), input.to_str().unwrap()], "");
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "Good day, world");
}

#[test]
fn a_parameter_without_an_equals_sign_is_refused() {
  let sandbox = Sandbox::new("bad-param");
  let stylesheet = sandbox.file("s.xsl", STYLESHEET);

  let output = xenolith(&["transform", "--param", "greeting", stylesheet.to_str().unwrap()], DOCUMENT);
  assert_eq!(output.status.code(), Some(2));
  assert!(stderr(&output).contains("name=value"), "{}", stderr(&output));
}

#[test]
fn the_result_can_be_written_to_a_file() {
  let sandbox = Sandbox::new("output");
  let stylesheet = sandbox.file("s.xsl", STYLESHEET);
  let result = sandbox.path.join("out.txt");

  let output = xenolith(&["transform", "--output", result.to_str().unwrap(), stylesheet.to_str().unwrap()], DOCUMENT);
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "");
  assert_eq!(fs::read_to_string(&result).unwrap(), "Hello, world");
}

#[test]
fn what_the_stylesheet_imports_is_fetched_beside_it() {
  let sandbox = Sandbox::new("import");
  sandbox.file(
    "base.xsl",
    r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:template match="name">imported: <xsl:value-of select="."/></xsl:template>
</xsl:stylesheet>
"#,
  );
  let main = sandbox.file(
    "main.xsl",
    r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:import href="base.xsl"/>
  <xsl:output method="text"/>
</xsl:stylesheet>
"#,
  );

  // The import is relative, so it is found only if the stylesheet's own path became its system
  // identifier.
  let output = xenolith(&["transform", main.to_str().unwrap()], DOCUMENT);
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "imported: world");
}

#[test]
fn a_message_goes_to_standard_error_and_leaves_the_result_alone() {
  let sandbox = Sandbox::new("message");
  let stylesheet = sandbox.file(
    "s.xsl",
    r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="text"/>
  <xsl:template match="/"><xsl:message>a note</xsl:message>result</xsl:template>
</xsl:stylesheet>
"#,
  );

  let output = xenolith(&["transform", stylesheet.to_str().unwrap()], DOCUMENT);
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "result");
  assert!(stderr(&output).contains("a note"), "{}", stderr(&output));
}

#[test]
fn an_expression_prints_one_node_per_line() {
  let output = xenolith(&["xpath", "//item"], "<r><item>a</item><item>b</item></r>");
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "<item>a</item>\n<item>b</item>\n");
}

#[test]
fn an_expression_that_is_not_a_node_set_prints_as_a_string() {
  let output = xenolith(&["xpath", "count(//item)"], "<r><item/><item/></r>");
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output).trim_end(), "2");
}

#[test]
fn a_namespace_binding_is_honoured() {
  let output =
    xenolith(&["xpath", "--namespace", "x=urn:d", "//x:a/text()"], "<r xmlns:d='urn:d'><d:a>found</d:a></r>");
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output).trim_end(), "found");
}

#[test]
fn selecting_nothing_is_only_a_failure_when_asked_for() {
  let quiet = xenolith(&["xpath", "//missing"], "<r/>");
  assert!(quiet.status.success());
  assert_eq!(stdout(&quiet), "");

  let strict = xenolith(&["xpath", "--fail-on-empty", "//missing"], "<r/>");
  assert_eq!(strict.status.code(), Some(1));
}

#[test]
fn an_expression_that_will_not_compile_is_a_failed_request() {
  let output = xenolith(&["xpath", "//["], "<r/>");
  assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn a_valid_document_and_an_invalid_one_are_told_apart() {
  let sandbox = Sandbox::new("validate");
  let valid = sandbox.file("valid.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>");
  let invalid = sandbox.file("invalid.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r><x/></r>");

  let good = xenolith(&["validate", valid.to_str().unwrap()], "");
  assert!(good.status.success(), "{}", stdout(&good));
  assert!(stdout(&good).contains("valid"));

  let bad = xenolith(&["validate", invalid.to_str().unwrap()], "");
  assert_eq!(bad.status.code(), Some(1));
  assert!(stdout(&bad).contains("validity error"), "{}", stdout(&bad));
}

#[test]
fn a_document_with_no_doctype_is_not_called_valid() {
  // There is nothing to be valid against, which is neither a pass nor a violation; it must not
  // be reported as either.
  let output = xenolith(&["validate"], "<r/>");
  assert_eq!(output.status.code(), Some(1));
  assert!(stdout(&output).contains("no DOCTYPE"), "{}", stdout(&output));
}

#[test]
fn several_documents_are_all_reported() {
  let sandbox = Sandbox::new("validate-many");
  let one = sandbox.file("one.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>");
  let two = sandbox.file("two.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>");

  let output = xenolith(&["validate", one.to_str().unwrap(), two.to_str().unwrap()], "");
  assert!(output.status.success(), "{}", stdout(&output));
  assert_eq!(stdout(&output).lines().count(), 2);
}

#[test]
fn a_document_is_written_out_indented() {
  let output = xenolith(&["format"], "<r><a><b>x</b></a></r>");
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "<r>\n  <a>\n    <b>x</b>\n  </a>\n</r>");
}

#[test]
fn the_indent_width_is_the_one_asked_for() {
  let output = xenolith(&["format", "--indent", "4"], "<r><a/></r>");
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "<r>\n    <a/>\n</r>");
}

#[test]
fn a_file_that_is_not_there_is_a_failed_request_not_a_no() {
  let output = xenolith(&["format", "no-such-file.xml"], "");
  assert_eq!(output.status.code(), Some(2));
  assert!(stderr(&output).contains("no-such-file.xml"), "{}", stderr(&output));
}

#[test]
fn xml_that_is_not_well_formed_is_a_failed_request() {
  let output = xenolith(&["format"], "<r><a></r>");
  assert_eq!(output.status.code(), Some(2));
  assert!(stderr(&output).contains("<stdin>"), "{}", stderr(&output));
}
