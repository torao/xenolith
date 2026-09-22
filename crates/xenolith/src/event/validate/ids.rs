//! Validation of the `xml:id` attribute within an ID space shared with DTD `ID` attributes.
//!
//! The xml:id specification (§4) mandates that the value of an `xml:id` attribute must be an `NCName` and must be
//! unique among all IDs in the document, including attributes declared as `ID` in a DTD. To ensure that an `xml:id`
//! and a DTD, declared `ID` sharing the same value are reported as duplicates,
//! [`DtdValidator`](crate::dtd::validate::DtdValidator) records them in a shared table and invokes this module's
//! validation logic. For documents lacking a DTD, [`XmlIdValidator`] performs a similar check using its own internal
//! table.

use std::collections::HashMap;

use crate::attr::{AttributeRef, Attributes};
use crate::error::{Location, Result};
use crate::event::{EventHandler, EventRef};
use crate::name::XML_NS_URI;

use crate::event::validate::{Validator, ValidityError};

/// Checks a single `xml:id` attribute. Specifically, it verifies that the value is an `NCName` and that it is not
/// already present in `ids` (a table tracking IDs encountered earlier in the document). Since the DTD validator also
/// records declared `ID` values in this table as it processes them, uniqueness is verified collectively for both
/// types of IDs. If a violation is found, it is added to `errors` in association with the value.
pub(crate) fn check_xml_id(
  attribute: &AttributeRef<'_>,
  ids: &mut HashMap<String, Location>,
  errors: &mut Vec<ValidityError>,
) {
  let at = &attribute.value_location;
  // For `xml:id` (§4), the value is normalized as an attribute of type ID before it is checked. While the parser
  // performs this processing automatically when `ParserConfig::xml_id` is enabled, values obtained from other sources
  // may contain leading, trailing, or duplicate whitespace. Normalizing a value that has already been normalized
  // yields the same result.
  let value = normalized(attribute.value);
  let value = value.as_str();
  if !crate::chars::is_ncname(value) {
    errors.push(ValidityError::new(format!("xml:id value \"{value}\" is not an NCName"), at.clone()));
  }
  // A value that is not an NCName is still recorded, so that a later repetition of it is reported as well.
  if ids.insert(value.to_owned(), at.clone()).is_some() {
    errors.push(ValidityError::new(format!("xml:id \"{value}\" is used more than once"), at.clone()));
  }
}

/// Normalized `value` for an attribute of type ID (XML 1.0 §3.3.3): all whitespace characters are replaced by spaces,
/// leading and trailing spaces are removed, and consecutive spaces are collapsed into a single space.
fn normalized(value: &str) -> String {
  value.split(crate::chars::is_whitespace).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The first `xml:id` attribute within `attributes`. Returns `None` if it does not exist.
pub(crate) fn xml_id_of<'a>(attributes: Attributes<'a>) -> Option<AttributeRef<'a>> {
  attributes.iter().find(is_xml_id)
}

/// Whether the attribute is `xml:id`. It is identified by the XML namespace and the local name `id`, rather than by
/// the written prefix.
pub(crate) fn is_xml_id(attribute: &AttributeRef<'_>) -> bool {
  attribute.namespace == Some(XML_NS_URI) && attribute.local == "id"
}

/// A [`Validator`] that verifies that all `xml:id` values in the document are `NCName`s and are unique within the
/// document.
///
/// Use this for documents that do not have a DTD. For documents with a DTD, validation is performed by a
/// [`DtdValidator`] with [`with_xml_id`] enabled (in which case `xml:id` values and declared `ID` values are
/// recorded in a single table). If this validator is applied in addition to that, `xml:id` violations will be reported
/// twice, and it will fail to detect `xml:id` values that conflict with declared `ID`s.
///
/// [`DtdValidator`]: crate::dtd::validate::DtdValidator
/// [`with_xml_id`]: crate::dtd::validate::DtdValidator::with_xml_id
#[derive(Debug, Default)]
pub struct XmlIdValidator {
  /// `xml:id` values observed so far in the document and their respective first occurrences.
  ids: HashMap<String, Location>,
  /// Violations detected so far, in document order.
  errors: Vec<ValidityError>,
}

impl XmlIdValidator {
  /// Create a validator that has not yet encountered any `xml:id` values.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }
}

impl EventHandler for XmlIdValidator {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    // Only a start element carries attributes, so no other event can hold an `xml:id`.
    let EventRef::StartElement(event) = event else { return Ok(()) };
    if let Some(attribute) = xml_id_of(event.attributes) {
      check_xml_id(&attribute, &mut self.ids, &mut self.errors);
    }
    Ok(())
  }
}

impl Validator for XmlIdValidator {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}
