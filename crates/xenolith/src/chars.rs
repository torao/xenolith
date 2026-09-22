//! Character classes of XML 1.0 (Fifth Edition).
//!
//! In the 5th edition, `Name` is redefined using broad Unicode ranges, replacing the enumerated character classes used
//! in the 4th edition. Note that all descriptions here comply with the 5th edition, and that the two editions are *not
//! interchangeable*.
//!

#[cfg(test)]
mod test;

/// `S ::= (#x20 | #x9 | #xD | #xA)+`
///
/// A single XML whitespace character.
#[inline]
#[must_use]
pub const fn is_whitespace(c: char) -> bool {
  matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// `Char ::= #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`
///
/// Evaluates whether a character is recognized as a valid character in XML. Characters not included in this set must
/// not appear in an XML document, even via character references.
///
/// To validate a value as a code point (character code), use [`char::from_u32`]. This returns `None` as a "format
/// error" for surrogates or values exceeding `0x10FFFF`. You can then perform further validation on values that pass
/// this check.
///
/// # Examples
///
/// ```
/// use xenolith::chars::is_char;
///
/// assert!(is_char('\t'));
/// assert!(!is_char('\u{0}')); // NUL is never permitted
/// assert!(!is_char('\u{B}')); // nor is a vertical tab
/// assert!(is_char('\u{10FFFF}'));
///
/// // `&#xD800;` refers to a surrogate, which is not a character at all.
/// assert!(char::from_u32(0xD800).is_none());
/// ```
#[inline]
#[must_use]
pub const fn is_char(c: char) -> bool {
  matches!(c, '\u{9}' | '\u{A}' | '\u{D}'
  | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// `NameStartChar` of XML 1.0 Fifth Edition, including `:`.
///
/// The colon is part of the XML production even though Namespaces in XML forbid it in an
/// [`NCName`](is_ncname_start_char); see [`is_ncname_start_char`].
#[inline]
#[must_use]
pub const fn is_name_start_char(c: char) -> bool {
  matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z'
    | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
    | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
    | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// `NameChar` of XML 1.0 Fifth Edition, including `:`.
#[inline]
#[must_use]
pub const fn is_name_char(c: char) -> bool {
  is_name_start_char(c)
    || matches!(c, '-' | '.' | '0'..='9'
      | '\u{B7}'
      | '\u{300}'..='\u{36F}'
      | '\u{203F}'..='\u{2040}')
}

/// `NCNameStartChar` — `NameStartChar` minus `:` (Namespaces in XML 1.0).
#[inline]
#[must_use]
pub const fn is_ncname_start_char(c: char) -> bool {
  c != ':' && is_name_start_char(c)
}

/// `NCNameChar` — `NameChar` minus `:` (Namespaces in XML 1.0).
#[inline]
#[must_use]
pub const fn is_ncname_char(c: char) -> bool {
  c != ':' && is_name_char(c)
}

/// `PubidChar ::= #x20 | #xD | #xA | [a-zA-Z0-9] | [-'()+,./:=?;!*#@$_%]`
#[inline]
#[must_use]
pub const fn is_pubid_char(c: char) -> bool {
  matches!(c, ' ' | '\r' | '\n' | 'a'..='z' | 'A'..='Z' | '0'..='9'
    | '-' | '\'' | '(' | ')' | '+' | ',' | '.' | '/' | ':' | '=' | '?' | ';' | '!' | '*' | '#' | '@' | '$' | '_' | '%')
}

/// True if `s` matches the contents of the `PubidLiteral`, excluding the quotation marks.
#[must_use]
pub fn is_pubid_literal(s: &str) -> bool {
  s.chars().all(is_pubid_char)
}

/// True if `s` matches `Name ::= NameStartChar (NameChar)*`.
///
/// A `Name` may contain colons; use [`is_ncname`] where Namespaces in XML applies.
///
/// # Examples
///
/// ```
/// use xenolith::chars::is_name;
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
  matches!(chars.next(), Some(c) if is_name_start_char(c)) && chars.all(is_name_char)
}

/// True if `s` matches `NCName ::= NameStartChar (NameChar)*` without colons.
///
/// # Examples
///
/// ```
/// use xenolith::chars::is_ncname;
///
/// assert!(is_ncname("template"));
/// assert!(!is_ncname("xsl:template")); // prefixes and local parts are checked separately
/// ```
#[must_use]
pub fn is_ncname(s: &str) -> bool {
  let mut chars = s.chars();
  matches!(chars.next(), Some(c) if is_ncname_start_char(c)) && chars.all(is_ncname_char)
}

/// True if `s` matches `EncName ::= [A-Za-z] ([A-Za-z0-9._] | '-')*`.
///
/// Validates the encoding names that can be specified in an XML declaration.
///
/// # Examples
///
/// ```
/// use xenolith::chars::is_enc_name;
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

/// Splits a lexical QName into `(prefix, local-part)`, validating both parts.
///
/// Returns `None` if `s` is not a `QName ::= PrefixedName | UnprefixedName`. A name with more than one colon, an empty
/// prefix, or an empty local part is rejected.
///
/// # Examples
///
/// ```
/// use xenolith::chars::split_qname;
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
