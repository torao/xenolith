//! Running a transformation, the way `javax.xml.transform` does.
//!
//! Everything here can be done with the crates underneath — compile a [`Stylesheet`], build a
//! [`DomModel`], call `Transform::run`. This is the same thing arranged the way a caller coming
//! from JAXP expects to find it: one object that holds the stylesheet, the parameters, and where
//! documents come from, and which can be used over and over.
//!
//! # Coming from Java
//!
//! | JAXP | Here |
//! |---|---|
//! | `TransformerFactory.newTransformer(Source)` | [`Transformer::compile`] |
//! | `TransformerFactory.newTransformer()` | [`Transformer::identity`] |
//! | `Transformer.setParameter(name, value)` | [`Transformer::with_parameter`] |
//! | `Transformer.setURIResolver(r)` | [`Transformer::with_resolver`] |
//! | — (XSLT 2.0's `xsl:result-document`) | [`Transformer::with_results`], for `exsl:document` |
//! | `Transformer.transform(source, result)` | [`Transformer::transform`] |
//! | `StreamSource` / `DOMSource` | [`Source::bytes`] / [`Source::document`] |
//! | `StreamResult` | [`Transformed::write`] and [`Transformed::text`] |
//! | `ErrorListener` | [`Transformed::messages`] — see below |
//!
//! The differences are deliberate, and follow the project's third decision: what W3C specifies is
//! followed as specified, and what JAXP invented is redesigned to fit Rust. So a transformer is
//! built by consuming methods rather than by setters, a failure is a [`Result`] rather than an
//! exception, and there is no `ErrorListener` to install — what `xsl:message` said comes back
//! beside the result, and everything that would be a fatal error is the `Err`.
//!
//! # Examples
//!
//! ```
//! use xylogue::transform::{Source, Transformer};
//!
//! let transformer = Transformer::compile(Source::bytes(
//!   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
//!         <xsl:output method="text"/>
//!         <xsl:param name="greeting">Hello</xsl:param>
//!         <xsl:template match="/"><xsl:value-of select="$greeting"/>, <xsl:value-of select="//name"/>
//!         </xsl:template>
//!       </xsl:stylesheet>"#,
//! ))?
//! .with_parameter("greeting", "Good day");
//!
//! let result = transformer.transform(Source::bytes(b"<doc><name>Ada</name></doc>"))?;
//! assert_eq!(result.text().trim(), "Good day, Ada");
//! # Ok::<(), xylogue::Error>(())
//! ```

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use xylogue_core::error::{Error, Result};
use xylogue_dom::{Document, build};
use xylogue_xdm::{Documents, DomModel, DomNode};
use xylogue_xpath::Functions;
use xylogue_xslt::{DocumentSource, LoadedDocuments, Loader, NoLoader, ResultSink, Stylesheet, Transform, TreeSpace};

/// The method `xsl:output` asked the result to be written by.
pub use xylogue_xslt::OutputMethod;

/// Where a document comes from: JAXP's `Source`.
///
/// Bytes are parsed; a [`Document`] already built is used as it stands. A system identifier is
/// what relative references — `xsl:import`, `document()` — are resolved against, so a source
/// that names one can be part of a stylesheet built from several files.
#[derive(Debug)]
pub struct Source<'a> {
  content: Content<'a>,
  system_id: Option<String>,
}

