//! Driving a validator over a document.
//!
//! [`validate`] reads a document with the parser and feeds each event to the DTD validator
//! that the document's own `DOCTYPE` calls for. Well-formedness errors come back as the parser
//! finds them; validity errors are gathered into the [`Report`], so one call surfaces both.

use std::io::Read;

use xylograph_core::error::Result;
use xylograph_parser::{EventKind, Reader};

use crate::dtd::DtdValidator;
use crate::{CollectErrors, Validator, ValidityError};

/// The outcome of validating a document.
#[derive(Debug)]
pub struct Report {
  errors: Vec<ValidityError>,
  had_dtd: bool,
}

impl Report {
  /// True if the document is valid: it had a DTD and broke none of its constraints.
  ///
  /// A document with no `DOCTYPE` has nothing to be valid against; see [`had_dtd`](Self::had_dtd).
  #[must_use]
  pub fn is_valid(&self) -> bool {
    self.had_dtd && self.errors.is_empty()
  }

  /// True if the document declared a DTD to validate against.
  #[must_use]
  pub fn had_dtd(&self) -> bool {
    self.had_dtd
  }

  /// The validity errors found, in document order.
  #[must_use]
  pub fn errors(&self) -> &[ValidityError] {
    &self.errors
  }
}

/// Validates a document read from `source` against its own DTD.
///
/// Well-formedness errors are fatal and returned as [`Err`]; validity errors are collected into
/// the [`Report`]. A document with no `DOCTYPE` is read to the end and reported as having no DTD.
///
/// External entities and an external DTD subset are not resolved here — this convenience uses a
/// reader with no resolver. Drive a [`Reader`] with a resolver and [`validate_reader`] for
/// those.
///
/// # Errors
///
/// Returns the parser's error if the document is not well-formed, or if reading `source` fails.
pub fn validate<R: Read>(source: R) -> Result<Report> {
  validate_reader(Reader::new(source))
}

/// Validates a document from a prepared [`Reader`], so a resolver can be attached first.
///
/// # Errors
///
/// As [`validate`].
pub fn validate_reader<R: Read>(mut reader: Reader<R>) -> Result<Report> {
  let mut validator: Option<DtdValidator> = None;
  let mut errors = CollectErrors::default();

  while let Some(kind) = reader.advance()? {
    match kind {
      EventKind::Doctype => {
        // The schema is the document's own DTD, now available.
        let parser = reader.parser();
        if let (Some(dtd), Some(root)) = (parser.dtd(), parser.doctype_name()) {
          validator = Some(DtdValidator::new(dtd.clone(), root));
        }
      }
      _ => {
        if let Some(validator) = validator.as_mut() {
          if feed(validator, &reader, kind, &mut errors).is_break() {
            break;
          }
        }
      }
    }
  }

  if let Some(validator) = validator.as_mut() {
    let _ = validator.finish(reader.parser().pool(), &mut errors);
  }

  Ok(Report { errors: errors.take(), had_dtd: validator.is_some() })
}

/// Passes one event to the validator.
fn feed<R: Read>(
  validator: &mut DtdValidator,
  reader: &Reader<R>,
  kind: EventKind,
  errors: &mut CollectErrors,
) -> std::ops::ControlFlow<()> {
  let parser = reader.parser();
  let pool = parser.pool();
  let at = parser.location();
  match kind {
    EventKind::StartElement => {
      let attributes: Vec<_> = parser.attributes().collect();
      validator.start_element(parser.name(), &attributes, pool, &at, errors)
    }
    EventKind::EndElement => validator.end_element(parser.name(), pool, &at, errors),
    EventKind::Text => {
      let text = parser.text();
      let whitespace_only = text.chars().all(xylograph_core::chars::is_whitespace);
      validator.characters(text, whitespace_only, pool, &at, errors)
    }
    // A CDATA section is significant character data, never ignorable whitespace.
    EventKind::CData => validator.characters(parser.text(), false, pool, &at, errors),
    // Comments, PIs and the XML declaration carry nothing a DTD constrains.
    _ => std::ops::ControlFlow::Continue(()),
  }
}
