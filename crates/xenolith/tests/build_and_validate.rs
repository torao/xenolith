//! Building a DOM while validating against the document's own DTD, in a single pass over the reader.
//!
//! The DOM builder is a handler, so it takes its place beside the validation in a dispatch. One read of the source
//! both checks the document and produces the tree.

use xenolith::dom::build::DomBuilder;
use xenolith::event::validate::ValidatorSet;
use xenolith::event::{Dispatch, EventCursor, EventSource};
use xenolith::io::StreamSource;

#[test]
fn builds_a_dom_while_validating_against_the_document_dtd_in_one_pass() {
  let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)><!ATTLIST item key ID #IMPLIED>]>\
             <r><item key='k1'>one</item><item key='k2'>two</item></r>";

  let mut validation = ValidatorSet::new().validating_dtd(true);
  let mut builder = DomBuilder::new();
  {
    let mut lane = Dispatch::new().with_handler(&mut validation).with_handler(&mut builder);
    StreamSource::new(xml.as_bytes()).with_handler(&mut lane).emit().expect("well-formed");
  }

  // Validated against the document's own DTD in the same pass.
  let report = validation.report();
  assert!(report.is_valid(), "unexpected errors: {:?}", report.errors());

  // And the tree was built, with the DTD's ID attribute marked so it is found by id.
  let doc = builder.into_document();
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(root), "r");
  assert_eq!(doc.children(root).count(), 2);
  assert_eq!(doc.get_element_by_id("k1").map(|n| doc.node_name(n)).as_deref(), Some("item"));
}

#[test]
fn a_dtd_violation_is_reported_and_the_tree_is_still_built() {
  // `r` is declared `(item+)` but holds a `bad` element. The validator set records the violation; building goes on, since a
  // validity error is recoverable.
  let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)>]><r><bad/></r>";

  let mut validation = ValidatorSet::new().validating_dtd(true);
  let mut builder = DomBuilder::new();
  {
    let mut lane = Dispatch::new().with_handler(&mut validation).with_handler(&mut builder);
    StreamSource::new(xml.as_bytes()).with_handler(&mut lane).emit().expect("well-formed");
  }

  let report = validation.report();
  assert!(!report.is_valid(), "the content model was violated");
  assert!(!report.errors().is_empty());

  let doc = builder.into_document();
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(doc.first_child(root).unwrap()), "bad", "the tree is built despite the violation");
}
