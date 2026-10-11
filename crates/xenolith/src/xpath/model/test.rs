use super::*;
use crate::dom::build::DomBuilder;
use crate::event::{EventCursor, EventProducer};
use crate::io::StreamSource;

/// Reads `xml` into a tree through the parser and the builder.
fn parse_document(xml: &str) -> Document {
  let mut builder = DomBuilder::new();
  StreamSource::new(xml.as_bytes()).add_consumer(&mut builder).emit().expect("well-formed");
  builder.into_document()
}

/// The document element of a parsed document, as an XPath node.
fn document_element(model: &DomModel<'_>) -> DomNode {
  model.children(model.root_node()).into_iter().find(|&n| model.kind(n) == NodeKind::Element).expect("an element")
}

#[test]
fn the_root_holds_the_document_element() {
  let doc = parse_document("<doc/>");
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
  let doc = parse_document("<a>one<![CDATA[two]]>three<b/>four</a>");
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
  let doc = parse_document("<a>x<b>y</b>z</a>");
  let model = DomModel::new(&doc);
  assert_eq!(model.string_value(document_element(&model)), "xyz");
}

#[test]
fn attributes_are_reached_by_the_attribute_axis_not_as_children() {
  let doc = parse_document("<a x='1' y='2'>t</a>");
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  // Only the text node is a child.
  assert_eq!(model.children(a).len(), 1);
  let attributes = model.attributes(a);
  assert_eq!(attributes.len(), 2);
  assert_eq!(model.kind(attributes[0]), NodeKind::Attribute);
  assert_eq!(model.parent(attributes[0]), Some(a));
  let names: Vec<_> = attributes.iter().map(|&n| model.expanded_name(n).expect("a name").local).collect();
  assert_eq!(names, ["x", "y"]);
  assert_eq!(model.string_value(attributes[0]), "1");
}

#[test]
fn namespace_declarations_are_not_attributes_but_namespace_nodes() {
  let doc = parse_document("<a xmlns='urn:d' xmlns:p='urn:p' x='1'/>");
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  // xmlns and xmlns:p are namespace nodes, not attributes; only x is an attribute.
  assert_eq!(model.attributes(a).len(), 1);
  let namespaces = model.namespaces(a);
  // default (urn:d), p (urn:p), and the implicit xml.
  let mut bindings: Vec<(String, String)> =
    namespaces.iter().map(|&n| (model.expanded_name(n).expect("a name").local, model.string_value(n))).collect();
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
  let doc = parse_document("<p:a xmlns:p='urn:p'/>");
  let model = DomModel::new(&doc);
  let name = model.expanded_name(document_element(&model)).expect("a name");
  assert_eq!(name, ExpandedName { namespace: Some("urn:p".to_owned()), local: "a".to_owned() });
}

#[test]
fn document_order_ranks_root_element_namespaces_attributes_then_children() {
  let doc = parse_document("<a xmlns:p='urn:p' x='1'><b/>t</a>");
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
fn a_dom_text_node_maps_to_the_text_node_of_its_run() {
  let doc = parse_document("<a>one<![CDATA[two]]></a>");
  let model = DomModel::new(&doc);
  let a = document_element(&model);
  let text = model.children(a)[0];
  // Whichever DOM node inside the run is asked for, the same text node comes back.
  let DomNode::Tree(element) = a else { panic!("an element is a tree node") };
  let cdata = doc.last_child(element).expect("the CDATA section");
  assert_eq!(model.node(cdata), text);
  assert_eq!(model.string_value(text), "onetwo");
}

#[test]
fn document_order_numbers_the_namespace_nodes_that_the_namespace_axis_gives() {
  // Declarations inherited, overridden, undeclared (the default; a prefix cannot be in XML 1.0) and redeclared on the
  // way down, and elements with none.
  let xml = "<a xmlns='urn:d' xmlns:p='urn:p'>\
               <b xmlns:q='urn:q'><c xmlns='' xmlns:p='urn:p2'><d xmlns:q='urn:q2'/></c></b>\
               <e/><f xmlns:z='urn:z' xmlns:b='urn:b'><g xmlns='urn:d2'/></f>\
             </a>";
  let doc = parse_document(xml);
  let model = DomModel::new(&doc);
  let order = model.numbered_nodes();
  let mut elements = vec![document_element(&model)];
  while let Some(element) = elements.pop() {
    let position = order[&element];
    let namespaces = model.namespaces(element);
    // The namespace nodes come right after their element, in the order the axis gives them.
    for (offset, namespace) in namespaces.iter().enumerate() {
      assert_eq!(order.get(namespace), Some(&(position + 1 + offset)), "{element:?} {namespace:?}");
    }
    let numbered =
      order.keys().filter(|node| matches!(node, DomNode::Namespace { element: e, .. } if DomNode::Tree(*e) == element));
    assert_eq!(numbered.count(), namespaces.len(), "{element:?} has no other namespace node");
    elements.extend(model.children(element).into_iter().filter(|&child| model.kind(child) == NodeKind::Element));
  }
}

#[test]
fn make_node_set_drops_every_duplicate_even_of_nodes_outside_the_tree() {
  // Nodes outside the tree have no position in document order, so they are ordered by their handles; otherwise they
  // would all compare equal and their duplicates need not end up next to each other.
  let mut doc = parse_document("<r><a/></r>");
  let x = DomNode::Tree(doc.create_element("x").expect("a name"));
  let y = DomNode::Tree(doc.create_element("y").expect("a name"));
  let model = DomModel::new(&doc);
  let a = model.children(document_element(&model))[0];
  let mut nodes = vec![y, x, a, y, x, a];
  make_node_set(&model, &mut nodes);
  assert_eq!(nodes, [a, x, y], "in document order, with what is outside the tree after it, in the order it was made");
}
