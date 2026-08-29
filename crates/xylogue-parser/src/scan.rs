//! Finding token boundaries.
//!
//! Scanning is an operation that determines whether the text contains a complete token and, if so, how long it is. It
//! does not consume characters or interpret what it finds. If the text runs out and scanning is interrupted, it report
//! that "more input required," and when more text becomes available, it resumes from the same position.
//!

use xylogue_core::chars;
use xylogue_core::error::{Error, Result};

use crate::config::Bounds;

/// The type of detected token in its pre-parsed internal state.
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
  /// Character data up to the next `<` or `&`.
  Text,
  /// `&name;` or `&#...;`: an entity or character reference, delimiters included.
  Reference,
}

impl Token {
  /// A human-readable description of this token type for diagnostic purpose.
  ///
  fn describe(self) -> &'static str {
    match self {
      Token::Reference => "a reference",
      Token::Comment => "a comment",
      Token::CData => "a CDATA section",
      Token::Pi => "a processing instruction",
      Token::StartTag => "a start tag",
      Token::EndTag => "a end tag",
      Token::Doctype => "a document type declaration",
      Token::Text => "text",
    }
  }

  /// The byte size limit applied to this token type, and the corresponding field name in [`Bounds`].
  ///
  fn limit(self, bounds: &Bounds) -> (Option<usize>, &'static str) {
    match self {
      Token::Reference => (bounds.max_reference, "max_reference"),
      Token::Comment => (bounds.max_comment, "max_comment"),
      Token::CData => (bounds.max_cdata, "max_cdata"),
      Token::Pi => (bounds.max_pi, "max_pi"),
      Token::StartTag | Token::EndTag => (bounds.max_tag, "max_tag"),
      Token::Doctype => (bounds.max_doctype, "max_doctype"),
      Token::Text => (None, "text_fragment_len"),
    }
  }
}

/// Prefixes that may follow `<!`.
const MARKUP_PREFIXES: [&str; 3] = ["<!--", "<![CDATA[", "<!DOCTYPE"];

/// The result of [`scan`]. Either a complete token was found, or further input is required to complete the token.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scan {
  /// A complete token: Its type and the number of bytes from the start of `rest.
  Found(Token, usize),
  /// The data in the buffer was insufficient to complete the token. The caller should add more input and scan again.
  Pending,
}

impl Scan {
  /// If the scan is still [`Pending`](Self::Pending) and the buffer has exceeded the byte limit stored in `bounds` by
  /// [`token`](Token::limit), the result is converted into an ill-formed error.
  ///
  fn bounded(self, token: Token, rest: &str, bounds: &Bounds) -> Result<Self> {
    if let Scan::Pending = self {
      let (limit, field) = token.limit(bounds);
      if let Some(max) = limit {
        if rest.len() > max {
          return Err(Error::well_formedness(format!(
            "{} longer than {max} bytes was found; increase Bounds::{field} if the input is valid",
            token.describe()
          )));
        }
      }
    }
    Ok(self)
  }
}

/// Determines the token at the beginning of `rest`.
///
/// Returns [`Scan::Pending`] if `rest` contains only a portion of a token and `complete` is false. In this case, the
/// caller should feed more input and scans again. When `complete` is true, an error is raised indicating an incomplete
/// token.
///
pub(crate) fn scan(rest: &str, complete: bool, bounds: &Bounds) -> Result<Scan> {
  if rest.is_empty() {
    return Err(Error::internal("scan was called with an empty remainder"));
  }
  // Reference, &...;
  if rest.starts_with('&') {
    return scan_reference(rest, complete)?.bounded(Token::Reference, rest, bounds);
  }
  // Text
  if !rest.starts_with('<') {
    // Text has no bound: it is streamed in fragments, not buffered whole.
    return scan_text(rest, complete, bounds.text_fragment_len);
  }
  match rest.as_bytes().get(1) {
    None => incomplete(Token::StartTag, rest, complete),
    Some(b'?') => delimited(rest, Token::Pi, 2, "?>", complete)?.bounded(Token::Pi, rest, bounds),
    Some(b'/') => delimited(rest, Token::EndTag, 2, ">", complete)?.bounded(Token::EndTag, rest, bounds),
    Some(b'!') => scan_markup_declaration(rest, complete, bounds),
    _ => scan_start_tag(rest, complete)?.bounded(Token::StartTag, rest, bounds),
  }
}

