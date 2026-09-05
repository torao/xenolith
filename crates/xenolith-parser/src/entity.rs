//! Entities and the entity stack currently being read.
//!
//! XML documents may reference multiple external entities internally through entity references. This module implements
//! the functionality to read XML documents as a tree of entities. This structure forms a path from the document entity
//! to the entity currently being read at any given point in the reading process, and this module maintains this path
//! as a stack. The current position, the base URI, and the scope of the boundaries are determined by the entity
//! currently being read (in other words, the top or innermost of the stack).
//!
//! # The physical structure of entities
//!
//! An XML document has two distinct structures. One is the logical structure formed by nested elements. This is the
//! *tree* that many people picture, and it is what the DOM represents. The other is the physical reference structure
//! formed by entities, and this module implements the latter. An entity is a unit of storage (the document itself, an
//! external resource, a run of replacement text specified in the DTD), and the content of one entity is inserted into
//! another via references such as `&e;`. This structure differs from the DOM, and a single element may contain multiple
//! entities. Similar to a function call, reading delves deeply into the referenced entity, and once reading that entity
//! is complete, it returns to the referring one.
//!
//! For example, suppose a book has been split into separate files for each chapter, resulting in the following three
//! entities: `book.xml` is a document entity, and `chapter1.xml` and `chapter2.xml` are external entities.
//!
//! `book.xml`:
//!
//! ```xml
//! <!DOCTYPE book [
//!   <!ENTITY chapter1 SYSTEM "chapter1.xml">
//!   <!ENTITY chapter2 SYSTEM "chapter2.xml">
//! ]>
//! <book>
//!   &chapter1;
//!   &chapter2;
//! </book>
//! ```
//!
//! `chapter1.xml` (and `chapter2.xml` likewise):
//!
//! ```xml
//! <chapter>
//!   <title>Introduction</title>
//! </chapter>
//! ```
//!
//! The logical structure (the DOM) is a single tree with `book` as the root, under which each `chapter` is placed.
//! The physical structure consists of three entities: `book.xml`, `chapter1.xml`, and `chapter2.xml`. The tag of
//! `book` element is located in `book.xml`, but its child element, `chapter`, is located in two other files and is
//! incorporated into `book.xml` via the references `&chapter1;` and `&chapter2;`. Therefore, a single `book` element
//! spans three entities, and a single DOM tree is composed of all three of them.
//!

use std::sync::Arc;

use xenolith_core::error::{Error, Location, Result};
use xenolith_core::uri::UriReference;

use xenolith_core::stream::CharStream;

/// What kind of entity is being read.
///
/// XML entities are classified into four categories along two independent axes × tow categories. In addition to these,
/// there are two types of nameless entities that serve as the foundation for parsing rather than being invoked by
/// reference.
///
/// 1. The location where they are referenced:
///    1. general entities, referenced within the document body using `&name;`.
///    2. parameter entities, referenced *only* within the DTD using `%name;`.
/// 2. The location of their content:
///    1. internal entities, where the replacement text is written directly in the declaration.
///    2. external entities, which refer to a separate resource (such as a file or URL).
/// 3. The document entity, which represents the entire document.
/// 4. the external subset, which is the external DTD referenced by the `DOCTYPE`.
///
/// The internal subset (`[ ... ]` within the `DOCTYPE`) is not an independent entity, but rather a part of the document
/// entity.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKind {
  /// Document entity. The outermost entity and the starting point for parsing. It is placed at the bottom of the entity
  /// stack and is never popped. It has no name.
  Document,
  /// External DTD subset. The external DTD file referenced by the `DOCTYPE` element using the `SYSTEM ID` or `PUBLIC
  /// ID`. It has no name.
  ExternalSubset,
  /// Internal general entity. A general entity whose replacement text is specified directly within the declaration.
  /// Referenced in the document body as `&name;`. A general entity whose replacement text was given in its declaration.
  /// Example: `<!ENTITY name "replacement text">`
  InternalGeneral,
  /// External general entity. A general entity whose content is loaded from a separate resource. Referenced in the
  /// document body as `&name;`. Example: `<!ENTITY name SYSTEM "chap1.xml">`
  ExternalGeneral,
  /// Internal parameter entity. A parameter entity for which the replacement text is specified directly within the
  /// declaration. Referenced only within the DTD as `%name;`. Example: `<!ENTITY % name "replacement text">`
  InternalParameter,
  /// External parameter entity. A parameter entity whose content is loaded from a separate resource. Referenced only
  /// within the DTD as `%name;`. Example: `<!ENTITY % name SYSTEM "common.dtd">`
  ExternalParameter,
}

