//! Fetching the stylesheet modules an `xsl:import` or `xsl:include` names.

use xylograph_core::error::{Error, ErrorKind, Result};

/// Fetches the bytes of a stylesheet module named by an absolute URI.
///
/// A stylesheet may be built from several documents, and reading them is I/O — the same trust
/// decision as fetching an external entity. So it is not built in: the caller supplies a loader,
/// and decides whether to serve from a directory, from a catalogue, or not at all.
pub trait Loader {
  /// Loads the module at `uri`, which has already been resolved to an absolute URI.
  ///
  /// # Errors
  ///
  /// Returns an error if the module cannot be provided. A stylesheet that names a module it
  /// cannot have is not a stylesheet, so this is fatal — unlike XInclude, XSLT has no fallback.
  fn load(&mut self, uri: &str) -> Result<Vec<u8>>;
}

/// A loader that serves nothing, for a stylesheet held in one document.
///
/// [`Stylesheet::compile`](crate::Stylesheet::compile) uses this, so a stylesheet that turns out
/// to name another module is refused with a message saying which entry point can load it.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLoader;

impl Loader for NoLoader {
  fn load(&mut self, uri: &str) -> Result<Vec<u8>> {
    let message = format!(
      "this stylesheet names the module {uri:?}, but no loader was given; \
       use Stylesheet::compile_with to supply one"
    );
    Err(Error::new(ErrorKind::Xslt, message))
  }
}