/// Scans one of the constructs introduced by `<!`.
fn scan_markup_declaration(rest: &str, complete: bool, bounds: &Bounds) -> Result<Scan> {
  // Comment, <!-- ... -->
  if rest.starts_with(MARKUP_PREFIXES[0]) {
    return delimited(rest, Token::Comment, 4, "-->", complete)?.bounded(Token::Comment, rest, bounds);
  }
  // CData Section, <![CDATA[ ... ]]>
  if rest.starts_with(MARKUP_PREFIXES[1]) {
    return delimited(rest, Token::CData, 9, "]]>", complete)?.bounded(Token::CData, rest, bounds);
  }
  // DOCTYPE, <!DOCTYPE ... >
  if rest.starts_with(MARKUP_PREFIXES[2]) {
    let doctype = match scan_doctype(rest) {
      Some(len) => Scan::Found(Token::Doctype, len),
      None if complete => return Err(Error::well_formedness("the document type declaration is not closed")),
      None => Scan::Pending,
    };
    return doctype.bounded(Token::Doctype, rest, bounds);
  }
  // Still too short to tell which of them it is.
  if !complete && MARKUP_PREFIXES.iter().any(|p| p.starts_with(rest)) {
    Ok(Scan::Pending)
  } else {
    Err(Error::well_formedness(format!("{} is not markup", clip(rest, 10))))
  }
}

/// Scans a token that end with a specified delimiter. Starts searching from `from`.
///
fn delimited(rest: &str, token: Token, from: usize, terminator: &str, complete: bool) -> Result<Scan> {
  if rest.len() <= from {
    return incomplete(token, rest, complete);
  }
  match rest[from..].find(terminator) {
    Some(i) => Ok(Scan::Found(token, from + i + terminator.len())),
    None if complete => Err(Error::well_formedness(format!(
      "{} is not terminated by {terminator:?}: {}",
      token.describe(),
      clip(rest, 20)
    ))),
    None => Ok(Scan::Pending),
  }
}

/// Scans `<name ...>`. Ignores any `>` characters within the attribute values.
///
fn scan_start_tag(rest: &str, complete: bool) -> Result<Scan> {
  let mut quote = None;
  for (i, c) in rest.char_indices().skip(1) {
    if let Some(q) = quote {
      if c == q {
        quote = None;
      }
    } else {
      match c {
        '"' | '\'' => quote = Some(c),
        '>' => return Ok(Scan::Found(Token::StartTag, i + 1)),
        '<' => return Err(Error::well_formedness("'<' may not appear inside a tag")),
        _ => {}
      }
    }
  }
  incomplete(Token::StartTag, rest, complete)
}

/// Scans `<!DOCTYPE ... >`, allowing an internal subset in brackets.
///
/// Interpreting declarations is the responsibility of the DTD parser and is not performed here. Comments and processing
/// instructions are skipped as-is, so even if they contain apostrophes or square brackets, such as `<!--doesn't-->`,
/// they will not be mistaken for markup.
///
fn scan_doctype(rest: &str) -> Option<usize> {
  let mut i = 0;
  let mut quote: Option<char> = None;
  let mut depth = 0usize;
  while i < rest.len() {
    let tail = &rest[i..];
    if quote.is_none() {
      // Skip a comment or PI in the internal subset before reading its content as markup.
      if let Some(after) = tail.strip_prefix("<!--") {
        i += 4 + after.find("-->").map_or(after.len(), |j| j + 3);
        continue;
      }
      if let Some(after) = tail.strip_prefix("<?") {
        i += 2 + after.find("?>").map_or(after.len(), |j| j + 2);
        continue;
      }
    }
    let c = tail.chars().next()?;
    match (quote, c) {
      (Some(q), c) if c == q => quote = None,
      (Some(_), _) => {}
      (None, '"' | '\'') => quote = Some(c),
      (None, '[') => depth += 1,
      (None, ']') => depth = depth.saturating_sub(1),
      (None, '>') if depth == 0 => return Some(i + 1),
      (None, _) => {}
    }
    i += c.len_utf8();
  }
  None
}

