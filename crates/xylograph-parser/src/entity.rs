//! Entities and the stack of entities being read.
//!
//! An XML document is a tree of entities, not a single stream: a reference may suspend the
//! current entity, read another to its end, and resume. Positions, base URIs and the limits
//! that keep expansion bounded are all properties of the *innermost* entity, which is why the
//! stack exists from the first phase rather than arriving with DTD support.

use std::sync::Arc;

use xylograph_core::error::{Error, ErrorKind, Location, Result};
use xylograph_core::uri::UriReference;

use crate::stream::CharStream;

/// What kind of entity is being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKind {
  /// The document entity: the outermost one, where parsing starts.
  Document,
  /// The external DTD subset.
  ExternalSubset,
  /// A general entity whose replacement text was given in its declaration.
  InternalGeneral,
  /// A general entity read from a separate resource.
  ExternalGeneral,
  /// A parameter entity whose replacement text was given in its declaration.
  InternalParameter,
  /// A parameter entity read from a separate resource.
  ExternalParameter,
}

impl EntityKind {
  /// True if the entity has its own resource, and therefore its own base URI.
  #[must_use]
  pub const fn is_external(self) -> bool {
    matches!(self, Self::Document | Self::ExternalSubset | Self::ExternalGeneral | Self::ExternalParameter)
  }

  /// True if the entity is a parameter entity, which is only referenced inside the DTD.
  #[must_use]
  pub const fn is_parameter(self) -> bool {
    matches!(self, Self::InternalParameter | Self::ExternalParameter)
  }

  /// True if the entity counts against the expansion budget.
  ///
  /// The document entity and the external subset are read once, however large; only entities
  /// pulled in by a reference can be multiplied by an attacker.
  #[must_use]
  pub const fn is_expansion(self) -> bool {
    !matches!(self, Self::Document | Self::ExternalSubset)
  }
}

/// One entity being read.
///
/// # Examples
///
/// ```
/// use xylograph_parser::{CharStream, Entity, EntityKind};
///
/// let doc = Entity::document(CharStream::new().with_system_id("file:///doc.xml"));
/// assert_eq!(doc.kind(), EntityKind::Document);
/// assert_eq!(doc.base_uri().map(ToString::to_string).as_deref(), Some("file:///doc.xml"));
/// ```
#[derive(Debug)]
pub struct Entity {
  name: Option<Arc<str>>,
  kind: EntityKind,
  stream: CharStream,
  base_uri: Option<UriReference>,
}

impl Entity {
  /// Wraps `stream` as the document entity.
  #[must_use]
  pub fn document(stream: CharStream) -> Self {
    Self::new(None, EntityKind::Document, stream, None)
  }

  /// Creates an entity.
  ///
  /// `inherited_base` is the base URI of the entity in which this one was *declared*. It is
  /// used only for internal entities, which have no resource of their own; an external entity
  /// takes its base URI from its own system identifier.
  #[must_use]
  pub fn new(
    name: Option<Arc<str>>,
    kind: EntityKind,
    stream: CharStream,
    inherited_base: Option<UriReference>,
  ) -> Self {
    let base_uri =
      if kind.is_external() { stream.system_id().and_then(|id| UriReference::parse(id).ok()) } else { inherited_base };
    Self { name, kind, stream, base_uri }
  }

  /// The entity's name, or `None` for the document entity and the external subset.
  #[must_use]
  pub fn name(&self) -> Option<&Arc<str>> {
    self.name.as_ref()
  }

  /// What kind of entity this is.
  #[must_use]
  pub const fn kind(&self) -> EntityKind {
    self.kind
  }

  /// The base URI against which relative references inside this entity resolve.
  ///
  /// This is the entity's own URI, or, for an internal entity, that of the entity in which it
  /// was declared. `xml:base` attributes are applied later, on the tree.
  #[must_use]
  pub fn base_uri(&self) -> Option<&UriReference> {
    self.base_uri.as_ref()
  }

  /// The character stream.
  #[must_use]
  pub fn stream(&self) -> &CharStream {
    &self.stream
  }

  /// The character stream, mutably.
  pub fn stream_mut(&mut self) -> &mut CharStream {
    &mut self.stream
  }
}

/// Bounds on how much work a document may cause.
///
/// Defaults are generous enough for real documents and tight enough that the classic
/// expansion attacks fail. Every limit can be raised, and [`Limits::unlimited`] removes them
/// for trusted input.
///
/// # Examples
///
/// ```
/// use xylograph_parser::Limits;
///
/// let limits = Limits::default().with_max_depth(8);
/// assert_eq!(limits.max_depth, 8);
/// assert_eq!(Limits::unlimited().max_expansions, u32::MAX);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
  /// Maximum number of entities open at once, the document entity included.
  pub max_depth: usize,
  /// Maximum number of entity expansions in one document.
  pub max_expansions: u32,
  /// Maximum number of characters all expansions may produce in total.
  pub max_expansion_chars: u64,
}

impl Default for Limits {
  fn default() -> Self {
    Self { max_depth: 64, max_expansions: 100_000, max_expansion_chars: 64 * 1024 * 1024 }
  }
}