#[derive(Debug)]
enum Content<'a> {
  Bytes(&'a [u8]),
  Document(&'a Document),
}

impl<'a> Source<'a> {
  /// XML to be parsed: JAXP's `StreamSource`.
  #[must_use]
  pub const fn bytes(bytes: &'a [u8]) -> Self {
    Self { content: Content::Bytes(bytes), system_id: None }
  }

  /// A document already built: JAXP's `DOMSource`.
  #[must_use]
  pub const fn document(document: &'a Document) -> Self {
    Self { content: Content::Document(document), system_id: None }
  }

  /// Says where this came from, which relative references are resolved against.
  #[must_use]
  pub fn with_system_id(mut self, system_id: &str) -> Self {
    self.system_id = Some(system_id.to_owned());
    self
  }

  /// The system identifier, or a placeholder naming what has none.
  fn system_id(&self) -> String {
    self.system_id.clone().unwrap_or_else(|| "urn:xylogue:unnamed-source".to_owned())
  }
}

/// Where documents a transformation names are fetched from: JAXP's `URIResolver`.
///
/// This is [`Loader`] under another name, because a stylesheet module and a `document()` tree
/// are fetched the same way; the transformer hands it to both.
pub trait Resolver: Loader {}

impl<L: Loader> Resolver for L {}

/// A compiled stylesheet with the settings to run it: JAXP's `Transformer`.
///
/// One can be used any number of times, over any number of documents.
pub struct Transformer {
  stylesheet: Option<Stylesheet>,
  transform: Transform,
  resolver: Option<Rc<dyn Fn() -> Box<dyn Loader>>>,
}

impl std::fmt::Debug for Transformer {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Transformer")
      .field("stylesheet", &self.stylesheet.is_some())
      .field("transform", &self.transform)
      .field("resolver", &self.resolver.is_some())
      .finish()
  }
}

impl Transformer {
  /// Compiles a stylesheet: JAXP's `newTransformer(Source)`.
  ///
  /// A stylesheet naming `xsl:import` or `xsl:include` needs a resolver, which
  /// [`with_resolver`](Self::with_resolver) supplies — but the modules are fetched when the
  /// stylesheet is compiled, so use [`compile_with`](Self::compile_with) for that.
  ///
  /// # Errors
  ///
  /// If the stylesheet is not well-formed, is not a stylesheet, or names a module with no
  /// resolver to fetch it.
  pub fn compile(source: Source<'_>) -> Result<Self> {
    Self::compile_with(source, &mut NoLoader)
  }

  /// Compiles a stylesheet, fetching the modules it names through `resolver`.
  ///
  /// # Errors
  ///
  /// As [`compile`](Self::compile), and whatever the resolver raises.
  pub fn compile_with<R: Resolver>(source: Source<'_>, resolver: &mut R) -> Result<Self> {
    let system_id = source.system_id();
    let stylesheet = match source.content {
      Content::Bytes(bytes) => Stylesheet::compile_with(bytes, &system_id, resolver)?,
      Content::Document(_) => {
        // A stylesheet is compiled from its text, because the modules it names are fetched
        // during compilation and each needs its own document.
        let message = "a stylesheet is compiled from bytes; give Source::bytes, not a Document".to_owned();
        return Err(Error::xslt(message));
      }
    };
    Ok(Self { stylesheet: Some(stylesheet), transform: Transform::new(), resolver: None })
  }

  /// A transformer with no stylesheet, which copies its input: JAXP's `newTransformer()`.
  ///
  /// Useful for writing a document out through the same path a transformation writes one.
  #[must_use]
  pub fn identity() -> Self {
    Self { stylesheet: None, transform: Transform::new(), resolver: None }
  }

  /// Supplies a value for a top-level `xsl:param`: JAXP's `setParameter`.
  #[must_use]
  pub fn with_parameter(mut self, name: &str, value: &str) -> Self {
    self.transform = self.transform.with_parameter(name, value);
    self
  }

  /// How deep template application may go before the transformation is refused.
  #[must_use]
  pub fn with_max_depth(mut self, depth: usize) -> Self {
    self.transform = self.transform.with_max_depth(depth);
    self
  }

  /// Where `document()` fetches from: JAXP's `setURIResolver`.
  ///
  /// The resolver is made afresh for each transformation, because one may be run several times
  /// and a resolver that had been used once should not carry anything into the next.
  #[must_use]
  pub fn with_resolver(mut self, resolver: impl Fn() -> Box<dyn Loader> + 'static) -> Self {
    self.resolver = Some(Rc::new(resolver));
    self
  }

