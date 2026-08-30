//! Writing a list of numbers the way `xsl:number`'s `format` asks (XSLT 1.0 §7.7.1).
//!
//! A format string alternates *tokens* — runs of alphanumeric characters, each saying how one
//! number is written — with *separators*, the runs between them. `1.1.1` numbers three levels
//! with dots between; `(a)` puts one number in brackets. A format with more numbers than tokens
//! repeats its last token and the separator before it, which is what lets `1.` number a tree of
//! any depth.
//!
//! # Specifications
//!
//! - [Number to string conversion attributes (§7.7.1)] — the format string, `letter-value`,
//!   `grouping-separator` and `grouping-size`
//!
//! Where §7.7.1 leaves something open it is named as such below, and `tests/behaviour.rs` in the
//! `xenolith` crate prints what this build does.
//!
//! [Number to string conversion attributes (§7.7.1)]: https://www.w3.org/TR/1999/REC-xslt-19991116#convert

/// How a token says its numbers are written.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sequence {
  /// `1`, `01`, `001` — decimal, padded to the token's width.
  Decimal { width: usize },
  /// `a` or `A` — a, b, … z, aa, ab.
  Alphabetic { upper: bool },
  /// `i` or `I` — Roman numerals.
  Roman { upper: bool },
}

/// A parsed `format` string.
#[derive(Clone, Debug)]
pub(crate) struct Format {
  /// What comes before the first number.
  prefix: String,
  /// Each number's token, with the separator that precedes it (empty for the first).
  parts: Vec<(String, Sequence)>,
  /// What comes after the last number.
  suffix: String,
}

/// How digits are grouped, from `grouping-separator` and `grouping-size`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grouping<'a> {
  pub(crate) separator: Option<&'a str>,
  pub(crate) size: Option<usize>,
}

impl Format {
  /// Reads a format string.
  ///
  /// A format with no token at all still has to number something, so it falls back to `1` — §7.7
  /// makes `1` the default format, and a format that names no sequence is no more informative
  /// than none at all.
  pub(crate) fn parse(format: &str, letter_value: LetterValue) -> Self {
    let characters: Vec<char> = format.chars().collect();
    let mut index = 0;
    let mut prefix = String::new();
    // A leading separator belongs to no number; it is written once, before them all.
    while index < characters.len() && !is_token_character(characters[index]) {
      prefix.push(characters[index]);
      index += 1;
    }

    let mut parts: Vec<(String, Sequence)> = Vec::new();
    let mut separator = String::new();
    while index < characters.len() {
      let mut token = String::new();
      while index < characters.len() && is_token_character(characters[index]) {
        token.push(characters[index]);
        index += 1;
      }
      parts.push((std::mem::take(&mut separator), sequence_of(&token, letter_value)));
      while index < characters.len() && !is_token_character(characters[index]) {
        separator.push(characters[index]);
        index += 1;
      }
    }
    // Whatever separator is left over came after the last token, so it is the suffix.
    let suffix = separator;

    if parts.is_empty() {
      parts.push((String::new(), Sequence::Decimal { width: 1 }));
    }
    Self { prefix, parts, suffix }
  }

  /// Writes a list of numbers, outermost first.
  ///
  /// An empty list writes nothing at all, not the punctuation around a number that is not there.
  /// §7.7.1 describes formatting *a list of numbers* and does not say what an empty one comes to;
  /// `level="multiple"` gives one whenever nothing the `count` pattern matches is an ancestor.
  /// Writing the prefix and suffix alone would put `. ` in the result and claim a number had been
  /// worked out.
  pub(crate) fn format(&self, numbers: &[f64], grouping: Grouping<'_>) -> String {
    if numbers.is_empty() {
      return String::new();
    }
    let mut written = self.prefix.clone();
    for (position, number) in numbers.iter().enumerate() {
      // More numbers than tokens: §7.7.1 repeats the last token, and the separator before it, so
      // that one token can number a tree of any depth.
      let last = self.parts.len() - 1;
      let (separator, sequence) = &self.parts[position.min(last)];
      if position > 0 {
        // The first token's separator is written by the prefix; between numbers, a format with
        // only one token has no separator of its own, and §7.7.1 makes that a full stop.
        if position <= last {
          written.push_str(separator);
        } else {
          let (repeated, _) = &self.parts[last];
          written.push_str(if repeated.is_empty() { "." } else { repeated });
        }
      }
      written.push_str(&write_one(*number, sequence, grouping));
    }
    written.push_str(&self.suffix);
    written
  }
}

