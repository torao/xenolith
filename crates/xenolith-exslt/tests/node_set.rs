//! `exsl:node-set()` — a result tree fragment read as a tree.

#![cfg(feature = "common")]

use std::rc::Rc;

use xenolith_core::error::Result;
use xenolith_dom::build;
use xenolith_xdm::{Documents, DomModel};
use xenolith_xpath::Functions;
use xenolith_xslt::{Stylesheet, Transform, TreeSpace, transform};

/// The namespace declarations these stylesheets need.
const PREFIXES: &str = "xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
                        xmlns:exsl=\"http://exslt.org/common\"";

/// Transforms `<a/>` with somewhere for a fragment to become a tree.
fn run(body: &str) -> Result<String> {
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl")?;
  let doc = build::parse("<a/>".as_bytes())?;
  let documents = Documents::new();
  let model = DomModel::with_documents(&doc, &documents);
  let space = Rc::new(TreeSpace::new(&documents));
  let functions = xenolith_exslt::register(Functions::new());
  let result = Transform::new().run_with_documents(&stylesheet, &model, model.root_node(), functions, space)?;
  Ok(result.text())
}

/// Transforms with nowhere to put a tree, which is the default.
fn run_without_a_space(body: &str) -> Result<String> {
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl")?;
  let doc = build::parse("<a/>".as_bytes())?;
  let model = DomModel::new(&doc);
  Ok(transform(&stylesheet, &model, model.root_node())?.text())
}

/// A template declaring a fragment of two `i` elements and then evaluating `expression`.
fn over_fragment(expression: &str) -> String {
  let body = format!(
    "<xsl:template match='/'>\
       <xsl:variable name='frag'><i k='1'>one</i><i k='2'>two</i></xsl:variable>\
       <xsl:value-of select=\"{expression}\"/>\
     </xsl:template>"
  );
  run(&body).expect("transforms")
}

#[test]
fn a_fragment_becomes_a_tree_that_can_be_walked() {
  assert_eq!(over_fragment("count(exsl:node-set($frag)/i)"), "2");
  assert_eq!(over_fragment("exsl:node-set($frag)/i[2]"), "two");
  assert_eq!(over_fragment("exsl:node-set($frag)/i[@k='1']"), "one");
}

#[test]
fn the_tree_has_a_root_of_its_own() {
  // The fragment's children hang from a root, so `/` inside it means that root.
  assert_eq!(over_fragment("name(exsl:node-set($frag)/*[1])"), "i");
  assert_eq!(over_fragment("count(exsl:node-set($frag)/node())"), "2");
}

#[test]
fn asking_twice_gives_the_same_tree() {
  // Or a union of the two would count each node twice, and identity would mean nothing.
  assert_eq!(over_fragment("count(exsl:node-set($frag)/i | exsl:node-set($frag)/i)"), "2");
}

#[test]
fn two_fragments_are_two_trees() {
  let body = "<xsl:template match='/'>\
                <xsl:variable name='one'><i/></xsl:variable>\
                <xsl:variable name='two'><i/></xsl:variable>\
                <xsl:value-of select=\"count(exsl:node-set($one)/i | exsl:node-set($two)/i)\"/>\
              </xsl:template>";
  assert_eq!(run(body).expect("transforms"), "2", "they say the same thing but are not the same node");
}

#[test]
fn a_fragment_is_still_a_string_where_node_set_was_not_asked_for() {
  // §11.1: converting is exactly what exsl:node-set() is for, and nothing else lifts it.
  assert_eq!(over_fragment("$frag"), "onetwo");
  assert_eq!(over_fragment("string-length($frag)"), "6");
}

#[test]
fn a_path_through_a_fragment_without_converting_it_is_refused() {
  // Which is what a conforming XSLT 1.0 processor does, so a stylesheet that works here works
  // elsewhere too.
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><i/></xsl:variable>\
                <xsl:value-of select='$frag/i'/>\
              </xsl:template>";
  assert!(run(body).is_err(), "a fragment is not a node-set until it is converted");
}

#[test]
fn a_converted_fragment_can_be_applied_templates_to() {
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><i>one</i><i>two</i></xsl:variable>\
                <xsl:apply-templates select='exsl:node-set($frag)/i'/>\
              </xsl:template>\
              <xsl:template match='i'>[<xsl:value-of select='.'/>]</xsl:template>";
  assert_eq!(run(body).expect("transforms"), "[one][two]");
}

#[test]
fn a_converted_fragment_can_be_sorted_and_counted() {
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><i>3</i><i>1</i><i>2</i></xsl:variable>\
                <xsl:for-each select='exsl:node-set($frag)/i'>\
                  <xsl:sort select='.' data-type='number'/>\
                  <xsl:value-of select='.'/>\
                </xsl:for-each>\
              </xsl:template>";
  assert_eq!(run(body).expect("transforms"), "123");
}

#[test]
fn a_fragment_that_reaches_a_template_as_a_parameter_converts_too() {
  let body = "<xsl:template match='/'>\
                <xsl:call-template name='count-them'>\
                  <xsl:with-param name='items'><i/><i/><i/></xsl:with-param>\
                </xsl:call-template>\
              </xsl:template>\
              <xsl:template name='count-them'><xsl:param name='items'/>\
                <xsl:value-of select='count(exsl:node-set($items)/i)'/></xsl:template>";
  assert_eq!(run(body).expect("transforms"), "3");
}

#[test]
fn without_somewhere_to_put_the_tree_it_says_so() {
  // Never an answer built from the wrong thing: the message names what to supply.
  let body = "<xsl:template match='/'>\
                <xsl:variable name='frag'><i/></xsl:variable>\
                <xsl:value-of select='count(exsl:node-set($frag)/i)'/>\
              </xsl:template>";
  let error = run_without_a_space(body).expect_err("there is nowhere to put it");
  assert!(error.message().contains("TreeSpace"), "{}", error.message());
}

#[test]
fn node_set_of_something_that_is_not_a_fragment_says_what_it_cannot_do() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"exsl:node-set('text')\"/></xsl:template>";
  let error = run(body).expect_err("a string is not a fragment");
  assert!(error.message().contains("result tree fragment"), "{}", error.message());
}

#[test]
fn node_set_of_a_node_set_is_that_node_set() {
  // EXSLT says so, and it falls out of the engine having nothing to lift.
  let body = "<xsl:template match='/'><xsl:value-of select=\"count(exsl:node-set(//a))\"/></xsl:template>";
  assert_eq!(run(body).expect("transforms"), "1");
}

#[test]
fn function_available_now_says_node_set_is_there() {
  let body = "<xsl:template match='/'><xsl:value-of select=\"function-available('exsl:node-set')\"/></xsl:template>";
  assert_eq!(run(body).expect("transforms"), "true");
}
