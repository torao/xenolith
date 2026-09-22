use super::*;

const FRAG: usize = 8 * 1024;
const UNBOUNDED: TokenLimits = TokenLimits::unlimited();

fn complete(text: &str) -> Scan {
  let scanned = scan(text, true, &UNBOUNDED, FRAG).expect("scans");
  assert_ne!(scanned, Scan::Pending, "{text:?} is not a complete token");
  scanned
}

fn partial(text: &str) -> Option<Scan> {
  match scan(text, false, &UNBOUNDED, FRAG).expect("scans") {
    Scan::Pending => None,
    scanned => Some(scanned),
  }
}

#[test]
fn recognizes_every_token() {
  assert_eq!(complete("<a>rest"), Scan::Found(TokenKind::StartElement, 3));
  assert_eq!(complete("<a/>rest"), Scan::Found(TokenKind::StartElement, 4));
  assert_eq!(complete("</a>rest"), Scan::Found(TokenKind::EndElement, 4));
  assert_eq!(complete("<!--c-->rest"), Scan::Found(TokenKind::Comment, 8));
  assert_eq!(complete("<![CDATA[x]]>rest"), Scan::Found(TokenKind::CData, 13));
  assert_eq!(complete("<?pi?>rest"), Scan::Found(TokenKind::ProcessingInstruction, 6));
  assert_eq!(complete("<?xml version='1.0'?>rest"), Scan::Found(TokenKind::ProcessingInstruction, 21));
  assert_eq!(complete("<!DOCTYPE a>rest"), Scan::Found(TokenKind::Doctype, 12));
  assert_eq!(complete("text<a/>"), Scan::Found(TokenKind::Text, 4));
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
    assert!(scan(text, true, &UNBOUNDED, FRAG).is_err(), "{text:?} should fail at end of entity");
  }
}

#[test]
fn a_tag_may_contain_delimiters_inside_attribute_values() {
  assert_eq!(complete("<a b='>'>rest"), Scan::Found(TokenKind::StartElement, 9));
  assert_eq!(complete("<a b=\"'>'\">"), Scan::Found(TokenKind::StartElement, 11));
  // An unterminated quote swallows the rest, so the tag is not complete.
  assert_eq!(partial("<a b='>"), None);
}

#[test]
fn a_doctype_may_contain_an_internal_subset() {
  assert_eq!(complete("<!DOCTYPE a [<!ENTITY e 'v'>]>rest"), Scan::Found(TokenKind::Doctype, 30));
  assert_eq!(complete("<!DOCTYPE a SYSTEM 'a>b'>"), Scan::Found(TokenKind::Doctype, 25));
  assert_eq!(partial("<!DOCTYPE a [<!ENTITY e 'v'>"), None);
}

#[test]
fn text_stops_at_the_next_markup() {
  assert_eq!(complete("hello<a/>"), Scan::Found(TokenKind::Text, 5));
  assert_eq!(partial("hello<"), Some(Scan::Found(TokenKind::Text, 5)));
}

#[test]
fn a_short_run_of_text_is_held_until_it_is_whole() {
  // Without a following `<` or `&` a short run may still grow, so nothing is taken yet.
  assert_eq!(partial("hello"), None);
  assert_eq!(partial("a]]"), None);
  // At the end of the entity there is nothing more to wait for.
  assert_eq!(complete("hello"), Scan::Found(TokenKind::Text, 5));
  assert_eq!(complete("a]]"), Scan::Found(TokenKind::Text, 3));
}

#[test]
fn a_long_run_of_text_is_emitted_in_fragments() {
  // A run shorter than the threshold is still held whole while it might grow.
  assert_eq!(partial(&"a".repeat(FRAG - 1)), None);
  // A run that reaches the threshold is emitted without waiting for a following `<` or `&`, so an
  // endless run cannot make the stream buffer without limit.
  assert_eq!(partial(&"a".repeat(FRAG)), Some(Scan::Found(TokenKind::Text, FRAG)));
  // A `<` still bounds the fragment when one is present.
  let bounded = format!("{}<a/>", "a".repeat(FRAG));
  assert_eq!(partial(&bounded), Some(Scan::Found(TokenKind::Text, FRAG)));
}

#[test]
fn the_text_fragment_threshold_is_configurable() {
  // A smaller threshold fragments a shorter run; below it, the run is still held.
  assert_eq!(scan(&"a".repeat(15), false, &UNBOUNDED, 16).unwrap(), Scan::Pending);
  assert_eq!(scan(&"a".repeat(16), false, &UNBOUNDED, 16).unwrap(), Scan::Found(TokenKind::Text, 16));
}

