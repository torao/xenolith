//! `format-number()` and the `xsl:decimal-format` it reads its symbols from (XSLT 1.0 §12.3).
//!
//! §12.3 does not define a pattern language of its own. It points at one: "the format pattern
//! string is in the syntax specified by the JDK 1.1 `DecimalFormat` class". So this is a subset
//! of that class, and where a question is not settled by XSLT the answer is taken from what Java
//! does — which is the comparison this library is meant to stand up to.
//!
//! A pattern is a prefix, a run of digit positions, and a suffix, optionally twice with a
//! `pattern-separator` between: the second half says how a negative number is written. Which
//! characters play those parts is not fixed either — `xsl:decimal-format` renames every one of
//! them, so a pattern written for a European locale can use `,` for the decimal point and `.`
//! for grouping.
//!
//! # Specifications
//!
//! - [`format-number()` and `xsl:decimal-format` (§12.3)]
//!
//! [`format-number()` and `xsl:decimal-format` (§12.3)]: https://www.w3.org/TR/1999/REC-xslt-19991116#format-number

use std::collections::HashMap;

/// The characters one `xsl:decimal-format` gives to patterns and to output (§12.3).
///
/// The defaults are the ones §12.3 lists, which are also `DecimalFormatSymbols`' for the root
/// locale — so a stylesheet that declares no `xsl:decimal-format` gets what a Java caller would.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Symbols {
  pub(crate) decimal_separator: char,
  pub(crate) grouping_separator: char,
  pub(crate) infinity: String,
  pub(crate) minus_sign: char,
  pub(crate) nan: String,
  pub(crate) percent: char,
  pub(crate) per_mille: char,
  pub(crate) zero_digit: char,
  pub(crate) digit: char,
  pub(crate) pattern_separator: char,
}

impl Default for Symbols {
  fn default() -> Self {
    Self {
      decimal_separator: '.',
      grouping_separator: ',',
      infinity: "Infinity".to_owned(),
      minus_sign: '-',
      nan: "NaN".to_owned(),
      percent: '%',
      per_mille: '\u{2030}',
      zero_digit: '0',
      digit: '#',
      pattern_separator: ';',
    }
  }
}

/// Every `xsl:decimal-format` a stylesheet declares, by name.
///
/// The one with no name is the default, which `format-number()` uses when it is given only two
/// arguments.
pub(crate) type Formats = HashMap<Option<(Option<String>, String)>, Symbols>;

/// One half of a pattern: how a positive, or a negative, number is written.
#[derive(Clone, Debug)]
struct Subpattern {
  prefix: String,
  suffix: String,
  /// Fewest integer digits, from the count of zero-digits before the decimal separator.
  minimum_integer: usize,
  /// Fewest fraction digits, from the count of zero-digits after it.
  minimum_fraction: usize,
  /// Most fraction digits, from every digit position after it.
  maximum_fraction: usize,
  /// How many digits a group holds, from where the last grouping separator sits.
  grouping: Option<usize>,
  /// 100 for a percent, 1000 for a per-mille, 1 otherwise.
  multiplier: f64,
}

/// A parsed `format-number()` pattern.
#[derive(Clone, Debug)]
pub(crate) struct Pattern {
  positive: Subpattern,
  /// The half after the pattern separator, if the pattern has one.
  negative: Option<Subpattern>,
}

impl Pattern {
  /// Reads a pattern against a set of symbols.
  ///
  /// # Errors
  ///
  /// If the pattern holds more than one `pattern-separator`, which would give a third half that
  /// `DecimalFormat` has no meaning for.
  pub(crate) fn parse(pattern: &str, symbols: &Symbols) -> Result<Self, String> {
    let halves = split_halves(pattern, symbols);
    match halves.len() {
      1 => Ok(Self { positive: Subpattern::parse(&halves[0], symbols), negative: None }),
      2 => Ok(Self {
        positive: Subpattern::parse(&halves[0], symbols),
        negative: Some(Subpattern::parse(&halves[1], symbols)),
      }),
      _ => Err(format!(
        "the format pattern {pattern:?} has more than one {:?}, so it gives more than a positive \
         and a negative form",
        symbols.pattern_separator
      )),
    }
  }