impl EntityKind {
  /// True if the entity has its own resource and, therefore, its own base URI.
  ///
  #[must_use]
  pub const fn is_external(self) -> bool {
    matches!(self, Self::Document | Self::ExternalSubset | Self::ExternalGeneral | Self::ExternalParameter)
  }

  /// True if the entity is a parameter entity and is referenced only within the DTD.
  ///
  #[must_use]
  pub const fn is_parameter(self) -> bool {
    matches!(self, Self::InternalParameter | Self::ExternalParameter)
  }

  /// True if the entity is included in the count for expansion.
  ///
  /// To prevent lengthy expansion attacks in malicious XML documents, the parser imposes a limit on the number of
  /// expansions. This function determines whether the target entity should be counted toward this expansion limit.
  /// Document entities and external subsets are excluded from the count because they are read only once, regardless
  /// of their size.
  ///
  #[must_use]
  pub const fn is_expansion(self) -> bool {
    !matches!(self, Self::Document | Self::ExternalSubset)
  }
}

/// One entity currently being read.
///
/// # Examples
///
/// ```
/// use xenolith_parser::{CharStream, Entity, EntityKind};
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
  /// `inherited_base` is the base URI of the entity in which this entity is *declared*. This is used only for *internal
  /// entities* that do not have their own resources. For external entities, this value is ignored, and their base URI
  /// is derived from the system identifier associated with the `stream`.
  ///
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

  /// The name of this entity. Or `None` for document entities and external subsets.
  ///
  #[must_use]
  pub fn name(&self) -> Option<&Arc<str>> {
    self.name.as_ref()
  }

  /// The type of this entity.
  #[must_use]
  pub const fn kind(&self) -> EntityKind {
    self.kind
  }

  /// The base URI used to resolve relative references within this entity.
  ///
  /// For an external entity, this is the entity's own URI; for an internal entity, it is the URI of the entity in which
  /// it is declared. Note that `xml:base` attribute is unrelated to this and is applied separately at
  /// a later stage as the base URI for content in the DOM (such as `<a href="../index.html">`, etc.).
  ///
  #[must_use]
  pub fn base_uri(&self) -> Option<&UriReference> {
    self.base_uri.as_ref()
  }

  /// The character stream.
  ///
  #[must_use]
  pub fn stream(&self) -> &CharStream {
    &self.stream
  }

  /// The mutable character stream.
  pub fn stream_mut(&mut self) -> &mut CharStream {
    &mut self.stream
  }
}

/// The maximum amount of work a document may trigger during parsing and entity resolution.
///
/// Each field is `Some(n)` for a limit of `n`, or `None` for no limit. The [default values](Limits::default) are set
/// generous enough for actual documents and tight enough that the classic expansion attacks fail. Each limit can be
/// relaxed or tightened, and specifying [`Limits::unlimited`] for trusted input removes these restrictions.
///
/// # Examples
///
/// ```
/// use xenolith_parser::Limits;
///
/// let limits = Limits::default().with_max_depth(8);
/// assert_eq!(limits.max_depth, Some(8));
/// assert_eq!(Limits::unlimited().max_expansions, None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
  /// Maximum number of entities, including the document entity, that can be opened simultaneously during reading.
  pub max_depth: Option<usize>,
  /// Maximum number of entity expansions in a single document.
  pub max_expansions: Option<u32>,
  /// Maximum total number of characters generated by all expansions.
  pub max_expansion_chars: Option<u64>,
  /// Maximum nesting depth of elements.
  ///
  /// As nesting depth increases, the parser's memory consumption increases; this impact is particularly noticeable
  /// during operations that recursively manipulate the generated tree structure.
  pub max_element_depth: Option<usize>,
}

