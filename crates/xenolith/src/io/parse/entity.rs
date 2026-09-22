//! The entities being read and the stack that holds them.
//!
//! A document is read as a nested structure of entities. Specifically, it consists of the document entity and the set
//! of entities incorporated via references. At any point during the reading process, the entities currently being read
//! form a path extending from the document entity to the innermost entity; this module maintains that path as a stack.
//! The innermost entity determines the current position and base URI, while the entire stack is used to track limits
//! on entity expansion.
//!
//! # The physical structure of a document
//!
//! Documents have two types of structures (XML 1.0 §4). The logical structure is the nested structure of elements,
//! corresponding to the tree structure most people envision or the structure represented by the DOM. The physical
//! structure, on the other hand, is the nested structure of entities; this module tracks the latter. An entity is a
//! unit of storage, such as the document itself, an external resource, or replacement text defined within a DTD.
//! Because a reference like `&e;` embeds the content of one entity into another, a single element may span multiple
//! entities. The reading process enters the referenced entity, much like a function call, and returns to the
//! referencing entity once reading is complete.
//!
//! For example, suppose a book is split into separate files by chapter, resulting in three entities: `book.xml`
//! (the document entity), `chapter1.xml` (an external entity), and `chapter2.xml` (an external entity).
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
//! The logical structure is a single tree, with a `book` element at the root and `chapter` elements corresponding to
//! each chapter located beneath it. In contrast, the physical structure consists of three entities. While the tags for
//! the `book` element are defined in `book.xml`, its child `chapter` elements reside in two other files and are
//! included via the references `&chapter1;` and `&chapter2;`. In other words, a single `book` element spans three
//! entities, and the tree is formed by combining all three of them.

#[cfg(test)]
mod test;

use std::sync::Arc;

use crate::error::{Error, Location, Result};
use crate::uri::UriReference;

use crate::io::parse::config::EntityLimits;
use crate::io::stream::CharStream;

/// The types of entities being read (XML 1.0 §4).
///
/// The stack processes the document entity and the general entities (`&name;`) referenced within its content. General
/// entities include "internal entities," where the replacement text is defined within the declaration, and "external
/// entities," where the text is sourced from a separate resource specified in the declaration. DTD text (including
/// parameter entities) is not read via the stack but is instead assembled by [`DtdAssembly`](crate::dtd::DtdAssembly).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKind {
  /// The document entity: The entity that serves as the starting point for reading. It has no name, is located at the
  /// bottom of the stack, and is never popped.
  Document,
  /// An internal general entity (e.g., `<!ENTITY name "replacement text">`). Referenced as `&name;`.
  InternalGeneral,
  /// An external general entity (e.g., `<!ENTITY name SYSTEM "chap1.xml">`). Referenced as `&name;`.
  ExternalGeneral,
}

impl EntityKind {
  /// True if the entity is a resource of its own, and so has its own base URI: the document entity and an external
  /// entity.
  #[must_use]
  pub const fn is_external(self) -> bool {
    matches!(self, Self::Document | Self::ExternalGeneral)
  }

  /// True if reading the entity counts as an expansion toward [`EntityLimits`]: every kind but the document entity,
  /// which is read once whatever the document references.
  #[must_use]
  pub const fn is_expansion(self) -> bool {
    !matches!(self, Self::Document)
  }
}

