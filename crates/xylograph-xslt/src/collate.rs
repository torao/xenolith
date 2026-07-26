//! Comparing two sort keys as text.
//!
//! XSLT 1.0 §10 says a text sort uses "the collating sequence for the language", and leaves what
//! that is to the processor: the specification does not define an order, and two conforming
//! processors may put the same two strings the other way round. So which order this gives is
//! **implementation-dependent**, and it depends further on how the crate was built:
//!
//! - with the `icu` feature (on by default), a language's own conventions, from CLDR through
//!   ICU4X — `ä` sorts beside `a` in German and after `z` in Swedish
//! - without it, by Unicode code point, which is stable and cheap but is nobody's alphabet
//!
//! `tests/behaviour.rs` in the `xylograph` crate prints which of the two is in force, rather than
//! leaving a reader to work it out from the build.
//!
//! `case-order` goes *into* the collation rather than on top of it. It is CLDR's `kf`, and a
//! collation already separates `A` from `a` at its own strength — so an implementation that
//! applied it only where the collation reported the two keys equal would never apply it at all.
//! Without collation data there is nothing to put it into, and the fallback has to ignore case
//! first for `case-order` to have anything left to decide.

use std::cmp::Ordering;

/// Whether `upper-first` or `lower-first` was asked for (XSLT 1.0 §10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaseOrder {
  /// Nothing was said, so case does not decide anything on its own.
  #[default]
  Unstated,
  /// `case-order="upper-first"`.
  UpperFirst,
  /// `case-order="lower-first"`.
  LowerFirst,
}

/// Compares text sort keys in one language.
#[derive(Debug)]
pub(crate) struct Collator {
  case_order: CaseOrder,
  /// Borrowed from the compiled-in CLDR data, so it is `'static` and owns nothing of ours.
  #[cfg(feature = "icu")]
  collator: Option<icu_collator::CollatorBorrowed<'static>>,
}

impl Collator {
  /// A collator for a language — `lang` as `xsl:sort` wrote it, a BCP 47 tag.
  ///
  /// A tag that cannot be read, or a language there is no data for, falls back to the root
  /// collation rather than being refused: §10 does not make an unknown language an error, and a
  /// stylesheet that names one still has to sort somehow.
  #[cfg(feature = "icu")]
  pub(crate) fn new(lang: Option<&str>, case_order: CaseOrder) -> Self {
    use icu_collator::preferences::CollationCaseFirst;
    use icu_collator::{CollatorBorrowed, CollatorPreferences, options::CollatorOptions};

    // The default preferences are the root collation, which is what an unstated or unreadable
    // language falls back to.
    let mut preferences: CollatorPreferences = match lang.and_then(|tag| tag.parse::<icu_locale_core::Locale>().ok()) {
      Some(locale) => (&locale).into(),
      None => CollatorPreferences::default(),
    };
    // `case-order` is CLDR's `kf`, so it belongs in the collation rather than being bolted on
    // afterwards: a collation separates `A` from `a` at its own strength, leaving nothing for a
    // tie-break to decide.
    preferences.case_first = match case_order {
      CaseOrder::Unstated => None,
      CaseOrder::UpperFirst => Some(CollationCaseFirst::Upper),
      CaseOrder::LowerFirst => Some(CollationCaseFirst::Lower),
    };
    let collator = CollatorBorrowed::try_new(preferences, CollatorOptions::default()).ok();
    Self { case_order, collator }
  }

  /// A collator for a language, with no collation data compiled in.
  #[cfg(not(feature = "icu"))]
  pub(crate) const fn new(lang: Option<&str>, case_order: CaseOrder) -> Self {
    // Named for the signature's sake: without the data there is nothing a language can change.
    let _ = lang;
    Self { case_order }
  }

  /// Compares two sort keys.
  #[cfg(feature = "icu")]
  pub(crate) fn compare(&self, a: &str, b: &str) -> Ordering {
    match &self.collator {
      // The collation already knows about `case-order`; there is nothing to add here.
      Some(collator) => collator.compare(a, b),
      // No data for the locale, and none for the root either; code points are what is left.
      None => fallback_compare(a, b, self.case_order),
    }
  }

  /// Compares two sort keys.
  #[cfg(not(feature = "icu"))]
  pub(crate) fn compare(&self, a: &str, b: &str) -> Ordering {
    fallback_compare(a, b, self.case_order)
  }
}

/// Comparison without collation data: by Unicode code point, with `case-order` on top.
///
/// Code-point order already puts every capital before every lower-case letter, so a stated
/// `case-order` would have nothing left to decide. To mean anything it has to compare without
/// regard to case first — which is what a collation does anyway — and let the case break the tie.
fn fallback_compare(a: &str, b: &str, case_order: CaseOrder) -> Ordering {
  if case_order == CaseOrder::Unstated {
    return a.cmp(b);
  }
  let ordering = a.to_lowercase().cmp(&b.to_lowercase());
  if ordering != Ordering::Equal {
    return ordering;
  }
  match case_order {
    CaseOrder::Unstated => Ordering::Equal,
    CaseOrder::UpperFirst => case_rank(a).cmp(&case_rank(b)),
    CaseOrder::LowerFirst => case_rank(b).cmp(&case_rank(a)),
  }
}

/// Ranks a key by the case of its first letter, upper before lower.
///
/// Only the first character that has a case counts. Two keys reach here only when the collation
/// already called them equal, which for a case-insensitive collation means they differ in case
/// alone — so the first such character is what separates them.
fn case_rank(text: &str) -> u8 {
  for character in text.chars() {
    if character.is_uppercase() {
      return 0;
    }
    if character.is_lowercase() {
      return 1;
    }
  }
  2
}

/// Whether language-aware collation was built in, which `tests/behaviour.rs` reports.
#[must_use]
pub const fn language_aware_collation() -> bool {
  cfg!(feature = "icu")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn case_order_decides_between_keys_that_differ_only_in_case() {
    let upper_first = Collator::new(None, CaseOrder::UpperFirst);
    assert_eq!(upper_first.compare("Apple", "apple"), Ordering::Less);
    let lower_first = Collator::new(None, CaseOrder::LowerFirst);
    assert_eq!(lower_first.compare("Apple", "apple"), Ordering::Greater);

    // Different words, so case-order has nothing to say about them.
    assert_eq!(upper_first.compare("apple", "banana"), Ordering::Less);
    assert_eq!(lower_first.compare("apple", "banana"), Ordering::Less);
  }

  #[test]
  fn case_rank_looks_at_the_first_character_that_has_a_case() {
    assert_eq!(case_rank("Apple"), 0);
    assert_eq!(case_rank("apple"), 1);
    assert_eq!(case_rank("123"), 2, "a key with no cased character ranks last");
    assert_eq!(case_rank("1Apple"), 0, "the digits are skipped over");
  }

  #[test]
  fn an_unreadable_language_tag_still_sorts() {
    // §10 does not make an unknown language an error, so this falls back rather than refusing.
    let collator = Collator::new(Some("not a tag at all"), CaseOrder::Unstated);
    assert_eq!(collator.compare("a", "b"), Ordering::Less);
  }
}
