//! The ID space shared by DTD `ID` attributes and `xml:id`.
//!
//! xml:id §4 places an `xml:id` value in the same ID space as any declared `ID`: it must be a
//! valid `NCName` and unique across the document. The check here is reused by the
//! [`DtdValidator`](crate::dtd::DtdValidator), so an `xml:id` and a declared `ID` with the same
//! value collide as they should; and it stands alone as [`XmlIdValidator`] for a document that
//! has no DTD.

use std::collections::HashMap;
use std::ops::ControlFlow;

use xenolith_core::error::Location;
use xenolith_core::name::{NameId, NamePool, QName};
use xenolith_parser::AttributeRef;

use crate::{ErrorListener, Validator, ValidityError};

/// Checks one `xml:id` value against `ids`: a valid `NCName`, then unique. `ids` is the same
/// table the DTD validator records `ID` attributes in, so the two kinds of ID share one space.
pub(crate) fn check_xml_id(
  value: &str,
  at: &Location,
  ids: &mut HashMap<String, Location>,
  errors: &mut dyn ErrorListener,
) -> ControlFlow<()> {
  // xml:id §4: an xml:id whose normalized value is not an NCName is an xml:id error.
  if !xenolith_core::chars::is_ncname(value) {
    errors.report(ValidityError::new(format!("xml:id value \"{value}\" is not an NCName"), at.clone()))?;
  }
  if ids.insert(value.to_owned(), at.clone()).is_some() {
    return errors.report(ValidityError::new(format!("xml:id \"{value}\" is used more than once"), at.clone()));
  }
  ControlFlow::Continue(())
}

/// The value of the `xml:id` attribute of a start tag, if it has one.
pub(crate) fn xml_id_of<'a>(attributes: &'a [AttributeRef<'_>], pool: &NamePool) -> Option<&'a str> {
  attributes.iter().find(|a| is_xml_id(a.name, pool)).map(|a| a.value)
}

/// Whether a resolved attribute name is `xml:id`.
pub(crate) fn is_xml_id(name: QName, pool: &NamePool) -> bool {
  name.namespace() == Some(NameId::XML_NS) && pool.resolve(name.local()) == "id"
}

/// Checks `xml:id` attributes for a document with no DTD: each a valid `NCName`, unique.
///
/// When a document has a DTD, the [`DtdValidator`](crate::dtd::DtdValidator) does this itself,
/// so the ID space stays unified. This standalone validator is for the case that there is no
/// DTD to fold it into.
#[derive(Debug, Default)]
pub struct XmlIdValidator {
  ids: HashMap<String, Location>,
}

impl XmlIdValidator {
  /// Creates an xml:id validator.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }
}

impl Validator for XmlIdValidator {
  fn start_element(
    &mut self,
    _name: QName,
    attributes: &[AttributeRef<'_>],
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    match xml_id_of(attributes, pool) {
      Some(value) => check_xml_id(value, at, &mut self.ids, errors),
      None => ControlFlow::Continue(()),
    }
  }

  fn characters(
    &mut self,
    _text: &str,
    _whitespace_only: bool,
    _pool: &NamePool,
    _at: &Location,
    _errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }

  fn end_element(
    &mut self,
    _name: QName,
    _pool: &NamePool,
    _at: &Location,
    _errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }

  fn finish(&mut self, _pool: &NamePool, _errors: &mut dyn ErrorListener) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }
}
