//! [`Schema`] and [`Validator`] for the XInclude 1.0 specification vocabulary.

use crate::Error;
use crate::attr::AttributeRef;
use crate::chars::{is_enc_name, is_whitespace};
use crate::error::{Location, Result};
use crate::event::validate::Schema;
use crate::event::validate::{Validator, ValidityError};
use crate::event::{EventConsumer, EventRef, Flow, StartElementEventRef};

use super::XINCLUDE_NS;

/// A schema for document structures conforming to [XInclude 1.0 (Second Edition)]. A validator based on this schema
/// verifies that documents containing the `xi:include` and `xi:fallback` elements conform to the constraints specified
/// in §3.1 and §3.2.
///
/// Note that four of the validations based on this schema are stricter than the specification (the specification does
/// not treat any of these cases as fatal errors):
///
/// 1. The `encoding` attribute must not be specified when the `parse` attribute is omitted or set to `"xml"`. Section
///    3.1 of the specification merely states that the `encoding` attribute has no effect in such cases.
/// 2. The value of the `encoding` attribute must be in the format specified for `EncName` in XML 1.0. Section 3.1 of
///    the specification merely recommends that it "should be a valid encoding name."
/// 3. Rejection of elements that have the XInclude namespace but are not defined in XInclude. Sections 3.1 and 3.2 of
///    the specification stipulate that such elements are rejected only when they appear as children of the two defined
///    elements.
/// 4. Reject attributes with the XInclude namespace. Although §3.1 permits attributes other than those listed, the
///    schema in §3 allows all namespaces other than the one it defines.
///
/// [XInclude 1.0 (Second Edition)]: https://www.w3.org/TR/2006/REC-xinclude-20061115/
///
/// # Example
///
/// ```
/// use xenolith::event::validate::ValidatorSet;
/// use xenolith::event::{EventCursor, EventProducer};
/// use xenolith::io::StreamSource;
/// use xenolith::xinclude;
///
/// let xml = "<doc xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include parse='html'/></doc>";
/// let mut validation = ValidatorSet::new().add_schema(&xinclude::XIncludeSchema);
/// StreamSource::new(xml.as_bytes()).add_consumer(&mut validation).emit()?;
///
/// // Note that although there are two errors in a single xi:include element, the document loads completely without
/// // stopping at the first validation failure. Using with_max_exceptions(Some(0)) allows the code to throw an error
/// // upon the first validation failure.
/// let report = validation.report();
/// let messages: Vec<&str> = report.errors().iter().map(|error| error.message()).collect();
/// assert_eq!(messages.len(), 2, "{messages:?}");
/// assert!(messages.iter().any(|message| message.contains("href")), "{messages:?}");
/// assert!(messages.iter().any(|message| message.contains("parse")), "{messages:?}");
/// # Ok::<(), xenolith::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct XIncludeSchema;

impl Schema for XIncludeSchema {
  fn validator(&self) -> Box<dyn Validator> {
    Box::new(XIncludeValidator::new())
  }
}

/// A validator provided by [`Schema`]. It validates the structure of a document using the XInclude vocabulary.
#[derive(Debug, Default)]
struct XIncludeValidator {
  /// Validation failures found so far.
  errors: Vec<ValidityError>,
  /// A stack of elements that have been started.
  open: Vec<Open>,
}

/// The identifier of the element nested up to the current event position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Open {
  /// The `xi:include` element. Also, whether the `xi:fallback` element has already appeared as its child.
  Include { fallback: bool },
  /// The `xi:fallback` element. It may contain additional `xi:include` elements.
  Fallback,
  /// Other elements. These typically appear directly under `xi:include` and may contain `xi:include` elements.
  Other,
}

impl XIncludeValidator {
  fn new() -> Self {
    Self::default()
  }

  /// Record one invalidity error.
  fn report(&mut self, message: impl Into<String>, at: &Location) {
    self.errors.push(ValidityError::new(message, at.clone()));
  }

