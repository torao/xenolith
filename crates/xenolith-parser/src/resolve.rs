//! Resolving external entities.
//!
//! Provides the parser with a means to access external resources. External references in an XML document are resolved
//! through the [`UriResolver`] specified by the application. The application can implement [`UriResolver`] to provide
//! resources for specific PUBLIC IDs from an internal catalog, or to retrieve external resources or refuse to resolve
//! them.
//!
//! When the parser encounters a reference to an external resource, it pauses processing and reports
//! [`Progress::NeedEntity`](crate::Progress::NeedEntity)., and creates an [`EntityRequest`]. The driver, which runs
//! the parser’s [`advance()`](crate::Parser::advance) loop and handles the I/O on its behalf, resolves the entity via
//! [`UriResolver`] and returns the byte data to the parser.
//!
//! **External entities are disabled by default**. A [`Reader`](crate::Reader) that does not have a resolver will refuse
//! to resolve external entities. Resolving external entities from untrusted XML sources constitutes an XXE (XML
//! External Entity) attack. It is recommended that applications specify resolvers only for trusted input and limit the
//! scope of resources that the resolver can access.
//!

use std::fmt;

/// The type of external resource that [`EntityRequest`] is seeking.
///
/// Internal entities, whose replacement text is specified within the declaration, are replaced by the parser. External
/// entities, on the other hand, are defined in external resources. For this reason, the parser does not attempt to read
/// external entities on its own; instead, it suspends parsing and returns a request to the driver to resolve the
/// external reference. Consequently, this behavior corresponds to three values within
/// [`EntityKind`](crate::entity::EntityKind) that represent external resources.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestKind {
  /// An external general entity [`EntityKind::ExternalGeneral`](crate::entity::EntityKind::ExternalGeneral) referenced
  /// from the document body.
  GeneralEntity,
  /// The external DTD subset [`EntityKind::ExternalSubset`](crate::entity::EntityKind::ExternalSubset) referenced by
  /// the `DOCTYPE`.
  ExternalSubset,
  /// An external parameter entity [`EntityKind::ExternalParameter`](crate::entity::EntityKind::ExternalParameter)
  /// referenced during DTD parsing.
  ParameterEntity,
}

/// A request for an external entity that the parser itself cannot read.
///
/// When the system identifier is a relative path, it is based on [`base_uri`](Self::base_uri). Resolve the location
/// relative to `base_uri` before resolving the reference. The resolver may completely ignore the URI of the system
/// identifier and return a response from an internal catalog using the public identifier or entity name as the key.
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
  pub(crate) fn new(
    name: Option<String>,
    public_id: Option<String>,
    system_id: String,
    base_uri: Option<String>,
    kind: RequestKind,
  ) -> Self {
    Self { name, public_id, system_id, base_uri, kind }
  }

  /// The name of this entity, for a named one.
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

  /// The base URI relative to which the system identifier is resolved. Typically, the entity in which the declaration
  /// was made.
  #[must_use]
  pub fn base_uri(&self) -> Option<&str> {
    self.base_uri.as_deref()
  }

  /// Which type of entity does this apply to.
  #[must_use]
  pub fn kind(&self) -> RequestKind {
    self.kind
  }

  /// The system identifier obtained by resolving the URI relative to the base URI. When the base URI is absolute (the
  /// typical case), the result is an absolute URI; however, when the base URI is relative or absent, a relative URI
  /// may be returned. Returns `None` if the URI cannot be parsed.
  ///
  /// The resolver that actually reads the contents of the URI should give priority to this over
  /// [`system_id`](Self::system_id); however, to account for cases where a document lacks an absolute base URI, it
  /// should also be able to handle relative results (or combine them with its own base).
  ///
  #[must_use]
  pub fn resolved_uri(&self) -> Option<String> {
    match &self.base_uri {
      Some(base) => xenolith_core::uri::resolve(base, &self.system_id).ok(),
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
/// types (such as external subsets and parameterized entities) are read into memory in their entirety.
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
/// use xenolith_parser::resolve::{EntityRequest, UriResolver};
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
  /// An error occurred while acquiring a resource. Please wrap application-specific errors (such as database or network
  /// failures) in [`Error::resolver`](xenolith_core::Error::resolver). This ensures that the error’s source is
  /// preserved, allowing the caller to restore the error by down-casting.
  ///
  fn resolve(
    &mut self,
    request: &EntityRequest,
  ) -> xenolith_core::error::Result<Option<Box<dyn std::io::Read + 'static>>>;
}

/// A reader that reads a byte sequence from an external entity for asynchronous drivers.
///
/// This wraps the entity’s asynchronous input stream so that resolver implementation do not have to depend on a
/// specific asynchronous runtime within the [`AsyncUriResolver`] signature. The interface of this class consists
/// solely of the runtime-independent [`futures_io::AsyncRead`]. It allows to generate this from any of those input
/// streams using [`from_async_read`](Self::from_async_read), which is directly implemented by the `async-std`, `smol`,
/// and `futures` crates. Under the `tokio` feature, [`from_tokio`](Self::from_tokio) adapts readers that implement
/// tokio’s own `AsyncRead` instead.
///
#[cfg(feature = "async")]
pub struct AsyncEntityReader {
  inner: Box<dyn futures_io::AsyncRead + Send + Unpin + 'static>,
}

#[cfg(feature = "async")]
impl AsyncEntityReader {
  /// Wraps any runtime-independent [`futures_io::AsyncRead`].
  ///
  #[must_use]
  pub fn from_async_read(reader: impl futures_io::AsyncRead + Send + Unpin + 'static) -> Self {
    Self { inner: Box::new(reader) }
  }

  /// Reads the next chunk into `buf` and returns the number of bytes read (return `0` at the end of the input). This
  /// is used when the driver reads entities chunk by chunk.
  ///
  pub(crate) async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    // `poll_read` resolves through the `dyn futures_io::AsyncRead` object type; no import needed.
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *self.inner).poll_read(cx, buf)).await
  }

  /// Reads the entity to the end and stores it in `out`. This is used by the driver corresponding to the type specified
  /// in the DTD.
  ///
  pub(crate) async fn read_to_end(&mut self, out: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut chunk = [0u8; 8 * 1024];
    let mut total = 0;
    loop {
      let read = self.read(&mut chunk).await?;
      if read == 0 {
        return Ok(total);
      }
      out.extend_from_slice(&chunk[..read]);
      total += read;
    }
  }
}

