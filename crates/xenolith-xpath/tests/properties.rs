//! Properties that must hold for every input, checked over generated ones.
//!
//! These are the checks a hand-written case cannot make: that the lexer never panics on
//! arbitrary text, that number formatting and number parsing are inverses, and that printing an
//! expression tree produces something that parses back to the same tree. A fixed test asserts
//! what its author thought to try; a property asserts a law.

use proptest::prelude::*;
use xenolith_xpath::{number_to_string, parse, string_to_number};

proptest! {
  /// Reading an expression is a total function: any text is either a tree or an error, never a
  /// panic. The lexer slices by byte offset, so this is mostly a check that it never cuts a
  /// character in half.
  #[test]
  fn parsing_never_panics(text in ".{0,80}") {
    let _ = parse(&text);
  }

  /// The same, over text made of the characters that actually mean something in XPath, so the
  /// generator spends its time inside the grammar rather than outside it.
  #[test]
  fn parsing_xpath_shaped_text_never_panics(text in "[a-z:*/@\\[\\]().,'\"$0-9 =<>!|+-]{0,60}") {
    let _ = parse(&text);
  }

  /// Reading a number is total too, and never yields anything but a number or `NaN`.
  #[test]
  fn reading_a_number_never_panics(text in ".{0,40}") {
    let value = string_to_number(&text);
    prop_assert!(value.is_nan() || value.is_finite() || value.is_infinite());
  }

  /// Writing a finite number and reading it back gives the same number: XPath's decimal form is
  /// lossless.
  #[test]
  fn a_finite_number_survives_being_written_and_read(value in proptest::num::f64::NORMAL) {
    let written = number_to_string(value);
    let read = string_to_number(&written);
    prop_assert_eq!(read, value, "wrote {:?}", written);
  }

  /// Printing a tree and parsing it again gives the same tree — so the printed form is not just
  /// readable, it is the same expression.
  #[test]
  fn printing_a_tree_and_parsing_it_again_is_stable(text in "[a-z]{1,3}(/[a-z]{1,3}){0,3}") {
    let once = parse(&text).expect("a path of names parses").to_string();
    let twice = parse(&once).expect("the printed form parses").to_string();
    prop_assert_eq!(once, twice);
  }
}

/// Every expression the crate's other tests use, printed and parsed again.
///
/// The property above generates only simple paths; this covers the shapes a generator would
/// take a long time to reach — predicates, operators, functions, every axis.
#[test]
fn the_printed_form_of_any_expression_parses_back_to_itself() {
  const EXPRESSIONS: &[&str] = &[
    "a",
    "/",
    "/a/b",
    ".",
    "..",
    "@x",
    "//a",
    "a//b",
    "../@x",
    "*",
    "p:a",
    "p:*",
    "node()",
    "text()",
    "comment()",
    "processing-instruction()",
    "processing-instruction('php')",
    "a[1]",
    "a[@x][b]",
    "a[b/c='x']",
    "1 + 2 * 3",
    "1 - 2 - 3",
    "a or b and c",
    "1 = 2 or 3 != 4",
    "1 < 2 = 3",
    "1 div 2 mod 3",
    "-1 + 2",
    "a | b | c",
    "(1 + 2) * 3",
    "'text'",
    "\"it's\"",
    "42",
    "1.5",
    ".5",
    "$x",
    "$p:x",
    "true()",
    "substring('abc', 1, 2)",
    "p:f(a)",
    "(a | b)[1]",
    "$x/a",
    "id('a')//b",
    "ancestor-or-self::a",
    "preceding-sibling::*[2]",
    "descendant::text()",
    "namespace::*",
    "count(//a[@b = 'c'])",
    "//a[position() = last() - 1]",
    "concat('a', 'b', 'c')",
    "-(2 + 3)",
  ];

  for expression in EXPRESSIONS {
    let once = parse(expression).unwrap_or_else(|e| panic!("{expression:?} should parse: {e}")).to_string();
    let twice = parse(&once)
      .unwrap_or_else(|e| panic!("the printed form of {expression:?} should parse: {once:?}: {e}"))
      .to_string();
    assert_eq!(once, twice, "printing {expression:?} is not stable");
  }
}

/// XPath's number syntax has no room for the infinities, so writing one and reading it back does
/// not return it. That is the specification's own asymmetry, not an oversight here: `Infinity` is
/// what `string()` produces, while `number('Infinity')` is `NaN`.
#[test]
fn the_infinities_are_written_but_cannot_be_read_back() {
  assert_eq!(number_to_string(f64::INFINITY), "Infinity");
  assert!(string_to_number("Infinity").is_nan());
  assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
  assert!(string_to_number("-Infinity").is_nan());
  // NaN behaves the same way, and is at least still NaN.
  assert_eq!(number_to_string(f64::NAN), "NaN");
  assert!(string_to_number("NaN").is_nan());
}
