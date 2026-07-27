//! Fetching the documents a stylesheet names — its own modules, and the trees `document()` asks
//! for.

use std::cell::RefCell;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_dom::build;
use xylograph_xdm::{Documents, DomNode};

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

/// Where XSLT's `document()` gets a tree from (XSLT 1.0 §12.1).
///
/// This is not the [`Loader`] above, and the difference is the point: a module is bytes, and a
/// document is a *node in the model's node space*. Whoever answers has to put the tree somewhere
/// the model will find it, which is what [`LoadedDocuments`] does with a shared
/// [`Documents`] handle.
///
/// It is a source of its own rather than a method on the model because the function that calls
/// it is registered before the transformation begins and outlives every step of it — it cannot
/// hold a borrow of the model, so what it holds must own, or share, instead.
pub trait DocumentSource<N> {
  /// The root node of the document at an absolute URI.
  ///
  /// `Ok(None)` means there is nothing there, which §12.1 lets a processor recover from by
  /// giving the empty node-set.
  ///
  /// # Errors
  ///
  /// If the document is there but cannot be served or read.
  fn document(&self, uri: &str) -> Result<Option<N>>;
}

/// A source that has nothing, so `document()` always finds nothing.
///
/// The default, because fetching a document a stylesheet names is I/O on the caller's behalf —
/// the same trust decision as [`Loader`], taken the same way.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDocuments;

impl<N> DocumentSource<N> for NoDocuments {
  fn document(&self, _uri: &str) -> Result<Option<N>> {
    Ok(None)
  }
}

/// Documents fetched through a [`Loader`] and put into a [`Documents`] handle.
///
/// The handle must be the one the model was built with, or the nodes this hands back will name
/// documents that model cannot read:
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::{DomModel, Documents};
/// use xylograph_xslt::{LoadedDocuments, Loader};
/// # use xylograph_core::error::Result;
///
/// struct FromMemory;
/// impl Loader for FromMemory {
///   fn load(&mut self, _uri: &str) -> Result<Vec<u8>> {
///     Ok(b"<extra>fetched</extra>".to_vec())
///   }
/// }
///
/// let source = build::parse("<a/>".as_bytes())?;
/// let documents = Documents::new();
/// let model = DomModel::with_documents(&source, &documents);
/// let available = LoadedDocuments::new(&documents, FromMemory);
/// # let _ = (model, available);
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Debug)]
pub struct LoadedDocuments<L> {
  documents: Documents,
  loader: RefCell<L>,
}

impl<L: Loader> LoadedDocuments<L> {
  /// A source that loads through `loader` and keeps what it loads in `documents`.
  pub fn new(documents: &Documents, loader: L) -> Self {
    Self { documents: documents.clone(), loader: RefCell::new(loader) }
  }
}

impl<L: Loader> DocumentSource<DomNode> for LoadedDocuments<L> {
  fn document(&self, uri: &str) -> Result<Option<DomNode>> {
    // §12.1: two calls naming the same URI give the same tree, so a document is fetched once
    // and the nodes of it compare equal however they were reached.
    if let Some(found) = self.documents.find(uri) {
      return Ok(Some(found));
    }
    let source = self.loader.borrow_mut().load(uri)?;
    let document = build::parse_with_system_id(source.as_slice(), uri)?;
    Ok(Some(self.documents.add(uri, document)))
  }
}
