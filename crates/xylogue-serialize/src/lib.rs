//! XML serialization for xylogue: a [DOM](xylogue_dom) subtree to well-formed XML text.
//!
//! The [`Serializer`] walks a tree and writes it, taking care of the parts that make the output
//! parse back to the same document:
//!
//! - **escaping** — the markup-significant characters in text and attribute values, and the
//!   whitespace an attribute would otherwise lose to normalization, become references.
//! - **namespace repair** — a prefix or default namespace used without a declaration in scope
//!   gets one written onto the element, so a tree built with `create_element_ns` and no `xmlns`
//!   attribute still serializes to well-formed XML.
//! - **indentation** — optional, and applied only to element content, so character data is
//!   never reflowed.
//!
//! Output is UTF-8; an XML declaration is opt-in.
//!
//! # Examples
//!
//! ```
//! use xylogue_dom::Document;
//! use xylogue_serialize::Serializer;
//!
//! let mut doc = Document::new();
//! let note = doc.create_element("note")?;
//! let text = doc.create_text_node("hi");
//! doc.append_child(note, text)?;
//! doc.append_child(doc.root(), note)?;
//!
//! let xml = Serializer::new().with_xml_declaration(true).to_string(&doc, doc.root());
//! assert_eq!(xml, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><note>hi</note>");
//! # Ok::<(), xylogue_dom::DomException>(())
//! ```
//!
//! For output produced call by call rather than from a tree, see [`XmlWriter`].

//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XML 1.0 (Fifth Edition)] — W3C Recommendation 26 November 2008. What has to be escaped
//!   ([§2.4]) and what a well-formed document looks like on the way out.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation 8 December 2009, for the
//!   namespace repair that makes a tree built in memory serialize to something readable back.
//!
//! Where the specification allows a choice — `<a/>` against `<a></a>`, which quotation mark
//! encloses an attribute, whether to escape every `>` — what this crate picks is recorded in the
//! behaviour report; see the README.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [§2.4]: https://www.w3.org/TR/2008/REC-xml-20081126/#syntax
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/

mod escape;
mod serializer;
mod writer;

pub use serializer::Serializer;
pub use writer::XmlWriter;
