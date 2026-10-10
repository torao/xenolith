use super::*;

#[test]
fn round_sends_a_half_towards_positive_infinity() {
  assert_eq!(round(1.5), 2.0);
  assert_eq!(round(-1.5), -1.0, "not away from zero, the way f64::round would");
  assert_eq!(round(2.5), 3.0);
  assert_eq!(round(1.4), 1.0);
  assert_eq!(round(-1.6), -2.0);
  assert!(round(f64::NAN).is_nan());
  assert_eq!(round(f64::INFINITY), f64::INFINITY);
}

#[test]
fn rounding_towards_zero_from_below_keeps_the_sign() {
  // §4.4: round(-0.5) is -0, not 0. Both print as "0", so the sign shows only in arithmetic.
  for value in [-0.5, -0.3, -0.0] {
    let rounded = round(value);
    assert_eq!(rounded, 0.0);
    assert!(rounded.is_sign_negative(), "round({value}) should be negative zero");
  }
  assert!(round(0.3).is_sign_positive(), "round(0.3) is positive zero");
}

#[test]
fn substring_handles_the_awkward_bounds_from_the_specification() {
  assert_eq!(substring("12345", 2.0, None), "2345");
  assert_eq!(substring("12345", 1.5, Some(2.6)), "234");
  assert_eq!(substring("12345", 0.0, Some(3.0)), "12");
  assert_eq!(substring("12345", f64::NAN, Some(3.0)), "");
  assert_eq!(substring("12345", 1.0, Some(f64::NAN)), "");
  assert_eq!(substring("12345", -42.0, Some(f64::INFINITY)), "12345");
  assert_eq!(substring("12345", f64::NEG_INFINITY, Some(f64::INFINITY)), "");
}

#[test]
fn translate_replaces_and_removes() {
  assert_eq!(translate("bar", "abc", "ABC"), "BAr");
  assert_eq!(translate("--aaa--", "abc-", "ABC"), "AAA", "a character with no replacement is dropped");
  assert_eq!(translate("aa", "aa", "xy"), "xx", "the first occurrence in the from-string wins");
}

#[test]
fn normalize_space_collapses_runs() {
  assert_eq!(normalize_space("  a  b \t\n c  "), "a b c");
  assert_eq!(normalize_space(" \t "), "");
}

#[test]
fn a_sublanguage_answers_to_its_language() {
  assert!(sublanguage_of("en", "EN"));
  assert!(sublanguage_of("en-GB", "en"));
  assert!(!sublanguage_of("england", "en"), "the match is on whole subtags");
  assert!(!sublanguage_of("en", "en-GB"));
}