/// Returns a token consist of a run of [`Text`](Token::Text) starting at the beginning of `rest`, or [`Scan::Pending`]
/// if waiting for more input.
///
/// A run continues until the next `<` or `&`, or until the end of the entity. If such a boundary is found within the
/// `rest`, the run terminates immediately before it. If no such boundary is found, and `complete` is true, the Text
/// extends to the end of the entity. Additionally, even if the entity has not reached its termination, the Text is
/// converted into a fragment once the run reaches `fragment_len` bytes. Otherwise, it returns [`Scan::Pending`] and
/// waits for further input.
///
fn scan_text(rest: &str, complete: bool, fragment_len: usize) -> Result<Scan> {
  match rest.find(['<', '&']) {
    // The caller has already verified that `rest` does not begin with `<` or `&`. To prevent returning a length of 0,
    // this returns an internal error.
    Some(0) => Err(Error::internal("scan_text ran on a text run that begins with '<' or '&'")),
    Some(i) => Ok(Scan::Found(Token::Text, i)),
    // Once input is complete, return the entire of `rest`. Otherwise, return the fragment as soon as it exceeds the
    // limited size, or hold it if it might be followed by `<` or `&`.
    None if complete => Ok(Scan::Found(Token::Text, rest.len())),
    None if rest.len() >= fragment_len => Ok(text_fragment(rest)),
    None => Ok(Scan::Pending),
  }
}

/// Returns the fragmented text boundary. This fragment does not contain `<` or `&`. If there is not text to fragment,
/// it returns [`Scan::Pending`].
///
/// XML 1.0 §2.4 defines the CharData must not contain `]]>`, so raw `]]>` cannot be placed within the body text (it
/// should be written as `]]&gt;`). Although it is the parser's responsibility to detect `]]>` in the Text, to ensure
/// that it is not split into fragments during scanning, if the `rest` ends with `]]` or `]`, it is required not to
/// include them in the fragment. If `rest` is `"]"` or `"]]"`, this function returns [`Scan::Pending`] to avoid
/// returning a fragment with 0-length (in other words, to prevent the progressing from stalling).
///
fn text_fragment(rest: &str) -> Scan {
  let held = rest.bytes().rev().take(2).take_while(|&b| b == b']').count();
  match rest.len() - held {
    0 => Scan::Pending,
    take => Scan::Found(Token::Text, take),
  }
}

/// Scans `&...;` to detect where the reference ends.
///
/// The scan simply detects the text from `&` to `;`. It is the parser's responsibility to perform strict validity
/// checks on the reference content (entity names or numeric portions). However, an error is raised if `<`, `&`, or a
/// space is detected in the reference.
///
fn scan_reference(rest: &str, complete: bool) -> Result<Scan> {
  let unterminated = || Error::well_formedness("a reference must end with \";\"");
  for (i, c) in rest.char_indices().skip(1) {
    if c == ';' {
      return Ok(Scan::Found(Token::Reference, i + 1));
    }
    if c == '<' || c == '&' || chars::is_whitespace(c) {
      return Err(unterminated());
    }
  }
  // The body ran out without a `;`: wait for more, or fail if the entity ends here.
  if complete { Err(unterminated()) } else { Ok(Scan::Pending) }
}

