//! Fetching the documents a stylesheet names — its own modules, and the trees `document()` asks
//! for.

use std::cell::RefCell;

use xylogue_core::error::{Error, Result};
use xylogue_dom::{Document, NodeId, build};
use xylogue_xdm::{Documents, DomNode};

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

/// A boxed loader is a loader, so one can be chosen at run time.
impl<L: Loader + ?Sized> Loader for Box<L> {
  fn load(&mut self, uri: &str) -> Result<Vec<u8>> {
    (**self).load(uri)
  }
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
    Err(Error::xslt(message))
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

  /// Takes a tree the transformation built and gives back its root as a node of the model.
  ///
  /// This is what `exsl:node-set()` needs: a result tree fragment lives in the engine's own
  /// result document, and turning it into a node-set means putting it where the model can read
  /// it. The default is to take nothing, so a caller who supplied nowhere gets an error saying
  /// so rather than a node-set of somebody else's tree.
  ///
  /// `root` is the node the tree hangs from, which is a document fragment rather than the
  /// document node: XSLT lets a result tree fragment hold several elements side by side, and an
  /// XML document node may not.
  ///
  /// # Errors
  ///
  /// If the tree cannot be taken in.
  fn adopt(&self, document: Document, root: NodeId) -> Result<Option<N>> {
    let _ = (document, root);
    Ok(None)
  }
}

/// Where a result other than the principal one goes: EXSLT's `exsl:document`.
///
/// A stylesheet using it writes several files from one transformation — a chapter per page, an
/// index beside them. The engine builds each result and writes it out as that element's own
/// output settings ask; where the bytes then go is the caller's business, exactly as fetching is.
///
/// Nothing is written unless a caller supplies one of these, for the same reason nothing is
/// fetched unless a [`Loader`] is supplied: a stylesheet is data, and data must not decide on its
/// own to write to a path of its choosing.
///
/// # Examples
///
/// ```
/// use std::cell::RefCell;
/// use std::collections::HashMap;
/// use std::rc::Rc;
/// use xylogue_xslt::ResultSink;
///
/// /// A sink that keeps what was written, which is what a test wants.
/// #[derive(Default)]
/// struct Collected(HashMap<String, Vec<u8>>);
///
/// impl ResultSink for Collected {
///   fn write(&mut self, href: &str, bytes: &[u8]) -> xylogue_core::Result<()> {
///     self.0.insert(href.to_owned(), bytes.to_vec());
///     Ok(())
///   }
/// }
///
/// let sink = Rc::new(RefCell::new(Collected::default()));
/// // `Transform::with_results(Rc::clone(&sink))`, and afterwards `sink.borrow()` has the files.
/// ```
pub trait ResultSink {
  /// Takes one secondary result: the `href` the stylesheet asked for, and the bytes.
  ///
  /// `href` is resolved against the base URI of the `exsl:document` element, so what arrives is
  /// absolute. A sink that will not write somewhere should say so rather than ignore it.
  ///
  /// # Errors
  ///
  /// Whatever writing it failed with, which stops the transformation.
  fn write(&mut self, href: &str, bytes: &[u8]) -> Result<()>;
}

/// A sink that writes nowhere, and says so.
///
/// The default. Refusing by name beats writing to the working directory because a stylesheet
/// said to — and beats silence, which would look like the file had been written.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoResults;

impl ResultSink for NoResults {
  fn write(&mut self, href: &str, _bytes: &[u8]) -> Result<()> {
    let message = format!(
      "exsl:document asked to write {href:?}, and no result sink was given; \
       supply one with Transform::with_results to say where a secondary result may go"
    );
    Err(Error::xslt(message))
  }
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
/// use xylogue_dom::build;
/// use xylogue_xdm::{DomModel, Documents};
/// use xylogue_xslt::{LoadedDocuments, Loader};
/// # use xylogue_core::error::Result;
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
/// # Ok::<(), xylogue_core::Error>(())
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

  fn adopt(&self, document: Document, root: NodeId) -> Result<Option<DomNode>> {
    Ok(Some(adopt_into(&self.documents, document, root)))
  }
}

/// A place for trees the transformation builds, with nothing to fetch.
///
/// `document()` finds nothing here — fetching is I/O and stays opt-in — but a result tree
/// fragment can be adopted, which is all `exsl:node-set()` asks for. Use it when a stylesheet
/// needs `exsl:node-set()` and no external documents. The handle must be the one the model was
/// built with, or the nodes it hands back name a document that model cannot read:
///
/// ```
/// use std::rc::Rc;
/// use xylogue_dom::build;
/// use xylogue_xdm::{DomModel, Documents};
/// use xylogue_xpath::Functions;
/// use xylogue_xslt::{Stylesheet, Transform, TreeSpace};
///
/// let stylesheet = Stylesheet::compile(
///   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
///         <xsl:template match="/">ran</xsl:template>
///       </xsl:stylesheet>"#,
///   "file:///s.xsl",
/// )?;
///
/// let source = build::parse("<a/>".as_bytes())?;
/// let documents = Documents::new();
/// let model = DomModel::with_documents(&source, &documents);
/// let space = Rc::new(TreeSpace::new(&documents));
///
/// let result = Transform::new()
///   .run_with_documents(&stylesheet, &model, model.root_node(), Functions::new(), space)?;
/// assert_eq!(result.text(), "ran");
/// # Ok::<(), xylogue_core::Error>(())
/// ```
///
/// The `xylogue-exslt` crate's `common` module has the example that actually calls
/// `exsl:node-set()`; it lives there because the function does.
#[derive(Clone, Debug)]
pub struct TreeSpace {
  documents: Documents,
}

impl TreeSpace {
  /// A place tied to the handle a model reads.
  #[must_use]
  pub fn new(documents: &Documents) -> Self {
    Self { documents: documents.clone() }
  }
}

impl DocumentSource<DomNode> for TreeSpace {
  fn document(&self, _uri: &str) -> Result<Option<DomNode>> {
    Ok(None)
  }

  fn adopt(&self, document: Document, root: NodeId) -> Result<Option<DomNode>> {
    Ok(Some(adopt_into(&self.documents, document, root)))
  }
}

/// Adds a tree the transformation built, under a URI no document can be fetched from.
///
/// A fragment has no URI of its own, and giving it one that could be fetched would make two
/// unrelated things collide in [`Documents::find`]. The counter keeps each adoption separate,
/// since two fragments are two trees even when they say the same thing.
fn adopt_into(documents: &Documents, document: Document, root: NodeId) -> DomNode {
  let uri = format!("urn:xylogue:result-tree-fragment:{}", documents.len());
  documents.add_rooted(&uri, document, root)
}
