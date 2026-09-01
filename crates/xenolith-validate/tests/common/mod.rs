//! Shared helpers for the validate crate's integration tests: the "validate a document against its own DTD"
//! convenience that the public API leaves to the pipeline.
#![allow(dead_code)] // Each test file uses a different subset of these.

use std::io::Read;

use xenolith_core::error::Result;
use xenolith_parser::Reader;
use xenolith_validate::{Report, Validatable};

/// Validates a document read from `source` against its own declared DTD.
pub(crate) fn validate<R: Read>(source: R) -> Result<Report> {
  validate_reader(Reader::new(source))
}

/// Validates a document from a prepared [`Reader`], so a resolver can be attached first.
pub(crate) fn validate_reader<R: Read>(reader: Reader<R>) -> Result<Report> {
  reader.with_validation().validating_dtd().run()
}