/// What `letter-value` said, which decides what a token like `i` means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LetterValue {
  /// Nothing was said. §7.7.1 leaves the choice to the processor — see [`sequence_of`].
  #[default]
  Unstated,
  /// `letter-value="alphabetic"`: `i` is the ninth letter.
  Alphabetic,
  /// `letter-value="traditional"`: `i` is the Roman numeral one.
  Traditional,
}

/// Which sequence a token names.
///
/// `i` and `I` are the awkward ones: they could begin the alphabet's ninth letter or the Roman
/// numerals, and §7.7.1 says only that `letter-value` disambiguates them — **it does not say
/// which to pick when `letter-value` is absent**. Roman is taken here, because a stylesheet that
/// writes `i` and means the letter has `a` available and would rarely start at the ninth letter,
/// while one that means Roman numerals has no other way to ask.
fn sequence_of(token: &str, letter_value: LetterValue) -> Sequence {
  let mut characters = token.chars();
  let Some(first) = characters.next() else {
    return Sequence::Decimal { width: 1 };
  };
  match first {
    'i' | 'I' if token.chars().count() == 1 && letter_value != LetterValue::Alphabetic => {
      Sequence::Roman { upper: first == 'I' }
    }
    'a' | 'A' | 'i' | 'I' if token.chars().count() == 1 => Sequence::Alphabetic { upper: first.is_uppercase() },
    // A token of digits: `01` pads to two, `1` to one. §7.7.1 defines this for any digit token
    // whose last character is a decimal digit.
    _ if token.chars().last().is_some_and(|last| last.is_ascii_digit()) => {
      Sequence::Decimal { width: token.chars().count() }
    }
    // §7.7.1: a token this does not recognise is numbered as `1`.
    _ => Sequence::Decimal { width: 1 },
  }
}

/// Whether a character belongs to a token rather than to a separator.
///
/// §7.7.1 says a token is a maximal sequence of alphanumeric characters, which is Unicode's
/// notion of alphanumeric rather than ASCII's — `一` and `١` are as much letters and digits as
/// `a` and `1`.
fn is_token_character(character: char) -> bool {
  character.is_alphanumeric()
}

/// Writes one number in one sequence.
fn write_one(number: f64, sequence: &Sequence, grouping: Grouping<'_>) -> String {
  // §7.7: the value is rounded to an integer. A value that is not a number, or is too large to
  // be one, has no numbering — writing it as a plain number says more than inventing letters
  // for it would.
  if !number.is_finite() {
    return crate::number::plain(number);
  }
  let rounded = number.round();
  match sequence {
    Sequence::Decimal { width } => group(&decimal(rounded, *width), grouping),
    // Both alphabetic and Roman start at one; anything below has no letter to be, so it falls
    // back to a plain number rather than to nothing.
    Sequence::Alphabetic { upper } if rounded >= 1.0 => alphabetic(rounded as u64, *upper),
    Sequence::Roman { upper } if (1.0..4000.0).contains(&rounded) => roman(rounded as u16, *upper),
    _ => group(&decimal(rounded, 1), grouping),
  }
}

/// A number that cannot be numbered, written as XPath would write it.
fn plain(number: f64) -> String {
  xenolith_xpath::number_to_string(number)
}

/// Writes an integer in decimal, padded with zeros to at least `width` digits.
fn decimal(number: f64, width: usize) -> String {
  let negative = number < 0.0;
  let digits = format!("{:0>width$}", (number.abs() as u64).to_string(), width = width);
  if negative { format!("-{digits}") } else { digits }
}

