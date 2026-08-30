//! Serialization: round trips through parse, namespace repair, indentation, and the prolog.

use xenolith_dom::{Document, build};
use xenolith_serialize::Serializer;

/// Parses `xml`, serializes its root element compactly, and returns the text.
fn roundtrip(xml: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  Serializer::new().to_string(&doc, doc.document_element().unwrap())
}

#[test]
fn round_trips_structure_and_escaping() {
  assert_eq!(roundtrip("<a x=\"1\"><b/>text<c/></a>"), "<a x=\"1\"><b/>text<c/></a>");
  // Markup characters in text and attributes come back escaped.
  assert_eq!(roundtrip("<a x=\"1 &lt; 2\">a &amp; b</a>"), "<a x=\"1 &lt; 2\">a &amp; b</a>");
}

#[test]
fn preserves_existing_namespace_declarations_without_duplicating_them() {
  assert_eq!(
    roundtrip("<a xmlns=\"urn:d\" xmlns:p=\"urn:p\"><p:b/></a>"),
    "<a xmlns=\"urn:d\" xmlns:p=\"urn:p\"><p:b/></a>"
  );
}

#[test]
fn repairs_missing_namespace_declarations() {
  let mut doc = Document::new();
  let a = doc.create_element_ns(Some("urn:d"), "a").unwrap();
  let b = doc.create_element_ns(Some("urn:p"), "p:b").unwrap();
  doc.append_child(a, b).unwrap();
  doc.append_child(doc.root(), a).unwrap();
  // Neither element carries an xmlns attribute; the serializer supplies them.
  assert_eq!(Serializer::new().to_string(&doc, a), "<a xmlns=\"urn:d\"><p:b xmlns:p=\"urn:p\"/></a>");
}

#[test]
fn does_not_redeclare_an_inherited_namespace() {
  let mut doc = Document::new();
  let a = doc.create_element_ns(Some("urn:d"), "a").unwrap();
  let b = doc.create_element_ns(Some("urn:d"), "b").unwrap();
  doc.append_child(a, b).unwrap();
  doc.append_child(doc.root(), a).unwrap();
  // b is in the same namespace as a, already the default in scope: no second xmlns.
  assert_eq!(Serializer::new().to_string(&doc, a), "<a xmlns=\"urn:d\"><b/></a>");
}

#[test]
fn indents_element_content_but_not_text() {
  let doc = build::parse("<a><b>text</b><c/></a>".as_bytes()).unwrap();
  let out = Serializer::new().with_indent("  ").to_string(&doc, doc.document_element().unwrap());
  assert_eq!(out, "<a>\n  <b>text</b>\n  <c/>\n</a>");
}

#[test]
fn writes_the_xml_declaration_with_standalone() {
  let doc = build::parse("<a/>".as_bytes()).unwrap();
  let out = Serializer::new()
    .with_xml_declaration(true)
    .with_standalone(Some(true))
    .to_string(&doc, doc.document_element().unwrap());
  assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><a/>");
}

#[test]
fn writes_comments_pis_and_cdata() {
  assert_eq!(roundtrip("<a><!--c--><?pi d?><![CDATA[<raw>]]></a>"), "<a><!--c--><?pi d?><![CDATA[<raw>]]></a>");
}

#[test]
fn writes_a_doctype_with_public_and_system_ids() {
  let mut doc = Document::new();
  let doctype = doc.create_document_type("html", Some("-//W3C//DTD XHTML//EN"), Some("x.dtd")).unwrap();
  doc.append_child(doc.root(), doctype).unwrap();
  let html = doc.create_element("html").unwrap();
  doc.append_child(doc.root(), html).unwrap();
  assert_eq!(
    Serializer::new().to_string(&doc, doc.root()),
    "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML//EN\" \"x.dtd\"><html/>"
  );
}

#[test]
fn a_rich_document_round_trips_through_dom_and_back() {
  // Namespaces, an escaped attribute, mixed content, a comment, a PI and a CDATA section — the
  // Phase 3 completion condition: parse to a DOM and serialize back without losing anything.
  let xml = "<doc xmlns=\"urn:d\" xmlns:p=\"urn:p\" id=\"1\">\
             text<p:child a=\"x &amp; y\"/><!--c--><?pi d?><![CDATA[<raw>]]>tail</doc>";
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  assert_eq!(Serializer::new().to_string(&doc, doc.document_element().unwrap()), xml);
}

#[test]
fn writes_a_whole_document_with_a_declaration_and_indent() {
  let doc = build::parse("<a><b/></a>".as_bytes()).unwrap();
  let out = Serializer::new().with_xml_declaration(true).with_indent("  ").to_string(&doc, doc.root());
  assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a>\n  <b/>\n</a>");
}
