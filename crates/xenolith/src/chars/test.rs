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
  assert!(is_pubid_literal(""));
  assert!(!is_pubid_literal("a\tb"));
  assert!(!is_pubid_literal("\"quoted\""));
}