/// Inserts `grouping-separator` every `grouping-size` digits, counting from the right.
///
/// §7.7.1 has the two work together; one without the other says nothing, so nothing is done.
fn group(digits: &str, grouping: Grouping<'_>) -> String {
  let (Some(separator), Some(size)) = (grouping.separator, grouping.size) else {
    return digits.to_owned();
  };
  if size == 0 {
    return digits.to_owned();
  }
  let (sign, body) = match digits.strip_prefix('-') {
    Some(rest) => ("-", rest),
    None => ("", digits),
  };
  let characters: Vec<char> = body.chars().collect();
  let mut grouped = String::new();
  for (index, character) in characters.iter().enumerate() {
    let remaining = characters.len() - index;
    if index > 0 && remaining % size == 0 {
      grouped.push_str(separator);
    }
    grouped.push(*character);
  }
  format!("{sign}{grouped}")
}

/// Writes a number as a, b, … z, aa, ab — a bijective base twenty-six.
fn alphabetic(number: u64, upper: bool) -> String {
  let first = if upper { b'A' } else { b'a' };
  let mut letters = Vec::new();
  let mut remaining = number;
  while remaining > 0 {
    // Bijective, so there is no zero: 26 is `z`, not `a0`, and the borrow happens before the
    // division rather than after it.
    let index = (remaining - 1) % 26;
    letters.push((first + index as u8) as char);
    remaining = (remaining - 1) / 26;
  }
  letters.iter().rev().collect()
}