  /// Check one attribute written with a prefix.
  ///
  /// Attributes such as `href` referenced by XInclude are defined as having **no namespace**. However, regarding
  /// attributes without a prefix, the specification states that "The namespace name for an unprefixed attribute name
  /// always has no value" and "default namespace declarations do not apply directly to attribute names" (Namespaces in
  /// XML §6.2). Therefore, `xi:href` is defined as a separate attribute from `href`, as described in §3.1.
  ///
  /// The schema for `xi:include` in §3 is defined as `anyAttribute namespace="##other"`, which permits attributes from
  /// any namespace **other than** the namespace defined by the schema itself (i.e., [`XINCLUDE_NS`]). Therefore,
  /// attributes belonging to the [`XINCLUDE_NS`] namespace are considered invalid.
  fn prefixed(&mut self, attribute: &AttributeRef<'_>) {
    if attribute.namespace == Some(XINCLUDE_NS) {
      let message = format!("xi:{} is not an attribute XInclude defines", attribute.local);
      self.report(message, &attribute.location);
    }
  }

  /// Validate a single `xi:include` start element and its attributes.
  fn include(&mut self, start: &StartElementEventRef<'_>) {
    let at = &start.location;
    // §3.1: "The occurrence of two or more `xi:fallback` elements, `xi:include` elements, or other elements belonging
    // to the `XInclude` namespace constitutes a fatal error." Within an `xi:fallback`, an `xi:include` element may be
    // placed anywhere (see §3.1 and §3.2), but it may not be placed immediately beneath an `xi:include` element.
    if let Some(Open::Include { .. }) = self.open.last() {
      self.report("an xi:include may hold another only within its xi:fallback", at);
    }
    let mut has_href = false;
    let mut has_xpointer = false;
    let mut is_text = false;
    let mut encoding: Option<Location> = None;
    for attribute in start.attributes.iter() {
      if attribute.namespace.is_some() {
        self.prefixed(&attribute);
        continue;
      }
      match attribute.local {
        "href" => {
          // §3.1: "Fragment identifiers must not be used; their presence constitutes a fatal error." It cannot specify
          // the fragment `#` as the value of `href`.
          if attribute.value.contains('#') {
            self.report("href may not hold a fragment identifier", &attribute.value_location);
          }
          has_href = true;
        }
        "parse" => {
          // §3.1: "Values other than "xml" and "text" are a fatal error."
          is_text = attribute.value == "text";
          if attribute.value != "xml" && !is_text {
            self.report(
              format!("parse={:?} is neither \"xml\" nor \"text\"", attribute.value),
              &attribute.value_location,
            );
          }
        }
        "xpointer" => has_xpointer = true,
        "encoding" => {
          // §3.1: "The value of this attribute should be a valid encoding name." Here, we will verify only the name
          // itself.
          if !is_enc_name(attribute.value) {
            self.report(format!("encoding={:?} is not an encoding name", attribute.value), &attribute.value_location);
          }
          encoding = Some(attribute.location);
        }
        "accept" | "accept-language" => {
          // §3.1: "Values containing characters outside the range #x20 through #x7E are disallowed in HTTP headers,
          // and must be flagged as fatal errors."
          if let Some(bad) = attribute.value.chars().find(|c| !matches!(c, '\u{20}'..='\u{7e}')) {
            let local = attribute.local;
            let message = format!("{local} may not hold U+{:04X}, which an HTTP header may not carry", u32::from(bad));
            self.report(message, &attribute.value_location);
          }
        }
        _ => {
          // §3.1: "Unprefixed attribute names are reserved for future versions of this specification, and must be
          // ignored by XInclude 1.0 processors."
        }
      }
    }

    if !has_xpointer {
      // §3.1: "If the xpointer attribute is absent, the href attribute must be present." It must be specified either
      // the `xpointer` or `href` attribute. Omitting `href` is equivalent to `href=""`, which means it refers to the
      // document itself.
      if !has_href {
        self.report("an xi:include without an xpointer needs an href", at);
      }
    } else if is_text {
      // §3.1: "The xpointer attribute must not be present when parse="text"." The `xpointer` attribute cannot be
      // specified for resources that are read as text.
      self.report("xpointer may not be present where parse=\"text\"", at);
    }
    if !is_text {
      if let Some(at) = encoding {
        // Since XML documents specify their own encoding, the `encoding` attribute cannot be specified. As noted in
        // the documentation for this schema, this is stricter than the specification in §3.1.
        self.report("encoding has no effect where parse=\"xml\"; an XML resource declares its own", &at);
      }
    }
  }

