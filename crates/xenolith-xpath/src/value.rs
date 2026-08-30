//! The four types an XPath expression can yield, and the conversions between them.
//!
//! XPath 1.0 has no type declarations: an expression yields a node-set, a boolean, a number or a
//! string, and an operator converts what it is given to the type it needs (§3, §4). The
//! conversions are not symmetric — every type converts to a boolean and a string, but only a
//! node-set converts *from* nothing else — so they live here as methods, named after the
//! functions that expose them.

use std::fmt::Debug;
use std::hash::Hash;

use xenolith_xdm::Model;

/// The value an XPath expression yields.
///
/// A node-set is kept in document order and free of duplicates, which is what
/// [`string`](Value::string) relies on and what every operation here produces.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<N> {
  /// A set of nodes, in document order.
  NodeSet(Vec<N>),
  /// A boolean.
  Boolean(bool),
  /// A number, an IEEE 754 double.
  Number(f64),
  /// A string.
  String(String),
}

impl<N: Copy + Eq + Hash + Debug> Value<N> {
  /// The value as a boolean (the `boolean` function, §4.3).
  ///
  /// A node-set is true when it has a node, a number when it is neither zero nor `NaN`, and a
  /// string when it is not empty.
  #[must_use]
  pub fn boolean(&self) -> bool {
    match self {
      Value::NodeSet(nodes) => !nodes.is_empty(),
      Value::Boolean(value) => *value,
      Value::Number(value) => *value != 0.0 && !value.is_nan(),
      Value::String(value) => !value.is_empty(),
    }
  }

  /// The value as a number (the `number` function, §4.4).
  ///
  /// A boolean is 1 or 0; a string that does not spell a number is `NaN`; a node-set goes
  /// through its string-value.
  #[must_use]
  pub fn number<M: Model<Node = N>>(&self, model: &M) -> f64 {
    match self {
      Value::Number(value) => *value,
      Value::Boolean(true) => 1.0,
      Value::Boolean(false) => 0.0,
      Value::String(value) => string_to_number(value),
      Value::NodeSet(_) => string_to_number(&self.string(model)),
    }
  }

  /// The value as a string (the `string` function, §4.2).
  ///
  /// A node-set becomes the string-value of its first node in document order, or the empty
  /// string when it has none.
  #[must_use]
  pub fn string<M: Model<Node = N>>(&self, model: &M) -> String {
    match self {
      Value::String(value) => value.clone(),
      Value::Boolean(true) => "true".to_owned(),
      Value::Boolean(false) => "false".to_owned(),
      Value::Number(value) => number_to_string(*value),
      Value::NodeSet(nodes) => nodes
        .iter()
        .min_by(|a, b| model.document_order(**a, **b))
        .map_or_else(String::new, |node| model.string_value(*node)),
    }
  }
}

impl<N> Value<N> {
  /// The nodes, if this is a node-set — otherwise `None`.
  ///
  /// XPath converts freely between the other three types but never *to* a node-set, so this is
  /// the one result that has to be asked for rather than converted.
  #[must_use]
  pub fn nodes(&self) -> Option<&[N]> {
    match self {
      Value::NodeSet(nodes) => Some(nodes),
      _ => None,
    }
  }

  /// The nodes, taken out of the value, if this is a node-set.
  #[must_use]
  pub fn into_nodes(self) -> Option<Vec<N>> {
    match self {
      Value::NodeSet(nodes) => Some(nodes),
      _ => None,
    }
  }

  /// The name of the type, for an error message.
  pub(crate) const fn type_name(&self) -> &'static str {
    match self {
      Value::NodeSet(_) => "a node-set",
      Value::Boolean(_) => "a boolean",
      Value::Number(_) => "a number",
      Value::String(_) => "a string",
    }
  }
}

/// Reads a string as an XPath number: optional whitespace, an optional minus sign, digits with
/// an optional fraction, optional whitespace. Anything else is `NaN` — there is no exponent and
/// no leading plus.
#[must_use]
pub fn string_to_number(text: &str) -> f64 {
  let trimmed = text.trim();
  let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
  let (integer, fraction) = match digits.split_once('.') {
    Some((integer, fraction)) => (integer, Some(fraction)),
    None => (digits, None),
  };
  let all_digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
  let well_formed = match fraction {
    None => !integer.is_empty() && all_digits(integer),
    // `1.`, `.5` and `1.5` are all numbers; a lone `.` is not.
    Some(fraction) => !(integer.is_empty() && fraction.is_empty()) && all_digits(integer) && all_digits(fraction),
  };
  if !well_formed {
    return f64::NAN;
  }
  trimmed.parse().unwrap_or(f64::NAN)
}

/// Writes a number the way XPath does (§4.2): no exponent, no trailing zeros, and the special
/// values spelled out.
#[must_use]
pub fn number_to_string(value: f64) -> String {
  if value.is_nan() {
    return "NaN".to_owned();
  }
  if value.is_infinite() {
    return if value > 0.0 { "Infinity".to_owned() } else { "-Infinity".to_owned() };
  }
  // Both zeros are written "0"; Rust would write the negative one "-0".
  if value == 0.0 {
    return "0".to_owned();
  }
  // Rust's shortest round-trip form is already what XPath asks for: a plain decimal.
  format!("{value}")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_the_numbers_xpath_recognizes() {
    assert_eq!(string_to_number("42"), 42.0);
    assert_eq!(string_to_number("  -1.5  "), -1.5);
    assert_eq!(string_to_number(".5"), 0.5);
    assert_eq!(string_to_number("1."), 1.0);
    for not_a_number in ["", ".", "+1", "1e5", "abc", "1.2.3", "- 1"] {
      assert!(string_to_number(not_a_number).is_nan(), "{not_a_number:?} is not an XPath number");
    }
  }

  #[test]
  fn writes_numbers_without_an_exponent() {
    assert_eq!(number_to_string(1.0), "1");
    assert_eq!(number_to_string(-1.5), "-1.5");
    assert_eq!(number_to_string(0.0), "0");
    assert_eq!(number_to_string(-0.0), "0", "both zeros are written the same");
    assert_eq!(number_to_string(f64::NAN), "NaN");
    assert_eq!(number_to_string(f64::INFINITY), "Infinity");
    assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
  }
}
