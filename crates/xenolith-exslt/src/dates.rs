//! `http://exslt.org/dates-and-times` — reading a date or a time apart.
//!
//! Every function here takes an ISO 8601 string of one of the types XML Schema Part 2 defines —
//! `dateTime`, `date`, `time`, `gYearMonth`, `gYear`, `gMonthDay`, `gMonth`, `gDay` — or nothing
//! at all, in which case the current date and time are read instead.
//!
//! A function whose component the argument does not carry answers with NaN, or with the empty
//! string where it answers with text. `date:month-in-year('2026-07-29')` is 7; on a `time` there
//! is no month to give, and inventing one would be worse than saying so.
//!
//! # The calendar
//!
//! The arithmetic is the proleptic Gregorian calendar, which is what XML Schema Part 2 §D
//! specifies — the Gregorian rules run backwards through the years before they were adopted,
//! rather than switching to Julian at some date. So `date:leap-year('1500')` is false here, as
//! Gregorian says, though the year *was* a leap year under the calendar in use at the time.
//!
//! # What is not here
//!
//! `date:add`, `date:sub`, `date:difference` and `date:duration` do arithmetic on durations, and
//! a duration holding months is not a fixed length of time: XML Schema Part 2 §E gives a specific
//! procedure for it, with its own rules for what adding a month to the 31st means. That is a
//! piece of work of its own rather than a few more accessors, and doing it approximately would
//! give answers that look right and drift. See `ROADMAP.md`.
//!
//! # Specifications
//!
//! - [`exslt:dates-and-times`](http://exslt.org/date/index.html)
//! - [XML Schema Part 2: Datatypes](https://www.w3.org/TR/2004/REC-xmlschema-2-20041028/), for
//!   what the lexical forms are and what the calendar is

use std::time::{SystemTime, UNIX_EPOCH};

use xenolith_xdm::Model;
use xenolith_xpath::{Context, Functions, Value};

use crate::support::arity;

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/dates-and-times";

/// The months, as `date:month-name` and `date:month-abbreviation` give them.
const MONTHS: [&str; 12] = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/// The days, from Sunday, which is day one in EXSLT's numbering.
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// A moment, as much of one as the string that was read carried.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Moment {
  year: Option<i64>,
  month: Option<u32>,
  day: Option<u32>,
  hour: Option<u32>,
  minute: Option<u32>,
  second: Option<f64>,
  /// The offset from UTC in minutes, if the string gave one.
  offset: Option<i32>,
}

impl Moment {
  /// The year, month and day, when the moment has all three.
  const fn date(&self) -> Option<(i64, u32, u32)> {
    match (self.year, self.month, self.day) {
      (Some(year), Some(month), Some(day)) => Some((year, month, day)),
      _ => None,
    }
  }
}

/// Registers one function of a moment.
///
/// A macro rather than a helper returning a closure: such a helper would be an opaque type
/// mentioning the model, and the registry could then only hold it for a model that outlives
/// everything, which a model borrowing a document does not. Expanding to a closure written in
/// place avoids that, and there are seventeen of these to write.
macro_rules! of_moment {
  ($functions:expr, $local:literal, $answer:ident, $part:expr) => {
    $functions.with(NAMESPACE, $local, |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity(concat!("date:", $local), &arguments, 0, Some(1))?;
      let read_part: fn(&Moment) -> Option<_> = $part;
      let part = read(&arguments, context).as_ref().and_then(read_part);
      Ok($answer(part))
    })
  };
}

/// A number, or NaN where the moment has no such part.
fn as_number<N>(part: Option<f64>) -> Value<N> {
  Value::Number(part.unwrap_or(f64::NAN))
}

/// Text, or the empty string where the moment has no such part.
fn as_text<N>(part: Option<String>) -> Value<N> {
  Value::String(part.unwrap_or_default())
}