impl Default for Limits {
  /// Protective defaults: generous for real documents, tight enough that the classic expansion attacks fail.
  fn default() -> Self {
    Self {
      max_depth: Some(64),
      max_expansions: Some(100_000),
      max_expansion_chars: Some(64 * 1024 * 1024),
      max_element_depth: Some(1024),
    }
  }
}

impl Limits {
  /// Limits with every restriction removed. This applies only to input that has been verified as trustworthy.
  ///
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_depth: None, max_expansions: None, max_expansion_chars: None, max_element_depth: None }
  }

  /// Returns a copy with [`max_depth`](Self::max_depth) set.
  #[must_use]
  pub const fn with_max_depth(mut self, max_depth: usize) -> Self {
    self.max_depth = Some(max_depth);
    self
  }

  /// Returns a copy with [`max_expansions`](Self::max_expansions) set.
  #[must_use]
  pub const fn with_max_expansions(mut self, max_expansions: u32) -> Self {
    self.max_expansions = Some(max_expansions);
    self
  }

  /// Returns a copy with [`max_expansion_chars`](Self::max_expansion_chars) set.
  #[must_use]
  pub const fn with_max_expansion_chars(mut self, max_expansion_chars: u64) -> Self {
    self.max_expansion_chars = Some(max_expansion_chars);
    self
  }

  /// Returns a copy with [`max_element_depth`](Self::max_element_depth) set.
  #[must_use]
  pub const fn with_max_element_depth(mut self, max_element_depth: usize) -> Self {
    self.max_element_depth = Some(max_element_depth);
    self
  }
}

/// The stack of entities currently being read, innermost last.
///
/// # Examples
///
/// ```
/// use xenolith_parser::{CharStream, Entity, EntityKind, EntityStack, Limits};
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
/// assert!(stack.current().stream().is_fully_read());
/// stack.pop();
/// assert_eq!(stack.depth(), 1);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
#[derive(Debug)]
pub struct EntityStack {
  entities: Vec<Entity>,
  limits: Limits,
  expansions: u32,
  expansion_chars: u64,
}

impl EntityStack {
  /// Creates a stack holding only the document entity.
  ///
  #[must_use]
  pub fn new(document: Entity, limits: Limits) -> Self {
    Self { entities: vec![document], limits, expansions: 0, expansion_chars: 0 }
  }

  /// Suspends reading from the current entity and begins reading from the `entity`.
  ///
  /// # Errors
  ///
  /// Returns [`Error::WellFormedness`] if the entity specified is already being read on this stack. This corresponds
  /// to the "No Recursion" Well-formedness constraint. Additionally, returns [`Error::Limit`] if the limits specified
  /// in [`Limits`] is exceeded.
  ///
  /// # Examples
  ///
  /// An entity that refers to itself is refused rather than expanded forever:
  ///
  /// ```
  /// use xenolith_core::Error;
  /// use xenolith_parser::{CharStream, Entity, EntityKind, EntityStack, Limits};
  ///
  /// let mut stack = EntityStack::new(Entity::document(CharStream::from_text("&e;")?), Limits::default());
  /// fn entity_e() -> Entity {
  ///   Entity::new(Some("e".into()), EntityKind::InternalGeneral, CharStream::from_text("&e;").unwrap(), None)
  /// }
  ///
  /// stack.push(entity_e())?;
  /// let err = stack.push(entity_e()).unwrap_err();
  /// assert!(matches!(err, Error::WellFormedness { .. }));
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  pub fn push(&mut self, entity: Entity) -> Result<()> {
    // Check that there are no circular references.
    if let Some(name) = entity.name() {
      if self.is_open(name) {
        let open: Vec<&str> = self.entities.iter().filter_map(|e| e.name().map(|n| &**n)).collect();
        let message =
          format!("entity \"{name}\" recursively references itself, through the path {}", open.join(" -> "));
        return Err(Error::well_formedness(message).at(self.location()));
      }
    }
    // Check whether the stack depth exceeds the maximum value.
    if let Some(max) = self.limits.max_depth {
      if self.entities.len() >= max {
        return Err(self.limit_exceeded(format!(
          "the maximum entity read depth {max} has been reached; increase Limits::max_depth if the document is correct"
        )));
      }
    }
    if entity.kind().is_expansion() {
      if let Some(max) = self.limits.max_expansions {
        if self.expansions >= max {
          return Err(self.limit_exceeded(format!(
            "the document expands more than {max} entities; increase Limits::max_expansions if it is correct"
          )));
        }
      }
      self.expansions += 1;
      // An internal entity arrives with its replacement text already decoded, so its already-decoded characters are
      // counted here; `feed` counts whatever is added later.
      self.count_expansion_chars(entity.stream().chars_decoded())?;
    }
    self.entities.push(entity);
    Ok(())
  }

