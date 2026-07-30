//! Replays the fuzzing corpus through the properties, on stable Rust and every platform.
//!
//! `cargo fuzz` needs a nightly toolchain and libFuzzer, so a run of it is something someone
//! chooses to do. This is what runs on every push instead: the same properties, over the seed
//! corpus and over the inputs past runs found interesting. It is not a search — it will not find
//! anything new — but it means a property that had stopped holding, or stopped compiling, fails
//! the build rather than waiting for the next fuzzing session.
//!
//! A crash the fuzzer finds is copied into `corpus/` beside the seeds, and from then on this test
//! is what keeps it fixed.

use std::path::{Path, PathBuf};

/// Every file in one corpus directory.
fn corpus(name: &str) -> Vec<(PathBuf, Vec<u8>)> {
  let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus").join(name);
  let mut inputs = Vec::new();
  let entries = std::fs::read_dir(&directory).unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_file() {
      let content = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
      inputs.push((path, content));
    }
  }
  assert!(!inputs.is_empty(), "the {name} corpus is empty; the seeds are part of the repository");
  inputs
}

/// Runs `check` over a corpus, saying which input failed if one does.
fn replay(name: &str, check: impl Fn(&[u8])) {
  for (path, content) in corpus(name) {
    // The panic a property raises carries the reason; this adds which file provoked it.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&content)));
    assert!(outcome.is_ok(), "{} did not survive the property", path.display());
  }
}

#[test]
fn every_document_parses_or_is_refused() {
  replay("documents", xylograph_fuzz::parse_document);
  replay("documents", xylograph_fuzz::parse_document_in_pieces);
  replay("documents", xylograph_fuzz::validate_document);
}

#[test]
fn every_document_that_builds_a_tree_can_be_written_and_read_back() {
  replay("documents", xylograph_fuzz::build_and_serialize);
}

#[test]
fn every_expression_prints_to_something_that_parses_to_the_same_tree() {
  replay("expressions", |data| {
    if let Ok(text) = std::str::from_utf8(data) {
      xylograph_fuzz::compile_expression(text);
      xylograph_fuzz::evaluate_expression(text);
    }
  });
}

#[test]
fn every_stylesheet_compiles_or_is_refused() {
  replay("stylesheets", xylograph_fuzz::transform);
}

#[test]
fn the_properties_survive_what_is_not_xml_at_all() {
  // The corpus is XML-shaped by design, and the fuzzer spends most of its time nowhere near
  // that. These are the shapes it reaches first.
  for input in [
    b"".as_slice(),
    b"\x00\x01\x02",
    b"<",
    b"<?",
    b"<!--",
    b"<![CDATA[",
    b"&#x",
    b"\xff\xfe",
    b"\xef\xbb\xbf",
    "<r>\u{10FFFF}</r>".as_bytes(),
  ] {
    xylograph_fuzz::parse_document(input);
    xylograph_fuzz::parse_document_in_pieces(input);
    xylograph_fuzz::validate_document(input);
    xylograph_fuzz::build_and_serialize(input);
    xylograph_fuzz::transform(input);
    if let Ok(text) = std::str::from_utf8(input) {
      xylograph_fuzz::compile_expression(text);
      xylograph_fuzz::evaluate_expression(text);
    }
  }
}
