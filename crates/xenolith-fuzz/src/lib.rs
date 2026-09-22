//! The properties xenolith is fuzzed against.
//!
//! A fuzzer's finding is only as good as the property it was checking, so the properties live
//! here rather than inside the fuzz targets: the targets in `fuzz/fuzz_targets` call these, and
//! so does an ordinary test that replays the seed corpus. That way `cargo test` on stable Rust
//! exercises the same checks a libFuzzer run does — a property that had rotted would fail the
//! build rather than quietly stop finding things.
//!
//! # What is being checked
//!
//! Every function here takes arbitrary bytes and must **return**. Panicking is the finding; so is
//! looping for ever, which the fuzzer reports as a timeout. Beyond that, one carries a property
//! stronger than "did not crash":
//!
//! - [`build_and_serialize`] — what the writer puts out parses back, and writing it again gives
//!   the same text. A writer that emitted something unreadable would be a bug no test of
//!   hand-written documents is likely to reach.
//!
//! The properties over XPath expressions and XSLT stylesheets went with the crates they exercised
//! (`xenolith-xpath`, `xenolith-xslt`), which are parked outside the workspace while the event
//! vocabulary settles. Their corpora are still here, and the properties return with the crates.
//!
//! # What is deliberately not checked
//!
//! That a document is *accepted*. Most random bytes are not XML, and refusing them is correct;
//! the interesting question is whether the refusal is orderly.

use std::io::Read;

use xenolith::dom::build::DomBuilder;
use xenolith::dom::{Document, DomSource};
use xenolith::event::validate::ValidatorSet;
use xenolith::event::{EventCursor, EventRef, EventSource};
use xenolith::io::StreamSource;
use xenolith::io::write::XmlWriter;

/// Reads `xml` into a tree through the parser and the builder.
fn build_tree(xml: &[u8]) -> xenolith::Result<Document> {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml).with_handler(&mut builder).emit()?;
  Ok(builder.into_document())
}

/// Writes `document` out as XML text.
fn write_tree(document: &Document) -> xenolith::Result<String> {
  let mut writer = XmlWriter::new(Vec::new());
  DomSource::new(document).with_handler(&mut writer).emit()?;
  Ok(String::from_utf8(writer.into_inner()).expect("the writer writes UTF-8 unless told otherwise"))
}

/// Reads a document with the pull parser, touching every event.
///
/// The fields are read rather than the events only counted: an event that cannot be read is as
/// much a bug as one that cannot be reached, and reading is where the borrowed buffers are.
pub fn parse_document(data: &[u8]) {
  let mut source = StreamSource::with_system_id(data, "urn:fuzz");
  loop {
    match source.next() {
      Ok(Some(event)) => match event {
        EventRef::StartElement(event) => {
          let _ = event.lexical();
          let _ = event.base_uri;
          for attribute in event.attributes.iter() {
            let _ = attribute.value;
            let _ = attribute.declares_namespace();
          }
        }
        EventRef::EndElement(event) => {
          let _ = event.lexical();
        }
        EventRef::Characters(event) => {
          let _ = event.text;
        }
        EventRef::Cdata(event) => {
          let _ = event.text;
        }
        EventRef::Comment(event) => {
          let _ = event.text;
        }
        EventRef::ProcessingInstruction(event) => {
          let _ = (event.target, event.data);
        }
        EventRef::Doctype(event) => {
          let _ = (event.name, event.public_id, event.system_id);
        }
        EventRef::StartDocument | EventRef::EndDocument => {}
      },
      // The end of the document, or a refusal. Both are orderly.
      Ok(None) | Err(_) => return,
    }
  }
}

/// Reads a document through anything that yields bytes, as a caller streaming from a file does.
///
/// The same parser, driven a byte at a time, so a token split across two reads is exercised —
/// which the slice above never does.
pub fn parse_document_in_pieces(data: &[u8]) {
  let mut source = StreamSource::new(OneByteAtATime(data, 0));
  while let Ok(Some(_)) = source.next() {}
}

/// A reader that hands over one byte per call, to split tokens across reads.
struct OneByteAtATime<'a>(&'a [u8], usize);

impl Read for OneByteAtATime<'_> {
  fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
    if buffer.is_empty() || self.1 >= self.0.len() {
      return Ok(0);
    }
    buffer[0] = self.0[self.1];
    self.1 += 1;
    Ok(1)
  }
}

/// Validates a document against its own DTD.
///
/// The property is only that it returns: a document with no `DOCTYPE` can still be reported
/// against — `xml:id` is checked whether or not a DTD declares anything — so "errors imply a
/// DTD" would be a property that is not true, and a fuzzer would rightly find it.
pub fn validate_document(data: &[u8]) {
  let mut validation = ValidatorSet::new().validating_dtd(true).checking_xml_id(true);
  {
    let mut source = StreamSource::new(data).with_handler(&mut validation);
    let _ = source.emit();
  }
  let report = validation.report();
  let _ = report.is_valid();
  for error in report.errors() {
    let _ = error.message();
    let _ = error.location();
  }
}

/// Builds a DOM, writes it out, and reads it back.
///
/// The property: **what this writer puts out, this parser reads** — and writing the tree that
/// comes back gives the same text. A document that survives parsing but cannot be written down
/// again is a bug that no test of documents a person wrote is likely to reach.
pub fn build_and_serialize(data: &[u8]) {
  let Ok(document) = build_tree(data) else { return };
  if document.document_element().is_none() {
    return;
  }
  let Ok(written) = write_tree(&document) else { return };

  let reread = build_tree(written.as_bytes())
    .unwrap_or_else(|error| panic!("what the writer put out will not parse: {}\n{written}", error.message()));
  assert!(reread.document_element().is_some(), "what the writer put out has no document element: {written}");
  let again = write_tree(&reread).unwrap_or_else(|error| panic!("the tree read back will not write: {error}"));
  assert_eq!(written, again, "writing the same tree twice gave two different texts");
}
