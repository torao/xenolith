//! Writing through a DTD, so invalid XML is never emitted in the first place.
//!
//! This is the write side of the same `Validator` that checks parsed input and a built tree. The DTD is read as a
//! schema of its own, and the writer refuses a write that breaks it rather than emitting it and reporting afterwards.

#![cfg(feature = "validate")]

use xenolith_core::Error;
use xenolith_parser::dtd::DtdReader;
use xenolith_serialize::ValidatingWriter;
use xenolith_validate::{DtdSchema, Schema};

const SCHEMA: &str = "<!ELEMENT note (body)>\
                      <!ELEMENT body (#PCDATA)>\
                      <!ATTLIST note id ID #IMPLIED>";

fn schema() -> DtdSchema {
  let (dtd, pool) = DtdReader::new(SCHEMA.as_bytes()).read().expect("a well-formed DTD");
  DtdSchema::new(dtd, pool).with_root("note")
}

#[test]
fn a_document_that_follows_the_dtd_is_written() {
  let mut w = ValidatingWriter::new(Vec::new(), schema().validator());
  w.write_start_element("note").unwrap();
  w.write_attribute("id", "n1").unwrap();
  w.write_start_element("body").unwrap();
  w.write_characters("hi").unwrap();
  w.write_end_element().unwrap();
  w.write_end_element().unwrap();

  let out = String::from_utf8(w.finish().unwrap()).unwrap();
  assert_eq!(out, "<note id=\"n1\"><body>hi</body></note>");
}

#[test]
fn an_element_the_dtd_does_not_allow_is_refused() {
  // The writer holds a pool of its own, and the DTD's names were interned while it was read. The two never meet by
  // id, so this only works because the validator matches on the lexical form.
  let mut w = ValidatingWriter::new(Vec::new(), schema().validator());
  w.write_start_element("note").unwrap();
  w.write_start_element("wrong").unwrap();

  let error = w.write_end_element().unwrap_err();
  assert!(matches!(error, Error::Validity { .. }), "{error}");
  assert!(error.to_string().contains("wrong"), "the offending element appears in the message: {error}");
}

#[test]
fn an_undeclared_attribute_is_refused() {
  let mut w = ValidatingWriter::new(Vec::new(), schema().validator());
  w.write_start_element("note").unwrap();
  w.write_attribute("unknown", "1").unwrap();

  let error = w.write_start_element("body").unwrap_err();
  assert!(matches!(error, Error::Validity { .. }), "{error}");
  assert!(error.to_string().contains("unknown"), "{error}");
}

#[test]
fn a_root_the_schema_does_not_ask_for_is_refused() {
  let mut w = ValidatingWriter::new(Vec::new(), schema().validator());
  w.write_start_element("body").unwrap();

  let error = w.finish().unwrap_err();
  assert!(matches!(error, Error::Validity { .. }), "{error}");
  assert!(error.to_string().contains("root"), "{error}");
}
