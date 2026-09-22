//! Resolving external entities.
//!
//! External references within an XML document are resolved via an application-provided [`UriResolver`]. By
//! implementing a custom [`UriResolver`], an application can serve resources from an internal catalog for specific
//! PUBLIC IDs, fetch external resources, or refuse resolution entirely.
//!
//! Since the reader itself cannot perform I/O operations, it pauses processing when it encounters an external
//! reference and issues an [`EntityRequest`]. The driver executing the reader resolves this request via the
//! [`UriResolver`] and returns the byte data. Both the document parser and the DTD reader issue requests in this
//! manner.
//!
//! **External entity resolution is disabled by default.** A reader without a resolver will refuse to resolve external
//! entities. Resolving external entities from untrusted XML sources can lead to XXE (XML External Entity) attacks.
//! Applications should specify a resolver only for trusted input and restrict the scope of resources accessible to the
//! resolver.

use std::fmt;

/// The type of external resource requested by [`EntityRequest`].
///
/// The parser substitutes internal entities for which replacement text is specified within the declaration. External
/// entities, on the other hand, are defined within external resources. Consequently, the parser does not attempt to
/// load external entities itself; instead, it pauses parsing and returns the request shown below to the driver to
/// resolve the external reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
  /// An external general entity referenced from the document body.
  GeneralEntity,
  /// An external DTD subset referenced by the `DOCTYPE` declaration.
  ExternalSubset,
  /// An external parameter entity referenced while loading the DTD.
  ParameterEntity,
}

/// A Request for an external entity. The parser itself cannot read.
///
/// If the system identifier is a relative path, it is resolved relative to [`base_uri`](Self::base_uri). Resolve the
/// location relative to `base_uri` before resolving the reference. The resolver may ignore the system identifier's
/// URI and return a response from an internal catalog using the public identifier or entity name as the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRequest {
  name: Option<String>,
  public_id: Option<String>,
  system_id: String,
  base_uri: Option<String>,
  kind: RequestKind,
}

impl EntityRequest {
  /// Constructs a request for an external entity, to be issued when the reader cannot load the resource itself.
  ///
  /// The `system_id` is treated exactly as specified; when the request is processed, it is resolved relative to the
  /// `base_uri` if it is a relative path. The `name` represents the entity name, or `None` in the case of an external
  /// subset without a name.
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
  /// The name of that entity, for items that have a name.
  #[must_use]
  pub fn name(&self) -> Option<&str> {
    self.name.as_deref()
  }

  /// The public identifier, PUBLIC ID, if specified in the declaration.
  #[must_use]
  pub fn public_id(&self) -> Option<&str> {
    self.public_id.as_deref()
  }

  /// The system identifier, SYSTEM ID, described in the declaration and may be relative.
  #[must_use]
  pub fn system_id(&self) -> &str {
    &self.system_id
  }

  /// The base URI used as a reference when resolving system identifiers. It typically refers to the entity in which
  /// the declaration was made.
  #[must_use]
  pub fn base_uri(&self) -> Option<&str> {
    self.base_uri.as_deref()
  }

  /// The type of request applied to the entity.
  #[must_use]
  pub fn kind(&self) -> RequestKind {
    self.kind
  }

  /// The system identifier obtained by resolving the URI against the [base URI](Self::base_uri). If the base URI is an
  /// absolute URI (the common case), the result is an absolute URI; however, if the base URI is a relative URI or is
  /// absent, a relative URI may be returned. Returns `None` if the URI cannot be parsed.
  ///
  /// A resolver reading the URI's content should prioritize this value over [`system_id`](Self::system_id). However,
  /// it must also properly handle relative URI results (or combine them with its own base URI) to accommodate cases
  /// where the document lacks an absolute base URI.
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

/// Resolves an entity's input stream based on an [`EntityRequest`]. This resolver is intended for blocking
/// (synchronous) drivers.
///
/// A result of `Ok(None)` indicates that the request was rejected. In this case, the parser reports that the entity
/// could not be resolved; this constitutes a fatal error for well-formed documents.
///
/// Once the input stream is returned, the entity's content, including any encoding or text declarations, is passed
/// to the parser. The parser determines the encoding from the input stream and strips away the text declaration
/// itself. General entities are processed in chunks or are subject to input size limits, whereas DTD, specific types
/// (such as external subsets or parameter entities) are loaded entirely into memory.
///
/// # Examples
///
/// The following is an example of a resolver that uses an in-memory map as its backend. This approach is similar to
/// the format used for testing or static catalogs. For small entities, the simplest approach is to return the byte
/// sequence as a [`Cursor`](std::io::Cursor).
///
/// ```
/// use std::collections::HashMap;
/// use std::io::{Cursor, Read};
/// use xenolith::io::resolve::{EntityRequest, UriResolver};
///
/// struct Catalog(HashMap<String, Vec<u8>>);
///
/// impl UriResolver for Catalog {
///   fn resolve(&mut self, request: &EntityRequest) -> xenolith::Result<Option<Box<dyn Read>>> {
///     let entry = request.name().and_then(|name| self.0.get(name)).cloned();
///     Ok(entry.map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read>))
///   }
/// }
/// ```
pub trait UriResolver {
  /// Resolves the external reference resolution request `request` and returns the result as an entity input stream.
  /// Returns `None` if the request is rejected.
  ///
  /// # Errors
  ///
  /// Returns an error if an issue occurs while retrieving the resource. Wrap application-specific errors, such as
  /// database or network failures, in [`Error::resolver`](crate::Error::resolver). This preserves the error's origin
  /// information, allowing the caller to recover the original error via down-casting.
  fn resolve(&mut self, request: &EntityRequest) -> crate::error::Result<Option<Box<dyn std::io::Read + 'static>>>;
}
