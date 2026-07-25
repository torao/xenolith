//! Building a DOM from parsed XML.

#![cfg(feature = "parse")]

use xylograph_dom::{Document, NodeType, build};

fn parse(xml: &str) -> Document {
  build::parse(xml.as_bytes()).expect("well-formed")
}

#[test]
fn builds_elements_text_and_nesting() {
  let doc = parse("<doc><p>Hello</p><p>World</p></doc>");
  let root = doc.document_element().unwrap();
  assert_eq!(doc.node_name(root), "doc");
  let ps: Vec<_> = doc.children(root).collect();
  assert_eq!(ps.len(), 2);
  assert_eq!(doc.node_name(ps[0]), "p");
  assert_eq!(doc.text_content(ps[0]), "Hello");
  assert_eq!(doc.text_content(root), "HelloWorld");
}

#[test]
fn carries_attributes_and_their_values() {
  let doc = parse("<a x='1' y='two'/>");
  let a = doc.document_element().unwrap();
  assert_eq!(doc.attribute(a, "x"), Some("1"));
  assert_eq!(doc.attribute(a, "y"), Some("two"));
}

#[test]
fn resolves_namespaces_onto_names() {
  let doc = parse("<a xmlns='urn:d' xmlns:p='urn:p'><p:b/></a>");
  let a = doc.document_element().unwrap();
  assert_eq!(doc.namespace_uri(a), Some("urn:d"));
  let b = doc.first_child(a).unwrap();
  assert_eq!(doc.namespace_uri(b), Some("urn:p"));
  assert_eq!(doc.prefix(b), Some("p"));
  assert_eq!(doc.local_name(b), Some("b"));
  // The namespace declaration is kept as an attribute in the XMLNS namespace.
  assert_eq!(doc.attribute(a, "xmlns"), Some("urn:d"));
}

#[test]
fn builds_comments_pis_and_cdata() {
  let doc = parse("<doc><!--c--><?pi data?><![CDATA[<raw>]]></doc>");
  let root = doc.document_element().unwrap();
  let kids: Vec<_> = doc.children(root).map(|n| doc.node_type(n)).collect();
  assert_eq!(kids, [NodeType::Comment, NodeType::ProcessingInstruction, NodeType::CdataSection]);
  let cdata = doc.last_child(root).unwrap();
  assert_eq!(doc.node_value(cdata), Some("<raw>"));
}

#[test]
fn a_prolog_comment_becomes_a_child_of_the_document() {
  let doc = parse("<!--intro--><doc/>");
  let first = doc.first_child(doc.root()).unwrap();
  assert_eq!(doc.node_type(first), NodeType::Comment);
  assert_eq!(doc.node_value(first), Some("intro"));
}

#[test]
fn builds_the_document_type_node() {
  let doc = parse("<!DOCTYPE greeting [<!ELEMENT greeting (#PCDATA)>]><greeting>hi</greeting>");
  let doctype = doc.doctype().unwrap();
  assert_eq!(doc.node_type(doctype), NodeType::DocumentType);
  assert_eq!(doc.node_name(doctype), "greeting");
}

#[test]
fn xml_id_is_marked_so_get_element_by_id_finds_it() {
  let doc = parse("<r><a xml:id='x1'/><b xml:id='x2'/></r>");
  let a = doc.get_element_by_id("x1").unwrap();
  assert_eq!(doc.node_name(a), "a");
  assert_eq!(doc.get_element_by_id("x2").map(|n| doc.node_name(n)).as_deref(), Some("b"));
  assert_eq!(doc.get_element_by_id("nope"), None);
}

#[test]
fn records_base_uris_from_the_system_id_and_xml_base() {
  use xylograph_parser::Reader;
  let xml = "<a><b xml:base='../c/'><d/></b></a>";
  let doc = build::parse_reader(Reader::with_system_id(xml.as_bytes(), "file:///a/b/doc.xml")).expect("well-formed");
  let a = doc.document_element().unwrap();
  assert_eq!(doc.base_uri(a).as_deref(), Some("file:///a/b/doc.xml"));
  let b = doc.first_child(a).unwrap();
  assert_eq!(doc.base_uri(b).as_deref(), Some("file:///a/c/"), "xml:base is resolved against the document URI");
  let d = doc.first_child(b).unwrap();
  assert_eq!(doc.base_uri(d).as_deref(), Some("file:///a/c/"), "a child with no xml:base inherits");
}

#[test]
fn base_uri_is_none_without_a_system_id_or_xml_base() {
  let doc = parse("<a><b/></a>");
  assert_eq!(doc.base_uri(doc.document_element().unwrap()), None);
}

#[test]
fn captures_the_doctype_public_and_system_ids() {
  use xylograph_parser::Reader;
  use xylograph_parser::resolve::{EntityRequest, UriResolver};

  // A resolver that serves the external subset as empty — enough for the DOCTYPE to be read.
  struct Empty;
  impl UriResolver for Empty {
    fn resolve(&mut self, _request: &EntityRequest) -> Result<Option<Vec<u8>>, xylograph_core::Error> {
      Ok(Some(Vec::new()))
    }
  }

  let xml = "<!DOCTYPE a PUBLIC \"pub-id\" \"a.dtd\"><a/>";
  let doc = build::parse_reader(Reader::new(xml.as_bytes()).with_resolver(Empty)).expect("well-formed");
  let doctype = doc.doctype().unwrap();
  assert_eq!(doc.node_name(doctype), "a");
  assert_eq!(doc.public_id(doctype), Some("pub-id"));
  assert_eq!(doc.system_id(doctype), Some("a.dtd"));
}

#[test]
fn a_dtd_id_attribute_is_marked() {
  let xml = "<!DOCTYPE r [<!ELEMENT r (item)><!ELEMENT item EMPTY><!ATTLIST item key ID #IMPLIED>]>\
             <r><item key='k1'/></r>";
  let doc = parse(xml);
  let item = doc.get_element_by_id("k1").unwrap();
  assert_eq!(doc.node_name(item), "item");
  // A non-ID attribute of the same value is not found.
  assert_eq!(doc.get_element_by_id("r"), None);
}