/// Writes a number as a Roman numeral, 1 to 3999.
fn roman(number: u16, upper: bool) -> String {
  const VALUES: [(u16, &str); 13] = [
    (1000, "m"),
    (900, "cm"),
    (500, "d"),
    (400, "cd"),
    (100, "c"),
    (90, "xc"),
    (50, "l"),
    (40, "xl"),
    (10, "x"),
    (9, "ix"),
    (5, "v"),
    (4, "iv"),
    (1, "i"),
  ];
  let mut written = String::new();
  let mut remaining = number;
  for (value, numeral) in VALUES {
    while remaining >= value {
      written.push_str(numeral);
      remaining -= value;
    }
  }
  if upper { written.to_uppercase() } else { written }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn no_grouping() -> Grouping<'static> {
    Grouping { separator: None, size: None }
  }

  fn write(format: &str, numbers: &[f64]) -> String {
    Format::parse(format, LetterValue::Unstated).format(numbers, no_grouping())
  }

  #[test]
  fn a_format_alternates_tokens_and_separators() {
    assert_eq!(write("1", &[3.0]), "3");
    assert_eq!(write("1.1", &[1.0, 2.0]), "1.2");
    assert_eq!(write("1.1.1", &[1.0, 2.0, 3.0]), "1.2.3");
    assert_eq!(write("(1)", &[7.0]), "(7)");
    assert_eq!(write("[1-1]", &[4.0, 5.0]), "[4-5]");
  }

  #[test]
  fn more_numbers_than_tokens_repeat_the_last_token() {
    // §7.7.1: the last token and the separator before it are reused, which is what lets one
    // token number a tree of any depth.
    assert_eq!(write("1.", &[1.0, 2.0, 3.0]), "1.2.3.");
    assert_eq!(write("1", &[1.0, 2.0, 3.0]), "1.2.3", "with no separator of its own, a full stop");
    assert_eq!(write("A-1", &[1.0, 2.0, 3.0]), "A-2-3");
  }

  #[test]
  fn fewer_numbers_than_tokens_uses_only_what_it_needs() {
    assert_eq!(write("1.1.1", &[9.0]), "9");
  }

  #[test]
  fn a_digit_token_pads_to_its_own_width() {
    assert_eq!(write("01", &[7.0]), "07");
    assert_eq!(write("001", &[7.0]), "007");
    assert_eq!(write("001", &[1234.0]), "1234", "padding never truncates");
  }

  #[test]
  fn letters_run_a_to_z_then_aa() {
    assert_eq!(alphabetic(1, false), "a");
    assert_eq!(alphabetic(26, false), "z");
    assert_eq!(alphabetic(27, false), "aa");
    assert_eq!(alphabetic(28, false), "ab");
    assert_eq!(alphabetic(52, false), "az");
    assert_eq!(alphabetic(53, false), "ba");
    assert_eq!(alphabetic(702, false), "zz");
    assert_eq!(alphabetic(703, false), "aaa");
    assert_eq!(alphabetic(1, true), "A");
  }

  #[test]
  fn roman_numerals_subtract_where_they_should() {
    assert_eq!(roman(1, false), "i");
    assert_eq!(roman(4, false), "iv");
    assert_eq!(roman(9, false), "ix");
    assert_eq!(roman(14, false), "xiv");
    assert_eq!(roman(40, false), "xl");
    assert_eq!(roman(1990, false), "mcmxc");
    assert_eq!(roman(2024, true), "MMXXIV");
    assert_eq!(roman(3999, false), "mmmcmxcix");
  }

  #[test]
  fn a_token_that_is_a_single_i_is_roman_unless_told_otherwise() {
    // §7.7.1 does not say which to pick when letter-value is absent; this build takes Roman.
    assert_eq!(write("i", &[4.0]), "iv");
    assert_eq!(write("I", &[4.0]), "IV");
    let alphabetic = Format::parse("i", LetterValue::Alphabetic).format(&[4.0], no_grouping());
    assert_eq!(alphabetic, "d", "letter-value='alphabetic' makes it the fourth letter");
    let traditional = Format::parse("i", LetterValue::Traditional).format(&[4.0], no_grouping());
    assert_eq!(traditional, "iv");
  }

  #[test]
  fn a_number_with_no_letter_to_be_falls_back_to_digits() {
    assert_eq!(write("a", &[0.0]), "0", "there is no zeroth letter");
    assert_eq!(write("i", &[0.0]), "0");
    assert_eq!(write("i", &[4000.0]), "4000", "beyond what Roman numerals reach");
    assert_eq!(write("a", &[-3.0]), "-3");
  }

  #[test]
  fn a_token_that_names_no_sequence_is_numbered_as_one() {
    assert_eq!(write("\u{3b1}", &[3.0]), "3", "a Greek alpha is alphanumeric, but names no sequence here");
  }

  #[test]
  fn a_format_with_no_token_still_numbers() {
    assert_eq!(write("", &[3.0]), "3");
    assert_eq!(write("--", &[3.0]), "--3");
  }

  #[test]
  fn grouping_counts_from_the_right() {
    let grouping = Grouping { separator: Some(","), size: Some(3) };
    let format = Format::parse("1", LetterValue::Unstated);
    assert_eq!(format.format(&[1234567.0], grouping), "1,234,567");
    assert_eq!(format.format(&[100.0], grouping), "100");
    assert_eq!(format.format(&[1000.0], grouping), "1,000");
    let by_two = Grouping { separator: Some(" "), size: Some(2) };
    assert_eq!(format.format(&[12345.0], by_two), "1 23 45");
  }

  #[test]
  fn grouping_needs_both_halves_to_do_anything() {
    let format = Format::parse("1", LetterValue::Unstated);
    assert_eq!(format.format(&[1234567.0], Grouping { separator: Some(","), size: None }), "1234567");
    assert_eq!(format.format(&[1234567.0], Grouping { separator: None, size: Some(3) }), "1234567");
  }

  #[test]
  fn a_value_that_is_not_a_number_is_written_plainly() {
    assert_eq!(write("1", &[f64::NAN]), "NaN");
    assert_eq!(write("a", &[f64::INFINITY]), "Infinity");
  }

  #[test]
  fn a_value_is_rounded_before_it_is_numbered() {
    assert_eq!(write("1", &[3.7]), "4");
    assert_eq!(write("a", &[2.2]), "b");
  }
}
