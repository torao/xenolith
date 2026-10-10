//! The four value types an XPath expression can produce and the conversions between them.
//!
//! XPath 1.0 expressions produce a node-set, boolean, number, or string. Operators and functions convert these values
//! to the types they require (§3, §4). This module implements conversions to boolean, number, and string; XPath does
//! not define a conversion from a scalar value to a node-set. The conversion methods use the names of the XPath
//! functions that expose the corresponding conversions.

#[cfg(test)]
mod test;

use crate::xpath::model::Model;

/// A value produced by an XPath expression, with node references from a [`Model`].
///
/// The evaluator keeps node-sets in document order and removes duplicate nodes. The [`string`](Object::string)
/// conversion selects the first node in document order, even if a caller supplies nodes in another order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Object<N> {
  /// A duplicate-free set of nodes in document order.
  NodeSet(Vec<N>),
  /// A boolean value.
  Boolean(bool),
  /// A number represented as an IEEE 754 binary64 value.
  Number(f64),
  /// A string value.
  String(String),
}

impl<N: Copy> Object<N> {
  /// Converts the value to a boolean using the XPath `boolean()` rules (§4.3).
  ///
  /// A node-set is true when it contains at least one node. A number is true when it is nonzero and not `NaN`. A
  /// string is true when it is nonempty. A boolean remains unchanged.
  pub(crate) fn boolean(&self) -> bool {
    match self {
      Object::NodeSet(nodes) => !nodes.is_empty(),
      Object::Boolean(value) => *value,
      Object::Number(value) => *value != 0.0 && !value.is_nan(),
      Object::String(value) => !value.is_empty(),
    }
  }

  /// Converts the value to a number using the XPath `number()` rules (§4.4).
  ///
  /// `true` converts to `1`, and `false` converts to `0`. A string that does not match XPath's numeric syntax
  /// converts to `NaN`. A node-set first converts to the string-value of its first node in document order, then to a
  /// number.
  pub(crate) fn number<M: Model<Node = N>>(&self, model: &M) -> f64 {
    match self {
      Object::Number(value) => *value,
      Object::Boolean(true) => 1.0,
      Object::Boolean(false) => 0.0,
      Object::String(value) => string_to_number(value),
      Object::NodeSet(_) => string_to_number(&self.string(model)),
    }
  }

  /// Converts the value to a string using the XPath `string()` rules (§4.2).
  ///
  /// A node-set converts to the string-value of its first node in document order. An empty node-set converts to the
  /// empty string.
  pub(crate) fn string<M: Model<Node = N>>(&self, model: &M) -> String {
    match self {
      Object::String(value) => value.clone(),
      Object::Boolean(true) => "true".to_owned(),
      Object::Boolean(false) => "false".to_owned(),
      Object::Number(value) => number_to_string(*value),
      Object::NodeSet(nodes) => nodes
        .iter()
        .min_by(|a, b| model.document_order(**a, **b))
        .map_or_else(String::new, |node| model.string_value(*node)),
    }
  }
}

impl<N> Object<N> {
  /// Returns the type name used in error messages.
  pub(crate) const fn type_name(&self) -> &'static str {
    match self {
      Object::NodeSet(_) => "a node-set",
      Object::Boolean(_) => "a boolean",
      Object::Number(_) => "a number",
      Object::String(_) => "a string",
    }
  }
}

/// Converts `text` to an XPath number.
///
/// The accepted syntax has optional surrounding XML whitespace (space, tab, carriage return and line feed; not other
/// Unicode spaces), an optional leading minus, and decimal digits with an optional decimal point. At least one digit is
/// required; forms such as `1.`, `.5`, and `1.5` are accepted. A leading plus sign, exponent notation, or any other
/// invalid form produces `NaN`.
pub(crate) fn string_to_number(text: &str) -> f64 {
  let trimmed = text.trim_matches(crate::chars::is_whitespace);
  let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
  let (integer, fraction) = match digits.split_once('.') {
    Some((integer, fraction)) => (integer, Some(fraction)),
    None => (digits, None),
  };
  let all_digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
  let well_formed = match fraction {
    None => !integer.is_empty() && all_digits(integer),
    // XPath permits a decimal point with digits on either side, but requires at least one digit overall.
    Some(fraction) => !(integer.is_empty() && fraction.is_empty()) && all_digits(integer) && all_digits(fraction),
  };
  if !well_formed {
    return f64::NAN;
  }
  trimmed.parse().unwrap_or(f64::NAN)
}

/// Formats a number as an XPath string-value (§4.2).
///
/// This formatter spells out `NaN` and infinities, formats both positive and negative zero as `0`, and writes finite
/// numbers in decimal notation without exponent notation or unnecessary fractional zeros.
pub(crate) fn number_to_string(value: f64) -> String {
  if value.is_nan() {
    return "NaN".to_owned();
  }
  if value.is_infinite() {
    return if value > 0.0 { "Infinity".to_owned() } else { "-Infinity".to_owned() };
  }
  // XPath formats positive and negative zero identically as `0`.
  if value == 0.0 {
    return "0".to_owned();
  }
  // Rust's shortest round-trip representation uses decimal notation, as required here.
  format!("{value}")
}
