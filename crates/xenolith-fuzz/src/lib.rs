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
//! looping for ever, which the fuzzer reports as a timeout. Beyond that, several carry a property
//! stronger than "did not crash":
//!
//! - [`build_and_serialize`] — what the serializer writes parses back, and writing it again gives
//!   the same text. A serializer that emitted something unreadable would be a bug no test of
//!   hand-written documents is likely to reach.
//! - [`compile_expression`] — printing a parsed expression yields text that parses to the same
//!   tree. This is the property the proptest suite checks over generated expressions; the fuzzer
//!   reaches shapes a generator will not.
//! - [`transform`] — a stylesheet that compiles either runs or fails, and what it produces can be
//!   written out.
//!
//! # What is deliberately not checked
//!
//! That a document is *accepted*. Most random bytes are not XML, and refusing them is correct;
//! the interesting question is whether the refusal is orderly.

use std::io::Read;

use xenolith_dom::build;
use xenolith_parser::{EventRef, Reader};
use xenolith_validate::Validatable;
use xenolith_xdm::DomModel;
use xenolith_xpath::XPath;
use xenolith_xslt::{Stylesheet, Transform};

/// How deep a fuzzed transformation may recurse before it is stopped.
///
/// Far below the default: a fuzzer that has found unbounded recursion should be told quickly,
/// and a stylesheet that legitimately needs more depth is not what this is looking for.
const FUZZ_MAX_DEPTH: usize = 20;

/// The document a fuzzed stylesheet or expression is run against.
///
/// Small, and with something of everything a pattern might match: elements at two depths, an
/// attribute, text, a comment, a processing instruction, and a namespace.
const SUBJECT: &[u8] = br#"<?xml version="1.0"?>
<r xmlns:p="urn:p" k="v"><a id="1">one</a><p:b>two</p:b><!--c--><?pi d?>tail</r>"#;

/// Reads a document with the pull parser, touching every event.
///
/// The accessors are called rather than only the events counted: an event that cannot be read is
/// as much a bug as one that cannot be reached, and reading is where the borrowed buffers are.
pub fn parse_document(data: &[u8]) {
  let mut reader = Reader::with_system_id(data, "urn:fuzz");
  loop {
    match reader.advance() {
      Ok(Some(_)) => {
        let parser = reader.parser();
        match parser.event_ref() {
          Some(EventRef::StartElement { attributes, .. }) => {
            let _ = parser.local_name();
            for attribute in attributes.iter() {
              let _ = attribute.value;
              let _ = attribute.declares_namespace;
            }
          }
          Some(EventRef::EndElement { .. }) => {
            let _ = parser.local_name();
          }
          Some(EventRef::Text(text) | EventRef::CData(text) | EventRef::Comment(text)) => {
            let _ = text;
          }
          _ => {}
        }
      }
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
  let mut reader = Reader::new(OneByteAtATime(data, 0));
  while let Ok(Some(_)) = reader.advance() {}
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
  if let Ok(report) = Reader::new(data).with_validation().validating_dtd().run() {
    let _ = report.is_valid();
    for error in report.errors() {
      let _ = error.message();
      let _ = error.location();
    }
  }
}

/// Builds a DOM, writes it out, and reads it back.
///
/// The property: **what this serializer writes, this parser reads** — and writing the tree that
/// comes back gives the same text. A document that survives parsing but cannot be written down
/// again is a bug that no test of documents a person wrote is likely to reach.
pub fn build_and_serialize(data: &[u8]) {
  let Ok(document) = build::parse(data) else { return };
  let Some(root) = document.document_element() else { return };
  let written = xenolith_serialize::Serializer::new().to_string(&document, root);

  let reread = build::parse(written.as_bytes())
    .unwrap_or_else(|error| panic!("what the serializer wrote will not parse: {}\n{written}", error.message()));
  let Some(reread_root) = reread.document_element() else {
    panic!("what the serializer wrote has no document element: {written}");
  };
  let again = xenolith_serialize::Serializer::new().to_string(&reread, reread_root);
  assert_eq!(written, again, "writing the same tree twice gave two different texts");
}

/// Parses an XPath expression, prints it, and parses it again.
///
/// The property: printing a tree gives text that parses back to **the same tree**. The printed
/// form is the unabbreviated one, so this also says that expanding `//`, `@` and the rest is
/// faithful.
///
/// It compares the trees rather than the printed text. Comparing the text asks only that the
/// printer be self-consistent, which it can be while printing something that means a different
/// thing — `(//a)[1]` printed as `//a[1]` is stable and selects a different node-set. Two
/// findings hid behind the weaker form before it was corrected to the one this documentation had
/// been claiming all along.
pub fn compile_expression(text: &str) {
  let Ok(expression) = xenolith_xpath::parse(text) else { return };
  let printed = expression.to_string();
  let reparsed = xenolith_xpath::parse(&printed)
    .unwrap_or_else(|error| panic!("printing {text:?} gave {printed:?}, which will not parse: {}", error.message()));
  assert_eq!(reparsed, expression, "printing {text:?} gave {printed:?}, which parses to a different tree");
}

/// Evaluates an expression over a fixed document.
///
/// Parsing an expression and running one are different machinery, and only the second reaches the
/// axes, the conversions and the function library.
pub fn evaluate_expression(text: &str) {
  let Ok(document) = build::parse(SUBJECT) else { return };
  let model = DomModel::new(&document);
  let Ok(expression) = XPath::new().with_namespace("p", "urn:p").compile(text) else { return };
  if let Ok(value) = expression.evaluate(&model, model.root_node()) {
    // Every value converts to every other type; §4 leaves none of them undefined.
    let _ = value.string(&model);
    let _ = value.number(&model);
    let _ = value.boolean();
  }
}

/// Compiles a stylesheet and runs it over a fixed document.
pub fn transform(data: &[u8]) {
  let Ok(stylesheet) = Stylesheet::compile(data, "urn:fuzz") else { return };
  let Ok(document) = build::parse(SUBJECT) else { return };
  let model = DomModel::new(&document);
  let Ok(result) = Transform::new().with_max_depth(FUZZ_MAX_DEPTH).run(&stylesheet, &model, model.root_node()) else {
    return;
  };
  // A result that was built must be writable: §16 has an answer for every tree.
  let _ = result.serialize();
  let _ = result.to_bytes();
}
