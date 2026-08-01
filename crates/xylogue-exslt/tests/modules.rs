//! The EXSLT modules, run through the XSLT engine the way a stylesheet reaches them.

use xylogue_dom::build;
use xylogue_xdm::DomModel;
use xylogue_xpath::Functions;
use xylogue_xslt::{Stylesheet, Transform};

/// The namespace declarations a stylesheet needs to reach every module.
const PREFIXES: &str = "xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
                        xmlns:exsl=\"http://exslt.org/common\" \
                        xmlns:math=\"http://exslt.org/math\" \
                        xmlns:set=\"http://exslt.org/set\"";

/// Transforms `xml` with a stylesheet whose one template is `body`.
fn run(body: &str, xml: &str) -> String {
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let functions = xylogue_exslt::register(Functions::new());
  Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect("transforms").text()
}

/// Evaluates one expression over `xml`.
fn value_of(expression: &str, xml: &str) -> String {
  run(&format!("<xsl:template match='/'><xsl:value-of select=\"{expression}\"/></xsl:template>"), xml)
}

/// The message a transformation fails with. Only the math tests ask for one.
#[cfg(feature = "math")]
fn error(expression: &str, xml: &str) -> String {
  let body = format!("<xsl:template match='/'><xsl:value-of select=\"{expression}\"/></xsl:template>");
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let functions = xylogue_exslt::register(Functions::new());
  Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect_err("fails").message().to_owned()
}

/// A few numbers to work over.
const NUMBERS: &str = "<r><n>3</n><n>11</n><n>7</n><n>11</n></r>";

// --- math (http://exslt.org/math) ----------------------------------------------------------------

#[cfg(feature = "math")]
mod math {
  use super::*;

  #[test]
  fn the_least_and_the_greatest_of_a_node_set() {
    assert_eq!(value_of("math:min(//n)", NUMBERS), "3");
    assert_eq!(value_of("math:max(//n)", NUMBERS), "11");
  }

  #[test]
  fn an_empty_node_set_has_no_extreme() {
    assert_eq!(value_of("math:min(//nothing)", NUMBERS), "NaN");
    assert_eq!(value_of("string(math:max(//nothing))", NUMBERS), "NaN");
  }

  #[test]
  fn a_node_that_is_not_a_number_makes_the_whole_answer_nan() {
    // Which is what the arithmetic over it would have given.
    assert_eq!(value_of("math:max(//n)", "<r><n>1</n><n>oops</n></r>"), "NaN");
  }

  #[test]
  fn highest_and_lowest_answer_with_nodes() {
    // Every node carrying the extreme, not just the first — there are two elevens here.
    assert_eq!(value_of("count(math:highest(//n))", NUMBERS), "2");
    assert_eq!(value_of("count(math:lowest(//n))", NUMBERS), "1");
    assert_eq!(value_of("math:lowest(//n)", NUMBERS), "3");
  }

  #[test]
  fn the_arithmetic_xpath_leaves_out() {
    assert_eq!(value_of("math:abs(-3)", NUMBERS), "3");
    assert_eq!(value_of("math:sqrt(16)", NUMBERS), "4");
    assert_eq!(value_of("math:power(2, 10)", NUMBERS), "1024");
    assert_eq!(value_of("math:exp(0)", NUMBERS), "1");
    assert_eq!(value_of("math:log(1)", NUMBERS), "0");
    assert_eq!(value_of("math:sin(0)", NUMBERS), "0");
    assert_eq!(value_of("math:cos(0)", NUMBERS), "1");
  }

  #[test]
  fn a_constant_to_the_precision_asked_for() {
    assert_eq!(value_of("math:constant('PI', 2)", NUMBERS), "3.14");
    assert_eq!(value_of("math:constant('E', 3)", NUMBERS), "2.718");
    assert_eq!(value_of("math:constant('TAU', 3)", NUMBERS), "NaN", "a constant nobody named");
  }

  #[test]
  fn a_string_where_a_node_set_was_meant_is_reported() {
    let message = error("math:max('nonsense')", NUMBERS);
    assert!(message.contains("node-set"), "{message}");
  }

  #[test]
  fn the_wrong_number_of_arguments_is_reported() {
    assert!(error("math:abs(1, 2)", NUMBERS).contains("math:abs"));
  }
}

// --- set (http://exslt.org/set) ------------------------------------------------------------------

#[cfg(feature = "sets")]
mod sets {
  use super::*;

  /// Two overlapping groups, so that difference and intersection have something to say.
  const GROUPS: &str = "<r><a>1</a><a>2</a><b>2</b><b>3</b></r>";