/// Adds this module's functions.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>) -> Functions<M> {
  let functions = functions
    .with(NAMESPACE, "date-time", |arguments: Vec<Value<M::Node>>, _: &Context<'_, M>| {
      arity("date:date-time", &arguments, 0, Some(0))?;
      // The only function here with no argument form of its own: it *is* the current moment.
      Ok(Value::String(now_as_text()))
    })
    .with(NAMESPACE, "leap-year", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("date:leap-year", &arguments, 0, Some(1))?;
      let Some(moment) = read(&arguments, context) else { return Ok(Value::Number(f64::NAN)) };
      // A boolean would be the obvious answer, but EXSLT says NaN where there is no year, and
      // a boolean cannot say that.
      Ok(moment.year.map_or(Value::Number(f64::NAN), |year| Value::Boolean(is_leap(year))))
    })
    .with(NAMESPACE, "seconds", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("date:seconds", &arguments, 0, Some(1))?;
      let written = match arguments.first() {
        Some(value) => value.string(context.model),
        None => now_as_text(),
      };
      Ok(Value::Number(seconds(&written)))
    });

  let functions = of_moment!(functions, "date", as_text, |moment| {
    let (year, month, day) = moment.date()?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
  });
  let functions = of_moment!(functions, "time", as_text, |moment| {
    let (hour, minute, second) = (moment.hour?, moment.minute?, moment.second?);
    Some(format!("{hour:02}:{minute:02}:{:02}", second.trunc() as u32))
  });
  let functions = of_moment!(functions, "month-name", as_text, |moment| {
    moment.month.and_then(|month| MONTHS.get(month as usize - 1).map(|name| (*name).to_owned()))
  });
  let functions = of_moment!(functions, "month-abbreviation", as_text, |moment| {
    moment.month.and_then(|month| MONTHS.get(month as usize - 1).map(|name| name[..3].to_owned()))
  });
  let functions = of_moment!(functions, "day-name", as_text, |moment| {
    let (year, month, day) = moment.date()?;
    Some(DAYS[day_of_week(year, month, day) as usize - 1].to_owned())
  });
  let functions = of_moment!(functions, "day-abbreviation", as_text, |moment| {
    let (year, month, day) = moment.date()?;
    Some(DAYS[day_of_week(year, month, day) as usize - 1][..3].to_owned())
  });

  let functions = of_moment!(functions, "year", as_number, |moment| moment.year.map(|year| year as f64));
  let functions = of_moment!(functions, "month-in-year", as_number, |moment| moment.month.map(f64::from));
  let functions = of_moment!(functions, "day-in-month", as_number, |moment| moment.day.map(f64::from));
  let functions = of_moment!(functions, "hour-in-day", as_number, |moment| moment.hour.map(f64::from));
  let functions = of_moment!(functions, "minute-in-hour", as_number, |moment| moment.minute.map(f64::from));
  let functions = of_moment!(functions, "second-in-minute", as_number, |moment| moment.second);
  let functions = of_moment!(functions, "day-in-week", as_number, |moment| {
    let (year, month, day) = moment.date()?;
    Some(f64::from(day_of_week(year, month, day)))
  });
  let functions = of_moment!(functions, "day-in-year", as_number, |moment| {
    let (year, month, day) = moment.date()?;
    Some(f64::from(day_of_year(year, month, day)))
  });
  let functions = of_moment!(functions, "day-of-week-in-month", as_number, |moment| {
    // Which Tuesday of the month this is, counting from one.
    let (_, _, day) = moment.date()?;
    Some(f64::from((day - 1) / 7 + 1))
  });
  of_moment!(functions, "week-in-year", as_number, |moment| {
    let (year, month, day) = moment.date()?;
    Some(f64::from(iso_week(year, month, day)))
  })
}

/// The moment a call is about: its argument, or the current one when it has none.
fn read<M: Model>(arguments: &[Value<M::Node>], context: &Context<'_, M>) -> Option<Moment> {
  let written = match arguments.first() {
    Some(value) => value.string(context.model),
    None => now_as_text(),
  };
  parse(&written)
}

