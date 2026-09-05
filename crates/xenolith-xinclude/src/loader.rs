//! Fetching the resources XInclude refers to.

use xenolith_core::Error;

/// Fetches the bytes of a resource given by an absolute URI.
///
/// XInclude does no I/O of its own — it hands each resolved `href` to a loader. That keeps the
/// fetch, and its trust decisions, in the caller's hands: serve only from a directory, only from
/// a catalogue, or refuse. A loader that cannot supply a resource returns an error; the
/// processor then uses the `xi:include`'s fallback, or fails if it has none.
pub trait Loader {
  /// Loads the resource at `uri`, which has already been resolved to an absolute URI.
  ///
  /// # Errors
  ///
  /// Returns an error if the resource cannot be provided; this is treated as a recoverable
  /// resource error, so a fallback may still be used.
  fn load(&mut self, uri: &str) -> Result<Vec<u8>, Error>;
}
