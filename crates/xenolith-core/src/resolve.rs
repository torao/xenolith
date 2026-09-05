//! Resolving external entities.
//!
//! This module lets the parser access external resources. External references in an XML document are resolved through
//! the [`UriResolver`] specified by the application. The application can implement [`UriResolver`] to provide
//! resources for specific PUBLIC IDs from an internal catalog, retrieve external resources, or refuse to resolve them.
//!
//! A reader cannot itself perform I/O, so it pauses when it meets such a reference and raises an [`EntityRequest`].
//! The driver running it resolves the request through a [`UriResolver`] and returns the bytes. Both the document
//! parser and the DTD reader ask this way, which is why the request lives here rather than with either of them.
//!
//! **External entities are disabled by default**. A reader with no resolver refuses to resolve external entities.
//! Resolving external entities from untrusted XML sources constitutes an XXE (XML External Entity) attack.
//! Applications should specify resolvers only for trusted input and limit the scope of resources the resolver can
//! access.
//!

use std::fmt;

/// The type of external resource that [`EntityRequest`] is seeking.
///
/// The parser replaces internal entities whose replacement text is specified in the declaration. External entities,
/// by contrast, are defined in external resources. For this reason, the parser does not attempt to read external
/// entities on its own; instead, it suspends parsing and returns a request to the driver to resolve the external
/// reference. Three kinds of references are external, and each arrives here.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
  /// An external general entity referenced from the document body.
  GeneralEntity,
  /// The external DTD subset the `DOCTYPE` declaration refers to.
  ExternalSubset,
  /// An external parameter entity referenced while reading a DTD.
  ParameterEntity,
}

/// A request for an external entity that the parser itself cannot read.
///
/// When the system identifier is a relative path, it is based on [`base_uri`](Self::base_uri). Resolve the location
/// relative to `base_uri` before resolving the reference. The resolver may ignore the system identifier URI and return
/// a response from an internal catalog using the public identifier or entity name as the key.
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRequest {
  name: Option<String>,
  public_id: Option<String>,
  system_id: String,
  base_uri: Option<String>,
  kind: RequestKind,
}

impl EntityRequest {
  /// Builds a request for an external entity that a reader raises when it cannot read the resource itself.
  ///
  /// `system_id` is taken as it was written, so a relative one is resolved against `base_uri` when the request is
  /// answered. `name` is the entity's name, and `None` for the external subset, which has none.
  ///
  #[must_use]
  pub fn new(
    name: Option<String>,
    public_id: Option<String>,
    system_id: String,
    base_uri: Option<String>,
    kind: RequestKind,
  ) -> Self {
    Self { name, public_id, system_id, base_uri, kind }
  }

  /// The name of this entity, for a named one.
  ///
  #[must_use]
  pub fn name(&self) -> Option<&str> {
    self.name.as_deref()
  }

  /// The public identifier, PUBLIC ID, if specified in the declaration.
  ///
  #[must_use]
  pub fn public_id(&self) -> Option<&str> {
    self.public_id.as_deref()
  }

  /// The system identifier, SYSTEM ID, described in the declaration and may be relative.
  ///
  #[must_use]
  pub fn system_id(&self) -> &str {
    &self.system_id
  }

  /// The base URI relative to which the system identifier is resolved. Typically, the entity in which the declaration
  /// was made.
  ///
  #[must_use]
  pub fn base_uri(&self) -> Option<&str> {
    self.base_uri.as_deref()
  }

  /// Which type of entity does this apply to.
  ///
  #[must_use]
  pub fn kind(&self) -> RequestKind {
    self.kind
  }

  /// The system identifier obtained by resolving the URI relative to the base URI. When the base URI is absolute (the
  /// typical case), the result is an absolute URI; when the base URI is relative or absent, it may return a relative
  /// URI. Returns `None` if the URI cannot be parsed.
  ///
  /// The resolver that reads the URI contents should prioritize this over [`system_id`](Self::system_id); however, to
  /// handle cases where a document lacks an absolute base URI, it should also handle relative results (or combine them
  /// with its own base).
  ///
  #[must_use]
  pub fn resolved_uri(&self) -> Option<String> {
    match &self.base_uri {
      Some(base) => crate::uri::resolve(base, &self.system_id).ok(),
      None => Some(self.system_id.clone()),
    }
  }
}

impl fmt::Display for EntityRequest {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match &self.name {
      Some(name) => write!(f, "entity \"{name}\" (system id {:?})", self.system_id),
      None => write!(f, "external resource (system id {:?})", self.system_id),
    }
  }
}

/// Resolves the input stream from an entity for [`EntityRequest`]. This resolver is for blocking drivers.
///
/// A result of `Ok(None)` indicates that the request was rejected. In this case, the parser reports that the entity
/// cannot be resolved. This constitutes a fatal error in a well-formed document.
///
/// When an input stream is returned, the contents of the entity, including the encoding and text declaration, are
/// passed to the parser. The parser determines the encoding from the input stream and removes the text declaration
/// itself. General entities are processed in chunks or are subject to input size limitations. However, DTD-specific
/// types (such as external subsets and parameterized entities) are read entirely into memory.
///
/// # Examples
///
/// A resolver that uses an in-memory map as its backend. This is similar to the format used for tests and fixed
/// catalogs. For small entities, the simplest approach is to return them as a [`Cursor`](std::io::Cursor) for their
/// bytes:
///
/// ```
/// use std::collections::HashMap;
/// use std::io::{Cursor, Read};
/// use xenolith_core::resolve::{EntityRequest, UriResolver};
///
/// struct Catalog(HashMap<String, Vec<u8>>);
///
/// impl UriResolver for Catalog {
///   fn resolve(&mut self, request: &EntityRequest) -> xenolith_core::Result<Option<Box<dyn Read>>> {
///     let entry = request.name().and_then(|name| self.0.get(name)).cloned();
///     Ok(entry.map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read>))
///   }
/// }
/// ```
pub trait UriResolver {
  /// Resolves the external reference resolution request `request` as the entity's input stream. Otherwise, returns
  /// `None` if the request is rejected.
  ///
  /// # Errors
  ///
  /// An error occurred while acquiring a resource. Please wrap application-specific errors (such as database or
  /// network failures) in [`Error::resolver`](crate::Error::resolver). This preserves the error's source, allowing
  /// the caller to restore it by down-casting.
  ///
  fn resolve(&mut self, request: &EntityRequest) -> crate::error::Result<Option<Box<dyn std::io::Read + 'static>>>;
}