  /// Writes a number.
  pub(crate) fn format(&self, number: f64, symbols: &Symbols) -> String {
    if number.is_nan() {
      // §12.3 gives NaN a symbol of its own, and it stands alone: there are no digits for a
      // prefix and suffix to be attached to.
      return symbols.nan.clone();
    }
    let negative = number.is_sign_negative();
    // With no negative half, §12.3 follows DecimalFormat: the positive form with the minus sign
    // in front of it.
    let (half, sign) = match (&self.negative, negative) {
      (Some(negative_half), true) => (negative_half, String::new()),
      (None, true) => (&self.positive, symbols.minus_sign.to_string()),
      (_, false) => (&self.positive, String::new()),
    };

    let magnitude = number.abs() * half.multiplier;
    let digits = if magnitude.is_infinite() { symbols.infinity.clone() } else { half.write_digits(magnitude, symbols) };
    format!("{}{}{}{}", half.prefix, sign, digits, half.suffix)
  }
}

impl Subpattern {
  /// Reads one half of a pattern.
  fn parse(half: &str, symbols: &Symbols) -> Self {
    let is_body = |character: char| {
      character == symbols.zero_digit
        || character == symbols.digit
        || character == symbols.decimal_separator
        || character == symbols.grouping_separator
    };

    // A quoted `#` is a literal `#`, not a digit position, so the scan has to know where the
    // quotes are before it can say where the digits begin.
    let mut prefix = String::new();
    let mut body: Vec<char> = Vec::new();
    let mut suffix = String::new();
    let mut quoted = false;
    let mut seen_body = false;
    for character in half.chars() {
      if character == '\'' {
        quoted = !quoted;
      }
      if !quoted && character != '\'' && is_body(character) {
        // A digit position after the suffix has begun would be a second run; DecimalFormat has
        // no meaning for one, so it stays with the suffix.
        if suffix.is_empty() {
          body.push(character);
          seen_body = true;
          continue;
        }
      }
      if seen_body {
        suffix.push(character);
      } else {
        prefix.push(character);
      }
    }

    // A percent or per-mille scales the number, which is what makes `0%` write 0.25 as 25%. It
    // can only be in the prefix or the suffix, being no kind of digit position — and a quoted
    // one is only a character, so quoting is what decides.
    let multiplier = if contains_unquoted(half, symbols.percent) {
      100.0
    } else if contains_unquoted(half, symbols.per_mille) {
      1000.0
    } else {
      1.0
    };

    let point = body.iter().position(|character| *character == symbols.decimal_separator);
    let (integer, fraction) = match point {
      Some(point) => (&body[..point], &body[point + 1..]),
      None => (&body[..], &[][..]),
    };

    // `DecimalFormat` carries one grouping size, not one per interval, and takes the interval
    // nearest the decimal point: `#,##0` groups by three, and so does `#,##,##0`.
    let grouping = integer
      .iter()
      .rposition(|character| *character == symbols.grouping_separator)
      .map(|last| integer.len() - last - 1)
      .filter(|size| *size > 0);

    Self {
      prefix: unquote(&prefix),
      suffix: unquote(&suffix),
      minimum_integer: integer.iter().filter(|character| **character == symbols.zero_digit).count(),
      minimum_fraction: fraction.iter().filter(|character| **character == symbols.zero_digit).count(),
      maximum_fraction: fraction
        .iter()
        .filter(|character| **character == symbols.zero_digit || **character == symbols.digit)
        .count(),
      grouping,
      multiplier,
    }
  }

