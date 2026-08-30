//! Events that own their data.
//!
//! You can read what the parser produces in two ways. Driving [`Parser::advance`] and reading the current event
//! through [`Parser::event_ref`], whose [`EventRef`] borrows the parser, copies nothing, but that value borrows the
//! parser, so the next [`advance`](Parser::advance) invalidates it, and it cannot be stored or collected.
//!
//! An [`Event`] is one event copied into an owned value, through [`Event::capture`] or the [`events`](Parser::events)
//! iterator. It can outlive the call, go into a `Vec`, and be compared or sent elsewhere. Use it when you need to keep
//! events; stay with the borrowing accessors when you handle each one in place.
//!

use xenolith_core::error::{Error, Result};
use xenolith_core::name::QName;

use crate::parser::{EventKind, EventRef, Parser, XmlSpace};

/// An attribute with its value owned; the copied counterpart of [`AttributeRef`](crate::AttributeRef).
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
  /// The attribute's name. An unprefixed attribute is in no namespace, never the default one.
  pub name: QName,
  /// The value after attribute-value normalization (XML 1.0 §3.3.3), and the tokenized collapse the DTD applies when
  /// the attribute has a tokenized type.
  pub value: String,
  /// True if this attribute is a namespace declaration (`xmlns` or `xmlns:p`).
  pub declares_namespace: bool,
}

/// One parse event, owning its data.
///
/// # Examples
///
/// ```
/// use xenolith_parser::{Event, Parser};
///
/// let mut parser = Parser::new();
/// parser.feed(b"<a>hi</a>", true)?;
///
/// let events: Vec<Event> = parser.events().collect::<Result<_, _>>()?;
/// assert_eq!(events.len(), 3);
/// assert_eq!(events[1].text(), Some("hi"));
/// # Ok::<(), xenolith_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
  /// The XML declaration.
  XmlDeclaration {
    /// The version, always `1.x`.
    version: String,
    /// The declared encoding, which need not be the one actually in use.
    encoding: Option<String>,
    /// The standalone declaration, if there was one.
    standalone: Option<bool>,
  },
  /// A document type declaration, the whole `<!DOCTYPE ...>` text held verbatim.
  Doctype(String),
  /// The start of an element.
  StartElement {
    /// The element's name.
    name: QName,
    /// The attributes, in document order, namespace declarations included.
    attributes: Vec<Attribute>,
    /// The value of `xml:space` in effect inside this element.
    xml_space: XmlSpace,
    /// The value of `xml:lang` in effect inside this element.
    xml_lang: Option<String>,
  },
  /// The end of an element, including the implied end of an empty one.
  EndElement {
    /// The element's name.
    name: QName,
  },
  /// Character data, with references expanded. One run may arrive as several adjacent `Text` events; a consumer that
  /// wants one maximal node coalesces them.
  Text(String),
  /// The content of a CDATA section: everything between `<![CDATA[` and `]]>`, with no reference expansion and nothing
  /// trimmed.
  CData(String),
  /// A comment's text: everything between `<!--` and `-->`, verbatim.
  Comment(String),
  /// A processing instruction.
  ProcessingInstruction {
    /// The target: the name right after `<?`, ending at the first whitespace, or at `?>` when there is no data.
    target: String,
    /// Everything after the target and the whitespace separating it, up to `?>`: the separating whitespace is dropped,
    /// nothing else is trimmed, and it is empty when the instruction is only a target.
    data: String,
  },
}

impl Event {
  /// Copies the parser's current event into an owned [`Event`].
  ///
  /// Call this while driving [`Parser::advance`] yourself to keep the current event past the next call. It is the
  /// primitive the [`events`](Parser::events) iterator is built on; reach for it directly when that iterator will not
  /// do, for instance, when you feed the input in pieces or resolve external entities as you go.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_parser::{Event, Parser, Progress};
  ///
  /// let mut parser = Parser::new();
  /// parser.feed(b"<a>hi</a>", true)?;
  ///
  /// let mut events = Vec::new();
  /// while let Progress::Event(_) = parser.advance()? {
  ///   events.push(Event::capture(&parser)?);
  /// }
  /// assert_eq!(events.len(), 3);
  /// assert_eq!(events[1].text(), Some("hi"));
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`Error::Internal`] if the parser has no current event, which is the case before the first
  /// [`Parser::advance`] and after it reports [`Progress::Eof`](crate::Progress::Eof).
  ///
  pub fn capture(parser: &Parser) -> Result<Self> {
    parser
      .event_ref()
      .map(Into::into)
      .ok_or_else(|| Error::internal("capture called while the parser has no current event"))
  }

  /// Which kind of event this is.
  #[must_use]
  pub const fn kind(&self) -> EventKind {
    match self {
      Self::XmlDeclaration { .. } => EventKind::XmlDeclaration,
      Self::Doctype(_) => EventKind::Doctype,
      Self::StartElement { .. } => EventKind::StartElement,
      Self::EndElement { .. } => EventKind::EndElement,
      Self::Text(_) => EventKind::Text,
      Self::CData(_) => EventKind::CData,
      Self::Comment(_) => EventKind::Comment,
      Self::ProcessingInstruction { .. } => EventKind::ProcessingInstruction,
    }
  }

  /// The element's name for a start or end element, or `None` for other kinds.
  #[must_use]
  pub const fn name(&self) -> Option<QName> {
    match self {
      Self::StartElement { name, .. } | Self::EndElement { name } => Some(*name),
      _ => None,
    }
  }

  /// The character data of a text, CDATA, or comment event, or `None` for other kinds.
  ///
  /// It does not cover a processing instruction's data or a `DOCTYPE`'s body, which are not character data.
  #[must_use]
  pub fn text(&self) -> Option<&str> {
    match self {
      Self::Text(text) | Self::CData(text) | Self::Comment(text) => Some(text),
      _ => None,
    }
  }

  /// The attributes of a start element, or an empty slice for other kinds.
  #[must_use]
  pub fn attributes(&self) -> &[Attribute] {
    match self {
      Self::StartElement { attributes, .. } => attributes,
      _ => &[],
    }
  }
}