/// Reads any of the XML Schema date and time forms.
///
/// The forms differ in which parts they carry, and a `-` may begin a year or mark a missing one
/// (`--07` is July of no year, `---29` the 29th of no month), so what a string is is decided by
/// its shape before any of it is read as a number.
fn parse(written: &str) -> Option<Moment> {
  let written = written.trim();
  let (body, offset) = split_offset(written)?;

  // A time on its own has a colon before any dash could appear.
  if let Some(rest) = body.strip_prefix("---") {
    let day = rest.parse::<u32>().ok().filter(|day| (1..=31).contains(day))?;
    return Some(Moment { day: Some(day), offset, ..Moment::default() });
  }
  if let Some(rest) = body.strip_prefix("--") {
    // `--MM` is a month, `--MM-DD` a month and a day.
    let (month, day) = match rest.split_once('-') {
      None => (rest.parse::<u32>().ok()?, None),
      Some((month, day)) => (month.parse::<u32>().ok()?, Some(day.parse::<u32>().ok()?)),
    };
    if !(1..=12).contains(&month) {
      return None;
    }
    return Some(Moment { month: Some(month), day, offset, ..Moment::default() });
  }
  if body.contains(':') && !body.contains('T') && !body.contains('-') {
    let (hour, minute, second) = parse_time(body)?;
    return Some(Moment { hour: Some(hour), minute: Some(minute), second: Some(second), offset, ..Moment::default() });
  }

  let (date_part, time_part) = match body.split_once('T') {
    Some((date, time)) => (date, Some(time)),
    None => (body, None),
  };
  let mut moment = parse_date(date_part)?;
  moment.offset = offset;
  if let Some(time) = time_part {
    let (hour, minute, second) = parse_time(time)?;
    moment.hour = Some(hour);
    moment.minute = Some(minute);
    moment.second = Some(second);
  }
  Some(moment)
}

/// Splits a trailing time-zone offset off, giving it in minutes.
fn split_offset(written: &str) -> Option<(&str, Option<i32>)> {
  if let Some(body) = written.strip_suffix('Z') {
    return Some((body, Some(0)));
  }
  // An offset is `+hh:mm` or `-hh:mm` at the very end, which only a time zone can be: a date's
  // own dashes are never followed by a colon two characters later.
  if written.len() > 6 {
    let (body, tail) = written.split_at(written.len() - 6);
    let sign = match tail.as_bytes().first() {
      Some(b'+') => 1,
      Some(b'-') => -1,
      _ => return Some((written, None)),
    };
    if let Some((hours, minutes)) = tail[1..].split_once(':') {
      let hours: i32 = hours.parse().ok()?;
      let minutes: i32 = minutes.parse().ok()?;
      return Some((body, Some(sign * (hours * 60 + minutes))));
    }
  }
  Some((written, None))
}

/// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`, the year possibly negative.
fn parse_date(written: &str) -> Option<Moment> {
  let (negative, rest) = match written.strip_prefix('-') {
    Some(rest) => (true, rest),
    None => (false, written),
  };
  let mut parts = rest.split('-');
  let year: i64 = parts.next()?.parse().ok()?;
  let year = if negative { -year } else { year };
  let month = match parts.next() {
    Some(month) => Some(month.parse::<u32>().ok().filter(|month| (1..=12).contains(month))?),
    None => None,
  };
  let day = match parts.next() {
    Some(day) => Some(day.parse::<u32>().ok().filter(|day| (1..=31).contains(day))?),
    None => None,
  };
  if parts.next().is_some() {
    return None;
  }
  Some(Moment { year: Some(year), month, day, ..Moment::default() })
}

