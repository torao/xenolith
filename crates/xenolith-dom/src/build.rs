//! Building a [`Document`] from parsed XML.
//!
//! This drives the [parser](xenolith_parser) and turns its event stream into a tree: a start
//! tag becomes an element with its attributes, character data becomes text and CDATA nodes,
//! comments and processing instructions become their own nodes, and a `DOCTYPE` becomes the
//! document type node. Namespaces the parser resolved carry over onto the element and attribute
//! names.
//!
//! ID-typed attributes are marked as they are added — an `xml:id`, and any attribute a DTD
//! declares `ID` — so [`get_element_by_id`](Document::get_element_by_id) works on the result.
//! Each element's base URI is recorded too, resolved from `xml:base` and the document's system
//! identifier, so [`base_uri`](Document::base_uri) reports it.
//!
//! Behind the `parse` feature, which turns on the parser's XML Base and `xml:id` for this.
//!
//! # Examples
//!
//! ```
//! use xenolith_dom::build;
//!
//! let doc = build::parse("<doc><p>Hello</p></doc>".as_bytes())?;
//! let root = doc.document_element().unwrap();
//! assert_eq!(doc.node_name(root), "doc");
//! assert_eq!(doc.text_content(root), "Hello");
//! # Ok::<(), xenolith_core::Error>(())
//! ```

use std::io::Read;

use xenolith_core::error::{Error, Result};
use xenolith_core::name::{NameId, QName};
use xenolith_parser::dtd::AttType;
use xenolith_parser::{Attributes, EventRef, Parser, Reader};

use crate::{Document, DomException, NodeId};

/// Builds a [`Document`] from XML read from `source`.
///
/// External entities and an external DTD subset are not resolved — this convenience uses a
/// reader with no resolver. Use [`parse_reader`] with a configured [`Reader`] when a resolver,
/// explicit limits, or a system identifier are needed.
///
/// # Errors
///
/// Returns the parser's error if the document is not well-formed, or if reading `source` fails.
pub fn parse<R: Read>(source: R) -> Result<Document> {
  parse_reader(Reader::new(source))
}

/// Builds a [`Document`] from XML read from `source`, with its system identifier known.
///
/// The identifier is the document's base URI, so relative `xml:base` values in it resolve
/// correctly — what a post-processor such as XInclude needs when it parses an included resource.
///
/// # Errors
///
/// As [`parse`].
pub fn parse_with_system_id<R: Read>(source: R, system_id: &str) -> Result<Document> {
  parse_reader(Reader::with_system_id(source, system_id))
}

/// Builds a [`Document`] from a prepared [`Reader`], so a resolver or configuration can be set
/// first.
///
/// # Errors
///
/// As [`parse`].
pub fn parse_reader<R: Read>(mut reader: Reader<R>) -> Result<Document> {
  let mut doc = Document::new();
  // The open nodes, outermost first; the document is always at the bottom.
  let mut open: Vec<NodeId> = vec![doc.root()];
  let mut document_base_set = false;

  while reader.advance()?.is_some() {
    let parent = *open.last().expect("the document is always open");
    let parser = reader.parser();
    // The document's base URI is its system identifier, taken once, before any xml:base applies.
    if !document_base_set {
      document_base_set = true;
      let system_id = parser.location().system_id.clone();
      doc.set_document_base(system_id.as_deref());
    }
    match parser.event_ref() {
      Some(EventRef::StartElement { name, attributes, .. }) => {
        let element = build_element(&mut doc, parser, name, attributes)?;
        let base = parser.base_uri();
        doc.set_element_base(element, base.as_deref());
        dom_result(doc.append_child(parent, element))?;
        open.push(element);
      }
      Some(EventRef::EndElement { .. }) => {
        open.pop();
      }
      Some(EventRef::Text(text)) => {
        // Coalesce here: the parser may deliver a long run of text in fragments (see its `Text`
        // event), and the data model wants adjacent character data as a single text node.
        dom_result(doc.append_text(parent, text))?;
      }
      Some(EventRef::CData(text)) => {
        let node = doc.create_cdata_section(text);
        dom_result(doc.append_child(parent, node))?;
      }
      Some(EventRef::Comment(text)) => {
        let node = doc.create_comment(text);
        dom_result(doc.append_child(parent, node))?;
      }
      Some(EventRef::ProcessingInstruction { target, data, .. }) => {
        let node = dom_result(doc.create_processing_instruction(target, data))?;
        dom_result(doc.append_child(parent, node))?;
      }
      Some(EventRef::Doctype(_)) => {
        if let Some(name) = parser.doctype_name() {
          let name = parser.pool().resolve(name).to_owned();
          let public_id = parser.doctype_public_id().map(ToOwned::to_owned);
          let system_id = parser.doctype_system_id().map(ToOwned::to_owned);
          let node = dom_result(doc.create_document_type(&name, public_id.as_deref(), system_id.as_deref()))?;
          dom_result(doc.append_child(parent, node))?;
        }
      }
      // The XML declaration is not part of the tree, and `advance` reported an event so there is always one.
      _ => {}
    }
  }
  Ok(doc)
}

/// Creates the element for the start tag, with its attributes and ID marks.
fn build_element(doc: &mut Document, parser: &Parser, name: QName, attributes: Attributes) -> Result<NodeId> {
  let lexical = name.to_lexical(parser.pool());
  let namespace = parser.namespace_uri().map(ToOwned::to_owned);
  let element = dom_result(doc.create_element_ns(namespace.as_deref(), &lexical))?;

  for attr in attributes.iter() {
    let name = attr.name.to_lexical(parser.pool());
    let namespace = attr.name.namespace().map(|ns| parser.pool().resolve(ns).to_owned());
    dom_result(match namespace {
      Some(ns) => doc.set_attribute_ns(element, Some(&ns), &name, attr.value),
      None => doc.set_attribute(element, &name, attr.value),
    })?;
  }

  mark_id_attributes(doc, element, parser, name, attributes);
  Ok(element)
}

/// Marks the ID-typed attributes of the element: `xml:id`, and any the DTD declares `ID`, so
/// they are found by [`Document::get_element_by_id`].
fn mark_id_attributes(doc: &mut Document, element: NodeId, parser: &Parser, name: QName, attributes: Attributes) {
  for attr in attributes.iter() {
    let is_xml_id = attr.name.namespace() == Some(NameId::XML_NS) && parser.pool().resolve(attr.name.local()) == "id";
    if is_xml_id {
      let name = attr.name.to_lexical(parser.pool());
      let _ = doc.set_id_attribute(element, &name, true);
    }
  }

  let Some(dtd) = parser.dtd() else { return };
  let element_name = name.to_lexical(parser.pool());
  let Some(element_id) = parser.pool().get(&element_name) else { return };
  let Some(defs) = dtd.attlist(element_id) else { return };
  for def in defs {
    if matches!(def.att_type, AttType::Id) {
      let name = parser.pool().resolve(def.name).to_owned();
      if doc.has_attribute(element, &name) {
        let _ = doc.set_id_attribute(element, &name, true);
      }
    }
  }
}

/// Maps a DOM exception during construction to a crate error. A well-formed document does not
/// raise one, so reaching this is an internal inconsistency rather than a document problem.
fn dom_result<T>(result: std::result::Result<T, DomException>) -> Result<T> {
  result.map_err(|error| Error::internal(format!("building the DOM: {error}")))
}
