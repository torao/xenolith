//! Shared helpers for the validate crate's integration tests: the "validate a document against its own DTD"
//! convenience that the public API leaves to the pipeline.
#![allow(dead_code)] // Each test file uses a different subset of these.

use std::io::Read;

use xenolith::error::Result;
use xenolith::event::validate::{Report, ValidatorSet};
use xenolith::event::{EventCursor, EventHandler, EventSource, Outcome};
use xenolith::io::StreamSource;

/// Validates a document read from `source` against its own declared DTD.
pub(crate) fn validate<R: Read>(source: R) -> Result<Report> {
  let mut validation = ValidatorSet::new().validating_dtd(true);
  StreamSource::new(source).with_handler(&mut validation).emit()?;
  Ok(validation.report())
}

/// Validates a document from a prepared [`StreamSource`], so a resolver can be attached first.
///
/// The source was built by the caller, so the validation, made here, cannot be installed on it. The events are pulled
/// and handed over instead, and the validation is told the run completed, which a source only tells its own handlers.
///
pub(crate) fn validate_reader<R: Read>(mut reader: StreamSource<'_, R>) -> Result<Report> {
  let mut validation = ValidatorSet::new().validating_dtd(true);
  while let Some(event) = reader.next()? {
    validation.handle(&event)?;
  }
  validation.finish(Outcome::Completed);
  Ok(validation.report())
}

/// Validates a document against its own DTD, with `xml:id` attributes checked as IDs as well.
///
/// Checking them is the validation's own policy, so it is asked for here rather than read off the parser's
/// configuration.
///
pub(crate) fn validate_checking_xml_id<R: Read>(source: R) -> Result<Report> {
  let mut validation = ValidatorSet::new().validating_dtd(true).checking_xml_id(true);
  StreamSource::new(source).with_handler(&mut validation).emit()?;
  Ok(validation.report())
}