/// `hh:mm:ss` with optional fractional seconds.
fn parse_time(written: &str) -> Option<(u32, u32, f64)> {
  let mut parts = written.split(':');
  let hour: u32 = parts.next()?.parse().ok()?;
  let minute: u32 = parts.next()?.parse().ok()?;
  let second: f64 = parts.next().unwrap_or("0").parse().ok()?;
  if parts.next().is_some() || hour > 24 || minute > 59 || !(0.0..61.0).contains(&second) {
    return None;
  }
  Some((hour, minute, second))
}

/// `date:seconds`: seconds since the epoch for a moment, or the length of a duration.
fn seconds(written: &str) -> f64 {
  let written = written.trim();
  if written.starts_with('P') || written.starts_with("-P") {
    return duration_seconds(written);
  }
  let Some(moment) = parse(written) else { return f64::NAN };
  let Some((year, month, day)) = moment.date() else { return f64::NAN };
  let days = days_from_civil(year, month, day);
  let hour = f64::from(moment.hour.unwrap_or(0));
  let minute = f64::from(moment.minute.unwrap_or(0));
  let second = moment.second.unwrap_or(0.0);
  let offset = f64::from(moment.offset.unwrap_or(0));
  (days as f64) * 86_400.0 + hour * 3600.0 + minute * 60.0 + second - offset * 60.0
}

/// The length of an `xs:duration` in seconds.
///
/// A duration holding years or months has no fixed length — how long a month is depends on which
/// month — so EXSLT's own note is followed here: a year counts as 365 days and a month as 30,
/// which is what makes `date:seconds` answer at all rather than NaN.
fn duration_seconds(written: &str) -> f64 {
  let (sign, rest) = match written.strip_prefix('-') {
    Some(rest) => (-1.0, rest),
    None => (1.0, written),
  };
  let Some(rest) = rest.strip_prefix('P') else { return f64::NAN };
  let (date_part, time_part) = match rest.split_once('T') {
    Some((date, time)) => (date, time),
    None => (rest, ""),
  };

  let mut total = 0.0;
  let mut number = String::new();
  for (part, units) in [
    (date_part, [('Y', 31_536_000.0), ('M', 2_592_000.0), ('D', 86_400.0)].as_slice()),
    (time_part, [('H', 3600.0), ('M', 60.0), ('S', 1.0)].as_slice()),
  ] {
    for character in part.chars() {
      if character.is_ascii_digit() || character == '.' {
        number.push(character);
        continue;
      }
      let Some((_, seconds)) = units.iter().find(|(unit, _)| *unit == character) else { return f64::NAN };
      let Ok(value) = number.parse::<f64>() else { return f64::NAN };
      total += value * seconds;
      number.clear();
    }
  }
  if number.is_empty() { sign * total } else { f64::NAN }
}

