//! A DTD used as a schema of its own, apart from the document that might have declared it.
//!
//! The DTD is read from its own source, held as a [`DtdSchema`], and checked against whatever the events come from: a
//! reader over a document, or a walk over a tree already built. Nothing here needs the document to carry a `DOCTYPE`.

use xenolith::dom::DomSource;
use xenolith::dom::build::DomBuilder;
use xenolith::dtd::DtdReader;
use xenolith::dtd::validate::DtdSchema;
use xenolith::event::validate::{Report, ValidatorSet};
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;

/// Reads `xml` into a tree through the parser and the builder.
fn parse_document(xml: &[u8]) -> xenolith::Result<xenolith::dom::Document> {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml).with_handler(&mut builder).emit()?;
  Ok(builder.into_document())
}

const SCHEMA: &str = "<!ELEMENT note (body)>\
                      <!ELEMENT body (#PCDATA)>\
                      <!ATTLIST note id ID #IMPLIED>";

fn schema() -> DtdSchema {
  let (dtd, pool) = DtdReader::new(SCHEMA.as_bytes()).read().expect("a well-formed DTD");
  DtdSchema::new(dtd, pool).with_root("note")
}

/// Checks the events of a walk over `doc` against `schema`.
fn check_tree(doc: &xenolith::dom::Document, schema: &DtdSchema) -> Report {
  let mut validation = ValidatorSet::new().with_schema(schema);
  DomSource::new(doc).with_handler(&mut validation).emit().expect("emitted");
  validation.report()
}

#[test]
fn a_document_stream_is_checked_against_a_dtd_of_its_own() {
  let schema = schema();
  let xml = "<note id='n1'><body>hi</body></note>";

  let mut validation = ValidatorSet::new().with_schema(&schema);
  StreamSource::new(xml.as_bytes()).with_handler(&mut validation).emit().expect("well-formed");
  let report = validation.report();
  assert!(report.errors().is_empty(), "unexpected errors: {:?}", report.errors());
  assert!(report.is_valid(), "a schema is something to be valid against, even without a DOCTYPE");
}

#[test]
fn a_tree_already_built_is_checked_against_the_same_schema() {
  // The point of the whole exercise: the tree interned its names in its own pool, and the schema's DTD is keyed by
  // the pool it was read in. The two never meet by id, so this only works because the validator matches by name.
  let schema = schema();
  let doc = parse_document("<note id='n1'><body>hi</body></note>".as_bytes()).expect("well-formed");

  let report = check_tree(&doc, &schema);
  assert!(report.errors().is_empty(), "unexpected errors: {:?}", report.errors());
}

#[test]
fn a_tree_that_breaks_the_content_model_is_caught() {
  let schema = schema();
  let doc = parse_document("<note><wrong/></note>".as_bytes()).expect("well-formed");

  let report = check_tree(&doc, &schema);
  let messages: Vec<String> = report.errors().iter().map(ToString::to_string).collect();
  assert!(messages.iter().any(|m| m.contains("wrong")), "the offending element must be named: {messages:?}");
}

#[test]
fn the_root_is_checked_when_the_schema_names_one() {
  let schema = schema();
  let doc = parse_document("<body>hi</body>".as_bytes()).expect("well-formed");

  let report = check_tree(&doc, &schema);
  let messages: Vec<String> = report.errors().iter().map(ToString::to_string).collect();
  assert!(messages.iter().any(|m| m.contains("root")), "{messages:?}");
}

#[test]
fn a_schema_that_names_no_root_leaves_the_root_alone() {
  // A DTD read on its own declares no root, so without `with_root` any declared element may stand at the top.
  let (dtd, pool) = DtdReader::new(SCHEMA.as_bytes()).read().expect("a well-formed DTD");
  let schema = DtdSchema::new(dtd, pool);
  let doc = parse_document("<body>hi</body>".as_bytes()).expect("well-formed");

  let report = check_tree(&doc, &schema);
  assert!(report.errors().is_empty(), "unexpected errors: {:?}", report.errors());
}

#[test]
fn one_schema_serves_several_documents() {
  // `Schema::validator` hands out a fresh validator each time, so what one validation gathered, the ID values among
  // them, does not leak into the next.
  let schema = schema();
  for _ in 0..3 {
    let mut validation = ValidatorSet::new().with_schema(&schema);
    StreamSource::new("<note id='n1'><body>x</body></note>".as_bytes())
      .with_handler(&mut validation)
      .emit()
      .expect("well-formed");
    let report = validation.report();
    assert!(report.errors().is_empty(), "a repeated run must not see the previous run's IDs: {:?}", report.errors());
  }
}
