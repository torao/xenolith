//! The lexical constraints the parser leaves to `StrictXmlValidator`, checked end to end: the parser delivers the
//! event, and the validator refuses it with the message and the position the parser used to give itself.

use xenolith::error::Error;
use xenolith::event::strict::StrictXmlValidator;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;

/// Reads `xml` through the strict validator and returns the violation it refused the document with, if any.
fn strict(xml: &str) -> Option<Error> {
  let mut validator = StrictXmlValidator::new();
  StreamSource::new(xml.as_bytes()).with_handler(&mut validator).emit().err()
}

fn refused(xml: &str) -> Error {
  strict(xml).expect("the strict validator refuses this")
}

#[test]
fn a_well_formed_document_passes() {
  assert!(strict("<a x='1'><!-- c --><?p d?><b:c xmlns:b='u'/></a>").is_none());
}

#[test]
fn duplicate_attributes_are_refused_only_when_the_expanded_names_are_the_same() {
  assert!(refused("<a x='1' x='2'/>").message().contains("appears twice"));
  // Different prefixes bound to the same namespace still collide.
  assert!(refused("<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>").message().contains("appears twice"));
  // The same local name in different namespaces does not.
  assert!(strict("<a xmlns:p='u' xmlns:q='v' p:x='1' q:x='2'/>").is_none());
}

#[test]
fn the_duplicate_attribute_is_located_at_the_attribute_that_repeats_it() {
  let error = refused("<a>\n  <b x='1' x='2'/>\n</a>");
  let at = error.location();
  assert_eq!((at.line, at.column), (2, 12), "the second `x`, not the tag it is in");
}

#[test]
fn an_attribute_name_that_is_not_a_qname_is_located_at_that_attribute() {
  let error = refused("<a b^c='1'/>");
  let at = error.location();
  assert_eq!((at.line, at.column), (1, 4), "where the name begins");
}

#[test]
fn a_fault_in_an_attribute_value_is_located_inside_the_value() {
  let error = refused("<a xml:space='maybe'/>");
  assert!(error.message().contains("xml:space"), "{error}");
  let at = error.location();
  assert_eq!((at.line, at.column), (1, 15), "the first character of the value, past the quotation mark");
}

#[test]
fn a_name_that_is_not_a_qname_is_refused_naming_the_fault() {
  assert!(matches!(refused("<a:b:c xmlns:a='u'/>"), Error::Namespace { .. }));
  assert!(refused("<a:b:c xmlns:a='u'/>").message().contains("more than one colon"));
  assert!(refused("<1a/>").message().contains("may not begin a name"));
  assert!(refused("<a b^c='1'/>").message().contains("'^'"));
}

#[test]
fn a_comment_holding_two_dashes_is_refused_at_the_dashes() {
  let error = refused("<a>\n  <!-- a -- b -->\n</a>");
  assert!(error.message().contains("--"));
  assert_eq!((error.location().line, error.location().column), (2, 10));
}

#[test]
fn a_comment_ending_in_a_dash_is_refused() {
  // The scanner closes the comment at the first `-->`, so `<!--a--->` is the body `a-` and then the close.
  assert!(refused("<a><!--a---></a>").message().contains("end with"));
}

#[test]
fn a_processing_instruction_target_that_is_not_a_name_is_refused() {
  assert!(refused("<a><?1a x?></a>").message().contains("processing instruction target"));
}