  /// Writes the digits of a non-negative, finite number.
  fn write_digits(&self, magnitude: f64, symbols: &Symbols) -> String {
    // Rust rounds a formatted float to nearest with ties to even, which is `DecimalFormat`'s own
    // default rounding — so this agrees with Java rather than merely being close to it.
    let rendered = format!("{magnitude:.*}", self.maximum_fraction);
    let (integer, fraction) = match rendered.split_once('.') {
      Some((integer, fraction)) => (integer.to_owned(), fraction.to_owned()),
      None => (rendered, String::new()),
    };

    // A `#`-only integer part writes nothing for a zero, which is what makes `#.##` give `.5`.
    let integer = integer.trim_start_matches('0');
    let mut integer = integer.to_owned();
    while integer.chars().count() < self.minimum_integer {
      integer.insert(0, '0');
    }

    // Trailing zeros beyond the fewest asked for are not written; `#` positions are optional.
    let mut fraction = fraction;
    while fraction.chars().count() > self.minimum_fraction && fraction.ends_with('0') {
      fraction.pop();
    }

    let mut written = translate(&group(&integer, self.grouping, symbols.grouping_separator), symbols.zero_digit);
    if !fraction.is_empty() {
      written.push(symbols.decimal_separator);
      written.push_str(&translate(&fraction, symbols.zero_digit));
    }
    written
  }
}

/// Whether a character appears outside quotes.
fn contains_unquoted(text: &str, wanted: char) -> bool {
  let mut quoted = false;
  for character in text.chars() {
    if character == '\'' {
      quoted = !quoted;
      continue;
    }
    if !quoted && character == wanted {
      return true;
    }
  }
  false
}

/// Splits a pattern at its `pattern-separator`, respecting quoted literals.
fn split_halves(pattern: &str, symbols: &Symbols) -> Vec<String> {
  let mut halves = vec![String::new()];
  let mut quoted = false;
  for character in pattern.chars() {
    if character == '\'' {
      quoted = !quoted;
    }
    if character == symbols.pattern_separator && !quoted {
      halves.push(String::new());
      continue;
    }
    halves.last_mut().expect("there is always a half being built").push(character);
  }
  halves
}

/// Removes the quotes `DecimalFormat` uses to put a pattern character into a prefix or suffix.
///
/// `''` is one apostrophe; anything else between quotes is taken as it stands.
fn unquote(text: &str) -> String {
  let mut written = String::new();
  let mut characters = text.chars().peekable();
  while let Some(character) = characters.next() {
    if character != '\'' {
      written.push(character);
      continue;
    }
    if characters.peek() == Some(&'\'') {
      characters.next();
      written.push('\'');
    }
  }
  written
}

/// Inserts a grouping separator every `size` digits, counting from the right.
fn group(digits: &str, size: Option<usize>, separator: char) -> String {
  let Some(size) = size else { return digits.to_owned() };
  let characters: Vec<char> = digits.chars().collect();
  let mut grouped = String::new();
  for (index, character) in characters.iter().enumerate() {
    if index > 0 && (characters.len() - index) % size == 0 {
      grouped.push(separator);
    }
    grouped.push(*character);
  }
  grouped
}

