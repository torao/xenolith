//! Validating against the document's own declared DTD.
//!
//! Every other [`Validator`] is built from a schema the caller already has. This one has no schema until the document
//! gives it one: it waits for the `DOCTYPE`, builds a [`DtdValidator`] from the DTD the parser read, and checks the
//! rest of the document against it. That is the counterpart of Java's `setValidating(true)`.
//!
//! It is an [`EventHandler`] like any other validator, so it runs beside an application's own handler in one pass
//! through a [`Dispatch`](crate::event::Dispatch): the application is called with data the schema has already
//! judged.
//!

#[cfg(test)]
mod test;

use crate::error::Result;
use crate::event::{EventHandler, EventRef, Outcome};

use crate::dtd::validate::DtdValidator;
use crate::event::validate::ids::XmlIdValidator;
use crate::event::validate::{Validator, ValidityError};

/// A [`Validator`] whose schema is the document's own DTD, built when the `DOCTYPE` is read.
///
/// A document with no `DOCTYPE` has no DTD to be checked against, and this reports nothing for one; ask
/// [`had_dtd`](Self::had_dtd) which it was. The DTD is complete by the [`Doctype`](EventRef::Doctype) event, both subsets
/// included, so the validator it builds is whole before the first element arrives.
///
/// # Examples
///
/// ```
/// use xenolith::event::{EventCursor, EventSource};
/// use xenolith::io::StreamSource;
/// use xenolith::dtd::validate::DocumentDtd;
/// use xenolith::event::validate::Validator;
///
/// // The element `c` is used but never declared: a validity error, kept, not thrown.
/// let xml = "<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>";
/// let mut validating = DocumentDtd::new();
/// StreamSource::new(xml.as_bytes()).with_handler(&mut validating).emit()?;
///
/// assert!(validating.had_dtd());
/// assert!(validating.errors().iter().any(|e| e.to_string().contains("c")));
/// # Ok::<(), xenolith::Error>(())
/// ```
///
pub struct DocumentDtd {
  /// The validator built from the `DOCTYPE`, once one has been read.
  dtd: Option<DtdValidator>,
  /// Whether `xml:id` attributes are checked as IDs.
  xml_id: bool,
  /// The standalone `xml:id` check, used only for a document that turned out to have no DTD. With a DTD, the DTD
  /// validator makes the same check in its own ID space, so the two are never both in use.
  ids: Option<XmlIdValidator>,
  /// Whether the one-time setup at the first content event has run.
  lazy_done: bool,
}

impl DocumentDtd {
  /// Creates a validator with no DTD yet. It takes one from the document's `DOCTYPE`.
  #[must_use]
  pub fn new() -> Self {
    Self { dtd: None, xml_id: false, ids: None, lazy_done: false }
  }

  /// Also checks `xml:id` attributes as IDs. With a DTD, they fall into the DTD validator's ID space, so an `xml:id`
  /// and a declared `ID` of the same value collide; without one, they are checked on their own. Off by default.
  ///
  #[must_use]
  pub fn checking_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// Whether the document declared a DTD to be checked against. Meaningful once the run has reached the root element.
  ///
  /// A document with no `DOCTYPE` is not invalid; there was nothing for it to be valid against, which is a different
  /// answer from having broken a rule.
  ///
  #[must_use]
  pub fn had_dtd(&self) -> bool {
    self.dtd.is_some()
  }

  /// One-time setup at the first content event: with `xml:id` asked for and no DTD in hand, the `xml:id` values are
  /// checked on their own.
  fn ensure_lazy(&mut self) {
    if self.lazy_done {
      return;
    }
    self.lazy_done = true;
    if self.xml_id && self.dtd.is_none() {
      self.ids = Some(XmlIdValidator::new());
    }
  }
}

impl Default for DocumentDtd {
  fn default() -> Self {
    Self::new()
  }
}

impl std::fmt::Debug for DocumentDtd {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("DocumentDtd").field("had_dtd", &self.had_dtd()).finish_non_exhaustive()
  }
}

impl EventHandler for DocumentDtd {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if let EventRef::Doctype(event) = event {
      // The DTD is complete by this event, so the validator it builds is whole. A DOCTYPE with no name declares no
      // root, and there is nothing to build from it.
      if let Some(root) = event.name.and_then(|name| event.pool.get(name)) {
        let validator = DtdValidator::new(event.dtd.clone(), event.pool.fork(), Some(root));
        let validator = validator.with_xml_id(self.xml_id);
        self.dtd = Some(validator);
      }
      return Ok(());
    }

    self.ensure_lazy();
    if let Some(dtd) = &mut self.dtd {
      return dtd.handle(event);
    }
    if let Some(ids) = &mut self.ids {
      return ids.handle(event);
    }
    Ok(())
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    // Passed on to the validator this one delegates to, which may hold something of its own for the run.
    if let Some(dtd) = &mut self.dtd {
      dtd.finish(outcome);
    }
    if let Some(ids) = &mut self.ids {
      ids.finish(outcome);
    }
  }
}

impl Validator for DocumentDtd {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    // At most one of the two is ever in use: the standalone xml:id check is set up only for a document that reached
    // its content without a DOCTYPE.
    if let Some(dtd) = &self.dtd {
      return dtd.errors();
    }
    if let Some(ids) = &self.ids {
      return ids.errors();
    }
    std::borrow::Cow::Borrowed(&[])
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}
