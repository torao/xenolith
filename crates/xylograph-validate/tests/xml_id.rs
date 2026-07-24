//! xml:id checking: a valid NCName, unique across the document, with and without a DTD.

#![cfg(feature = "xml-id")]

use xylograph_validate::validate;

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
