//! Finding token boundaries.
//!
//! Scanning answers one question: does the text hold a complete token, and how long is it?
//! It never consumes anything and never interprets what it finds, so a scan that runs out of
//! text simply reports that more input is needed and is retried from the same place once the
//! text has grown. That is what makes the parser resumable at any byte boundary.

use xylograph_core::error::{Error, ErrorKind, Result};

/// The kind of token found, before it is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Token {
  /// `<?target data?>`, which includes the XML declaration.
  Pi,
  /// `<!-- ... -->`
  Comment,
  /// `<![CDATA[ ... ]]>`
  CData,
  /// `<!DOCTYPE ... >`
  Doctype,
  /// `<name ...>` or `<name .../>`
  StartTag,
  /// `</name>`
  EndTag,
  /// Character data up to the next `<`.
  Text,
}

/// Prefixes that may follow `<!`.
const MARKUP_PREFIXES: [&str; 3] = ["<!--", "<![CDATA[", "<!DOCTYPE"];

/// Finds the token at the start of `rest`.
///
/// Returns `Ok(None)` when `rest` holds only part of a token and `complete` is false, meaning
/// the caller should feed more input and scan again. `complete` says that `rest` is all the
/// text the entity will ever have, which turns a partial token into an error.
pub(crate) fn scan(rest: &str, complete: bool) -> Result<Option<(Token, usize)>> {
  debug_assert!(!rest.is_empty());
  if !rest.starts_with('<') {
    return Ok(scan_text(rest, complete).map(|len| (Token::Text, len)));
  }
  match rest.as_bytes().get(1) {
    None => incomplete("<", complete),
    Some(b'?') => delimited(rest, Token::Pi, 2, "?>", complete),
    Some(b'/') => delimited(rest, Token::EndTag, 2, ">", complete),
    Some(b'!') => scan_markup_declaration(rest, complete),
    _ => scan_start_tag(rest, complete),
  }
}

/// Scans one of the constructs introduced by `<!`.
fn scan_markup_declaration(rest: &str, complete: bool) -> Result<Option<(Token, usize)>> {
  if rest.starts_with(MARKUP_PREFIXES[0]) {
    return delimited(rest, Token::Comment, 4, "-->", complete);
  }
  if rest.starts_with(MARKUP_PREFIXES[1]) {
    return delimited(rest, Token::CData, 9, "]]>", complete);
  }
  if rest.starts_with(MARKUP_PREFIXES[2]) {
    return Ok(scan_doctype(rest).map(|len| (Token::Doctype, len)));
  }
  // Still too short to tell which of them it is.
  if !complete && MARKUP_PREFIXES.iter().any(|p| p.starts_with(rest)) {
    return Ok(None);
  }
  Err(Error::new(ErrorKind::WellFormedness, format!("{} is not markup", clip(rest, 10))))
}

/// Scans a token that ends at a fixed delimiter, searching from `from`.
fn delimited(
  rest: &str,
  token: Token,
  from: usize,
  terminator: &str,
  complete: bool,
) -> Result<Option<(Token, usize)>> {
  if rest.len() <= from {
    return incomplete(rest, complete);
  }
  match rest[from..].find(terminator) {
    Some(i) => Ok(Some((token, from + i + terminator.len()))),
    None if complete => {
      Err(Error::new(ErrorKind::WellFormedness, format!("{} is not terminated by {terminator:?}", clip(rest, 20))))
    }
    None => Ok(None),
  }
}

/// Scans `<name ...>`, ignoring `>` inside attribute values.
fn scan_start_tag(rest: &str, complete: bool) -> Result<Option<(Token, usize)>> {
  let mut quote = None;
  for (i, c) in rest.char_indices().skip(1) {
    match (quote, c) {
      (Some(q), c) if c == q => quote = None,
      (Some(_), _) => {}
      (None, '"' | '\'') => quote = Some(c),
      (None, '>') => return Ok(Some((Token::StartTag, i + 1))),
      (None, '<') => {
        return Err(Error::new(ErrorKind::WellFormedness, "'<' may not appear inside a tag"));
      }
      (None, _) => {}
    }
  }
  incomplete(rest, complete)
}

/// Scans `<!DOCTYPE ... >`, allowing an internal subset in brackets.
///
/// The declaration is not interpreted here; that arrives with DTD support in phase 2.
fn scan_doctype(rest: &str) -> Option<usize> {
  let mut quote = None;
  let mut depth = 0usize;
  for (i, c) in rest.char_indices() {
    match (quote, c) {
      (Some(q), c) if c == q => quote = None,
      (Some(_), _) => {}
      (None, '"' | '\'') => quote = Some(c),
      (None, '[') => depth += 1,
      (None, ']') => depth = depth.saturating_sub(1),
      (None, '>') if depth == 0 => return Some(i + 1),
      (None, _) => {}
    }
  }
  None
}