  #[test]
  fn difference_and_intersection_compare_by_identity() {
    // Not by what the nodes say: `a` saying 2 and `b` saying 2 are different nodes.
    assert_eq!(value_of("count(set:difference(//a, //b))", GROUPS), "2");
    assert_eq!(value_of("count(set:intersection(//a, //b))", GROUPS), "0");

    // The same nodes on both sides, and now they do meet.
    assert_eq!(value_of("count(set:intersection(//a, //a))", GROUPS), "2");
    assert_eq!(value_of("count(set:difference(//a, //a))", GROUPS), "0");
  }

  #[test]
  fn distinct_compares_by_what_the_nodes_say() {
    // Which is what makes it useful for grouping: three values among four nodes.
    assert_eq!(value_of("count(set:distinct(//a | //b))", GROUPS), "3");
    assert_eq!(value_of("set:distinct(//a | //b)", GROUPS), "1", "the first in document order is kept");
  }

  #[test]
  fn has_same_node_asks_whether_the_two_meet_at_all() {
    assert_eq!(value_of("set:has-same-node(//a, //b)", GROUPS), "false");
    assert_eq!(value_of("set:has-same-node(//a, //a)", GROUPS), "true");
  }

  #[test]
  fn leading_and_trailing_cut_at_the_first_node_of_the_second_set() {
    let xml = "<r><i>1</i><i>2</i><mark/><i>3</i><i>4</i></r>";
    assert_eq!(value_of("count(set:leading(//i, //mark))", xml), "2");
    assert_eq!(value_of("count(set:trailing(//i, //mark))", xml), "2");
    assert_eq!(value_of("set:trailing(//i, //mark)", xml), "3");
  }

  #[test]
  fn an_empty_second_set_marks_nothing() {
    let xml = "<r><i>1</i><i>2</i></r>";
    assert_eq!(value_of("count(set:leading(//i, //nothing))", xml), "0");
    assert_eq!(value_of("count(set:trailing(//i, //nothing))", xml), "0");
  }

  #[test]
  fn a_result_is_a_node_set_and_so_is_in_document_order() {
    let xml = "<r><i>1</i><i>2</i><i>3</i></r>";
    // The union is written back to front; what comes out is not.
    assert_eq!(value_of("set:distinct(//i[3] | //i[1])", xml), "1");
  }
}

// --- common (http://exslt.org/common) ------------------------------------------------------------

#[cfg(feature = "common")]
mod common {
  use super::*;

  #[test]
  fn object_type_names_the_four_xpath_types() {
    assert_eq!(value_of("exsl:object-type(//n)", NUMBERS), "node-set");
    assert_eq!(value_of("exsl:object-type('a')", NUMBERS), "string");
    assert_eq!(value_of("exsl:object-type(1)", NUMBERS), "number");
    assert_eq!(value_of("exsl:object-type(true())", NUMBERS), "boolean");
  }

  #[test]
  fn node_set_is_the_identity_on_something_that_is_already_a_node_set() {
    // Converting a result tree fragment is `tests/node_set.rs`; this is the other half, which
    // needs no engine help at all.
    assert_eq!(value_of("count(exsl:node-set(//n))", NUMBERS), "4");
    assert_eq!(value_of("function-available('exsl:node-set')", NUMBERS), "true");
  }
}

// --- What the build has --------------------------------------------------------------------------

#[test]
fn function_available_agrees_with_the_features_this_build_was_made_with() {
  // Nothing keeps these in step by hand: `function-available()` asks the registry, and the
  // registry holds what `register` put in it, which is what the features decided.
  assert_eq!(value_of("function-available('math:max')", NUMBERS), cfg!(feature = "math").to_string());
  assert_eq!(value_of("function-available('set:distinct')", NUMBERS), cfg!(feature = "sets").to_string());
  assert_eq!(value_of("function-available('exsl:object-type')", NUMBERS), cfg!(feature = "common").to_string());
}

#[test]
fn the_modules_listed_are_the_modules_built() {
  let modules = xylogue_exslt::modules();
  assert_eq!(modules.contains(&"http://exslt.org/math"), cfg!(feature = "math"));
  assert_eq!(modules.contains(&"http://exslt.org/set"), cfg!(feature = "sets"));
  assert_eq!(modules.contains(&"http://exslt.org/common"), cfg!(feature = "common"));
}

#[test]
fn a_function_of_a_module_this_build_lacks_is_reported_rather_than_guessed() {
  // `strings` is not built at all yet, so a stylesheet calling it is told so.
  let source = format!(
    "<xsl:stylesheet version=\"1.0\" {PREFIXES} xmlns:str=\"http://exslt.org/strings\">\
       <xsl:template match='/'><xsl:value-of select=\"str:tokenize('a b')\"/></xsl:template>\
     </xsl:stylesheet>"
  );
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(NUMBERS.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let functions = xylogue_exslt::register(Functions::new());
  let error = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect_err("fails");
  assert!(error.message().contains("tokenize"), "{}", error.message());
}