#[cfg(feature = "tokio")]
impl AsyncEntityReader {
  /// Wraps an input stream that implements the tokio-specific [`AsyncRead`](tokio::io::AsyncRead) and bridges it to
  /// [`futures_io::AsyncRead`]. Available under the `tokio` feature.
  ///
  #[must_use]
  pub fn from_tokio(reader: impl tokio::io::AsyncRead + Send + Unpin + 'static) -> Self {
    use tokio_util::compat::TokioAsyncReadCompatExt;
    Self::from_async_read(reader.compat())
  }
}

#[cfg(feature = "async")]
impl std::fmt::Debug for AsyncEntityReader {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AsyncEntityReader").finish_non_exhaustive()
  }
}

/// Resolves an [`EntityRequest`] for an asynchronous driver. This corresponds to [`UriResolver`].
///
/// This trait and the blocking [`UriResolver`] serve the same purpose but are intentionally separated. This is so that
/// asynchronous drivers do not need to blocking the runtime thread to retrieve entities, and blocking drivers do not
/// need to host the runtime. See [`UriResolver`] for the resolution specification. Here, explains only the difference
/// in the asynchronous implementation.
///
/// This returns a non-blocking input stream [`AsyncEntityReader`]. You can construct this from a runtime-specific
/// asynchronous reader using [`AsyncEntityReader::from_async_read`] (or [`from_tokio`](AsyncEntityReader::from_tokio)
/// under the `tokio` feature). Neither resolving the request nor reading the entity blocks the runtime thread because
/// the driver uses `.await` to process that input stream,
///
#[cfg(feature = "async")]
pub trait AsyncUriResolver {
  /// Resolves `request` to a reader over the entity's bytes, or `None` to decline it.
  ///
  /// # Errors
  ///
  /// A failure while fetching the resource; the parser passes it through. Wrap an application-specific error (a database
  /// or network failure, say) with [`Error::resolver`](xenolith_core::Error::resolver), which keeps it as the error's
  /// source so a caller can recover it by downcasting.
  fn resolve(
    &mut self,
    request: &EntityRequest,
  ) -> impl std::future::Future<Output = xenolith_core::error::Result<Option<AsyncEntityReader>>> + Send;
}
