//! Character classes of XML 1.0 (Fifth Edition).
//!
//! The Fifth Edition redefined `Name` in terms of broad Unicode ranges instead of the
//! enumerated character classes used up to the Fourth Edition. Everything here follows the
//! Fifth Edition; the two are *not* interchangeable.

/// `S ::= (#x20 | #x9 | #xD | #xA)+` — a single XML whitespace character.
#[inline]
#[must_use]
pub const fn is_whitespace(c: char) -> bool {
  matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// `Char ::= #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`
///
/// Characters outside this set may not appear in an XML document, not even via a character
/// reference.
///
/// # Examples
///
/// ```
/// use xylograph_core::chars::is_char;
///
/// assert!(is_char('\t'));
/// assert!(!is_char('\u{0}')); // NUL is never permitted
/// assert!(!is_char('\u{B}')); // nor is a vertical tab
/// assert!(is_char('\u{10FFFF}'));
/// ```
#[inline]
#[must_use]
pub const fn is_char(c: char) -> bool {
  matches!(c,
    '\u{9}' | '\u{A}' | '\u{D}'
    | '\u{20}'..='\u{D7FF}'
    | '\u{E000}'..='\u{FFFD}'
    | '\u{10000}'..='\u{10FFFF}')
}

/// True if `code` is a scalar value permitted by [`is_char`].
///
/// Character references are parsed as integers, so they have to be checked before a [`char`]
/// exists. Surrogate code points fail here rather than being replaced.
///
/// # Examples
///
/// ```
/// use xylograph_core::chars::is_char_code;
///
/// // `&#xD800;` is a well-formedness error, not a replacement character.
/// assert!(!is_char_code(0xD800));
/// assert!(is_char_code(0x3042));
/// ```
#[inline]
#[must_use]
pub const fn is_char_code(code: u32) -> bool {
  matches!(code,
    0x9 | 0xA | 0xD
    | 0x20..=0xD7FF
    | 0xE000..=0xFFFD
    | 0x10000..=0x10FFFF)
}

/// `NameStartChar` of XML 1.0 Fifth Edition, including `:`.
///
/// The colon is part of the XML production even though Namespaces in XML forbids it in an
/// [`NCName`](is_ncname_start_char); see [`is_ncname_start_char`].
#[inline]
#[must_use]
pub const fn is_name_start_char(c: char) -> bool {
  matches!(c, ':') || is_ncname_start_char(c)
}

/// `NameChar` of XML 1.0 Fifth Edition, including `:`.
#[inline]
#[must_use]
pub const fn is_name_char(c: char) -> bool {
  matches!(c, ':') || is_ncname_char(c)
}

/// `NCNameStartChar` — `NameStartChar` minus `:` (Namespaces in XML 1.0).
#[inline]
#[must_use]
pub const fn is_ncname_start_char(c: char) -> bool {
  matches!(c,
    'A'..='Z' | '_' | 'a'..='z'
    | '\u{C0}'..='\u{D6}'
    | '\u{D8}'..='\u{F6}'
    | '\u{F8}'..='\u{2FF}'
    | '\u{370}'..='\u{37D}'
    | '\u{37F}'..='\u{1FFF}'
    | '\u{200C}'..='\u{200D}'
    | '\u{2070}'..='\u{218F}'
    | '\u{2C00}'..='\u{2FEF}'
    | '\u{3001}'..='\u{D7FF}'
    | '\u{F900}'..='\u{FDCF}'
    | '\u{FDF0}'..='\u{FFFD}'
    | '\u{10000}'..='\u{EFFFF}')
}

/// `NCNameChar` — `NameChar` minus `:` (Namespaces in XML 1.0).
#[inline]
#[must_use]
pub const fn is_ncname_char(c: char) -> bool {
  is_ncname_start_char(c)
    || matches!(c,
      '-' | '.' | '0'..='9'
      | '\u{B7}'
      | '\u{300}'..='\u{36F}'
      | '\u{203F}'..='\u{2040}')
}

/// `PubidChar ::= #x20 | #xD | #xA | [a-zA-Z0-9] | [-'()+,./:=?;!*#@$_%]`
#[inline]
#[must_use]
pub const fn is_pubid_char(c: char) -> bool {
  matches!(c,
    ' ' | '\r' | '\n'
    | 'a'..='z' | 'A'..='Z' | '0'..='9'
    | '-' | '\'' | '(' | ')' | '+' | ',' | '.' | '/' | ':'
    | '=' | '?' | ';' | '!' | '*' | '#' | '@' | '$' | '_' | '%')
}

/// True if `s` matches `Name ::= NameStartChar (NameChar)*`.
///
/// A `Name` may contain colons; use [`is_ncname`] where Namespaces in XML applies.
///
/// # Examples
///
/// ```
/// use xylograph_core::chars::is_name;
///
/// assert!(is_name("item"));
/// assert!(is_name("要素")); // the Fifth Edition admits most of Unicode
/// assert!(is_name("xsl:template")); // a Name may contain a colon
/// assert!(!is_name("1st")); // a digit may not start one
/// assert!(!is_name(""));
/// ```
#[must_use]
pub fn is_name(s: &str) -> bool {
  let mut chars = s.chars();
  match chars.next() {
    Some(c) if is_name_start_char(c) => chars.all(is_name_char),
    _ => false,
  }
}

/// True if `s` matches `NCName ::= NameStartChar (NameChar)*` without colons.
///
/// # Examples
///
/// ```
/// use xylograph_core::chars::is_ncname;
///
/// assert!(is_ncname("template"));
/// assert!(!is_ncname("xsl:template")); // prefixes and local parts are checked separately
/// ```
#[must_use]
pub fn is_ncname(s: &str) -> bool {
  let mut chars = s.chars();
  match chars.next() {
    Some(c) if is_ncname_start_char(c) => chars.all(is_ncname_char),
    _ => false,
  }
}

/// True if `s` matches `EncName ::= [A-Za-z] ([A-Za-z0-9._] | '-')*`.
///
/// The names an XML declaration may give are a narrower set than the ones a decoder might
/// recognize: `" UTF-8"` names no encoding, whatever a lookup would make of it.
///
/// # Examples
///
/// ```
/// use xylograph_core::chars::is_enc_name;
///
/// assert!(is_enc_name("UTF-8"));
/// assert!(is_enc_name("ISO-8859-1"));
/// assert!(!is_enc_name(" UTF-8")); // no leading space
/// assert!(!is_enc_name("8859-1")); // must start with a letter
/// assert!(!is_enc_name(""));
/// ```
#[must_use]
pub fn is_enc_name(s: &str) -> bool {
  let mut chars = s.chars();
  matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// True if `s` matches `Nmtoken ::= (NameChar)+`.
#[must_use]
pub fn is_nmtoken(s: &str) -> bool {
  !s.is_empty() && s.chars().all(is_name_char)
}

/// True if every character of `s` is permitted by [`is_char`].
#[must_use]
pub fn is_char_data(s: &str) -> bool {
  s.chars().all(is_char)
}

/// True if `s` matches `PubidLiteral`'s character repertoire.
#[must_use]
pub fn is_pubid_literal(s: &str) -> bool {
  s.chars().all(is_pubid_char)
}

/// Splits a lexical QName into `(prefix, local-part)`, validating both parts.
///
/// Returns `None` if `s` is not a `QName ::= PrefixedName | UnprefixedName`. A name with more
/// than one colon, an empty prefix, or an empty local part is rejected.
///
/// # Examples
///
/// ```
/// use xylograph_core::chars::split_qname;
///
/// assert_eq!(split_qname("xsl:template"), Some((Some("xsl"), "template")));
/// assert_eq!(split_qname("template"), Some((None, "template")));
/// assert_eq!(split_qname("a:b:c"), None);
/// assert_eq!(split_qname(":local"), None);
/// ```
#[must_use]
pub fn split_qname(s: &str) -> Option<(Option<&str>, &str)> {
  match s.split_once(':') {
    None => is_ncname(s).then_some((None, s)),
    Some((prefix, local)) => (is_ncname(prefix) && is_ncname(local)).then_some((Some(prefix), local)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn char_range_boundaries() {
    assert!(!is_char('\u{0}'));
    assert!(!is_char('\u{8}'));
    assert!(is_char('\u{9}'));
    assert!(is_char('\u{A}'));
    assert!(!is_char('\u{B}'));
    assert!(!is_char('\u{C}'));
    assert!(is_char('\u{D}'));
    assert!(!is_char('\u{1F}'));
    assert!(is_char('\u{20}'));
    assert!(is_char('\u{D7FF}'));
    assert!(is_char('\u{E000}'));
    assert!(is_char('\u{FFFD}'));
    assert!(!is_char('\u{FFFE}'));
    assert!(!is_char('\u{FFFF}'));
    assert!(is_char('\u{10000}'));
    assert!(is_char('\u{10FFFF}'));
  }

  #[test]
  fn char_code_rejects_surrogates_and_out_of_range() {
    assert!(!is_char_code(0xD7FF + 1));
    assert!(!is_char_code(0xDC00));
    assert!(!is_char_code(0x110000));
    assert!(is_char_code(0x10FFFF));
  }

  #[test]
  fn name_start_char_boundaries() {
    // Fifth Edition ranges: the gaps matter more than the ranges themselves.
    assert!(!is_name_start_char('-'));
    assert!(!is_name_start_char('.'));
    assert!(!is_name_start_char('0'));
    assert!(is_name_start_char('_'));
    assert!(is_name_start_char(':'));
    assert!(!is_ncname_start_char(':'));
    assert!(!is_name_start_char('\u{BF}'));
    assert!(is_name_start_char('\u{C0}'));
    assert!(!is_name_start_char('\u{D7}')); // multiplication sign
    assert!(!is_name_start_char('\u{F7}')); // division sign
    assert!(!is_name_start_char('\u{37E}')); // greek question mark
    assert!(!is_name_start_char('\u{2000}'));
    assert!(!is_name_start_char('\u{3000}')); // ideographic space
    assert!(is_name_start_char('\u{3001}'));
    assert!(!is_name_start_char('\u{F0000}'));
  }

  #[test]
  fn name_char_adds_combining_and_digits() {
    assert!(is_name_char('-'));
    assert!(is_name_char('.'));
    assert!(is_name_char('5'));
    assert!(is_name_char('\u{B7}'));
    assert!(is_name_char('\u{300}'));
    assert!(is_name_char('\u{36F}'));
    assert!(is_name_char('\u{370}')); // also a NameStartChar
    assert!(is_name_char('\u{203F}'));
    assert!(!is_name_char('\u{2041}'));
  }

  #[test]
  fn names() {
    assert!(is_name("a"));
    assert!(is_name("a:b"));
    assert!(is_name(":a"));
    assert!(is_name("_-.0"));
    assert!(!is_name(""));
    assert!(!is_name("-a"));
    assert!(!is_name("0a"));
    assert!(!is_name("a b"));
    assert!(is_name("要素"));

    assert!(is_ncname("a"));
    assert!(!is_ncname("a:b"));
    assert!(!is_ncname(""));

    assert!(is_nmtoken("0"));
    assert!(is_nmtoken("-"));
    assert!(!is_nmtoken(""));
    assert!(!is_nmtoken("a b"));
  }

  #[test]
  fn qname_split() {
    assert_eq!(split_qname("a"), Some((None, "a")));
    assert_eq!(split_qname("p:a"), Some((Some("p"), "a")));
    assert_eq!(split_qname("p:a:b"), None);
    assert_eq!(split_qname(":a"), None);
    assert_eq!(split_qname("p:"), None);
    assert_eq!(split_qname(""), None);
  }

  #[test]
  fn pubid_chars() {
    assert!(is_pubid_literal("-//W3C//DTD XHTML 1.0 Strict//EN"));
    assert!(!is_pubid_literal("a\tb"));
    assert!(!is_pubid_literal("\"quoted\""));
  }
}
