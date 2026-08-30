//! `http://exslt.org/dates-and-times`, run through the XSLT engine.

#![cfg(feature = "dates")]

use xenolith_dom::build;
use xenolith_xdm::DomModel;
use xenolith_xpath::Functions;
use xenolith_xslt::{Stylesheet, Transform};

/// The namespace declarations these stylesheets need.
const PREFIXES: &str = "xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\" \
                        xmlns:date=\"http://exslt.org/dates-and-times\"";

/// Evaluates one expression.
fn value_of(expression: &str) -> String {
  let body = format!("<xsl:template match='/'><xsl:value-of select=\"{expression}\"/></xsl:template>");
  let source = format!("<xsl:stylesheet version=\"1.0\" {PREFIXES}>{body}</xsl:stylesheet>");
  let stylesheet = Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse("<a/>".as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let functions = xenolith_exslt::register(Functions::new());
  Transform::new().run_with(&stylesheet, &model, model.root_node(), functions).expect("transforms").text()
}

/// A Wednesday, the 29th of July 2026, at a quarter to two in the afternoon.
const MOMENT: &str = "'2026-07-29T13:45:30Z'";

#[test]
fn a_date_time_is_read_apart() {
  assert_eq!(value_of(&format!("date:year({MOMENT})")), "2026");
  assert_eq!(value_of(&format!("date:month-in-year({MOMENT})")), "7");
  assert_eq!(value_of(&format!("date:day-in-month({MOMENT})")), "29");
  assert_eq!(value_of(&format!("date:hour-in-day({MOMENT})")), "13");
  assert_eq!(value_of(&format!("date:minute-in-hour({MOMENT})")), "45");
  assert_eq!(value_of(&format!("date:second-in-minute({MOMENT})")), "30");
}

#[test]
fn the_date_and_the_time_can_be_taken_out_of_a_date_time() {
  assert_eq!(value_of(&format!("date:date({MOMENT})")), "2026-07-29");
  assert_eq!(value_of(&format!("date:time({MOMENT})")), "13:45:30");
}

#[test]
fn the_names_of_the_month_and_the_day() {
  assert_eq!(value_of(&format!("date:month-name({MOMENT})")), "July");
  assert_eq!(value_of(&format!("date:month-abbreviation({MOMENT})")), "Jul");
  assert_eq!(value_of(&format!("date:day-name({MOMENT})")), "Wednesday");
  assert_eq!(value_of(&format!("date:day-abbreviation({MOMENT})")), "Wed");
}

#[test]
fn where_a_day_falls_in_its_week_month_and_year() {
  assert_eq!(value_of(&format!("date:day-in-week({MOMENT})")), "4", "Wednesday, counting Sunday as one");
  assert_eq!(value_of(&format!("date:day-in-year({MOMENT})")), "210");
  assert_eq!(value_of(&format!("date:day-of-week-in-month({MOMENT})")), "5", "the fifth Wednesday");
  assert_eq!(value_of(&format!("date:week-in-year({MOMENT})")), "31");
}

#[test]
fn leap_years_answer_true_or_false_and_nothing_else() {
  assert_eq!(value_of("date:leap-year('2024')"), "true");
  assert_eq!(value_of("date:leap-year('2026')"), "false");
  assert_eq!(value_of("date:leap-year('1900')"), "false");
  assert_eq!(value_of("date:leap-year('2000')"), "true");
}

#[test]
fn a_part_the_argument_does_not_carry_is_not_invented() {
  // A time has no month; a date has no hour. Saying so beats making one up.
  assert_eq!(value_of("date:month-in-year('13:45:30')"), "NaN");
  assert_eq!(value_of("date:hour-in-day('2026-07-29')"), "NaN");
  assert_eq!(value_of("date:month-name('13:45:30')"), "");
  assert_eq!(value_of("date:date('13:45:30')"), "");
}

#[test]
fn the_shorter_schema_forms_are_read_too() {
  assert_eq!(value_of("date:year('2026-07')"), "2026");
  assert_eq!(value_of("date:month-in-year('2026-07')"), "7");
  assert_eq!(value_of("date:month-in-year('--07')"), "7", "a gMonth");
  assert_eq!(value_of("date:day-in-month('---29')"), "29", "a gDay");
  assert_eq!(value_of("date:day-in-month('--07-29')"), "29", "a gMonthDay");
}

#[test]
fn what_is_not_a_date_is_not_a_number() {
  assert_eq!(value_of("date:year('yesterday')"), "NaN");
  assert_eq!(value_of("date:year('2026-13-01')"), "NaN", "there is no thirteenth month");
  assert_eq!(value_of("date:day-name('nonsense')"), "");
}

#[test]
fn seconds_since_the_epoch_and_the_length_of_a_duration() {
  assert_eq!(value_of("date:seconds('1970-01-01T00:00:00Z')"), "0");
  assert_eq!(value_of("date:seconds('1970-01-02T00:00:00Z')"), "86400");
  // The same moment written in another time zone is the same number of seconds.
  assert_eq!(value_of("date:seconds('1970-01-01T09:00:00+09:00')"), "0");
  assert_eq!(value_of("date:seconds('PT1H')"), "3600");
  assert_eq!(value_of("date:seconds('-PT1H')"), "-3600");
}

#[test]
fn with_no_argument_the_current_moment_is_read() {
  // Not asserting what the clock says, only that it is a moment and that the pieces agree.
  let year: f64 = value_of("date:year(date:date-time())").parse().expect("a number");
  assert!(year >= 2020.0, "{year}");
  assert_eq!(value_of("date:year()"), value_of("date:year(date:date-time())"));
  assert_eq!(value_of("string-length(date:date()) > 0"), "true");
}

#[test]
fn a_negative_year_is_a_year_and_not_a_missing_one() {
  assert_eq!(value_of("date:year('-0044-03-15')"), "-44");
  assert_eq!(value_of("date:month-in-year('-0044-03-15')"), "3");
}

#[test]
fn function_available_says_what_this_build_has() {
  assert_eq!(value_of("function-available('date:year')"), "true");
  assert_eq!(value_of("function-available('date:seconds')"), "true");
  // Duration arithmetic is still to come, and says so.
  assert_eq!(value_of("function-available('date:add')"), "false");
  assert_eq!(value_of("function-available('date:difference')"), "false");
}
