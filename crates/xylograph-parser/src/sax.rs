//! A push interface over the pull parser: SAX-style event callbacks.
//!
//! The parser is a pull API — you ask it for the next event. Some code reads more naturally the
//! other way round, with the parser calling you. [`drive`] does that: it runs a [`Reader`] to
//! the end and calls a [`Handler`] for each event, the shape of SAX's `ContentHandler`. The
//! element callbacks receive the [`Parser`] so a handler reads names, namespaces and attributes
//! through its accessors without anything being copied.
//!
//! # Examples
//!
//! ```
//! use xylograph_parser::{Parser, Reader};
//! use xylograph_parser::sax::{Handler, drive};
//!
//! #[derive(Default)]
//! struct Depth { max: usize, current: usize }
//!
//! impl Handler for Depth {
//!   fn start_element(&mut self, _parser: &Parser) {
//!     self.current += 1;
//!     self.max = self.max.max(self.current);
//!   }
//!   fn end_element(&mut self, _parser: &Parser) {
//!     self.current -= 1;
//!   }
//! }
//!
//! let mut handler = Depth::default();
//! drive(&mut Reader::new("<a><b><c/></b></a>".as_bytes()), &mut handler)?;
//! assert_eq!(handler.max, 3);
//! # Ok::<(), xylograph_core::Error>(())
//! ```

use std::io::Read;

use xylograph_core::error::Result;

use crate::parser::{EventKind, Parser};
use crate::reader::Reader;

/// Receives parser events as they are read. Every method has a default that does nothing, so a
/// handler overrides only what it cares about.
///
/// The element methods are handed the [`Parser`] positioned on the event; read the name with
/// [`local_name`](Parser::local_name) and its neighbours, and the attributes with
/// [`attributes`](Parser::attributes).
pub trait Handler {
  /// Before any other event.
  fn start_document(&mut self) {}

  /// After the last event, once the document has been read in full.
  fn end_document(&mut self) {}

  /// The start of an element.
  fn start_element(&mut self, parser: &Parser) {
    let _ = parser;
  }

  /// The end of an element.
  fn end_element(&mut self, parser: &Parser) {
    let _ = parser;
  }

  /// Character data (a run of text).
  fn characters(&mut self, text: &str) {
    let _ = text;
  }

  /// The content of a CDATA section, reported separately from ordinary text.
  fn cdata(&mut self, text: &str) {
    let _ = text;
  }

  /// A comment's text.
  fn comment(&mut self, text: &str) {
    let _ = text;
  }

  /// A processing instruction.
  fn processing_instruction(&mut self, target: &str, data: &str) {
    let (_, _) = (target, data);
  }

  /// The document type declaration; read its name and identifiers from the parser.
  fn doctype(&mut self, parser: &Parser) {
    let _ = parser;
  }
}

/// Runs `reader` to the end, calling `handler` for each event.
///
/// # Errors
///
/// Returns the parser's error if the document is not well-formed, or if reading fails.
pub fn drive<R: Read, H: Handler>(reader: &mut Reader<R>, handler: &mut H) -> Result<()> {
  handler.start_document();
  while let Some(kind) = reader.advance()? {
    let parser = reader.parser();
    match kind {
      EventKind::StartElement => handler.start_element(parser),
      EventKind::EndElement => handler.end_element(parser),
      EventKind::Text => handler.characters(parser.text()),
      EventKind::CData => handler.cdata(parser.text()),
      EventKind::Comment => handler.comment(parser.text()),
      EventKind::ProcessingInstruction => handler.processing_instruction(parser.target(), parser.text()),
      EventKind::Doctype => handler.doctype(parser),
      // The XML declaration carries no content a SAX handler models.
      EventKind::XmlDeclaration => {}
    }
  }
  handler.end_document();
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[derive(Default)]
  struct Trace(Vec<String>);

  impl Handler for Trace {
    fn start_document(&mut self) {
      self.0.push("start".to_owned());
    }
    fn end_document(&mut self) {
      self.0.push("end".to_owned());
    }
    fn start_element(&mut self, parser: &Parser) {
      self.0.push(format!("<{}>", parser.local_name()));
    }
    fn end_element(&mut self, parser: &Parser) {
      self.0.push(format!("</{}>", parser.local_name()));
    }
    fn characters(&mut self, text: &str) {
      self.0.push(format!("t:{text}"));
    }
    fn comment(&mut self, text: &str) {
      self.0.push(format!("!:{text}"));
    }
    fn processing_instruction(&mut self, target: &str, data: &str) {
      self.0.push(format!("?:{target} {data}"));
    }
  }

  #[test]
  fn drives_events_in_order() {
    let mut trace = Trace::default();
    drive(&mut Reader::new("<a>hi<b/><!--c--><?p d?></a>".as_bytes()), &mut trace).unwrap();
    assert_eq!(trace.0, ["start", "<a>", "t:hi", "<b>", "</b>", "!:c", "?:p d", "</a>", "end"]);
  }

  #[test]
  fn a_not_well_formed_document_is_an_error() {
    let mut trace = Trace::default();
    assert!(drive(&mut Reader::new("<a></b>".as_bytes()), &mut trace).is_err());
  }
}
