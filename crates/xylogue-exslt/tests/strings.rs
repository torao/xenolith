//! `http://exslt.org/str`, run through the XSLT engine.

#![cfg(feature = "strings")]

use std::rc::Rc;

use xylogue_core::error::Result;
use xylogue_dom::build;
use xylogue_xdm::{Documents, DomModel};
use xylogue_xpath::Functions;
use xylogue_xslt::{DocumentSource, Stylesheet, Transform, TreeSpace};

/// The namespace declarations these stylesheets need.
const PREFIXES: &str = "xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
                        xmlns:str=\"http://exslt.org/str\"";

/// Evaluates one expression, with somewhere for a built tree to go.
fn value_of(expression: &str) -> String {
  evaluate(expression, true).expect("transforms")
}

/// Evaluates one expression, saying whether the functions get somewhere to build trees.
fn evaluate(expression: &str, with_space: bool) -> Result<String> {
  let body = format!("<xsl:template match='/'><xsl:value-of select=\"{expression}\"/></xsl:template>");
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl")?;
  let doc = build::parse("<r><n>a</n><n>b</n></r>".as_bytes())?;
  let documents = Documents::new();
  let model = DomModel::with_documents(&doc, &documents);
  let space: Rc<dyn DocumentSource<_>> = Rc::new(TreeSpace::new(&documents));

  let functions = if with_space {
    xylogue_exslt::register_with(Functions::new(), &space)
  } else {
    xylogue_exslt::register(Functions::new())
  };
  let result = Transform::new().run_with_documents(&stylesheet, &model, model.root_node(), functions, space)?;
  Ok(result.text())
}

#[test]
fn tokenize_splits_on_any_of_the_delimiters() {
  assert_eq!(value_of("count(str:tokenize('a b c'))"), "3");
  assert_eq!(value_of("str:tokenize('a b c')[2]"), "b");
  // Every character of the second argument is a delimiter of its own.
  assert_eq!(value_of("count(str:tokenize('a,b;c', ',;'))"), "3");
  assert_eq!(value_of("str:tokenize('a,b;c', ',;')[3]"), "c");
}

#[test]
fn tokenize_names_each_piece_a_token() {
  assert_eq!(value_of("name(str:tokenize('a b')[1])"), "token");
}

#[test]
fn tokenize_with_no_delimiters_gives_a_token_per_character() {
  assert_eq!(value_of("count(str:tokenize('abc', ''))"), "3");
  assert_eq!(value_of("str:tokenize('abc', '')[2]"), "b");
}

#[test]
fn split_takes_its_second_argument_as_one_whole_separator() {
  // Where tokenize would treat `--` as two delimiters, split treats it as one.
  assert_eq!(value_of("count(str:split('a--b--c', '--'))"), "3");
  assert_eq!(value_of("str:split('a--b--c', '--')[2]"), "b");
  assert_eq!(value_of("count(str:split('a b c'))"), "3", "a space by default");
}

#[test]
fn what_they_build_is_a_node_set_and_behaves_as_one() {
  assert_eq!(value_of("count(str:tokenize('a b c')[. != 'b'])"), "2");
  assert_eq!(value_of("string(str:tokenize('a b c')[last()])"), "c");
}

#[test]
fn concat_runs_the_string_values_of_a_node_set_together() {
  assert_eq!(value_of("str:concat(//n)"), "ab");
  assert_eq!(value_of("str:concat(str:tokenize('x y'))"), "xy");
}

#[test]
fn padding_makes_a_string_of_the_length_asked_for() {
  assert_eq!(value_of("string-length(str:padding(5))"), "5");
  assert_eq!(value_of("str:padding(5, '-')"), "-----");
  assert_eq!(value_of("str:padding(5, 'ab')"), "ababa");
}

#[test]
fn align_places_text_in_a_field_the_width_of_the_second_argument() {
  assert_eq!(value_of("str:align('x', '-----')"), "x----");
  assert_eq!(value_of("str:align('x', '-----', 'right')"), "----x");
  assert_eq!(value_of("str:align('x', '-----', 'center')"), "--x--");
}

#[test]
fn uri_escaping_goes_both_ways() {
  assert_eq!(value_of("str:encode-uri('a b', false())"), "a%20b");
  assert_eq!(value_of("str:encode-uri('a/b', true())"), "a%2Fb");
  assert_eq!(value_of("str:decode-uri('a%20b')"), "a b");
  assert_eq!(value_of("str:decode-uri(str:encode-uri('a b/c', true()))"), "a b/c");
}

#[test]
fn without_somewhere_to_build_a_tree_the_two_that_need_one_say_so() {
  // The other functions are unaffected, since they answer with strings.
  assert_eq!(evaluate("str:padding(3, '-')", false).expect("transforms"), "---");

  let error = evaluate("count(str:tokenize('a b'))", false).expect_err("nowhere to build");
  assert!(error.message().contains("str:tokenize"), "{}", error.message());
  assert!(error.message().contains("TreeSpace"), "{}", error.message());
}

#[test]
fn function_available_says_what_this_build_has() {
  assert_eq!(value_of("function-available('str:tokenize')"), "true");
  assert_eq!(value_of("function-available('str:padding')"), "true");
  // Still to come, and honest about it.
  assert_eq!(value_of("function-available('str:replace')"), "false");
}
