//! Building a DOM while validating against the document's own DTD, in a single pass over the reader.
//!
//! The DOM builder is a `Handler`, so it takes its place beside a validator in the validation pipeline. One read of
//! the source both checks the document and produces the tree.

#![cfg(feature = "parse")]

use xenolith::dom::build::DomBuilder;
use xenolith::parser::Reader;
use xenolith::validate::Validatable;

#[test]
fn builds_a_dom_while_validating_against_the_document_dtd_in_one_pass() {
  let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)><!ATTLIST item key ID #IMPLIED>]>\
             <r><item key='k1'>one</item><item key='k2'>two</item></r>";

  let mut builder = DomBuilder::new();
  let report = Reader::new(xml.as_bytes())
    .with_validation()
    .validating_dtd()
    .with_handler(&mut builder)
    .run()
    .expect("well-formed");

  // Validated against the document's own DTD in the same pass.
  assert!(report.is_valid(), "unexpected errors: {:?}", report.errors());

  // And the tree was built, with the DTD's ID attribute marked so it is found by id.
  let doc = builder.into_document().expect("built");
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(root), "r");
  assert_eq!(doc.children(root).count(), 2);
  assert_eq!(doc.get_element_by_id("k1").map(|n| doc.node_name(n)).as_deref(), Some("item"));
}

#[test]
fn a_dtd_violation_is_reported_and_the_tree_is_still_built() {
  // `r` is declared `(item+)` but holds a `bad` element. Validation records the violation; building goes on, since a
  // validity error is recoverable.
  let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)>]><r><bad/></r>";

  let mut builder = DomBuilder::new();
  let report = Reader::new(xml.as_bytes())
    .with_validation()
    .validating_dtd()
    .with_handler(&mut builder)
    .run()
    .expect("well-formed");

  assert!(!report.is_valid(), "the content model was violated");
  assert!(!report.errors().is_empty());

  let doc = builder.into_document().expect("built");
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(doc.first_child(root).unwrap()), "bad", "the tree is built despite the violation");
}
