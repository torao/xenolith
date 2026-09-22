use super::*;

/// The walk as a compact string, so the order and the nesting are both readable.
fn trace(doc: &Document, start: NodeId) -> String {
  doc
    .walk(start)
    .map(|(visit, node)| {
      let name = doc.node_name(node);
      match visit {
        Visit::Enter => format!("<{name}>"),
        Visit::Leave => format!("</{name}>"),
      }
    })
    .collect()
}

#[test]
fn reports_both_sides_of_every_node_in_order() {
  let mut doc = Document::new();
  let r = doc.create_element("r").unwrap();
  let a = doc.create_text_node("a");
  let b = doc.create_element("b").unwrap();
  let c = doc.create_element("c").unwrap();
  doc.append_child(r, a).unwrap();
  doc.append_child(r, b).unwrap();
  doc.append_child(b, c).unwrap();
  doc.append_child(doc.document_node(), r).unwrap();

  assert_eq!(trace(&doc, r), "<r><#text></#text><b><c></c></b></r>");
}

#[test]
fn a_leaf_is_entered_and_left() {
  // The element is never attached, so this also covers walking a subtree that hangs outside the tree.
  let mut doc = Document::new();
  let a = doc.create_element("a").unwrap();
  assert_eq!(trace(&doc, a), "<a></a>");
}

#[test]
fn the_walk_stays_inside_the_subtree_it_started_from() {
  // `b` has a following sibling, which the walk must not reach when it starts at `b`.
  let mut doc = Document::new();
  let r = doc.create_element("r").unwrap();
  let b = doc.create_element("b").unwrap();
  let after = doc.create_element("after").unwrap();
  doc.append_child(r, b).unwrap();
  doc.append_child(r, after).unwrap();
  doc.append_child(doc.document_node(), r).unwrap();

  assert_eq!(trace(&doc, b), "<b></b>", "the walk ends on leaving its own start");
}

#[test]
fn a_finished_walk_stays_finished() {
  let mut doc = Document::new();
  let a = doc.create_element("a").unwrap();
  let mut walk = doc.walk(a);
  assert_eq!(walk.by_ref().count(), 2, "entering and leaving the one node");
  assert_eq!(walk.next(), None);
  assert_eq!(walk.next(), None, "it reports nothing more, as a fused iterator");
}

#[test]
#[should_panic(expected = "another document")]
fn walking_with_a_node_from_another_document_panics() {
  let a = Document::new();
  let mut b = Document::new();
  let in_b = b.create_element("b").unwrap();
  // The check is at the call, not part-way through the walk.
  let _ = a.walk(in_b);
}
