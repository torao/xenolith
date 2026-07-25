//! xylograph: XML processing and XSLT 1.0 in Rust.
//!
//! This is the entry-point crate. The work is split across focused crates — so a caller who
//! wants only the parser does not compile the collation tables or the transformation engine —
//! and gathered here under one name and one dependency. Depend on `xylograph` and reach the
//! layers through their modules:
//!
//! - [`parser`] — the XML pull parser: readers, events, entity resolution, the DTD.
//! - [`validate`] — validation: a schema-agnostic `Validator` and the DTD validator.
//! - [`dom`] — an arena-based DOM tree: nodes, navigation, mutation, and `dom::build` to make
//!   one from parsed XML.
//! - the primitives every layer shares — [`Error`], [`QName`] and their neighbours — are
//!   re-exported at the crate root, with [`chars`], [`encoding`] and [`uri`] beside them.
//!
//! The DOM, XPath, serializer and XSLT layers appear here as they land; see `ROADMAP.md`.
//!
//! # Examples
//!
//! ```
//! use xylograph::parser::{EventKind, Reader};
//!
//! let mut reader = Reader::new("<greeting xml:lang='en'>Hello</greeting>".as_bytes());
//! let mut text = String::new();
//! while let Some(kind) = reader.advance()? {
//!   if kind == EventKind::Text {
//!     text.push_str(reader.parser().text());
//!   }
//! }
//! assert_eq!(text, "Hello");
//! # Ok::<(), xylograph::Error>(())
//! ```
//!
//! # Feature flags
//!
//! - `encodings` (default): encodings beyond UTF-8/UTF-16/US-ASCII/ISO-8859-1.
//! - `tokio`: the asynchronous reader, [`parser::AsyncReader`], over `tokio`'s `AsyncRead`.
//! - `xml-base`: per-node base URI computation from `xml:base` (XML Base).
//! - `xml-id`: `xml:id` as an ID-typed attribute, checked for NCName validity and uniqueness.

/// The XML pull parser: [`Reader`](parser::Reader), events, entity resolution, and the DTD.
pub use xylograph_parser as parser;

/// Validation: the schema-agnostic [`Validator`](validate::Validator) and the DTD validator.
pub use xylograph_validate as validate;

/// The DOM tree: an arena of nodes with a W3C-shaped, Rust-idiomatic API.
pub use xylograph_dom as dom;

pub use xylograph_core::{Error, ErrorKind, Location, Result, Severity};
pub use xylograph_core::{ExpandedName, NameId, NamePool, QName, UriReference, XML_NS_URI, XMLNS_NS_URI};
pub use xylograph_core::{chars, encoding, error, name, uri};

#[cfg(test)]
mod tests {
  use crate::parser::{EventKind, Reader};

  #[test]
  fn the_facade_reaches_the_parser() {
    let mut reader = Reader::new("<a><b/></a>".as_bytes());
    let mut starts = 0;
    while let Some(kind) = reader.advance().unwrap() {
      if kind == EventKind::StartElement {
        starts += 1;
      }
    }
    assert_eq!(starts, 2);
  }

  #[test]
  fn the_facade_reaches_the_dom() {
    let mut doc = crate::dom::Document::new();
    let root = doc.create_element("a").unwrap();
    doc.append_child(doc.root(), root).unwrap();
    assert_eq!(doc.document_element(), Some(root));
  }

  #[cfg(feature = "parse")]
  #[test]
  fn the_facade_builds_a_dom_from_xml() {
    let doc = crate::dom::build::parse("<a><b>x</b></a>".as_bytes()).unwrap();
    let root = doc.document_element().unwrap();
    assert_eq!(doc.node_name(root), "a");
    assert_eq!(doc.text_content(root), "x");
  }

  #[test]
  fn core_primitives_are_at_the_root() {
    // A name and a URI resolve through the crate root, without naming the inner crates.
    let mut pool = crate::NamePool::new();
    let a = pool.intern("a");
    assert_eq!(pool.resolve(a), "a");
    assert_eq!(crate::uri::resolve("file:///d/x.xml", "y.xml").unwrap(), "file:///d/y.xml");
  }
}
