//! The ID space shared by DTD `ID` attributes and `xml:id`.
//!
//! xml:id §4 places an `xml:id` value in the same ID space as any declared `ID`: it must be a valid `NCName` and
//! unique across the document. The [`DtdValidator`](crate::dtd::DtdValidator) runs the check here against the table it
//! uses to record declared `ID` values, so an `xml:id` and an `ID` with the same value collide. For a document with no
//! DTD, [`XmlIdValidator`] runs the same check itself.
//!

use std::collections::HashMap;
use std::ops::ControlFlow;

use xenolith_core::attr::Attributes;
use xenolith_core::error::Location;
use xenolith_core::name::{NameId, NamePool, QName};

use crate::{ErrorListener, Validator, ValidityError};

/// Checks one `xml:id` value against `ids`: a valid `NCName`, then unique. `ids` is the same table the DTD validator
/// records `ID` attributes in, so the two kinds of ID share one space.
pub(crate) fn check_xml_id(
  value: &str,
  at: &Location,
  ids: &mut HashMap<String, Location>,
  errors: &mut dyn ErrorListener,
) -> ControlFlow<()> {
  // xml:id §4: the value arrives already normalized, since the parser types `xml:id` as an ID, so check it as it
  // stands. A value that is not an NCName is still recorded below, so a document that repeats it hears about that too.
  if !xenolith_core::chars::is_ncname(value) {
    errors.report(ValidityError::new(format!("xml:id value \"{value}\" is not an NCName"), at.clone()))?;
  }
  if ids.insert(value.to_owned(), at.clone()).is_some() {
    return errors.report(ValidityError::new(format!("xml:id \"{value}\" is used more than once"), at.clone()));
  }
  ControlFlow::Continue(())
}

/// The value of the `xml:id` attribute among `attributes`, if there is one.
pub(crate) fn xml_id_of<'a>(attributes: Attributes<'a>, pool: &NamePool) -> Option<&'a str> {
  attributes.iter().find(|a| is_xml_id(a.name, pool)).map(|a| a.value)
}

/// Whether a resolved attribute name is `xml:id`, matched on the XML namespace rather than on the
/// prefix text.
pub(crate) fn is_xml_id(name: QName, pool: &NamePool) -> bool {
  name.namespace() == Some(NameId::XML_NS) && pool.resolve(name.local()) == "id"
}

/// Checks a document's `xml:id` attributes: each is a valid `NCName` and unique across the document.
///
/// Use it when the document has no DTD. When it does, the [`DtdValidator`](crate::dtd::DtdValidator) makes these
/// checks itself, recording `xml:id` values in the same table as the declared `ID` values so both kinds share one ID
/// space. Adding this validator alongside it would report each `xml:id` fault twice.
///
#[derive(Debug, Default)]
pub struct XmlIdValidator {
  ids: HashMap<String, Location>,
}

impl XmlIdValidator {
  /// Creates a validator with an empty ID space.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }
}

impl Validator for XmlIdValidator {
  fn start_element(
    &mut self,
    _name: QName,
    attributes: Attributes<'_>,
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

  fn finish(&mut self, _errors: &mut dyn ErrorListener) -> ControlFlow<()> {
    ControlFlow::Continue(())
  }
}
