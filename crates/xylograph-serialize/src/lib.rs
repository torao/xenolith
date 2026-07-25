//! XML serialization for xylograph: a [DOM](xylograph_dom) subtree to well-formed XML text.
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
//! use xylograph_dom::Document;
//! use xylograph_serialize::Serializer;
//!
//! let mut doc = Document::new();
//! let note = doc.create_element("note")?;
//! let text = doc.create_text_node("hi");
//! doc.append_child(note, text)?;
//! doc.append_child(doc.root(), note)?;
//!
//! let xml = Serializer::new().with_xml_declaration(true).to_string(&doc, doc.root());
//! assert_eq!(xml, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><note>hi</note>");
//! # Ok::<(), xylograph_dom::DomException>(())
//! ```
//!
//! For output produced call by call rather than from a tree, see [`XmlWriter`].

mod escape;
mod serializer;
mod writer;

pub use serializer::Serializer;
pub use writer::XmlWriter;
