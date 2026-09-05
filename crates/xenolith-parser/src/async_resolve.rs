//! Resolving external entities for an asynchronous driver.
//!
//! The counterparts of [`UriResolver`](xenolith_core::resolve::UriResolver) for a driver that reads with `async fn`.
//! They live here rather than beside the synchronous ones because they rest on `futures_io`, which only the
//! asynchronous reader pulls in.
//!

use std::fmt;

use xenolith_core::resolve::EntityRequest;

/// A reader that reads a byte sequence from an external entity for asynchronous drivers.
///
/// This wraps the entity's asynchronous input stream, so resolver implementations don't depend on a specific
/// asynchronous runtime in the [`AsyncUriResolver`] signature. The interface of this class consists solely of the
/// runtime-independent [`futures_io::AsyncRead`]. It lets you create this from any of those input streams using
/// [`from_async_read`](Self::from_async_read), which `async-std`, `smol`, and `futures` implement directly. Under the
/// `tokio` feature, [`from_tokio`](Self::from_tokio) adapts readers that implement tokio's own `AsyncRead` instead.
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

  /// Reads the next chunk into `buf` and returns the number of bytes read (returns `0` at the end of the input). The
  /// driver uses this when it reads entities chunk by chunk.
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

/// Resolves an [`EntityRequest`] for an asynchronous driver. This corresponds to
/// [`UriResolver`](xenolith_core::resolve::UriResolver).
///
/// This trait and the blocking [`UriResolver`](xenolith_core::resolve::UriResolver) serve the same purpose but are
/// intentionally separated. This lets asynchronous drivers avoid blocking the runtime thread to retrieve entities,
/// and blocking drivers avoid hosting the runtime. See [`UriResolver`](xenolith_core::resolve::UriResolver) for the
/// resolution specification. This explains only the difference in the asynchronous implementation.
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
  /// A failure while fetching the resource; the parser passes it through. Wrap an application-specific error (a
  /// database or network failure, say) with [`Error::resolver`](xenolith_core::Error::resolver), which keeps it as the
  /// error's source so a caller can recover it by downcasting.
  fn resolve(
    &mut self,
    request: &EntityRequest,
  ) -> impl std::future::Future<Output = xenolith_core::error::Result<Option<AsyncEntityReader>>> + Send;
}
