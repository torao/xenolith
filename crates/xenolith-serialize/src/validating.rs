//! A writer that checks each write against a schema before it is emitted.
//!
//! [`ValidatingWriter`] wraps an [`XmlWriter`] and a [`Validator`]. Every write is checked first, and a write the schema
//! forbids is refused, so the output never contains content that breaks the schema. This is the write side of the same
//! [`Validator`] that checks parsed input and a built tree.
//!
//! A schema is checked against a document's names. This writer holds its own name pool and interns each written name
//! into it, so a name-based validator, one that matches on the lexical form such as the DTD validator, must be built
//! against that pool. A validator that resolves names to strings itself has no such requirement. Namespace scope is not
//! resolved at write time; a name is validated by its lexical form.

use std::io;

use xenolith_core::attr::{AttributeList, AttributeRef, Attributes};
use xenolith_core::chars;
use xenolith_core::error::{Location, Result};
use xenolith_core::name::{NamePool, QName};
use xenolith_validate::{FailFast, Validator, ValidityError};

use crate::XmlWriter;

/// An [`XmlWriter`] that validates each write against a [`Validator`] and refuses one the schema forbids.
///
/// It is driven like an [`XmlWriter`]: open elements, write attributes and content, close elements. Each call is checked
/// before anything is written, so a violation returns an error and emits nothing. Once a write is refused the writer
/// stays failed, and every later call returns the same error, so a broken document is never partly written.
///
/// A start element is checked once its attributes are in, which is at the next content, the next start element, or the
/// matching end. The whole-document checks run from [`finish`](Self::finish).
///
pub struct ValidatingWriter<W> {
  inner: XmlWriter<W>,
  validator: Box<dyn Validator>,
  pool: NamePool,
  pending: Option<PendingStart>,
  open: Vec<QName>,
  failure: Option<ValidityError>,
}

/// A start tag whose name and attributes are collected but not yet emitted, so it can be checked as a whole first.
struct PendingStart {
  name: QName,
  lexical: String,
  attributes: Vec<PendingAttribute>,
}

/// One attribute of a pending start tag.
struct PendingAttribute {
  name: QName,
  lexical: String,
  value: String,
  declares_namespace: bool,
}

/// The attributes of a pending start tag, presented as an [`AttributeList`] for the validator.
struct PendingAttributes<'a> {
  attributes: &'a [PendingAttribute],
}

impl AttributeList for PendingAttributes<'_> {
  fn len(&self) -> usize {
    self.attributes.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    let attribute = self.attributes.get(index)?;
    Some(AttributeRef {
      name: attribute.name,
      value: &attribute.value,
      declares_namespace: attribute.declares_namespace,
    })
  }
}

impl<W: io::Write> ValidatingWriter<W> {
  /// Creates a writer over `out` that checks each write against `validator`.
  pub fn new(out: W, validator: Box<dyn Validator>) -> Self {
    Self {
      inner: XmlWriter::new(out),
      validator,
      pool: NamePool::new(),
      pending: None,
      open: Vec::new(),
      failure: None,
    }
  }

  /// Writes the XML declaration. It is not a validated event.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left, or an error from the underlying writer.
  pub fn write_declaration(&mut self, standalone: Option<bool>) -> Result<()> {
    self.guard()?;
    self.inner.write_declaration(standalone)?;
    Ok(())
  }

  /// Opens an element. Attributes may follow until the next content or end.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left, a validity error if the previous start element breaks the
  /// schema, or an error from the underlying writer.
  pub fn write_start_element(&mut self, name: &str) -> Result<()> {
    self.guard()?;
    self.flush_pending()?;
    let qname = self.intern(name);
    self.pending = Some(PendingStart { name: qname, lexical: name.to_owned(), attributes: Vec::new() });
    Ok(())
  }

  /// Adds an attribute to the element just opened.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left.
  ///
  /// # Panics
  ///
  /// If no start tag is open.
  pub fn write_attribute(&mut self, name: &str, value: &str) -> Result<()> {
    self.guard()?;
    let qname = self.intern(name);
    let declares_namespace = name == "xmlns" || name.starts_with("xmlns:");
    let pending = self.pending.as_mut().expect("write_attribute must follow write_start_element");
    pending.attributes.push(PendingAttribute {
      name: qname,
      lexical: name.to_owned(),
      value: value.to_owned(),
      declares_namespace,
    });
    Ok(())
  }

