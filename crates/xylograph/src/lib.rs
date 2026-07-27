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
//! - [`serialize`] — writing a DOM subtree back to XML text.
//! - [`xdm`] — the XPath data model: a node-model trait and its DOM implementation.
//! - [`xpath`] — XPath 1.0: compiling an expression and evaluating it.
//! - [`xslt`] — XSLT 1.0: patterns, stylesheets, the engine, and writing the result.
//! - `exslt` (feature `exslt`) — EXSLT extension functions for XSLT.
//! - `xinclude` (feature `xinclude`) — expanding `xi:include` over a DOM.
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

//! # Specifications
//!
//! Every layer names the documents it was written from, at dated URLs so that the text read
//! while writing it can still be found. Together they are:
//!
//! | Document | Version | Where |
//! |---|---|---|
//! | [XML 1.0 (Fifth Edition)] | REC 2008-11-26 | [`parser`], [`validate`], [`serialize`], [`core`](xylograph_core) |
//! | [Namespaces in XML 1.0 (Third Edition)] | REC 2009-12-08 | [`parser`], [`serialize`], [`xdm`], [`xpath`] |
//! | [XPath 1.0] | REC 1999-11-16 | [`xdm`], [`xpath`] |
//! | [DOM Level 3 Core] | REC 2004-04-07 | [`dom`] |
//! | [XInclude 1.0 (Second Edition)] | REC 2006-11-15 | `xinclude` |
//! | [XPointer Framework] / [`element()`][xptr-element] / [`xmlns()`][xptr-xmlns] | REC 2003-03-25 | `xinclude` |
//! | [XML Base (Second Edition)] | REC 2009-01-28 | [`parser`], [`dom`], `xinclude` |
//! | [xml:id 1.0] | REC 2005-09-09 | [`parser`], [`validate`] |
//! | [RFC 3986] | STD 66, 2005-01 | [`core`](xylograph_core) |
//!
//! XSLT 1.0 and EXSLT arrive in later phases; see `ROADMAP.md`.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [XPath 1.0]: https://www.w3.org/TR/1999/REC-xpath-19991116/
//! [DOM Level 3 Core]: https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/
//! [XInclude 1.0 (Second Edition)]: https://www.w3.org/TR/2006/REC-xinclude-20061115/
//! [XPointer Framework]: https://www.w3.org/TR/2003/REC-xptr-framework-20030325/
//! [xptr-element]: https://www.w3.org/TR/2003/REC-xptr-element-20030325/
//! [xptr-xmlns]: https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/
//! [xml:id 1.0]: https://www.w3.org/TR/2005/REC-xml-id-20050909/
//! [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986

/// The XML pull parser: [`Reader`](parser::Reader), events, entity resolution, and the DTD.
pub use xylograph_parser as parser;

/// Validation: the schema-agnostic [`Validator`](validate::Validator) and the DTD validator.
pub use xylograph_validate as validate;

/// The DOM tree: an arena of nodes with a W3C-shaped, Rust-idiomatic API.
pub use xylograph_dom as dom;

/// Serialization: a DOM subtree to well-formed XML text.
pub use xylograph_serialize as serialize;

/// The XPath 1.0 data model: a node-model trait and its DOM implementation.
pub use xylograph_xdm as xdm;

/// XPath 1.0: the lexer, the parser, and the expression tree.
pub use xylograph_xpath as xpath;

/// XSLT 1.0: patterns, stylesheets, the transformation engine and what `xsl:output` asks for.
pub use xylograph_xslt as xslt;

/// EXSLT extension functions for XSLT. Behind the `exslt` feature.
#[cfg(feature = "exslt")]
pub use xylograph_exslt as exslt;

/// XInclude: expanding `xi:include` over a DOM. Behind the `xinclude` feature.
#[cfg(feature = "xinclude")]
pub use xylograph_xinclude as xinclude;

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
  fn the_facade_parses_an_xpath_expression() {
    let expr = crate::xpath::parse("//a[1]").unwrap();
    assert_eq!(expr.to_string(), "/descendant-or-self::node()/child::a[1]");
  }

  #[test]
  fn the_facade_serializes_a_dom() {
    let mut doc = crate::dom::Document::new();
    let a = doc.create_element("a").unwrap();
    doc.append_child(doc.root(), a).unwrap();
    assert_eq!(crate::serialize::Serializer::new().to_string(&doc, a), "<a/>");
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