  /// Terminates the innermost entity and resumes execution of the entity that was referencing it.
  ///
  /// Returns `None` if only the last document entity remains on the stack. This entity will never be popped.
  ///
  pub fn pop(&mut self) -> Option<Entity> {
    if self.entities.len() <= 1 {
      return None;
    }
    self.entities.pop()
  }

  /// Feeds bytes to the innermost entity.
  ///
  /// # Errors
  ///
  /// In addition to the error returned by [`CharStream::feed`], [`Error::Limit`] is also returned if the expansion
  /// limit is exceeded.
  ///
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    let counts = self.current().kind().is_expansion();
    let grew = self.current_mut().stream_mut().feed(bytes, last)? as u64;
    if counts {
      self.count_expansion_chars(grew)?;
    }
    Ok(())
  }

  /// Adds `chars` to the cumulative count of expanded characters, and returns [`Error::Limit`] if the total exceeds
  /// [`Limits::max_expansion_chars`].
  ///
  fn count_expansion_chars(&mut self, chars: u64) -> Result<()> {
    self.expansion_chars = self.expansion_chars.saturating_add(chars);
    if let Some(max) = self.limits.max_expansion_chars {
      if self.expansion_chars > max {
        return Err(self.limit_exceeded(format!(
          "entity expansion has produced more than {max} characters; \
           increase Limits::max_expansion_chars if the document is correct"
        )));
      }
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
    Error::limit(message).at(self.location())
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
    assert!(matches!(err, Error::WellFormedness { .. }));
    assert!(err.message().contains("\"a\""));
  }

  /// A programmer who hits a limit is told which one to raise; the number alone would leave
  /// them grepping. See the guidance in `xenolith_core::error`.
  #[test]
  fn limit_errors_name_the_limit_to_raise() {
    let mut stack =
      EntityStack::new(Entity::document(CharStream::from_text("x").unwrap()), Limits::default().with_max_depth(1));
    assert!(stack.push(text_entity("a", "x")).unwrap_err().message().contains("Limits::max_depth"));

    let mut stack =
      EntityStack::new(Entity::document(CharStream::from_text("x").unwrap()), Limits::default().with_max_expansions(0));
    assert!(stack.push(text_entity("a", "x")).unwrap_err().message().contains("Limits::max_expansions"));

    let mut stack = EntityStack::new(
      Entity::document(CharStream::from_text("x").unwrap()),
      Limits::default().with_max_expansion_chars(1),
    );
    assert!(stack.push(text_entity("a", "long")).unwrap_err().message().contains("Limits::max_expansion_chars"));
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
    let mut stack =
      EntityStack::new(Entity::document(CharStream::from_text("x").unwrap()), Limits::default().with_max_depth(3));
    stack.push(text_entity("a", "x")).unwrap();
    stack.push(text_entity("b", "x")).unwrap();
    let err = stack.push(text_entity("c", "x")).unwrap_err();
    assert!(matches!(err, Error::Limit { .. }));
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
    assert!(matches!(stack.push(text_entity("c", "x")).unwrap_err(), Error::Limit { .. }));
  }

  #[test]
  fn expanded_characters_are_bounded_when_the_text_is_already_decoded() {
    let mut stack = EntityStack::new(
      Entity::document(CharStream::from_text("x").unwrap()),
      Limits::default().with_max_expansion_chars(10),
    );
    stack.push(text_entity("a", "0123456789")).unwrap();
    stack.pop();
    assert!(matches!(stack.push(text_entity("b", "x")).unwrap_err(), Error::Limit { .. }));
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
    assert!(matches!(stack.feed(b"e", true).unwrap_err(), Error::Limit { .. }));
  }

  #[test]
  fn the_document_entity_is_not_counted_toward_the_expansion_limit() {
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