  /// Ends the innermost open element.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left, a validity error if the element's content or end breaks the
  /// schema, or an error from the underlying writer.
  ///
  /// # Panics
  ///
  /// If no element is open.
  pub fn write_end_element(&mut self) -> Result<()> {
    self.guard()?;
    self.flush_pending()?;
    let name = self.open.pop().expect("write_end_element with no open element");
    let mut listener = FailFast::default();
    let _ = self.validator.end_element(name, &self.pool, &Location::unknown(), &mut listener);
    self.record(listener)?;
    self.inner.write_end_element()?;
    Ok(())
  }

  /// Writes character data, escaped.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left, a validity error if character data may not appear here, or an
  /// error from the underlying writer.
  pub fn write_characters(&mut self, text: &str) -> Result<()> {
    self.guard()?;
    self.flush_pending()?;
    let whitespace_only = text.chars().all(chars::is_whitespace);
    let mut listener = FailFast::default();
    let _ = self.validator.characters(text, whitespace_only, &Location::unknown(), &mut listener);
    self.record(listener)?;
    self.inner.write_characters(text)?;
    Ok(())
  }

  /// Writes a CDATA section, splitting any `]]>` so it cannot close early.
  ///
  /// # Errors
  ///
  /// As [`write_characters`](Self::write_characters): a CDATA section is significant character data.
  pub fn write_cdata(&mut self, text: &str) -> Result<()> {
    self.guard()?;
    self.flush_pending()?;
    let mut listener = FailFast::default();
    let _ = self.validator.characters(text, false, &Location::unknown(), &mut listener);
    self.record(listener)?;
    self.inner.write_cdata(text)?;
    Ok(())
  }

  /// Writes a comment. It is not a validated event.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left, a validity error if a pending start element breaks the schema, or
  /// an error from the underlying writer.
  pub fn write_comment(&mut self, text: &str) -> Result<()> {
    self.guard()?;
    self.flush_pending()?;
    self.inner.write_comment(text)?;
    Ok(())
  }

  /// Writes a processing instruction. It is not a validated event.
  ///
  /// # Errors
  ///
  /// As [`write_comment`](Self::write_comment).
  pub fn write_processing_instruction(&mut self, target: &str, data: &str) -> Result<()> {
    self.guard()?;
    self.flush_pending()?;
    self.inner.write_processing_instruction(target, data)?;
    Ok(())
  }

  /// The number of elements currently open, a pending start element included.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len() + usize::from(self.pending.is_some())
  }

  /// Runs the whole-document checks and returns the underlying writer.
  ///
  /// # Errors
  ///
  /// Returns the failure a refused earlier write left, or a validity error from a whole-document check, such as an
  /// `IDREF` that matched no `ID`.
  pub fn finish(mut self) -> Result<W> {
    self.guard()?;
    self.flush_pending()?;
    let mut listener = FailFast::default();
    let _ = self.validator.finish(&mut listener);
    self.record(listener)?;
    Ok(self.inner.into_inner())
  }

  /// Checks and emits a pending start tag, if there is one.
  fn flush_pending(&mut self) -> Result<()> {
    let Some(pending) = self.pending.take() else { return Ok(()) };
    let backing = PendingAttributes { attributes: &pending.attributes };
    let mut listener = FailFast::default();
    let _ = self.validator.start_element(
      pending.name,
      Attributes::new(&backing),
      &self.pool,
      &Location::unknown(),
      &mut listener,
    );
    self.record(listener)?;
    self.inner.write_start_element(&pending.lexical)?;
    for attribute in &pending.attributes {
      self.inner.write_attribute(&attribute.lexical, &attribute.value)?;
    }
    self.open.push(pending.name);
    Ok(())
  }

  /// Interns a written name into the pool as a [`QName`] with its lexical prefix and local part.
  fn intern(&mut self, name: &str) -> QName {
    match name.split_once(':') {
      Some((prefix, local)) => {
        let prefix = self.pool.intern(prefix);
        let local = self.pool.intern(local);
        QName::new(Some(prefix), None, local)
      }
      None => QName::new(None, None, self.pool.intern(name)),
    }
  }

  /// Returns the stored failure, if a write has already been refused.
  fn guard(&self) -> Result<()> {
    match &self.failure {
      Some(error) => Err(error.to_error()),
      None => Ok(()),
    }
  }

  /// Records a validity error a check reported, marking the writer failed and returning the error.
  fn record(&mut self, listener: FailFast) -> Result<()> {
    if let Some(error) = listener.first() {
      self.failure = Some(error.clone());
      return Err(error.to_error());
    }
    Ok(())
  }
}

