use super::*;

fn text_entity(name: &str, text: &str) -> Entity {
  Entity::new(Some(name.into()), EntityKind::InternalGeneral, CharStream::from_text(text).unwrap(), None)
}

fn stack_of(text: &str) -> EntityStack {
  EntityStack::new(Entity::document(CharStream::from_text(text).unwrap()), EntityLimits::default())
}

#[test]
fn the_document_entity_is_never_popped() {
  let mut stack = stack_of("<a/>");
  assert_eq!(stack.depth(), 1);
  assert!(stack.pop().is_none());
  assert_eq!(stack.depth(), 1);
}

#[test]
fn push_and_pop_switch_the_current_entity() {
  let mut stack = stack_of("<a>&e;</a>");
  stack.push(text_entity("e", "inner")).unwrap();
  assert_eq!(stack.current().stream().remainder(), "inner");
  assert_eq!(stack.current().name().map(|n| &**n), Some("e"));
  stack.pop();
  assert_eq!(stack.current().stream().remainder(), "<a>&e;</a>");
}

#[test]
fn recursion_is_refused_directly_and_indirectly() {
  let mut stack = stack_of("&a;");
  stack.push(text_entity("a", "&b;")).unwrap();
  stack.push(text_entity("b", "&a;")).unwrap();
  let err = stack.push(text_entity("a", "&b;")).unwrap_err();
  assert!(matches!(err, Error::WellFormedness { .. }));
  assert!(err.message().contains("\"a\""));
}

/// A programmer who hits a limit is told which one to raise; the number alone would leave
/// them grepping. See the guidance in `crate::error`.
#[test]
fn limit_errors_name_the_limit_to_raise() {
  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_depth: Some(1), ..EntityLimits::default() },
  );
  assert!(stack.push(text_entity("a", "x")).unwrap_err().message().contains("limits.entities.max_depth"));

  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_expansions: Some(0), ..EntityLimits::default() },
  );
  assert!(stack.push(text_entity("a", "x")).unwrap_err().message().contains("limits.entities.max_expansions"));

  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_expansion_chars: Some(1), ..EntityLimits::default() },
  );
  assert!(stack.push(text_entity("a", "long")).unwrap_err().message().contains("limits.entities.max_expansion_chars"));
}

/// Recursion is easiest to fix when the cycle is spelled out.
#[test]
fn a_recursive_entity_error_shows_the_cycle() {
  let mut stack = stack_of("&a;");
  stack.push(text_entity("a", "&b;")).unwrap();
  stack.push(text_entity("b", "&a;")).unwrap();
  let message = stack.push(text_entity("a", "&b;")).unwrap_err().message().to_owned();
  assert!(message.contains("a -> b"), "{message:?}");
}

#[test]
fn depth_is_bounded() {
  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_depth: Some(3), ..EntityLimits::default() },
  );
  stack.push(text_entity("a", "x")).unwrap();
  stack.push(text_entity("b", "x")).unwrap();
  let err = stack.push(text_entity("c", "x")).unwrap_err();
  assert!(matches!(err, Error::Limit { .. }));
}

#[test]
fn the_number_of_expansions_is_bounded() {
  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_expansions: Some(2), ..EntityLimits::default() },
  );
  // Sibling expansions, not nested ones: the count is cumulative.
  stack.push(text_entity("a", "x")).unwrap();
  stack.pop();
  stack.push(text_entity("b", "x")).unwrap();
  stack.pop();
  assert!(matches!(stack.push(text_entity("c", "x")).unwrap_err(), Error::Limit { .. }));
}

#[test]
fn expanded_characters_are_bounded_when_the_text_is_already_decoded() {
  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_expansion_chars: Some(10), ..EntityLimits::default() },
  );
  stack.push(text_entity("a", "0123456789")).unwrap();
  stack.pop();
  assert!(matches!(stack.push(text_entity("b", "x")).unwrap_err(), Error::Limit { .. }));
}

#[test]
fn expanded_characters_are_bounded_as_an_external_entity_is_read() {
  let mut stack = EntityStack::new(
    Entity::document(CharStream::from_text("x").unwrap()),
    EntityLimits { max_expansion_chars: Some(4), ..EntityLimits::default() },
  );
  stack
    .push(Entity::new(Some("e".into()), EntityKind::ExternalGeneral, CharStream::with_encoding("UTF-8").unwrap(), None))
    .unwrap();
  stack.feed(b"abcd", false).unwrap();
  assert!(matches!(stack.feed(b"e", true).unwrap_err(), Error::Limit { .. }));
}

#[test]
fn the_document_entity_is_not_counted_toward_the_expansion_limit() {
  let mut stack = EntityStack::new(
    Entity::document(CharStream::with_encoding("UTF-8").unwrap()),
    EntityLimits { max_expansion_chars: Some(4), ..EntityLimits::default() },
  );
  stack.feed("a very long document".as_bytes(), true).unwrap();
}

#[test]
fn base_uri_comes_from_the_innermost_entity_that_has_one() {
  let doc = Entity::document(CharStream::from_text("x").unwrap().with_system_id("file:///a/doc.xml"));
  let mut stack = EntityStack::new(doc, EntityLimits::default());
  assert_eq!(stack.base_uri().map(ToString::to_string).as_deref(), Some("file:///a/doc.xml"));

  // An internal entity inherits the base URI of where it was declared.
  stack.push(text_entity("e", "x")).unwrap();
  assert_eq!(stack.base_uri().map(ToString::to_string).as_deref(), Some("file:///a/doc.xml"));

  // An external entity brings its own.
  stack
    .push(Entity::new(
      Some("x".into()),
      EntityKind::ExternalGeneral,
      CharStream::from_text("y").unwrap().with_system_id("file:///b/part.ent"),
      None,
    ))
    .unwrap();
  assert_eq!(stack.base_uri().map(ToString::to_string).as_deref(), Some("file:///b/part.ent"));
  stack.pop();
  assert_eq!(stack.base_uri().map(ToString::to_string).as_deref(), Some("file:///a/doc.xml"));
}

#[test]
fn locations_report_the_innermost_entity() {
  let doc = Entity::document(CharStream::from_text("<a>&e;</a>").unwrap().with_system_id("file:///doc.xml"));
  let mut stack = EntityStack::new(doc, EntityLimits::default());
  stack.current_mut().stream_mut().advance_chars(3);

  let inner = Entity::new(
    Some("e".into()),
    EntityKind::InternalGeneral,
    CharStream::from_text("hi").unwrap().with_system_id("file:///e.ent"),
    None,
  );
  stack.push(inner).unwrap();
  stack.current_mut().stream_mut().advance_chars(1);

  let at = stack.location();
  assert_eq!(at.system_id.as_deref(), Some("file:///e.ent"));
  assert_eq!(at.column, 2, "the position inside the entity, not the document");

  stack.pop();
  let at = stack.location();
  assert_eq!(at.system_id.as_deref(), Some("file:///doc.xml"));
  assert_eq!(at.column, 4);
}