/// A single entity being read: its type, name (if any), character stream, and base URI.
///
/// # Examples
///
/// ```
/// use xenolith::io::{CharStream, Entity, EntityKind};
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
  /// The document entity, read from `stream`. Its base URI is the stream's system identifier.
  #[must_use]
  pub fn document(stream: CharStream) -> Self {
    Self::new(None, EntityKind::Document, stream, None)
  }

  /// An entity of `kind` named `name`, read from `stream`.
  ///
  /// An external entity's base URI is its stream's system identifier, and `inherited_base` is ignored. An internal
  /// entity has no resource of its own, so its base URI is `inherited_base`: the base URI in effect where it is
  /// referenced, since its content becomes part of that entity's content.
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

  /// The entity's name, or `None` for the document entity.
  #[must_use]
  pub fn name(&self) -> Option<&Arc<str>> {
    self.name.as_ref()
  }

  /// The entity's kind.
  #[must_use]
  pub const fn kind(&self) -> EntityKind {
    self.kind
  }

  /// The entity's base URI: its own system identifier for an external entity, and the base URI in effect where it was
  /// referenced for an internal entity. `None` if neither is known.
  ///
  /// This is the base URI of the entity, not of an element. The parser starts from it and applies each element's
  /// `xml:base` on top (XML Base) to give [`Parser::base_uri`](crate::io::Parser::base_uri).
  #[must_use]
  pub fn base_uri(&self) -> Option<&UriReference> {
    self.base_uri.as_ref()
  }

  /// The entity's character stream.
  #[must_use]
  pub fn stream(&self) -> &CharStream {
    &self.stream
  }

  /// The entity's character stream, mutably, to feed it or consume from it.
  pub fn stream_mut(&mut self) -> &mut CharStream {
    &mut self.stream
  }
}

/// The entities being read are managed such that the innermost entity is the last one, with the document entity always
/// placed at the bottom.
///
/// [`EntityLimits`] are applied when entities are pushed (added) and supplied, limiting the number of entities that
/// can be open simultaneously, the number of expansions triggered by the document, and the total number of characters
/// generated by them. Additionally, the system refuses to load entities that are already open, preventing infinite
/// expansion loops.
///
/// # Examples
///
/// ```
/// use xenolith::io::{CharStream, Entity, EntityKind, EntityLimits, EntityStack};
///
/// let mut stack = EntityStack::new(Entity::document(CharStream::from_text("<a>&e;</a>")?), EntityLimits::default());
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
/// # Ok::<(), xenolith::Error>(())
/// ```
#[derive(Debug)]
pub struct EntityStack {
  entities: Vec<Entity>,
  limits: EntityLimits,
  expansions: u32,
  expansion_chars: u64,
}

impl EntityStack {
  /// A stack holding only `document`, enforcing `limits`.
  #[must_use]
  pub fn new(document: Entity, limits: EntityLimits) -> Self {
    Self { entities: vec![document], limits, expansions: 0, expansion_chars: 0 }
  }

  /// Suspends the current entity and makes `entity` the innermost, to be read until it is popped.
  ///
  /// # Errors
  ///
  /// [`Error::WellFormedness`] if an entity of the same name is already open, since it would include itself (WFC: No
  /// Recursion). [`Error::Limit`] if pushing it would exceed [`EntityLimits::max_depth`] or
  /// [`EntityLimits::max_expansions`], or its text already exceeds [`EntityLimits::max_expansion_chars`].
  ///
  /// # Examples
  ///
  /// An entity that refers to itself is refused rather than expanded forever:
  ///
  /// ```
  /// use xenolith::Error;
  /// use xenolith::io::{CharStream, Entity, EntityKind, EntityLimits, EntityStack};
  ///
  /// let mut stack = EntityStack::new(Entity::document(CharStream::from_text("&e;")?), EntityLimits::default());
  /// fn entity_e() -> Entity {
  ///   Entity::new(Some("e".into()), EntityKind::InternalGeneral, CharStream::from_text("&e;").unwrap(), None)
  /// }
  ///
  /// stack.push(entity_e())?;
  /// let err = stack.push(entity_e()).unwrap_err();
  /// assert!(matches!(err, Error::WellFormedness { .. }));
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  pub fn push(&mut self, entity: Entity) -> Result<()> {
    // WFC: No Recursion. An entity that is already open would include itself.
    if let Some(name) = entity.name() {
      if self.is_open(name) {
        let open: Vec<&str> = self.entities.iter().filter_map(|e| e.name().map(|n| &**n)).collect();
        let message =
          format!("entity \"{name}\" recursively references itself, through the path {}", open.join(" -> "));
        return Err(Error::well_formedness(message).at(self.location()));
      }
    }
    // The document entity counts toward the depth.
    if let Some(max) = self.limits.max_depth {
      if self.entities.len() >= max {
        return Err(self.limit_exceeded(format!(
          "the maximum entity read depth {max} has been reached; \
           increase ParserConfig.limits.entities.max_depth if the document is correct"
        )));
      }
    }
    if entity.kind().is_expansion() {
      if let Some(max) = self.limits.max_expansions {
        if self.expansions >= max {
          return Err(self.limit_exceeded(format!(
            "the document expands more than {max} entities; \
             increase ParserConfig.limits.entities.max_expansions if it is correct"
          )));
        }
      }
      self.expansions += 1;
      // Text already decoded when the entity arrives is counted here: all of an internal entity's, and all of an
      // external entity's that was handed over whole. `feed` counts what is decoded later.
      self.count_expansion_chars(entity.stream().chars_decoded())?;
    }
    self.entities.push(entity);
    Ok(())
  }