impl<W> std::fmt::Debug for ValidatingWriter<W> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let depth = self.open.len() + usize::from(self.pending.is_some());
    f.debug_struct("ValidatingWriter")
      .field("depth", &depth)
      .field("failed", &self.failure.is_some())
      .finish_non_exhaustive()
  }
}

#[cfg(test)]
mod tests {
  use std::ops::ControlFlow;

  use xenolith_core::error::Error;

  use super::*;

  /// A schema that allows only the element names it was given, resolving names to strings itself.
  struct AllowedElements {
    allowed: Vec<String>,
  }

  impl Validator for AllowedElements {
    fn start_element(
      &mut self,
      name: QName,
      _attributes: Attributes<'_>,
      pool: &NamePool,
      at: &Location,
      errors: &mut dyn xenolith_validate::ErrorListener,
    ) -> ControlFlow<()> {
      let local = pool.resolve(name.local());
      if self.allowed.iter().any(|a| a == local) {
        ControlFlow::Continue(())
      } else {
        errors.report(ValidityError::new(format!("element \"{local}\" is not allowed"), at.clone()))
      }
    }

    fn characters(
      &mut self,
      _text: &str,
      _ws: bool,
      _at: &Location,
      _errors: &mut dyn xenolith_validate::ErrorListener,
    ) -> ControlFlow<()> {
      ControlFlow::Continue(())
    }

    fn end_element(
      &mut self,
      _name: QName,
      _pool: &NamePool,
      _at: &Location,
      _errors: &mut dyn xenolith_validate::ErrorListener,
    ) -> ControlFlow<()> {
      ControlFlow::Continue(())
    }

    fn finish(&mut self, _errors: &mut dyn xenolith_validate::ErrorListener) -> ControlFlow<()> {
      ControlFlow::Continue(())
    }
  }

  fn allowing(names: &[&str]) -> Box<dyn Validator> {
    Box::new(AllowedElements { allowed: names.iter().map(|s| (*s).to_owned()).collect() })
  }

  #[test]
  fn a_conforming_document_is_written() {
    let mut w = ValidatingWriter::new(Vec::new(), allowing(&["a", "b"]));
    w.write_start_element("a").unwrap();
    w.write_attribute("x", "1").unwrap();
    w.write_start_element("b").unwrap();
    w.write_end_element().unwrap();
    w.write_end_element().unwrap();
    let out = String::from_utf8(w.finish().unwrap()).unwrap();
    assert_eq!(out, "<a x=\"1\"><b/></a>");
  }

  #[test]
  fn a_disallowed_element_is_refused_at_finish() {
    let mut w = ValidatingWriter::new(Vec::new(), allowing(&["a"]));
    w.write_start_element("bad").unwrap(); // buffered, not yet checked
    let error = w.finish().unwrap_err(); // flushing checks it
    assert!(matches!(error, Error::Validity { .. }), "{error}");
    assert!(error.to_string().contains("bad"));
  }

  #[test]
  fn a_refusal_poisons_later_writes() {
    let mut w = ValidatingWriter::new(Vec::new(), allowing(&["a"]));
    w.write_start_element("a").unwrap();
    w.write_start_element("bad").unwrap(); // `a` flushed and allowed; `bad` now pending
    let error = w.write_end_element().unwrap_err(); // flushing `bad` is refused
    assert!(matches!(error, Error::Validity { .. }), "{error}");
    // The writer is failed, so a later write returns the same error rather than emitting more.
    assert!(w.write_characters("x").is_err());
  }
}
