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
fn only_xml_whitespace_surrounds_a_number() {
  assert_eq!(string_to_number(" \t\r\n5 \t\r\n"), 5.0);
  // Other Unicode spaces are not whitespace to XPath, so they make the string something other than a number.
  for spaced in ["\u{3000}5", "5\u{a0}", "\u{2003}5"] {
    assert!(string_to_number(spaced).is_nan(), "{spaced:?}");
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