/// Rewrites ASCII digits in the digit set `zero-digit` begins.
fn translate(digits: &str, zero_digit: char) -> String {
  if zero_digit == '0' {
    return digits.to_owned();
  }
  let base = zero_digit as u32;
  digits
    .chars()
    .map(|character| match character.to_digit(10) {
      Some(digit) => char::from_u32(base + digit).unwrap_or(character),
      None => character,
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(pattern: &str, number: f64) -> String {
    let symbols = Symbols::default();
    Pattern::parse(pattern, &symbols).expect("a pattern").format(number, &symbols)
  }

  #[test]
  fn zero_digits_are_always_written_and_hashes_only_when_needed() {
    assert_eq!(write("0", 5.0), "5");
    assert_eq!(write("000", 5.0), "005");
    assert_eq!(write("#", 5.0), "5");
    assert_eq!(write("#", 0.0), "", "a lone # writes nothing for a zero");
    assert_eq!(write("0", 0.0), "0");
  }

  #[test]
  fn the_fraction_part_says_how_many_places_are_kept() {
    assert_eq!(write("0.00", 1.5), "1.50");
    assert_eq!(write("0.##", 1.5), "1.5");
    assert_eq!(write("0.##", 1.0), "1");
    assert_eq!(write("0.0#", 1.0), "1.0");
    assert_eq!(write("0.##", 1.234), "1.23");
    assert_eq!(write("0", 1.6), "2", "no fraction places at all, so it rounds to an integer");
  }

  #[test]
  fn rounding_goes_to_even_on_a_tie_as_java_does() {
    // DecimalFormat's default is HALF_EVEN, and §12.3 sends the reader to DecimalFormat.
    assert_eq!(write("0", 0.5), "0");
    assert_eq!(write("0", 1.5), "2");
    assert_eq!(write("0", 2.5), "2");
    assert_eq!(write("0.0", 0.25), "0.2");
    assert_eq!(write("0.0", 0.35), "0.3", "0.35 is just below the tie in binary");
  }

  #[test]
  fn grouping_takes_its_size_from_where_the_last_separator_sits() {
    assert_eq!(write("#,##0", 1234567.0), "1,234,567");
    assert_eq!(write("#,##0", 100.0), "100");
    assert_eq!(write("#,#0", 1234567.0), "1,23,45,67", "the last interval is two, so every group is");
    // DecimalFormat carries one grouping size, not one per interval, and it takes the interval
    // nearest the decimal point — so this groups by three throughout rather than by two then
    // three.
    assert_eq!(write("#,##,##0", 1234567.0), "1,234,567");
    assert_eq!(write("0", 1234567.0), "1234567", "no separator, no grouping");
  }

  #[test]
  fn a_prefix_and_suffix_are_carried_through() {
    assert_eq!(write("$#,##0.00", 1234.5), "$1,234.50");
    assert_eq!(write("0 apples", 3.0), "3 apples");
  }

  #[test]
  fn a_percent_scales_the_number() {
    assert_eq!(write("0%", 0.25), "25%");
    assert_eq!(write("0.0%", 0.125), "12.5%");
    assert_eq!(write("0\u{2030}", 0.25), "250\u{2030}", "per-mille scales by a thousand");
  }

  #[test]
  fn the_second_half_says_how_a_negative_number_is_written() {
    assert_eq!(write("0.00;(0.00)", -1.5), "(1.50)");
    assert_eq!(write("0.00;(0.00)", 1.5), "1.50");
    // With no second half, the minus sign goes in front of the positive form.
    assert_eq!(write("0.00", -1.5), "-1.50");
    assert_eq!(write("$0.00", -1.5), "$-1.50", "which is where DecimalFormat puts it");
  }

  #[test]
  fn nan_and_infinity_have_symbols_of_their_own() {
    assert_eq!(write("0.00", f64::NAN), "NaN");
    assert_eq!(write("0.00", f64::INFINITY), "Infinity");
    assert_eq!(write("0.00", f64::NEG_INFINITY), "-Infinity");
    assert_eq!(write("$0.00", f64::INFINITY), "$Infinity", "the prefix still applies");
  }

  #[test]
  fn a_quoted_character_in_a_prefix_is_a_literal() {
    assert_eq!(write("'#'0", 5.0), "#5");
    assert_eq!(write("0''", 5.0), "5'");
  }

  #[test]
  fn the_symbols_can_all_be_renamed() {
    let symbols =
      Symbols { decimal_separator: ',', grouping_separator: '.', minus_sign: '\u{2212}', ..Symbols::default() };
    let pattern = Pattern::parse("#.##0,00", &symbols).expect("a pattern");
    assert_eq!(pattern.format(1234.5, &symbols), "1.234,50");
    assert_eq!(pattern.format(-1234.5, &symbols), "\u{2212}1.234,50");
  }

  #[test]
  fn another_digit_set_can_be_used() {
    // zero-digit fixes the start of a run of ten digits, so the whole set moves with it.
    let symbols = Symbols { zero_digit: '\u{0660}', ..Symbols::default() };
    let pattern = Pattern::parse("\u{0660}\u{0660}\u{0660}", &symbols).expect("a pattern");
    assert_eq!(pattern.format(123.0, &symbols), "\u{661}\u{662}\u{663}");
  }

  #[test]
  fn more_than_two_halves_is_refused() {
    let symbols = Symbols::default();
    let error = Pattern::parse("0;0;0", &symbols).expect_err("three halves");
    assert!(error.contains("more than"), "{error}");
  }
}