/// The current moment, as an `xs:dateTime` in UTC.
fn now_as_text() -> String {
  let Ok(since_epoch) = SystemTime::now().duration_since(UNIX_EPOCH) else {
    // Before 1970, which a clock should not be; there is nothing useful to say about it.
    return String::new();
  };
  let total = since_epoch.as_secs();
  let days = (total / 86_400) as i64;
  let rest = total % 86_400;
  let (year, month, day) = civil_from_days(days);
  format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// Whether a year is a leap year in the proleptic Gregorian calendar.
const fn is_leap(year: i64) -> bool {
  (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// How many days each month holds.
const fn days_in_month(year: i64, month: u32) -> u32 {
  match month {
    1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
    4 | 6 | 9 | 11 => 30,
    2 if is_leap(year) => 29,
    2 => 28,
    _ => 0,
  }
}

/// Which day of the year a date is, counting from one.
fn day_of_year(year: i64, month: u32, day: u32) -> u32 {
  (1..month).map(|earlier| days_in_month(year, earlier)).sum::<u32>() + day
}

/// Days from 1970-01-01 to a date, negative before it.
///
/// Howard Hinnant's `days_from_civil`, which is exact for every year the proleptic Gregorian
/// calendar covers rather than only for those near the epoch.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
  let year = if month <= 2 { year - 1 } else { year };
  let era = if year >= 0 { year } else { year - 399 } / 400;
  let year_of_era = year - era * 400;
  let month = i64::from(month);
  let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
  let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  era * 146_097 + day_of_era - 719_468
}

/// The date a count of days from 1970-01-01 names; the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
  let days = days + 719_468;
  let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
  let day_of_era = days - era * 146_097;
  let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
  let year = year_of_era + era * 400;
  let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let month_prime = (5 * day_of_year + 2) / 153;
  let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
  let month = (if month_prime < 10 { month_prime + 3 } else { month_prime - 9 }) as u32;
  (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Which day of the week a date is: 1 for Sunday, as EXSLT numbers them.
fn day_of_week(year: i64, month: u32, day: u32) -> u32 {
  // 1970-01-01 was a Thursday, which is day 5 in this numbering.
  let days = days_from_civil(year, month, day);
  (((days + 4).rem_euclid(7)) + 1) as u32
}

/// The ISO 8601 week a date falls in.
///
/// A week belongs to the year holding its Thursday, so the first days of January can be week 52
/// or 53 of the year before, and the last days of December week 1 of the year after.
fn iso_week(year: i64, month: u32, day: u32) -> u32 {
  // ISO counts Monday as day 1 and Sunday as day 7; EXSLT counts Sunday as day 1.
  let weekday = match day_of_week(year, month, day) {
    1 => 7,
    other => other - 1,
  };
  let ordinal = day_of_year(year, month, day);
  let week = (ordinal as i64 - i64::from(weekday) + 10) / 7;
  if week < 1 {
    // It belongs to the last week of the year before, which has 53 only when that year began or
    // ended on a Thursday.
    return weeks_in_year(year - 1);
  }
  if week > i64::from(weeks_in_year(year)) {
    return 1;
  }
  week as u32
}

/// How many ISO weeks a year holds: 53 when it begins on a Thursday, or is a leap year beginning
/// on a Wednesday; 52 otherwise.
fn weeks_in_year(year: i64) -> u32 {
  let first = day_of_week(year, 1, 1);
  // 5 is Thursday and 4 is Wednesday in EXSLT's numbering, which begins at Sunday.
  if first == 5 || (is_leap(year) && first == 4) { 53 } else { 52 }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_date_time_is_read_apart() {
    let moment = parse("2026-07-29T13:45:30Z").expect("a dateTime");
    assert_eq!(moment.year, Some(2026));
    assert_eq!(moment.month, Some(7));
    assert_eq!(moment.day, Some(29));
    assert_eq!(moment.hour, Some(13));
    assert_eq!(moment.minute, Some(45));
    assert_eq!(moment.second, Some(30.0));
    assert_eq!(moment.offset, Some(0));
  }

  #[test]
  fn the_shorter_forms_carry_only_what_they_name() {
    assert_eq!(parse("2026-07-29").expect("a date").hour, None);
    assert_eq!(parse("2026-07").expect("a gYearMonth").day, None);
    assert_eq!(parse("2026").expect("a gYear").month, None);
    assert_eq!(parse("--07").expect("a gMonth").month, Some(7));
    assert_eq!(parse("--07").expect("a gMonth").year, None);
    assert_eq!(parse("--07-29").expect("a gMonthDay").day, Some(29));
    assert_eq!(parse("---29").expect("a gDay").day, Some(29));
    assert_eq!(parse("13:45:30").expect("a time").hour, Some(13));
    assert_eq!(parse("13:45:30").expect("a time").year, None);
  }

  #[test]
  fn a_time_zone_is_read_and_a_negative_year_is_not_mistaken_for_one() {
    assert_eq!(parse("2026-07-29T00:00:00+09:00").expect("a dateTime").offset, Some(540));
    assert_eq!(parse("2026-07-29T00:00:00-05:00").expect("a dateTime").offset, Some(-300));
    assert_eq!(parse("-0044-03-15").expect("a date").year, Some(-44), "the ides of March, BCE");
  }

  #[test]
  fn what_is_not_a_date_is_not_read() {
    assert_eq!(parse("yesterday"), None);
    assert_eq!(parse("2026-13-01"), None, "there is no thirteenth month");
    assert_eq!(parse("2026-07-29T25:00:00"), None, "there is no twenty-fifth hour");
  }

  #[test]
  fn leap_years_follow_the_gregorian_rule_all_the_way_back() {
    assert!(is_leap(2024));
    assert!(!is_leap(2026));
    assert!(!is_leap(1900), "a century that is not a fourth one");
    assert!(is_leap(2000));
    assert!(!is_leap(1500), "proleptic Gregorian, whatever the calendar of the day said");
  }

  #[test]
  fn the_day_of_the_week_is_right_at_dates_that_are_known() {
    // 1970-01-01 was a Thursday, which is 5 counting Sunday as 1.
    assert_eq!(day_of_week(1970, 1, 1), 5);
    assert_eq!(day_of_week(2000, 1, 1), 7, "a Saturday");
    assert_eq!(day_of_week(2026, 7, 29), 4, "a Wednesday");
  }

  #[test]
  fn the_day_of_the_year_counts_the_months_before_it() {
    assert_eq!(day_of_year(2026, 1, 1), 1);
    assert_eq!(day_of_year(2026, 12, 31), 365);
    assert_eq!(day_of_year(2024, 12, 31), 366, "a leap year");
    assert_eq!(day_of_year(2024, 3, 1), 61, "the day after the 29th of February");
  }

  #[test]
  fn days_from_the_epoch_go_both_ways() {
    for (year, month, day) in [(1970, 1, 1), (2026, 7, 29), (1900, 2, 28), (2000, 2, 29), (1066, 10, 14)] {
      let days = days_from_civil(year, month, day);
      assert_eq!(civil_from_days(days), (year, month, day), "{year}-{month}-{day}");
    }
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(1970, 1, 2), 1);
    assert_eq!(days_from_civil(1969, 12, 31), -1);
  }

  #[test]
  fn iso_weeks_belong_to_the_year_holding_their_thursday() {
    // 2026-01-01 is a Thursday, so it is week 1 of 2026.
    assert_eq!(iso_week(2026, 1, 1), 1);
    // 2021-01-01 is a Friday, so it belongs to the last week of 2020.
    assert_eq!(iso_week(2021, 1, 1), 53);
    assert_eq!(iso_week(2026, 12, 31), 53);
  }

  #[test]
  fn seconds_since_the_epoch_account_for_the_time_zone() {
    assert_eq!(seconds("1970-01-01T00:00:00Z"), 0.0);
    assert_eq!(seconds("1970-01-02T00:00:00Z"), 86_400.0);
    assert_eq!(seconds("1970-01-01T09:00:00+09:00"), 0.0, "the same moment, written elsewhere");
    assert!(seconds("not a date").is_nan());
  }

  #[test]
  fn a_duration_is_measured_in_seconds() {
    assert_eq!(seconds("PT1H"), 3600.0);
    assert_eq!(seconds("P1D"), 86_400.0);
    assert_eq!(seconds("PT1M30S"), 90.0);
    assert_eq!(seconds("-PT1H"), -3600.0);
    assert!(seconds("P").is_nan() || seconds("P") == 0.0, "an empty duration says nothing useful");
  }

  #[test]
  fn the_current_moment_reads_back_as_a_date_time() {
    let now = now_as_text();
    let moment = parse(&now).expect("what it writes, it can read");
    assert!(moment.year.is_some_and(|year| year >= 2020));
    assert_eq!(moment.offset, Some(0), "written in UTC");
  }
}