/// Finds how much character data can be taken from the front of `rest`.
///
/// A run of character data is only ever taken whole: up to the next `<`, or to the end of the
/// entity. Emitting it piecewise would be cheaper, but the XPath data model requires text
/// nodes to be maximal, so a run split by the arrival of input would have to be stitched back
/// together by every caller. It also means the events a document produces do not depend on
/// how its bytes were divided.
fn scan_text(rest: &str, complete: bool) -> Option<usize> {
  match rest.find('<') {
    Some(i) => (i > 0).then_some(i),
    None => complete.then_some(rest.len()),
  }
}

fn incomplete(rest: &str, complete: bool) -> Result<Option<(Token, usize)>> {
  if complete {
    return Err(Error::new(ErrorKind::WellFormedness, format!("the entity ends inside {}", clip(rest, 20))));
  }
  Ok(None)
}

/// Shortens text for an error message, on a character boundary.
fn clip(s: &str, max_chars: usize) -> String {
  match s.char_indices().nth(max_chars) {
    Some((i, _)) => format!("{:?}...", &s[..i]),
    None => format!("{s:?}"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn complete(text: &str) -> (Token, usize) {
    scan(text, true).expect("scans").expect("is complete")
  }

  fn partial(text: &str) -> Option<(Token, usize)> {
    scan(text, false).expect("scans")
  }

  #[test]
  fn recognizes_every_token() {
    assert_eq!(complete("<a>rest"), (Token::StartTag, 3));
    assert_eq!(complete("<a/>rest"), (Token::StartTag, 4));
    assert_eq!(complete("</a>rest"), (Token::EndTag, 4));
    assert_eq!(complete("<!--c-->rest"), (Token::Comment, 8));
    assert_eq!(complete("<![CDATA[x]]>rest"), (Token::CData, 13));
    assert_eq!(complete("<?pi?>rest"), (Token::Pi, 6));
    assert_eq!(complete("<?xml version='1.0'?>rest"), (Token::Pi, 21));
    assert_eq!(complete("<!DOCTYPE a>rest"), (Token::Doctype, 12));
    assert_eq!(complete("text<a/>"), (Token::Text, 4));
  }

  #[test]
  fn a_partial_token_asks_for_more_input() {
    for text in ["<", "<a", "<a ", "</", "</a", "<!", "<!-", "<!--", "<!-- c", "<!-- c--", "<?", "<?pi", "<?pi?"] {
      assert_eq!(partial(text), None, "{text:?} should be incomplete");
    }
    for text in ["<![", "<![CDATA[", "<![CDATA[x]]"] {
      assert_eq!(partial(text), None, "{text:?} should be incomplete");
    }
    assert_eq!(partial("<!DOCTYPE a"), None);
  }

  #[test]
  fn a_partial_token_at_the_end_of_the_entity_is_an_error() {
    for text in ["<", "<a", "<!-- c", "<?pi", "<![CDATA[x", "<!D"] {
      assert!(scan(text, true).is_err(), "{text:?} should fail at end of entity");
    }
  }

  #[test]
  fn a_tag_may_contain_delimiters_inside_attribute_values() {
    assert_eq!(complete("<a b='>'>rest"), (Token::StartTag, 9));
    assert_eq!(complete("<a b=\"'>'\">"), (Token::StartTag, 11));
    // An unterminated quote swallows the rest, so the tag is not complete.
    assert_eq!(partial("<a b='>"), None);
  }

  #[test]
  fn a_doctype_may_contain_an_internal_subset() {
    assert_eq!(complete("<!DOCTYPE a [<!ENTITY e 'v'>]>rest"), (Token::Doctype, 30));
    assert_eq!(complete("<!DOCTYPE a SYSTEM 'a>b'>"), (Token::Doctype, 25));
    assert_eq!(partial("<!DOCTYPE a [<!ENTITY e 'v'>"), None);
  }

  #[test]
  fn text_stops_at_the_next_markup() {
    assert_eq!(complete("hello<a/>"), (Token::Text, 5));
    assert_eq!(partial("hello<"), Some((Token::Text, 5)));
  }

  #[test]
  fn a_run_of_text_is_only_taken_whole() {
    // Without a following `<` the run may still grow, so nothing is taken yet: text nodes
    // have to be maximal, and a reference or a "]]>" could straddle the boundary.
    assert_eq!(partial("hello"), None);
    assert_eq!(partial("a&am"), None);
    assert_eq!(partial("a]]"), None);
    // At the end of the entity there is nothing more to wait for.
    assert_eq!(complete("hello"), (Token::Text, 5));
    assert_eq!(complete("a]]"), (Token::Text, 3));
  }

  #[test]
  fn a_stray_less_than_inside_a_tag_is_rejected() {
    assert!(scan("<a <b>", false).is_err());
  }

  #[test]
  fn unknown_markup_is_rejected_once_it_is_long_enough_to_tell() {
    assert!(scan("<!x", false).is_err());
    assert!(scan("<!-x", false).is_err());
    assert!(scan("<![CD@TA[", false).is_err());
  }
}
