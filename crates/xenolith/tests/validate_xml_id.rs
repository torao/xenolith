//! xml:id checking: a valid NCName, unique across the document, with and without a DTD.

mod common;
use common::validate_checking_xml_id as validate;

/// The validity-error messages from validating `xml`, which must be well-formed.
fn errors(xml: &str) -> Vec<String> {
  let report = validate(xml.as_bytes()).expect("well-formed");
  report.errors().iter().map(|e| e.message().to_owned()).collect()
}

#[test]
fn unique_ncname_xml_ids_are_accepted() {
  assert!(errors("<a xml:id='x1'><b xml:id='x2'/></a>").is_empty());
}

#[test]
fn a_duplicate_xml_id_is_reported() {
  let errs = errors("<a xml:id='x'><b xml:id='x'/></a>");
  assert_eq!(errs.len(), 1, "{errs:?}");
  assert!(errs[0].contains("more than once"), "{errs:?}");
}

#[test]
fn an_xml_id_that_is_not_an_ncname_is_reported() {
  // A leading digit and a colon each disqualify an NCName.
  assert!(errors("<a xml:id='1bad'/>").iter().any(|e| e.contains("NCName")));
  assert!(errors("<a xml:id='p:q'/>").iter().any(|e| e.contains("NCName")));
}

#[test]
fn whitespace_around_an_xml_id_is_normalized_before_checking() {
  // Tokenized normalization trims the value, so this is the NCName "x", not " x ".
  assert!(errors("<a xml:id='  x  '/>").is_empty());
}

#[test]
fn an_xml_id_from_a_source_that_did_not_normalize_it_is_normalized_before_checking() {
  use xenolith::attr::{Attribute, Attributes};
  use xenolith::event::validate::{Validator, XmlIdValidator};
  use xenolith::event::{EventHandler, EventRef, StartElementEventRef, XmlSpace};
  use xenolith::{Location, XML_NS_URI};

  let mut ids = XmlIdValidator::new();
  // The first value holds a tab and a line end around the name, which only normalization removes.
  for value in ["\t x \n", "x"] {
    let attributes = vec![Attribute {
      prefix: Some("xml".into()),
      local: "id".into(),
      namespace: Some(XML_NS_URI.into()),
      value: value.into(),
      location: Location::unknown(),
      value_location: Location::unknown(),
    }];
    let event = EventRef::StartElement(StartElementEventRef::new(
      None,
      "a",
      None,
      Attributes::new(&attributes),
      XmlSpace::Default,
      None,
      None,
      Location::unknown(),
    ));
    ids.handle(&event).unwrap();
  }

  let reported: Vec<String> = ids.errors().iter().map(|e| e.message().to_owned()).collect();
  assert_eq!(reported.len(), 1, "not an NCName fault, only the repetition: {reported:?}");
  assert!(reported[0].contains("\"x\" is used more than once"), "{reported:?}");
}

#[test]
fn an_xml_id_fault_is_located_at_the_value() {
  let report = validate("<a>\n  <b xml:id='1bad'/>\n</a>".as_bytes()).expect("well-formed");
  let at = report.errors()[0].location();
  assert_eq!((at.line, at.column), (2, 14), "the first character of the value, not the start tag");
}

#[test]
fn an_undeclared_xml_id_is_not_faulted_under_a_dtd() {
  // The DTD declares the element but not the xml:id attribute; xml:id needs no declaration.
  let xml = "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a xml:id='x'/>";
  let report = validate(xml.as_bytes()).expect("well-formed");
  assert!(report.is_valid(), "{:?}", report.errors());
}

#[test]
fn an_xml_id_shares_the_id_space_with_a_declared_id() {
  // xml:id "dup" on the root and a declared ID "dup" on a child are the same ID: a collision.
  let xml = "<!DOCTYPE r [<!ELEMENT r (c)><!ELEMENT c EMPTY><!ATTLIST c k ID #IMPLIED>]>\
             <r xml:id='dup'><c k='dup'/></r>";
  assert!(errors(xml).iter().any(|e| e.contains("more than once")), "{:?}", errors(xml));
}