  /// Validate a single `xi:fallback` element.
  fn fallback(&mut self, start: &StartElementEventRef<'_>) {
    // set a flag and first close any open elements before reporting anything
    let message = match self.open.last_mut() {
      Some(Open::Include { fallback }) => {
        // §3.1: "The appearance of more than one xi:fallback element ... is a fatal error." Only one `xi:fallback`
        // element can be specified directly under `xi:include`. The same applies to resource loading errors discussed
        // in §4.4.: "It is a fatal error if there is zero or more than one xi:fallback element."
        let second = *fallback;
        *fallback = true;
        second.then_some("an xi:include may have only one xi:fallback")
      }
      _ => {
        // §3.2: "It is a fatal error for an xi:fallback element to appear in a document anywhere other than as the
        // direct child of the xi:include."
        Some("xi:fallback may appear only as the direct child of xi:include")
      }
    };
    if let Some(message) = message {
      self.report(message, &start.location);
    }
    // rejects attributes with the XInclude namespace, just like `xi:include`
    for attribute in start.attributes.iter() {
      self.prefixed(&attribute);
    }
  }
}

impl EventConsumer for XIncludeValidator {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    let before = self.errors.len();
    match event {
      EventRef::StartDocument => {
        // clear the previous state at the beginning of the document
        self.errors.clear();
        self.open.clear();
      }
      EventRef::StartElement(start) => {
        let open = if start.namespace == Some(XINCLUDE_NS) {
          match start.local {
            "include" => {
              self.include(start);
              Open::Include { fallback: false }
            }
            "fallback" => {
              self.fallback(start);
              Open::Fallback
            }
            // Rejects unknown elements with the "XInclude" namespace. As noted in the documentation for this schema,
            // this behavior is stricter than the specification.
            other => {
              let message = format!("xi:{other} is not an element XInclude defines");
              self.report(message, &start.location);
              Open::Other
            }
          }
        } else {
          Open::Other
        };
        self.open.push(open);
      }
      EventRef::EndElement(_) => {
        self.open.pop();
      }
      _ => {}
    }
    // A `StartDocument` clears the list, so what it held before is not counted against this event.
    Ok(Flow::Continue(self.errors.len().saturating_sub(before)))
  }
}

impl Validator for XIncludeValidator {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_consumer(&mut self) -> &mut dyn EventConsumer {
    self
  }
}

/// Verifies that the document included as the root of the document itself has the correct document structure.
///
/// Immediately beneath the document, one root element, multiple comments, multiple processing instructions, and
/// multiple whitespace characters are permitted. If the document's root element is `xi:include` and the content it
/// includes or the result of `xi:fallback` contains multiple elements, or if it consists of text containing
/// non-whitespace characters, this must be treated as an invalid document structure and result in a fatal error
/// (§4.5): "It is a fatal error to attempt to replace an xi:include element appearing as the document (top-level)
/// element in the source infoset with something other than a list of zero or more comments, zero or more processing
/// instructions, and one element." Since the document itself — or the document being imported — is in a structured
/// format, if such a structure appears when it is sent to the subsequent consumer, it must have been introduced by
/// `xi:include`.
///
/// If the inclusion removes the root element entirely, this also violates the "one element" clause and is therefore
/// treated as a fatal error.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RootElementConstraints {
  /// The number of open elements.
  depth: usize,
  /// The number of elements observed at a position where none of the elements have started (the position of the root
  /// element).
  elements: usize,
}

impl EventConsumer for RootElementConstraints {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    if self.depth == 0 {
      match event {
        EventRef::StartElement(start) => {
          self.elements += 1;
          if self.elements > 1 {
            let message = "an inclusion put more than one element where the document element goes";
            return Err(Error::xinclude(message).at(start.location.clone()));
          }
        }
        EventRef::Characters(text) if !text.text.chars().all(is_whitespace) => {
          let message = "an inclusion put character data where the document element goes";
          return Err(Error::xinclude(message).at(text.location.clone()));
        }
        EventRef::Cdata(cdata) => {
          let message = "an inclusion put character data where the document element goes";
          return Err(Error::xinclude(message).at(cdata.location.clone()));
        }
        EventRef::EndDocument if self.elements == 0 => {
          // Since `EndDocument` has no position, the location of the error remains unknown.
          let message = "an inclusion left no element where the document element goes";
          return Err(Error::xinclude(message));
        }
        EventRef::StartDocument => *self = Self::default(),
        _ => {}
      }
    }
    match event {
      EventRef::StartElement(..) => self.depth += 1,
      EventRef::EndElement(..) => self.depth = self.depth.saturating_sub(1),
      _ => {}
    }
    // A violation refuses the document, so no error is ever recorded to be counted.
    Ok(Flow::Continue(0))
  }
}
