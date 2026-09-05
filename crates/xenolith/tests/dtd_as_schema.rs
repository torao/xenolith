//! A DTD used as a schema of its own, apart from the document that might have declared it.
//!
//! The DTD is read from its own source, held as a [`DtdSchema`], and checked against whatever the events come from: a
//! reader over a document, or a walk over a tree already built. Nothing here needs the document to carry a `DOCTYPE`.

#![cfg(feature = "parse")]

use xenolith::dom::{DomSource, build};
use xenolith::dtd::DtdReader;
use xenolith::parser::Reader;
use xenolith::validate::{DtdSchema, Validatable};

const SCHEMA: &str = "<!ELEMENT note (body)>\
                      <!ELEMENT body (#PCDATA)>\
                      <!ATTLIST note id ID #IMPLIED>";

fn schema() -> DtdSchema {
  let (dtd, pool) = DtdReader::new(SCHEMA.as_bytes()).read().expect("a well-formed DTD");
  DtdSchema::new(dtd, pool).with_root("note")
}

#[test]
fn a_document_stream_is_checked_against_a_dtd_of_its_own() {
  let schema = schema();
  let xml = "<note id='n1'><body>hi</body></note>";

  let report = Reader::new(xml.as_bytes()).with_validation().with_schema(&schema).run().expect("well-formed");
  assert!(report.errors().is_empty(), "unexpected errors: {:?}", report.errors());
}

#[test]
fn a_tree_already_built_is_checked_against_the_same_schema() {
  // The point of the whole exercise: the tree interned its names in its own pool, and the schema's DTD is keyed by
  // the pool it was read in. The two never meet by id, so this only works because the validator matches by name.
  let schema = schema();
  let doc = build::parse("<note id='n1'><body>hi</body></note>".as_bytes()).expect("well-formed");

  let report = DomSource::new(&doc).with_validation().with_schema(&schema).run().expect("emitted");
  assert!(report.errors().is_empty(), "unexpected errors: {:?}", report.errors());
}

#[test]
fn a_tree_that_breaks_the_content_model_is_caught() {
  let schema = schema();
  let doc = build::parse("<note><wrong/></note>".as_bytes()).expect("well-formed");

  let report = DomSource::new(&doc).with_validation().with_schema(&schema).run().expect("emitted");
  let messages: Vec<String> = report.errors().iter().map(ToString::to_string).collect();
  assert!(messages.iter().any(|m| m.contains("wrong")), "the offending element must be named: {messages:?}");
}

#[test]
fn the_root_is_checked_when_the_schema_names_one() {
  let schema = schema();
  let doc = build::parse("<body>hi</body>".as_bytes()).expect("well-formed");

  let report = DomSource::new(&doc).with_validation().with_schema(&schema).run().expect("emitted");
  let messages: Vec<String> = report.errors().iter().map(ToString::to_string).collect();
  assert!(messages.iter().any(|m| m.contains("root")), "{messages:?}");
}

#[test]
fn a_schema_that_names_no_root_leaves_the_root_alone() {
  // A DTD read on its own declares no root, so without `with_root` any declared element may stand at the top.
  let (dtd, pool) = DtdReader::new(SCHEMA.as_bytes()).read().expect("a well-formed DTD");
  let schema = DtdSchema::new(dtd, pool);
  let doc = build::parse("<body>hi</body>".as_bytes()).expect("well-formed");

  let report = DomSource::new(&doc).with_validation().with_schema(&schema).run().expect("emitted");
  assert!(report.errors().is_empty(), "unexpected errors: {:?}", report.errors());
}

#[test]
fn one_schema_serves_several_documents() {
  // `Schema::validator` hands out a fresh validator each time, so what one run gathered, the ID values among them,
  // does not leak into the next.
  let schema = schema();
  for _ in 0..3 {
    let report = Reader::new("<note id='n1'><body>x</body></note>".as_bytes())
      .with_validation()
      .with_schema(&schema)
      .run()
      .expect("well-formed");
    assert!(report.errors().is_empty(), "a repeated run must not see the previous run's IDs: {:?}", report.errors());
  }
}