impl Limits {
  /// Limits that permit anything; only for input that is known to be trustworthy.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_depth: usize::MAX, max_expansions: u32::MAX, max_expansion_chars: u64::MAX }
  }

  /// Returns a copy with [`max_depth`](Self::max_depth) set.
  #[must_use]
  pub const fn with_max_depth(mut self, max_depth: usize) -> Self {
    self.max_depth = max_depth;
    self
  }

  /// Returns a copy with [`max_expansions`](Self::max_expansions) set.
  #[must_use]
  pub const fn with_max_expansions(mut self, max_expansions: u32) -> Self {
    self.max_expansions = max_expansions;
    self
  }

  /// Returns a copy with [`max_expansion_chars`](Self::max_expansion_chars) set.
  #[must_use]
  pub const fn with_max_expansion_chars(mut self, max_expansion_chars: u64) -> Self {
    self.max_expansion_chars = max_expansion_chars;
    self
  }
}

/// The stack of entities currently being read, innermost last.
///
/// # Examples
///
/// ```
/// use xylograph_parser::{CharStream, Entity, EntityKind, EntityStack, Limits};
///
/// let mut stack = EntityStack::new(Entity::document(CharStream::from_text("<a>&e;</a>")?), Limits::default());
/// assert_eq!(stack.depth(), 1);
///
/// // A reference to `e` suspends the document entity and reads the replacement text.
/// stack.push(Entity::new(
///   Some("e".into()),
///   EntityKind::InternalGeneral,
///   CharStream::from_text("text")?,
///   stack.base_uri().cloned(),
/// ))?;
/// assert_eq!(stack.depth(), 2);
/// assert_eq!(stack.current().stream().remainder(), "text");
///
/// // Reading it to the end resumes the document entity.
/// stack.current_mut().stream_mut().advance_chars(4);
/// assert!(stack.current().stream().is_exhausted());
/// stack.pop();
/// assert_eq!(stack.depth(), 1);
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Debug)]
pub struct EntityStack {
  entities: Vec<Entity>,
  limits: Limits,
  expansions: u32,
  expansion_chars: u64,
}

impl EntityStack {
  /// Creates a stack holding just the document entity.
  #[must_use]
  pub fn new(document: Entity, limits: Limits) -> Self {
    Self { entities: vec![document], limits, expansions: 0, expansion_chars: 0 }
  }

  /// Suspends the current entity and begins reading `entity`.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::WellFormedness`] if the entity is already open, which is the
  /// well-formedness constraint "No Recursion", and [`ErrorKind::Limit`] if a bound in
  /// [`Limits`] would be exceeded.
  ///
  /// # Examples
  ///
  /// An entity that refers to itself is refused rather than expanded forever:
  ///
  /// ```
  /// use xylograph_core::ErrorKind;
  /// use xylograph_parser::{CharStream, Entity, EntityKind, EntityStack, Limits};
  ///
  /// let mut stack = EntityStack::new(Entity::document(CharStream::from_text("&e;")?), Limits::default());
  /// fn entity_e() -> Entity {
  ///   Entity::new(Some("e".into()), EntityKind::InternalGeneral, CharStream::from_text("&e;").unwrap(), None)
  /// }
  ///
  /// stack.push(entity_e())?;
  /// let err = stack.push(entity_e()).unwrap_err();
  /// assert_eq!(err.kind(), ErrorKind::WellFormedness);
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  pub fn push(&mut self, entity: Entity) -> Result<()> {
    if let Some(name) = entity.name() {
      if self.is_open(name) {
        return Err(
          Error::new(ErrorKind::WellFormedness, format!("entity \"{name}\" refers to itself")).at(self.location()),
        );
      }
    }
    if self.entities.len() >= self.limits.max_depth {
      return Err(self.limit_exceeded(format!("more than {} entities are open at once", self.limits.max_depth)));
    }
    if entity.kind().is_expansion() {
      self.expansions += 1;
      if self.expansions > self.limits.max_expansions {
        return Err(self.limit_exceeded(format!("more than {} entity expansions", self.limits.max_expansions)));
      }
      // An internal entity arrives with its replacement text already decoded, so charge for
      // what it holds now; `feed` charges for whatever it grows by later.
      self.charge_expansion(entity.stream().chars_decoded())?;
    }
    self.entities.push(entity);
    Ok(())
  }

  /// Finishes the innermost entity and resumes the one that referenced it.
  ///
  /// Returns `None` when only the document entity is left, which is never popped.
  pub fn pop(&mut self) -> Option<Entity> {
    if self.entities.len() <= 1 {
      return None;
    }
    self.entities.pop()
  }