#[test]
fn a_tiny_fragment_len_never_emits_an_empty_text_token() {
  // With a 1-byte threshold, a run of only `]` must not fragment into a zero-length token (which would
  // make the parser loop): the trailing `]` are held, so nothing is emitted until more arrives.
  assert_eq!(scan("]", false, &UNBOUNDED, 1).unwrap(), Scan::Pending);
  assert_eq!(scan("]]", false, &UNBOUNDED, 1).unwrap(), Scan::Pending);
  // Once the run is long enough to hold two and still emit one, it makes progress.
  assert_eq!(scan("]]]", false, &UNBOUNDED, 1).unwrap(), Scan::Found(TokenKind::Text, 1));
  // At the end of input the whole run is taken, since no `>` can follow.
  assert_eq!(scan("]]", true, &UNBOUNDED, 1).unwrap(), Scan::Found(TokenKind::Text, 2));
}

#[test]
fn a_text_fragment_never_ends_inside_a_forbidden_sequence() {
  // Up to two trailing `]` are held back, so a `>` in the next feed cannot complete `]]>` across the
  // split. The fragment stops before them.
  let one = format!("{}]", "a".repeat(FRAG));
  assert_eq!(partial(&one), Some(Scan::Found(TokenKind::Text, FRAG)));
  let two = format!("{}]]", "a".repeat(FRAG));
  assert_eq!(partial(&two), Some(Scan::Found(TokenKind::Text, FRAG)));
  // A run of only `]` still makes progress, since at most two are ever held.
  assert_eq!(partial(&"]".repeat(FRAG + 2)), Some(Scan::Found(TokenKind::Text, FRAG)));
}

#[test]
fn text_stops_before_a_reference() {
  assert_eq!(complete("ab&amp;cd"), Scan::Found(TokenKind::Text, 2));
  assert_eq!(complete("&amp;cd"), Scan::Reference(5));
  assert_eq!(complete("&#x41;"), Scan::Reference(6));
  // Incomplete references wait for their ';'.
  assert_eq!(partial("&am"), None);
  assert_eq!(partial("&"), None);
  // A '<' or a second '&' before the ';' is a bare ampersand, not a reference.
  assert!(scan("&amp cd", false, &UNBOUNDED, FRAG).is_err());
  assert!(scan("&foo<", false, &UNBOUNDED, FRAG).is_err());
}

#[test]
fn scan_delimits_a_reference_but_leaves_its_content_to_the_parser() {
  // A reference is delimited at its ';', whatever the content — the parser validates the name or the
  // character-reference digits.
  assert_eq!(complete("&#65;"), Scan::Reference(5));
  assert_eq!(complete("&#x41;"), Scan::Reference(6));
  assert_eq!(complete("&amp;"), Scan::Reference(5));
  // A wrong-radix digit, an uppercase `X`, or an invalid name is still delimited as a reference here.
  assert_eq!(complete("&#4a;"), Scan::Reference(5));
  assert_eq!(complete("&#X58;"), Scan::Reference(6));
  assert_eq!(complete("&123;"), Scan::Reference(5));
}

#[test]
fn a_reference_is_bounded_only_when_max_reference_is_set() {
  // By default nothing bounds a reference: an unterminated one just waits for more input.
  let long = format!("&{}", "a".repeat(1000));
  assert_eq!(partial(&long), None);
  // With a bound, a reference that grows past it is rejected before its `;` arrives.
  let bounds = TokenLimits { max_reference: Some(64), ..TokenLimits::default() };
  let err = scan(&long, false, &bounds, FRAG).unwrap_err();
  assert!(err.to_string().contains("limits.tokens.max_reference"), "{err}");
}

#[test]
fn a_stray_less_than_inside_a_tag_is_rejected() {
  assert!(scan("<a <b>", false, &UNBOUNDED, FRAG).is_err());
}

#[test]
fn unknown_markup_is_rejected_once_it_is_long_enough_to_tell() {
  assert!(scan("<!x", false, &UNBOUNDED, FRAG).is_err());
  assert!(scan("<!-x", false, &UNBOUNDED, FRAG).is_err());
  assert!(scan("<![CD@TA[", false, &UNBOUNDED, FRAG).is_err());
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
