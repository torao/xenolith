use super::*;

#[test]
fn an_owned_attribute_lends_back_the_attribute_it_was_made_from() {
  let at = Location { line: 2, column: 4, offset: 10, ..Location::unknown() };
  let value_at = Location { line: 2, column: 7, offset: 13, ..Location::unknown() };
  let borrowed = AttributeRef {
    prefix: Some("p"),
    local: "a",
    namespace: Some("urn:p"),
    value: "v",
    location: at,
    value_location: value_at,
  };
  let owned = Attribute::from(borrowed);
  let back = owned.as_attribute_ref();
  assert_eq!(
    (back.prefix, back.local, back.namespace, back.value, back.declares_namespace()),
    (Some("p"), "a", Some("urn:p"), "v", false)
  );
  assert_eq!((back.location.line, back.location.column), (2, 4), "the name's position survives the copy");
  assert_eq!((back.value_location.line, back.value_location.column), (2, 7));
}

#[test]
fn owned_attributes_are_read_through_the_same_view_as_any_other() {
  let list = vec![
    Attribute {
      prefix: None,
      local: "x".into(),
      namespace: None,
      value: "1".into(),
      location: Location::unknown(),
      value_location: Location::unknown(),
    },
    Attribute {
      prefix: Some("xmlns".into()),
      local: "p".into(),
      namespace: Some(crate::name::XMLNS_NS_URI.into()),
      value: "urn:p".into(),
      location: Location::unknown(),
      value_location: Location::unknown(),
    },
  ];
  let view = Attributes::new(&list);

  assert_eq!(view.len(), 2);
  assert_eq!(view.get_by_name(None, "x").map(|a| a.value), Some("1"));
  assert!(view.get(1).is_some_and(|a| a.declares_namespace()));
  assert!(view.get(2).is_none());
}
