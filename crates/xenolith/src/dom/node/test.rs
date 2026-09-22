use super::*;

#[test]
fn an_optional_handle_costs_no_more_than_a_handle() {
  // Carrying the document tag doubles a bare handle. The zero niche in `DocumentId` then pays that back on the
  // optional form, which is what a node's five links are made of, so a node costs what it did before.
  assert_eq!(size_of::<NodeId>(), 8);
  assert_eq!(size_of::<Option<NodeId>>(), size_of::<NodeId>());
}