fn incomplete(token: Token, rest: &str, complete: bool) -> Result<Scan> {
  if complete {
    let message = format!("the entity has ended within {}: {}", token.describe(), clip(rest, 20));
    return Err(Error::well_formedness(message));
  }
  Ok(Scan::Pending)
}

/// For error messages, truncate `s` to `max_chars` characters and enclose `s` in quotes.
///
/// If the length of `s` exceeds `max_chars`, an ellipsis `...` is shown to indicate that there are omitted characters
/// following the first `max_chars` characters (e.g., `"abc..."`).
///
fn clip(s: &str, max_chars: usize) -> String {
  match s.char_indices().nth(max_chars) {
    Some((end, _)) => format!("{:?}", format!("{}…", &s[..end])),
    None => format!("{s:?}"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const FRAG: usize = 8 * 1024;
  const UNBOUNDED: Bounds = Bounds {
    max_reference: None,
    max_comment: None,
    max_cdata: None,
    max_pi: None,
    max_tag: None,
    max_doctype: None,
    text_fragment_len: FRAG,
  };

  fn complete(text: &str) -> (Token, usize) {
    match scan(text, true, &UNBOUNDED).expect("scans") {
      Scan::Found(token, len) => (token, len),
      Scan::Pending => panic!("{text:?} is not a complete token"),
    }
  }

  fn partial(text: &str) -> Option<(Token, usize)> {
    match scan(text, false, &UNBOUNDED).expect("scans") {
      Scan::Found(token, len) => Some((token, len)),
      Scan::Pending => None,
    }
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
      assert!(scan(text, true, &UNBOUNDED).is_err(), "{text:?} should fail at end of entity");
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
  fn a_short_run_of_text_is_held_until_it_is_whole() {
    // Without a following `<` or `&` a short run may still grow, so nothing is taken yet.
    assert_eq!(partial("hello"), None);
    assert_eq!(partial("a]]"), None);
    // At the end of the entity there is nothing more to wait for.
    assert_eq!(complete("hello"), (Token::Text, 5));
    assert_eq!(complete("a]]"), (Token::Text, 3));
  }

  #[test]
  fn a_long_run_of_text_is_emitted_in_fragments() {
    // A run shorter than the threshold is still held whole while it might grow.
    assert_eq!(partial(&"a".repeat(FRAG - 1)), None);
    // A run that reaches the threshold is emitted without waiting for a following `<` or `&`, so an
    // endless run cannot make the stream buffer without limit.
    assert_eq!(partial(&"a".repeat(FRAG)), Some((Token::Text, FRAG)));
    // A `<` still bounds the fragment when one is present.
    let bounded = format!("{}<a/>", "a".repeat(FRAG));
    assert_eq!(partial(&bounded), Some((Token::Text, FRAG)));
  }

  #[test]
  fn the_text_fragment_threshold_is_configurable() {
    // A smaller threshold fragments a shorter run; below it, the run is still held.
    let bounds = Bounds::default().with_text_fragment_len(16);
    assert_eq!(scan(&"a".repeat(15), false, &bounds).unwrap(), Scan::Pending);
    assert_eq!(scan(&"a".repeat(16), false, &bounds).unwrap(), Scan::Found(Token::Text, 16));
  }

  #[test]
  fn a_tiny_fragment_len_never_emits_an_empty_text_token() {
    // With a 1-byte threshold, a run of only `]` must not fragment into a zero-length token (which would
    // make the parser loop): the trailing `]` are held, so nothing is emitted until more arrives.
    let bounds = Bounds::default().with_text_fragment_len(1);
    assert_eq!(scan("]", false, &bounds).unwrap(), Scan::Pending);
    assert_eq!(scan("]]", false, &bounds).unwrap(), Scan::Pending);
    // Once the run is long enough to hold two and still emit one, it makes progress.
    assert_eq!(scan("]]]", false, &bounds).unwrap(), Scan::Found(Token::Text, 1));
    // At the end of input the whole run is taken, since no `>` can follow.
    assert_eq!(scan("]]", true, &bounds).unwrap(), Scan::Found(Token::Text, 2));
  }

  #[test]
  fn a_text_fragment_never_ends_inside_a_forbidden_sequence() {
    // Up to two trailing `]` are held back, so a `>` in the next feed cannot complete `]]>` across the
    // split. The fragment stops before them.
    let one = format!("{}]", "a".repeat(FRAG));
    assert_eq!(partial(&one), Some((Token::Text, FRAG)));
    let two = format!("{}]]", "a".repeat(FRAG));
    assert_eq!(partial(&two), Some((Token::Text, FRAG)));
    // A run of only `]` still makes progress, since at most two are ever held.
    assert_eq!(partial(&"]".repeat(FRAG + 2)), Some((Token::Text, FRAG)));
  }

  #[test]
  fn text_stops_before_a_reference() {
    assert_eq!(complete("ab&amp;cd"), (Token::Text, 2));
    assert_eq!(complete("&amp;cd"), (Token::Reference, 5));
    assert_eq!(complete("&#x41;"), (Token::Reference, 6));
    // Incomplete references wait for their ';'.
    assert_eq!(partial("&am"), None);
    assert_eq!(partial("&"), None);
    // A '<' or a second '&' before the ';' is a bare ampersand, not a reference.
    assert!(scan("&amp cd", false, &UNBOUNDED).is_err());
    assert!(scan("&foo<", false, &UNBOUNDED).is_err());
  }

  #[test]
  fn scan_delimits_a_reference_but_leaves_its_content_to_the_parser() {
    // A reference is delimited at its ';', whatever the content — the parser validates the name or the
    // character-reference digits.
    assert_eq!(complete("&#65;"), (Token::Reference, 5));
    assert_eq!(complete("&#x41;"), (Token::Reference, 6));
    assert_eq!(complete("&amp;"), (Token::Reference, 5));
    // A wrong-radix digit, an uppercase `X`, or an invalid name is still delimited as a reference here.
    assert_eq!(complete("&#4a;"), (Token::Reference, 5));
    assert_eq!(complete("&#X58;"), (Token::Reference, 6));
    assert_eq!(complete("&123;"), (Token::Reference, 5));
  }

  #[test]
  fn a_reference_is_bounded_only_when_max_reference_is_set() {
    // By default nothing bounds a reference: an unterminated one just waits for more input.
    let long = format!("&{}", "a".repeat(1000));
    assert_eq!(partial(&long), None);
    // With a bound, a reference that grows past it is rejected before its `;` arrives.
    let bounds = Bounds::default().with_max_reference(64);
    let err = scan(&long, false, &bounds).unwrap_err();
    assert!(err.to_string().contains("Bounds::max_reference"), "{err}");
  }

  #[test]
  fn a_stray_less_than_inside_a_tag_is_rejected() {
    assert!(scan("<a <b>", false, &UNBOUNDED).is_err());
  }

  #[test]
  fn unknown_markup_is_rejected_once_it_is_long_enough_to_tell() {
    assert!(scan("<!x", false, &UNBOUNDED).is_err());
    assert!(scan("<!-x", false, &UNBOUNDED).is_err());
    assert!(scan("<![CD@TA[", false, &UNBOUNDED).is_err());
  }

  #[test]
  fn clip_quotes_and_marks_only_a_real_cut() {
    // Shorter than the limit: quoted whole, no ellipsis.
    assert_eq!(clip("hello", 20), "\"hello\"");
    // Exactly the limit: still whole, no ellipsis.
    assert_eq!(clip("hello", 5), "\"hello\"");
    // Longer: cut to the limit, with the ellipsis inside the quotes.
    assert_eq!(clip("hello!", 5), "\"hello…\"");
    // The cut lands on a character boundary.
    assert_eq!(clip("あいうえお", 2), "\"あい…\"");
  }
}