impl From<EventRef<'_>> for Event {
  /// Copies a borrowed [`EventRef`] into an owned [`Event`]; this is how [`capture`](Event::capture) owns the current
  /// event.
  fn from(event: EventRef<'_>) -> Self {
    match event {
      EventRef::XmlDeclaration { version, encoding, standalone } => {
        Self::XmlDeclaration { version: version.to_owned(), encoding: encoding.map(ToOwned::to_owned), standalone }
      }
      EventRef::Doctype(text) => Self::Doctype(text.to_owned()),
      EventRef::StartElement { name, attributes, xml_space, xml_lang } => Self::StartElement {
        name,
        attributes: attributes
          .iter()
          .map(|a| Attribute { name: a.name, value: a.value.to_owned(), declares_namespace: a.declares_namespace })
          .collect(),
        xml_space,
        xml_lang: xml_lang.map(ToOwned::to_owned),
      },
      EventRef::EndElement { name } => Self::EndElement { name },
      EventRef::Text(text) => Self::Text(text.to_owned()),
      EventRef::CData(text) => Self::CData(text.to_owned()),
      EventRef::Comment(text) => Self::Comment(text.to_owned()),
      EventRef::ProcessingInstruction { target, data, .. } => {
        Self::ProcessingInstruction { target: target.to_owned(), data: data.to_owned() }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use xenolith_core::error::Result;

  use super::*;
  use crate::Progress;

  fn events(xml: &str) -> Result<Vec<Event>> {
    let mut parser = Parser::new();
    parser.feed(xml.as_bytes(), true)?;
    parser.events().collect()
  }

  #[test]
  fn captures_every_kind() {
    let xml = "<?xml version='1.0' standalone='yes'?><!DOCTYPE a><!--c--><a x='1'><?p d?><![CDATA[<]]>t</a>";
    let events = events(xml).unwrap();
    let kinds: Vec<_> = events.iter().map(Event::kind).collect();
    assert_eq!(
      kinds,
      [
        EventKind::XmlDeclaration,
        EventKind::Doctype,
        EventKind::Comment,
        EventKind::StartElement,
        EventKind::ProcessingInstruction,
        EventKind::CData,
        EventKind::Text,
        EventKind::EndElement,
      ]
    );
    assert_eq!(events[0], Event::XmlDeclaration { version: "1.0".to_owned(), encoding: None, standalone: Some(true) });
    assert_eq!(events[4], Event::ProcessingInstruction { target: "p".to_owned(), data: "d".to_owned() });
    assert_eq!(events[5].text(), Some("<"));
  }

  #[test]
  fn events_outlive_the_call_that_produced_them() {
    // The point of owning: they can be collected and inspected afterwards.
    let events = events("<a xmlns:p='urn:p' p:x='1'/>").unwrap();
    let attributes = events[0].attributes();
    assert_eq!(attributes.len(), 2);
    assert_eq!(attributes[0].value, "urn:p");
    assert!(attributes[0].declares_namespace);
    assert_eq!(attributes[1].value, "1");
    assert!(!attributes[1].declares_namespace);
  }

  #[test]
  fn carries_xml_space_and_lang() {
    let events = events("<a xml:space='preserve' xml:lang='ja'/>").unwrap();
    let Event::StartElement { xml_space, xml_lang, .. } = &events[0] else { panic!("expected a start element") };
    assert_eq!(*xml_space, XmlSpace::Preserve);
    assert_eq!(xml_lang.as_deref(), Some("ja"));
  }

  #[test]
  fn the_iterator_stops_at_the_first_error() {
    let mut parser = Parser::new();
    parser.feed(b"<a>&nosuch;</a>", true).unwrap();
    let events: Vec<_> = parser.events().collect();
    assert_eq!(events.len(), 2, "the start element, then the failure");
    assert!(events[0].is_ok());
    assert!(events[1].is_err());
  }

  #[test]
  fn owned_and_borrowed_agree() {
    let xml = "<a xmlns='urn:d' x='1'>text<b/></a>";
    let owned = events(xml).unwrap();

    let mut parser = Parser::new();
    parser.feed(xml.as_bytes(), true).unwrap();
    let mut borrowed = Vec::new();
    while let Progress::Event(_) = parser.advance().unwrap() {
      borrowed.push(Event::capture(&parser).unwrap());
    }
    assert_eq!(owned, borrowed);
  }
}
