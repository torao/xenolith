use super::*;

/// Builds `<r a="1"><b/>text<c/></r>` under the document node, returning (r, b, c).
fn sample() -> (Document, NodeId, NodeId, NodeId) {
  let mut doc = Document::new();
  let r = doc.create_element("r").unwrap();
  doc.set_attribute(r, "a", "1").unwrap();
  let b = doc.create_element("b").unwrap();
  let text = doc.create_text_node("text");
  let c = doc.create_element("c").unwrap();
  doc.append_child(r, b).unwrap();
  doc.append_child(r, text).unwrap();
  doc.append_child(r, c).unwrap();
  doc.append_child(doc.document_node(), r).unwrap();
  (doc, r, b, c)
}

#[test]
fn navigates_the_tree() {
  let (doc, r, b, c) = sample();
  assert_eq!(doc.document_element(), Some(r));
  assert_eq!(doc.parent(r), Some(doc.document_node()));
  assert_eq!(doc.first_child(r), Some(b));
  assert_eq!(doc.last_child(r), Some(c));
  assert_eq!(doc.next_sibling(b).and_then(|t| doc.next_sibling(t)), Some(c));
  assert_eq!(doc.previous_sibling(c).and_then(|t| doc.previous_sibling(t)), Some(b));
  let children: Vec<_> = doc.children(r).collect();
  assert_eq!(children.len(), 3);
}

#[test]
fn reports_names_types_and_values() {
  let (doc, r, b, _) = sample();
  assert_eq!(doc.node_type(r), NodeType::ELEMENT_NODE);
  assert_eq!(doc.node_name(r), "r");
  assert_eq!(doc.local_name(b), Some("b"));
  let text = doc.next_sibling(b).unwrap();
  assert_eq!(doc.node_type(text), NodeType::TEXT_NODE);
  assert_eq!(doc.node_name(text), "#text");
  assert_eq!(doc.node_value(text), Some("text"));
  assert_eq!(doc.text_content(r), "text");
}

#[test]
fn namespaced_names_split_into_parts() {
  let mut doc = Document::new();
  let e = doc.create_element_ns(Some("urn:x"), "p:a").unwrap();
  assert_eq!(doc.node_name(e), "p:a");
  assert_eq!(doc.local_name(e), Some("a"));
  assert_eq!(doc.prefix(e), Some("p"));
  assert_eq!(doc.namespace_uri(e), Some("urn:x"));
}

#[test]
fn reads_and_removes_attributes() {
  let (mut doc, r, _, _) = sample();
  assert_eq!(doc.attribute(r, "a"), Some("1"));
  assert!(doc.has_attribute(r, "a"));
  doc.set_attribute(r, "a", "2").unwrap();
  assert_eq!(doc.attribute(r, "a"), Some("2"), "setting an existing attribute replaces it");
  doc.set_attribute(r, "b", "y").unwrap();
  assert_eq!(doc.attribute_names(r), ["a", "b"]);
  doc.remove_attribute(r, "a").unwrap();
  assert!(!doc.has_attribute(r, "a"));
}

#[test]
fn namespaced_attributes_are_found_by_namespace() {
  let mut doc = Document::new();
  let e = doc.create_element("e").unwrap();
  doc.set_attribute_ns(e, Some("urn:x"), "p:k", "v").unwrap();
  assert_eq!(doc.attribute_ns(e, Some("urn:x"), "k"), Some("v"));
}

#[test]
fn insert_before_places_a_node_among_its_siblings() {
  let (mut doc, r, b, c) = sample();
  let x = doc.create_element("x").unwrap();
  doc.insert_before(r, x, Some(c)).unwrap();
  assert_eq!(doc.previous_sibling(c), Some(x));
  assert_eq!(doc.first_child(r), Some(b));
}

#[test]
fn moving_a_node_detaches_it_from_its_old_parent() {
  let (mut doc, r, b, c) = sample();
  // Move b under c; it must leave r's child list, so r now starts with the text node.
  doc.append_child(c, b).unwrap();
  assert_eq!(doc.parent(b), Some(c));
  assert!(!doc.children(r).any(|n| n == b));
  assert_eq!(doc.node_type(doc.first_child(r).unwrap()), NodeType::TEXT_NODE);
}

