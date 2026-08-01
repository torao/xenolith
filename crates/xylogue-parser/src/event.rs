//! Events that own what they hold.
//!
//! The cursor API of [`Parser`] borrows from the parser, which costs nothing but means an
//! event cannot outlive the call that produced it and cannot be collected into a `Vec`. An
//! [`Event`] is the same information, copied. Reach for it when the borrow is in the way, and
//! stay with the cursor when it is not.

use xylogue_core::name::QName;

use crate::parser::{EventKind, Parser, XmlSpace};

/// An attribute, with its value copied out of the parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
  /// The attribute's name. An unprefixed attribute is in no namespace.
  pub name: QName,
  /// The normalized value.
  pub value: String,
  /// True if this attribute is a namespace declaration (`xmlns` or `xmlns:p`).
  pub declares_namespace: bool,
}

/// One parse event, owning its data.
///
/// # Examples
///
/// ```
/// use xylogue_parser::{Event, Parser};
///
/// let mut parser = Parser::new();
/// parser.feed(b"<a>hi</a>", true)?;
///
/// let events: Vec<Event> = parser.events().collect::<Result<_, _>>()?;
/// assert_eq!(events.len(), 3);
/// assert_eq!(events[1].text(), Some("hi"));
/// # Ok::<(), xylogue_core::Error>(())
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
  /// A document type declaration, kept verbatim until phase 2 interprets it.
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
  /// Character data, with references expanded.
  Text(String),
  /// The content of a CDATA section.
  CData(String),
  /// A comment, without its delimiters.
  Comment(String),
  /// A processing instruction.
  ProcessingInstruction {
    /// The target.
    target: String,
    /// Everything after the target and the whitespace following it.
    data: String,
  },
}

impl Event {
  /// Copies the parser's current event.
  ///
  /// # Panics
  ///
  /// If the parser has no current event, which is the case before the first call to
  /// [`Parser::advance`] and after it reports [`Progress::Eof`](crate::Progress::Eof).
  #[must_use]
  pub fn capture(parser: &Parser) -> Self {
    let kind = parser.event().expect("the parser has no current event to capture");
    match kind {
      EventKind::XmlDeclaration => Self::XmlDeclaration {
        version: parser.version().to_owned(),
        encoding: parser.declared_encoding().map(ToOwned::to_owned),
        standalone: parser.standalone(),
      },
      EventKind::Doctype => Self::Doctype(parser.text().to_owned()),
      EventKind::StartElement => Self::StartElement {
        name: parser.name(),
        attributes: parser
          .attributes()
          .map(|a| Attribute { name: a.name, value: a.value.to_owned(), declares_namespace: a.declares_namespace })
          .collect(),
        xml_space: parser.xml_space(),
        xml_lang: parser.xml_lang().map(ToOwned::to_owned),
      },
      EventKind::EndElement => Self::EndElement { name: parser.name() },
      EventKind::Text => Self::Text(parser.text().to_owned()),
      EventKind::CData => Self::CData(parser.text().to_owned()),
      EventKind::Comment => Self::Comment(parser.text().to_owned()),
      EventKind::ProcessingInstruction => {
        Self::ProcessingInstruction { target: parser.target().to_owned(), data: parser.text().to_owned() }
      }
    }
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

  /// The element name, for a start or end element.
  #[must_use]
  pub const fn name(&self) -> Option<QName> {
    match self {
      Self::StartElement { name, .. } | Self::EndElement { name } => Some(*name),
      _ => None,
    }
  }

  /// The character data of a text, CDATA or comment event.
  ///
  /// Note that this does not cover the data of a processing instruction or the body of a
  /// document type declaration, which are not character data.
  #[must_use]
  pub fn text(&self) -> Option<&str> {
    match self {
      Self::Text(text) | Self::CData(text) | Self::Comment(text) => Some(text),
      _ => None,
    }
  }

  /// The attributes of a start element.
  #[must_use]
  pub fn attributes(&self) -> &[Attribute] {
    match self {
      Self::StartElement { attributes, .. } => attributes,
      _ => &[],
    }
  }
}

#[cfg(test)]
mod tests {
  use xylogue_core::error::Result;

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
      borrowed.push(Event::capture(&parser));
    }
    assert_eq!(owned, borrowed);
  }
}