  /// Removes the innermost entity, once it is read, so that reading resumes in the entity that referenced it.
  ///
  /// Returns `None`, and removes nothing, when only the document entity is left: it is never popped.
  pub fn pop(&mut self) -> Option<Entity> {
    if self.entities.len() <= 1 {
      return None;
    }
    self.entities.pop()
  }

  /// Feeds `bytes` to the innermost entity's stream, as [`CharStream::feed`] does, and counts the characters decoded
  /// toward [`EntityLimits::max_expansion_chars`] if the entity is an expansion.
  ///
  /// # Errors
  ///
  /// Whatever [`CharStream::feed`] returns, and [`Error::Limit`] if the characters expanded exceed
  /// [`EntityLimits::max_expansion_chars`].
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    let counts = self.current().kind().is_expansion();
    let grew = self.current_mut().stream_mut().feed(bytes, last)? as u64;
    if counts {
      self.count_expansion_chars(grew)?;
    }
    Ok(())
  }

  /// Adds `chars` to the characters expanded so far, and fails with [`Error::Limit`] if the total exceeds
  /// [`EntityLimits::max_expansion_chars`].
  fn count_expansion_chars(&mut self, chars: u64) -> Result<()> {
    self.expansion_chars = self.expansion_chars.saturating_add(chars);
    if let Some(max) = self.limits.max_expansion_chars {
      if self.expansion_chars > max {
        return Err(self.limit_exceeded(format!(
          "entity expansion has produced more than {max} characters; \
           increase ParserConfig.limits.entities.max_expansion_chars if the document is correct"
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

  /// The innermost entity, mutably. Feeding it through here bypasses the expansion count; use
  /// [`feed`](Self::feed) for that.
  pub fn current_mut(&mut self) -> &mut Entity {
    self.entities.last_mut().expect("the document entity is never popped")
  }

  /// The document entity, at the bottom of the stack.
  #[must_use]
  pub fn document(&self) -> &Entity {
    self.entities.first().expect("the document entity is never popped")
  }

  /// The number of entities open, the document entity included.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.entities.len()
  }

  /// True if an entity named `name` is open.
  #[must_use]
  pub fn is_open(&self, name: &str) -> bool {
    self.entities.iter().any(|e| e.name().is_some_and(|n| &**n == name))
  }

  /// The reading position in the innermost entity.
  #[must_use]
  pub fn location(&self) -> Location {
    self.current().stream().location()
  }

  /// The base URI of the innermost entity that has one: the base an entity referenced here inherits.
  #[must_use]
  pub fn base_uri(&self) -> Option<&UriReference> {
    self.entities.iter().rev().find_map(Entity::base_uri)
  }

  /// Replaces the limits this stack enforces. The parser calls it when its settings change.
  pub(crate) fn set_limits(&mut self, limits: EntityLimits) {
    self.limits = limits;
  }

  /// The limits this stack enforces.
  #[must_use]
  pub const fn limits(&self) -> &EntityLimits {
    &self.limits
  }

  /// A limit error with `message`, located where reading has reached.
  fn limit_exceeded(&self, message: String) -> Error {
    Error::limit(message).at(self.location())
  }
}
