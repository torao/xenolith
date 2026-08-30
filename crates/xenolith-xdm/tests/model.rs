//! The DOM XPath data model: kinds, axes primitives, namespace nodes, text merging, order.

use std::cmp::Ordering;

use xenolith_dom::build;
use xenolith_xdm::{DomModel, ExpandedName, Model, NodeKind};

/// The document element of a parsed document, as an XPath node.
fn document_element(model: &DomModel<'_>) -> xenolith_xdm::DomNode {
  model.children(model.root_node()).into_iter().find(|&n| model.kind(n) == NodeKind::Element).unwrap()
}

#[test]
fn the_root_holds_the_document_element() {
  let doc = build::parse("<doc/>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let root = model.root_node();
  assert_eq!(model.kind(root), NodeKind::Root);
  let children = model.children(root);
  assert_eq!(children.len(), 1);
  assert_eq!(model.kind(children[0]), NodeKind::Element);
  assert_eq!(model.parent(children[0]), Some(root));
  assert_eq!(model.parent(root), None);
}

#[test]
fn adjacent_text_and_cdata_merge_into_one_text_node() {
  let doc = build::parse("<a>one<![CDATA[two]]>three<b/>four</a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  let kinds: Vec<_> = model.children(a).iter().map(|&n| model.kind(n)).collect();
  // "onetwothree" is a single text node, then <b/>, then "four".
  assert_eq!(kinds, [NodeKind::Text, NodeKind::Element, NodeKind::Text]);
  let text = model.children(a)[0];
  assert_eq!(model.string_value(text), "onetwothree");
}

#[test]
fn string_value_of_an_element_is_all_its_text() {
  let doc = build::parse("<a>x<b>y</b>z</a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  assert_eq!(model.string_value(document_element(&model)), "xyz");
}

#[test]
fn attributes_are_reached_by_the_attribute_axis_not_as_children() {
  let doc = build::parse("<a x='1' y='2'>t</a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  // Only the text node is a child.
  assert_eq!(model.children(a).len(), 1);
  let attributes = model.attributes(a);
  assert_eq!(attributes.len(), 2);
  assert_eq!(model.kind(attributes[0]), NodeKind::Attribute);
  assert_eq!(model.parent(attributes[0]), Some(a));
  let names: Vec<_> = attributes.iter().map(|&n| model.expanded_name(n).unwrap().local).collect();
  assert_eq!(names, ["x", "y"]);
  assert_eq!(model.string_value(attributes[0]), "1");
}

#[test]
fn namespace_declarations_are_not_attributes_but_namespace_nodes() {
  let doc = build::parse("<a xmlns='urn:d' xmlns:p='urn:p' x='1'/>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  // xmlns and xmlns:p are namespace nodes, not attributes; only x is an attribute.
  assert_eq!(model.attributes(a).len(), 1);
  let namespaces = model.namespaces(a);
  // default (urn:d), p (urn:p), and the implicit xml.
  let mut bindings: Vec<(String, String)> =
    namespaces.iter().map(|&n| (model.expanded_name(n).unwrap().local, model.string_value(n))).collect();
  bindings.sort();
  assert_eq!(
    bindings,
    [
      (String::new(), "urn:d".to_owned()),
      ("p".to_owned(), "urn:p".to_owned()),
      ("xml".to_owned(), "http://www.w3.org/XML/1998/namespace".to_owned()),
    ]
  );
}

#[test]
fn a_namespaced_element_reports_its_expanded_name() {
  let doc = build::parse("<p:a xmlns:p='urn:p'/>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let name = model.expanded_name(document_element(&model)).unwrap();
  assert_eq!(name, ExpandedName { namespace: Some("urn:p".to_owned()), local: "a".to_owned() });
}

#[test]
fn document_order_ranks_root_element_namespaces_attributes_then_children() {
  let doc = build::parse("<a xmlns:p='urn:p' x='1'><b/>t</a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let root = model.root_node();
  let a = document_element(&model);
  let namespace = model.namespaces(a)[0];
  let attribute = model.attributes(a)[0];
  let b = model.children(a)[0];
  let text = model.children(a)[1];

  let order = |x, y| model.document_order(x, y);
  assert_eq!(order(root, a), Ordering::Less);
  assert_eq!(order(a, namespace), Ordering::Less, "an element precedes its namespace nodes");
  assert_eq!(order(namespace, attribute), Ordering::Less, "namespace nodes precede attributes");
  assert_eq!(order(attribute, b), Ordering::Less, "attributes precede children");
  assert_eq!(order(b, text), Ordering::Less);
  assert_eq!(order(a, a), Ordering::Equal);
  assert_eq!(order(text, a), Ordering::Greater);
}

#[test]
fn the_node_accessor_maps_a_dom_text_node_to_its_run() {
  let doc = build::parse("<a>one<![CDATA[two]]></a>".as_bytes()).unwrap();
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  let text = model.children(a)[0];
  // Whichever DOM node inside the run is asked for, the same text node comes back.
  assert_eq!(model.string_value(text), "onetwo");
}