  /// Where a result other than the principal one goes: EXSLT's `exsl:document`.
  ///
  /// JAXP has no counterpart — secondary results arrived with XSLT 2.0's `xsl:result-document`,
  /// and in 1.0 they are this extension. Without a sink, a stylesheet that uses it is refused by
  /// name rather than writing to a path of its own choosing; see [`ResultSink`].
  ///
  /// The sink is shared rather than given away, so the caller still has it when the
  /// transformation is done.
  #[must_use]
  pub fn with_results(mut self, sink: Rc<RefCell<dyn ResultSink>>) -> Self {
    self.transform = self.transform.with_results(sink);
    self
  }

  /// Runs the transformation over a source document: JAXP's `transform`.
  ///
  /// # Errors
  ///
  /// If the source is not well-formed, or the transformation cannot be carried out.
  pub fn transform(&self, source: Source<'_>) -> Result<Transformed> {
    let system_id = source.system_id();
    let parsed;
    let document = match source.content {
      Content::Document(document) => document,
      Content::Bytes(bytes) => {
        parsed = build::parse_with_system_id(bytes, &system_id)?;
        &parsed
      }
    };

    // One handle shared by the model, `document()` and anything building a tree, so that every
    // node a transformation produces is one the model can read.
    let documents = Documents::new();
    let model = DomModel::with_documents(document, &documents);
    let trees: Rc<dyn DocumentSource<DomNode>> = match &self.resolver {
      Some(make) => Rc::new(LoadedDocuments::new(&documents, make())),
      None => Rc::new(TreeSpace::new(&documents)),
    };

    let Some(stylesheet) = &self.stylesheet else {
      return Ok(Transformed::copied(document));
    };

    let functions = extension_functions::<DomModel<'_>>(&trees);
    let result =
      self.transform.run_with_documents(stylesheet, &model, model.root_node(), functions, Rc::clone(&trees))?;
    Ok(Transformed { written: result.serialize(), bytes: result.to_bytes()?, messages: result.messages().to_vec() })
  }
}

/// The EXSLT functions, when this build has them.
#[cfg(feature = "exslt")]
fn extension_functions<M: xylogue_xdm::Model>(trees: &Rc<dyn DocumentSource<M::Node>>) -> Functions<M> {
  xylogue_exslt::register_with(Functions::new(), trees)
}

/// No extension functions, when EXSLT was not built in.
#[cfg(not(feature = "exslt"))]
fn extension_functions<M: xylogue_xdm::Model>(trees: &Rc<dyn DocumentSource<M::Node>>) -> Functions<M> {
  let _ = trees;
  Functions::new()
}

/// What a transformation produced: JAXP's `Result`, after the fact rather than before it.
///
/// JAXP has the caller hand in a `Result` to be filled. Here the transformation produces one and
/// the caller decides what to do with it, which is the same information the other way round and
/// needs no mutable borrow to outlive the call.
#[derive(Clone, Debug)]
pub struct Transformed {
  written: String,
  bytes: Vec<u8>,
  messages: Vec<String>,
}

impl Transformed {
  /// A transformer with no stylesheet: the document written out as it stands.
  fn copied(document: &Document) -> Self {
    let written = match document.document_element() {
      Some(root) => xylogue_serialize::Serializer::new().to_string(document, root),
      None => String::new(),
    };
    Self { bytes: written.as_bytes().to_vec(), written, messages: Vec::new() }
  }

  /// The result as text, written the way `xsl:output` asked.
  #[must_use]
  pub fn text(&self) -> &str {
    &self.written
  }

  /// The result as bytes, in the encoding `xsl:output` asked for.
  #[must_use]
  pub fn bytes(&self) -> &[u8] {
    &self.bytes
  }

  /// What `xsl:message` said, in the order it was said.
  ///
  /// This is where JAXP's `ErrorListener` would have received a `warning`. Anything that would
  /// have been an `error` or a `fatalError` is the `Err` of the transformation instead, so there
  /// is nothing to install and nothing that can be missed by not installing it.
  #[must_use]
  pub fn messages(&self) -> &[String] {
    &self.messages
  }

  /// Writes the result.
  ///
  /// # Errors
  ///
  /// Whatever the writer raises.
  pub fn write<W: io::Write>(&self, mut writer: W) -> io::Result<()> {
    writer.write_all(&self.bytes)
  }
}