#[test]
fn remove_child_detaches() {
  let (mut doc, r, b, _) = sample();
  doc.remove_child(r, b).unwrap();
  assert_eq!(doc.parent(b), None);
  assert!(!doc.children(r).any(|n| n == b));
}

#[test]
fn a_cycle_is_refused() {
  let (mut doc, r, b, _) = sample();
  let error = doc.append_child(b, r).unwrap_err();
  assert_eq!(error.code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
}

#[test]
fn a_leaf_cannot_take_children() {
  let mut doc = Document::new();
  let text = doc.create_text_node("t");
  let child = doc.create_element("c").unwrap();
  let error = doc.append_child(text, child).unwrap_err();
  assert_eq!(error.code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
}

#[test]
fn insert_before_a_non_child_is_not_found() {
  let (mut doc, r, _, _) = sample();
  let stray = doc.create_element("stray").unwrap();
  let x = doc.create_element("x").unwrap();
  let error = doc.insert_before(r, x, Some(stray)).unwrap_err();
  assert_eq!(error.code(), ExceptionCode::NOT_FOUND_ERR);
}

#[test]
fn an_invalid_name_is_rejected() {
  let mut doc = Document::new();
  assert_eq!(doc.create_element("1bad").unwrap_err().code(), ExceptionCode::INVALID_CHARACTER_ERR);
}

#[test]
fn a_pi_target_error_names_the_offending_character() {
  let mut doc = Document::new();
  // A non-printing character shows as its Unicode escape, not as an invisible byte.
  let bell = doc.create_processing_instruction("ab\u{7}c", "d").unwrap_err();
  assert_eq!(bell.code(), ExceptionCode::INVALID_CHARACTER_ERR);
  assert!(bell.message().contains(r"'\u{7}'"), "message was: {}", bell.message());

  // A character legal in a name but not at the start is reported as a start-position fault.
  let digit = doc.create_processing_instruction("1abc", "d").unwrap_err();
  assert!(digit.message().contains("start"), "message was: {}", digit.message());
  assert!(digit.message().contains("'1'"), "message was: {}", digit.message());
}

#[test]
fn a_qualified_name_error_explains_the_fault() {
  let mut doc = Document::new();
  // A structural fault reports the structure, not a single character.
  let two_colons = doc.create_element("a:b:c").unwrap_err();
  assert_eq!(two_colons.code(), ExceptionCode::INVALID_CHARACTER_ERR);
  assert!(two_colons.message().contains("more than one colon"), "message was: {}", two_colons.message());
  // A bad character inside a part is reported.
  let space = doc.create_element("a b").unwrap_err();
  assert!(space.message().contains("' '"), "message was: {}", space.message());
  // An empty prefix or local part is called out.
  let empty_local = doc.create_element("p:").unwrap_err();
  assert!(empty_local.message().contains("empty local part"), "message was: {}", empty_local.message());
}

#[test]
fn a_node_from_another_document_is_refused() {
  let mut a = Document::new();
  let mut b = Document::new();
  let in_a = a.create_element("a").unwrap();
  let in_b = b.create_element("b").unwrap();

  assert!(a.owns(in_a));
  assert!(!a.owns(in_b), "a handle carries the document that made it");

  // Every fallible method reports the mistake rather than reading or linking an unrelated node.
  assert_eq!(a.append_child(a.document_node(), in_b).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert_eq!(a.insert_before(in_a, in_b, None).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert_eq!(a.remove_child(a.document_node(), in_b).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert_eq!(a.replace_child(a.document_node(), in_b, in_a).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert_eq!(a.clone_node(in_b, false).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert_eq!(a.set_attribute(in_b, "k", "v").unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert_eq!(a.append_text(in_b, "t").unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);

  // `import_node` measures the node against the document it is read from, not the one it is copied into.
  assert_eq!(a.import_node(&b, in_a, true).unwrap_err().code(), ExceptionCode::WRONG_DOCUMENT_ERR);
  assert!(a.import_node(&b, in_b, true).is_ok(), "the node does belong to the source document");
}

#[test]
#[should_panic(expected = "another document")]
fn reading_through_a_node_from_another_document_panics() {
  let a = Document::new();
  let mut b = Document::new();
  let in_b = b.create_element("b").unwrap();
  // An accessor that returns a value has no way to report the mistake, so it stops the caller instead.
  let _ = a.node_type(in_b);
}

#[test]
fn attributes_are_nodes() {
  let (doc, r, _, _) = sample();
  let attr = doc.get_attribute_node(r, "a").unwrap();
  assert_eq!(doc.node_type(attr), NodeType::ATTRIBUTE_NODE);
  assert_eq!(doc.node_name(attr), "a");
  assert_eq!(doc.node_value(attr), Some("1"));
  let map = doc.attributes(r);
  assert_eq!(map.length(), 1);
  assert_eq!(map.item(0), Some(attr));
  assert_eq!(map.get_named_item("a"), Some(attr));
}

#[test]
fn set_and_remove_attribute_node() {
  let mut doc = Document::new();
  let e = doc.create_element("e").unwrap();
  let attr = doc.create_attribute("k").unwrap();
  doc.set_attribute_node(e, attr).unwrap();
  assert_eq!(doc.node_type(attr), NodeType::ATTRIBUTE_NODE);
  assert_eq!(doc.attributes(e).length(), 1);
  doc.remove_attribute_node(e, attr).unwrap();
  assert!(doc.attributes(e).is_empty());
}

#[test]
fn an_attribute_cannot_belong_to_two_elements() {
  let mut doc = Document::new();
  let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
  let attr = doc.create_attribute("k").unwrap();
  doc.set_attribute_node(a, attr).unwrap();
  assert_eq!(doc.set_attribute_node(b, attr).unwrap_err().code(), ExceptionCode::INUSE_ATTRIBUTE_ERR);
}

#[test]
fn an_attribute_is_not_a_child() {
  let mut doc = Document::new();
  let e = doc.create_element("e").unwrap();
  let attr = doc.create_attribute("k").unwrap();
  assert_eq!(doc.append_child(e, attr).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
}

#[test]
fn get_elements_by_tag_name_walks_in_document_order() {
  let (doc, _, _, _) = sample();
  let all = doc.get_elements_by_tag_name(doc.document_node(), "*");
  let names: Vec<_> = all.iter().map(|n| doc.node_name(n)).collect();
  assert_eq!(names, ["r", "b", "c"]);
  assert_eq!(doc.get_elements_by_tag_name(doc.document_node(), "b").length(), 1);
}

#[test]
fn get_elements_by_tag_name_ns_filters_on_namespace() {
  let mut doc = Document::new();
  let root = doc.create_element_ns(Some("urn:x"), "p:a").unwrap();
  let child = doc.create_element_ns(Some("urn:y"), "q:a").unwrap();
  doc.append_child(root, child).unwrap();
  doc.append_child(doc.document_node(), root).unwrap();
  let root_node = doc.document_node();
  assert_eq!(doc.get_elements_by_tag_name_ns(root_node, Some("urn:x"), "a").length(), 1);
  assert_eq!(doc.get_elements_by_tag_name_ns(root_node, Some("*"), "a").length(), 2);
  assert_eq!(doc.get_elements_by_tag_name_ns(root_node, Some("urn:y"), "*").length(), 1);
}

#[test]
fn get_element_by_id_finds_marked_ids() {
  let mut doc = Document::new();
  let root = doc.create_element("root").unwrap();
  doc.set_attribute(root, "id", "top").unwrap();
  doc.append_child(doc.document_node(), root).unwrap();
  // Not an ID until marked as one.
  assert_eq!(doc.get_element_by_id("top"), None);
  doc.set_id_attribute(root, "id", true).unwrap();
  assert_eq!(doc.get_element_by_id("top"), Some(root));
}

#[test]
fn replace_child_swaps_a_node() {
  let (mut doc, r, b, c) = sample();
  let x = doc.create_element("x").unwrap();
  let removed = doc.replace_child(r, x, b).unwrap();
  assert_eq!(removed, b);
  assert_eq!(doc.parent(b), None);
  assert_eq!(doc.first_child(r), Some(x));
  assert_eq!(doc.last_child(r), Some(c));
}

#[test]
fn a_document_fragment_inserts_its_children() {
  let mut doc = Document::new();
  let root = doc.create_element("root").unwrap();
  doc.append_child(doc.document_node(), root).unwrap();
  let fragment = doc.create_document_fragment();
  let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
  doc.append_child(fragment, a).unwrap();
  doc.append_child(fragment, b).unwrap();
  doc.append_child(root, fragment).unwrap();
  // The fragment's children moved in; the fragment is left empty.
  let names: Vec<_> = doc.children(root).map(|n| doc.node_name(n)).collect();
  assert_eq!(names, ["a", "b"]);
  assert_eq!(doc.first_child(fragment), None);
}

#[test]
fn a_document_takes_only_one_root_element() {
  let mut doc = Document::new();
  let first = doc.create_element("a").unwrap();
  let second = doc.create_element("b").unwrap();
  doc.append_child(doc.document_node(), first).unwrap();
  assert_eq!(doc.append_child(doc.document_node(), second).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
}

#[test]
fn a_document_refuses_bare_text() {
  let mut doc = Document::new();
  let text = doc.create_text_node("x");
  assert_eq!(doc.append_child(doc.document_node(), text).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
}

#[test]
fn a_fragment_of_two_elements_is_refused_by_the_document_whole() {
  let mut doc = Document::new();
  let fragment = doc.create_document_fragment();
  let (a, b) = (doc.create_element("a").unwrap(), doc.create_element("b").unwrap());
  doc.append_child(fragment, a).unwrap();
  doc.append_child(fragment, b).unwrap();
  // Two root elements at once: refused before anything is inserted.
  assert_eq!(doc.append_child(doc.document_node(), fragment).unwrap_err().code(), ExceptionCode::HIERARCHY_REQUEST_ERR);
  assert!(doc.document_element().is_none(), "the tree is untouched by the failed insert");
}

#[test]
fn namespace_rules_are_enforced() {
  let mut doc = Document::new();
  // A prefix with no namespace.
  assert_eq!(doc.create_element_ns(None, "p:a").unwrap_err().code(), ExceptionCode::NAMESPACE_ERR);
  // The xml prefix bound to the wrong namespace.
  assert_eq!(doc.create_element_ns(Some("urn:x"), "xml:a").unwrap_err().code(), ExceptionCode::NAMESPACE_ERR);
  // An xmlns attribute in the wrong namespace.
  let e = doc.create_element("e").unwrap();
  assert_eq!(doc.set_attribute_ns(e, Some("urn:x"), "xmlns:p", "v").unwrap_err().code(), ExceptionCode::NAMESPACE_ERR);
}

#[test]
fn base_uri_walks_to_the_nearest_recorded_base() {
  let mut doc = Document::new();
  doc.set_document_base(Some("file:///doc.xml"));
  let a = doc.create_element("a").unwrap();
  doc.append_child(doc.document_node(), a).unwrap();
  let b = doc.create_element("b").unwrap();
  doc.append_child(a, b).unwrap();
  doc.set_element_base(b, Some("file:///sub/"));
  let text = doc.create_text_node("x");
  doc.append_child(b, text).unwrap();

  assert_eq!(doc.base_uri(a).as_deref(), Some("file:///doc.xml"), "falls back to the document base");
  assert_eq!(doc.base_uri(b).as_deref(), Some("file:///sub/"), "uses its own recorded base");
  assert_eq!(doc.base_uri(text).as_deref(), Some("file:///sub/"), "a text node inherits the nearest element's base");
  let bare = Document::new();
  assert_eq!(bare.base_uri(bare.document_node()), None, "no base information means no base URI");
}

#[test]
fn get_elements_by_tag_name_searches_under_the_node_it_is_given() {
  // The DOM has this on an element as well as on a document. The sample is `<r a="1"><b/>text<c/></r>`.
  let (doc, root, b, _) = sample();
  let under_root = doc.get_elements_by_tag_name(root, "*");
  assert_eq!(under_root.iter().map(|n| doc.node_name(n)).collect::<Vec<_>>(), ["b", "c"]);
  assert_eq!(doc.get_elements_by_tag_name(root, "r").length(), 0, "the node it starts under is not in the list");
  assert_eq!(doc.get_elements_by_tag_name(b, "*").length(), 0, "nothing is under `b`");

  // The same lists through a `NodeRef`, which reads the way `Element.getElementsByTagName` does.
  assert_eq!(doc.node(root).get_elements_by_tag_name("*").length(), 2);
  assert_eq!(doc.node(root).get_elements_by_tag_name_ns(Some("*"), "c").length(), 1);
}