  /// Supplies bytes to the innermost entity.
  ///
  /// # Errors
  ///
  /// Whatever [`CharStream::feed`] returns, plus [`ErrorKind::Limit`] if the expansion budget
  /// is exhausted.
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    let before = self.current().stream().chars_decoded();
    let counts = self.current().kind().is_expansion();
    self.current_mut().stream_mut().feed(bytes, last)?;
    if counts {
      let grew = self.current().stream().chars_decoded() - before;
      self.charge_expansion(grew)?;
    }
    Ok(())
  }

  /// Adds `chars` to the expansion budget.
  fn charge_expansion(&mut self, chars: u64) -> Result<()> {
    self.expansion_chars = self.expansion_chars.saturating_add(chars);
    if self.expansion_chars > self.limits.max_expansion_chars {
      return Err(self.limit_exceeded(format!(
        "entity expansion produced more than {} characters",
        self.limits.max_expansion_chars
      )));
    }
    Ok(())
  }

  /// The innermost entity.
  #[must_use]
  pub fn current(&self) -> &Entity {
    self.entities.last().expect("the document entity is never popped")
  }

  /// The innermost entity, mutably.
  pub fn current_mut(&mut self) -> &mut Entity {
    self.entities.last_mut().expect("the document entity is never popped")
  }

  /// The document entity.
  #[must_use]
  pub fn document(&self) -> &Entity {
    self.entities.first().expect("the document entity is never popped")
  }

  /// Number of entities open, the document entity included.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.entities.len()
  }

  /// True if an entity of this name is already open.
  #[must_use]
  pub fn is_open(&self, name: &str) -> bool {
    self.entities.iter().any(|e| e.name().is_some_and(|n| &**n == name))
  }

  /// The position in the innermost entity.
  #[must_use]
  pub fn location(&self) -> Location {
    self.current().stream().location()
  }

  /// The base URI in effect, taken from the innermost entity that has one.
  #[must_use]
  pub fn base_uri(&self) -> Option<&UriReference> {
    self.entities.iter().rev().find_map(Entity::base_uri)
  }

  /// The limits this stack enforces.
  #[must_use]
  pub const fn limits(&self) -> &Limits {
    &self.limits
  }

  fn limit_exceeded(&self, message: String) -> Error {
    Error::new(ErrorKind::Limit, message).at(self.location())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn text_entity(name: &str, text: &str) -> Entity {
    Entity::new(Some(name.into()), EntityKind::InternalGeneral, CharStream::from_text(text).unwrap(), None)
  }

  fn stack_of(text: &str) -> EntityStack {
    EntityStack::new(Entity::document(CharStream::from_text(text).unwrap()), Limits::default())
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
    assert_eq!(err.kind(), ErrorKind::WellFormedness);
    assert!(err.message().contains("\"a\""));
  }

  #[test]
  fn depth_is_bounded() {
    let mut stack =
      EntityStack::new(Entity::document(CharStream::from_text("x").unwrap()), Limits::default().with_max_depth(3));
    stack.push(text_entity("a", "x")).unwrap();
    stack.push(text_entity("b", "x")).unwrap();
    let err = stack.push(text_entity("c", "x")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Limit);
  }

  #[test]
  fn the_number_of_expansions_is_bounded() {
    let mut stack =
      EntityStack::new(Entity::document(CharStream::from_text("x").unwrap()), Limits::default().with_max_expansions(2));
    // Sibling expansions, not nested ones: the count is cumulative.
    stack.push(text_entity("a", "x")).unwrap();
    stack.pop();
    stack.push(text_entity("b", "x")).unwrap();
    stack.pop();
    assert_eq!(stack.push(text_entity("c", "x")).unwrap_err().kind(), ErrorKind::Limit);
  }

  #[test]
  fn expanded_characters_are_bounded_when_the_text_is_already_decoded() {
    let mut stack = EntityStack::new(
      Entity::document(CharStream::from_text("x").unwrap()),
      Limits::default().with_max_expansion_chars(10),
    );
    stack.push(text_entity("a", "0123456789")).unwrap();
    stack.pop();
    assert_eq!(stack.push(text_entity("b", "x")).unwrap_err().kind(), ErrorKind::Limit);
  }

  #[test]
  fn expanded_characters_are_bounded_as_an_external_entity_is_read() {
    let mut stack = EntityStack::new(
      Entity::document(CharStream::from_text("x").unwrap()),
      Limits::default().with_max_expansion_chars(4),
    );
    stack
      .push(Entity::new(
        Some("e".into()),
        EntityKind::ExternalGeneral,
        CharStream::with_encoding("UTF-8").unwrap(),
        None,
      ))
      .unwrap();
    stack.feed(b"abcd", false).unwrap();
    assert_eq!(stack.feed(b"e", true).unwrap_err().kind(), ErrorKind::Limit);
  }

  #[test]
  fn the_document_entity_is_not_charged_to_the_expansion_budget() {
    let mut stack = EntityStack::new(
      Entity::document(CharStream::with_encoding("UTF-8").unwrap()),
      Limits::default().with_max_expansion_chars(4),
    );
    stack.feed("a very long document".as_bytes(), true).unwrap();
  }

  #[test]
  fn base_uri_comes_from_the_innermost_entity_that_has_one() {
    let doc = Entity::document(CharStream::from_text("x").unwrap().with_system_id("file:///a/doc.xml"));
    let mut stack = EntityStack::new(doc, Limits::default());
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
    let mut stack = EntityStack::new(doc, Limits::default());
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
}
