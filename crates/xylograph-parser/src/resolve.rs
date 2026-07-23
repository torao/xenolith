//! Resolving external entities.
//!
//! The parser reads no external resource itself. When it meets a reference to one it stops and
//! reports [`Progress::NeedEntity`](crate::Progress::NeedEntity), handing out an
//! [`EntityRequest`]; a driver resolves that request through a [`UriResolver`] and feeds the
//! bytes back. This keeps I/O — and its attendant risks — out of the core, and lets the same
//! parser be driven by a blocking reader, an async reader, or a caller with its own catalogue.
//!
//! **External entities are off by default.** A [`Reader`](crate::Reader) with no resolver
//! refuses them, which is what a document from an untrusted source needs: resolving them is
//! the XML external-entity (XXE) attack surface. Supply a resolver only for input you trust,
//! and scope it to the resources it should reach.

use std::fmt;

/// What kind of external resource an [`EntityRequest`] is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestKind {
  /// A general entity referenced in content.
  GeneralEntity,
}

/// A request for an external entity the parser cannot read itself.
///
/// The system identifier is relative to [`base_uri`](Self::base_uri); resolve it there before
/// dereferencing. A resolver is free to ignore the URIs entirely and answer from a catalogue
/// keyed on the public identifier or the entity name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRequest {
  name: Option<String>,
  public_id: Option<String>,
  system_id: String,
  base_uri: Option<String>,
  kind: RequestKind,
}

impl EntityRequest {
  pub(crate) fn new(
    name: Option<String>,
    public_id: Option<String>,
    system_id: String,
    base_uri: Option<String>,
    kind: RequestKind,
  ) -> Self {
    Self { name, public_id, system_id, base_uri, kind }
  }

  /// The entity's name, for a named entity.
  #[must_use]
  pub fn name(&self) -> Option<&str> {
    self.name.as_deref()
  }

  /// The public identifier, if the declaration gave one.
  #[must_use]
  pub fn public_id(&self) -> Option<&str> {
    self.public_id.as_deref()
  }

  /// The system identifier, as written in the declaration and so possibly relative.
  #[must_use]
  pub fn system_id(&self) -> &str {
    &self.system_id
  }

  /// The base URI the system identifier is relative to: the entity where the declaration was.
  #[must_use]
  pub fn base_uri(&self) -> Option<&str> {
    self.base_uri.as_deref()
  }

  /// What the entity is used for.
  #[must_use]
  pub fn kind(&self) -> RequestKind {
    self.kind
  }

  /// The system identifier resolved against the base URI, when both are known and combine to
  /// an absolute URI. A resolver that dereferences URIs should prefer this to
  /// [`system_id`](Self::system_id).
  #[must_use]
  pub fn resolved_uri(&self) -> Option<String> {
    match &self.base_uri {
      Some(base) => xylograph_core::uri::resolve(base, &self.system_id).ok(),
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

/// Resolves an [`EntityRequest`] to the bytes of the entity, for a blocking driver.
///
/// Returning `Ok(None)` declines the request: the parser then reports the entity as
/// unresolvable, which for a well-formed document is a fatal error. Returning the bytes hands
/// the parser the entity's content, encoding and text declaration included; the parser sniffs
/// the encoding and strips the text declaration itself.
///
/// # Examples
///
/// A resolver backed by an in-memory map, the shape a test or a fixed catalogue would take:
///
/// ```
/// use std::collections::HashMap;
/// use xylograph_parser::resolve::{EntityRequest, UriResolver};
///
/// struct Catalogue(HashMap<String, Vec<u8>>);
///
/// impl UriResolver for Catalogue {
///   fn resolve(&mut self, request: &EntityRequest) -> xylograph_core::Result<Option<Vec<u8>>> {
///     Ok(request.name().and_then(|name| self.0.get(name)).cloned())
///   }
/// }
/// ```
pub trait UriResolver {
  /// Resolves `request` to the entity's bytes, or `None` to decline it.
  ///
  /// # Errors
  ///
  /// An I/O failure while fetching the resource; the parser passes it through.
  fn resolve(&mut self, request: &EntityRequest) -> xylograph_core::error::Result<Option<Vec<u8>>>;
}

/// Resolves an [`EntityRequest`] for an asynchronous driver.
///
/// The blocking [`UriResolver`] and this one are deliberately separate: an async driver should
/// not have to block a runtime thread to fetch an entity, and a blocking one should not have
/// to host a runtime.
#[cfg(feature = "tokio")]
pub trait AsyncUriResolver {
  /// Resolves `request` to the entity's bytes, or `None` to decline it.
  ///
  /// # Errors
  ///
  /// An I/O failure while fetching the resource; the parser passes it through.
  fn resolve(
    &mut self,
    request: &EntityRequest,
  ) -> impl std::future::Future<Output = xylograph_core::error::Result<Option<Vec<u8>>>> + Send;
}
