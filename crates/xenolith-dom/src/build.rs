//! Building a [`Document`] from parsed XML.
//!
//! A [`DomBuilder`] is a [`Handler`]: a source calls it for each event, and it turns the stream into a tree. A start
//! tag becomes an element with its attributes, character data becomes text, CDATA sections, comments, and processing
//! instructions become their own nodes, and a `DOCTYPE` becomes the document type node. The parser-resolved namespaces
//! carry over to element and attribute names.
//!
//! ID-typed attributes are marked as they are added (an `xml:id`, and any attribute a DTD declares `ID`), so
//! [`get_element_by_id`](Document::get_element_by_id) works on the result. Each element's base URI is recorded too,
//! resolved from `xml:base` and the document's system identifier, so [`base_uri`](Document::base_uri) reports it.
//!
//! Because it is a [`Handler`], a builder runs beside other handlers in one pass. A validator can check the document
//! as the tree is built, in a single read of the source, rather than in a second pass. The [`parse`] functions are the
//! shorthand for the common case of building alone.
//!
//! This is behind the `parse` feature, which enables the parser's XML Base and `xml:id` support.
//!
//! # Examples
//!
//! ```
//! use xenolith_dom::build;
//!
//! let doc = build::parse("<doc><p>Hello</p></doc>".as_bytes())?;
//! let root = doc.document_element().unwrap();
//! assert_eq!(doc.node_name(root), "doc");
//! assert_eq!(doc.text_content(root), "Hello");
//! # Ok::<(), xenolith_core::Error>(())
//! ```

use std::io::Read;

use xenolith_core::error::{Error, Result};
use xenolith_core::name::NameId;
use xenolith_parser::Reader;
use xenolith_parser::dtd::{AttType, Dtd};
use xenolith_parser::sax::{
  CdataEvent, CharactersEvent, CommentEvent, DoctypeEvent, EndElementEvent, EventSource, Handler,
  ProcessingInstructionEvent, StartElementEvent,
};

use crate::{Document, DomException, NodeId};

/// Builds a [`Document`] from XML read from `source`.
///
/// External entities and an external DTD subset are not resolved because this convenience uses a reader with no
/// resolver. Use [`parse_reader`] with a configured [`Reader`] when a resolver, explicit limits, or a system
/// identifier are needed.
///
/// # Errors
///
/// Returns the parser's error if the document is not well-formed, or if reading `source` fails.
///
pub fn parse<R: Read>(source: R) -> Result<Document> {
  parse_reader(Reader::new(source))
}

/// Builds a [`Document`] from XML read from `source`, with its system identifier known.
///
/// The identifier is the document's base URI, so relative `xml:base` values in it resolve correctly. A post-processor
/// such as XInclude needs this when it parses an included resource.
///
/// # Errors
///
/// As [`parse`].
///
pub fn parse_with_system_id<R: Read>(source: R, system_id: &str) -> Result<Document> {
  parse_reader(Reader::with_system_id(source, system_id))
}

/// Builds a [`Document`] from a prepared [`Reader`], so a resolver or configuration can be set first.
///
/// # Errors
///
/// As [`parse`].
///
pub fn parse_reader<R: Read>(mut reader: Reader<R>) -> Result<Document> {
  let mut builder = DomBuilder::new();
  reader.emit(&mut builder)?;
  // The parser gives a well-formed event stream, so a DOM operation should never refuse it here. If one does, it is a
  // builder bug rather than a document problem.
  builder.into_document().map_err(|error| Error::internal(format!("building the DOM: {error}")))
}

/// A [`Handler`] that builds a [`Document`] from the events it is given.
///
/// Run it through any [`EventSource`], for example, a [`Reader`], with [`emit`](EventSource::emit), then take the tree
/// with [`into_document`](Self::into_document). Since it is a handler, it also runs alongside a validator in one pass,
/// so it checks the document as it builds the tree. The [`parse`] functions wrap the common case of building from a
/// reader alone.
///
/// # Examples
///
/// On its own, driven by a reader through [`emit`](EventSource::emit):
///
/// ```
/// use xenolith_dom::build::DomBuilder;
/// use xenolith_parser::Reader;
/// use xenolith_parser::sax::EventSource;
///
/// let mut builder = DomBuilder::new();
/// Reader::new("<doc>hi</doc>".as_bytes()).emit(&mut builder)?;
/// let doc = builder.into_document()?;
/// assert_eq!(doc.node_name(doc.document_element().unwrap()), "doc");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Building while validating against the document's own DTD in one pass. The builder joins the validation pipeline
/// as an application handler, so a single read of the source both checks the document and produces the tree:
///
/// ```
/// use xenolith_dom::build::DomBuilder;
/// use xenolith_parser::Reader;
/// use xenolith_validate::Validatable;
///
/// let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)>]><r><item>hi</item></r>";
/// let mut builder = DomBuilder::new();
/// let report = Reader::new(xml.as_bytes())
///     .with_validation()
///     .validating_dtd()
///     .with_handler(&mut builder)
///     .run()?;
/// assert!(report.is_valid());
///
/// let doc = builder.into_document()?;
/// assert_eq!(doc.node_name(doc.document_element().unwrap()), "r");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Building while validating against an application `Schema` in a single pass. Add `with_schema` in place of, or
/// beside, `validating_dtd`:
///
/// ```no_run
/// use xenolith_dom::build::DomBuilder;
/// use xenolith_parser::Reader;
/// use xenolith_validate::{Schema, Validatable};
///
/// # fn build_and_check(reader: Reader<&[u8]>, schema: &dyn Schema) -> Result<(), Box<dyn std::error::Error>> {
/// let mut builder = DomBuilder::new();
/// let report = reader
///     .with_validation()
///     .with_schema(schema)
///     .with_handler(&mut builder)
///     .run()?;
///
/// let doc = builder.into_document()?;
/// # let _ = (report.is_valid(), doc.document_element());
/// # Ok(()) }
/// ```
///
#[derive(Debug)]
pub struct DomBuilder {
  doc: Document,
  /// The open nodes, outermost first. The document is always at the bottom.
  open: Vec<NodeId>,
  document_base_set: bool,
  /// The DTD, captured at the `DOCTYPE` event, for marking the attributes it declares `ID`.
  dtd: Option<Dtd>,
  /// A DOM exception raised while building, held to stop the run and report afterward.
  failure: Option<DomException>,
}

impl Default for DomBuilder {
  fn default() -> Self {
    Self::new()
  }
}

impl DomBuilder {
  /// Creates a builder for a new, empty document.
  #[must_use]
  pub fn new() -> Self {
    let doc = Document::new();
    let document_node = doc.document_node();
    Self { doc, open: vec![document_node], document_base_set: false, dtd: None, failure: None }
  }

  /// The document built, or the [`DomException`] that stopped the build.
  ///
  /// # Errors
  ///
  /// Returns the [`DomException`] a DOM operation raised while building, for example, a [`HIERARCHY_REQUEST_ERR`] for
  /// an event stream that places two root elements. A well-formed document read by the parser raises none, so
  /// [`parse`] and its siblings treat this as a bug; a source of arbitrary events may reach it legitimately.
  ///
  /// [`HIERARCHY_REQUEST_ERR`]: crate::ExceptionCode::HIERARCHY_REQUEST_ERR
  ///
  pub fn into_document(self) -> std::result::Result<Document, DomException> {
    match self.failure {
      Some(error) => Err(error),
      None => Ok(self.doc),
    }
  }

  /// Records the document's base URI once, from the system identifier on the first event that carries one.
  ///
  fn ensure_document_base(&mut self, system_id: Option<&str>) {
    if !self.document_base_set {
      self.document_base_set = true;
      self.doc.set_document_base(system_id);
    }
  }

  /// Creates the element for the start tag, with its attributes, base URI, and ID marks.
  ///
  fn build_element(&mut self, event: &StartElementEvent<'_>) -> std::result::Result<NodeId, DomException> {
    let lexical = event.name.to_lexical(event.pool);
    let namespace = event.name.namespace().map(|ns| event.pool.resolve(ns).to_owned());
    let element = self.doc.create_element_ns(namespace.as_deref(), &lexical)?;

    for attr in event.attributes.iter() {
      let name = attr.name.to_lexical(event.pool);
      let namespace = attr.name.namespace().map(|ns| event.pool.resolve(ns).to_owned());
      match namespace {
        Some(ns) => self.doc.set_attribute_ns(element, Some(&ns), &name, attr.value)?,
        None => self.doc.set_attribute(element, &name, attr.value)?,
      }
    }

    self.doc.set_element_base(element, event.base_uri);
    self.mark_id_attributes(element, event);
    Ok(element)
  }

  /// Marks the ID-typed attributes of the element: `xml:id`, and any the DTD declares `ID`, so they are found by
  /// [`Document::get_element_by_id`].
  ///
  fn mark_id_attributes(&mut self, element: NodeId, event: &StartElementEvent<'_>) {
    for attr in event.attributes.iter() {
      let is_xml_id = attr.name.namespace() == Some(NameId::XML_NS) && event.pool.resolve(attr.name.local()) == "id";
      if is_xml_id {
        let name = attr.name.to_lexical(event.pool);
        let _ = self.doc.set_id_attribute(element, &name, true);
      }
    }

    let Some(dtd) = &self.dtd else { return };
    let element_name = event.name.to_lexical(event.pool);
    let Some(element_id) = event.pool.get(&element_name) else { return };
    let Some(defs) = dtd.attlist(element_id) else { return };
    for def in defs {
      if matches!(def.att_type, AttType::Id) {
        let name = event.pool.resolve(def.name).to_owned();
        if self.doc.has_attribute(element, &name) {
          let _ = self.doc.set_id_attribute(element, &name, true);
        }
      }
    }
  }

  /// Appends a node under the currently open node, holding any DOM exception so the run stops.
  ///
  fn append(&mut self, node: NodeId) {
    let parent = *self.open.last().expect("the document is always open");
    if let Err(error) = self.doc.append_child(parent, node) {
      self.failure = Some(error);
    }
  }
}

impl Handler for DomBuilder {
  fn start_element(&mut self, event: StartElementEvent<'_>) {
    if self.failure.is_some() {
      return;
    }
    self.ensure_document_base(event.location.system_id.as_deref());
    match self.build_element(&event) {
      Ok(element) => {
        self.append(element);
        self.open.push(element);
      }
      Err(error) => self.failure = Some(error),
    }
  }

  fn end_element(&mut self, _event: EndElementEvent<'_>) {
    self.open.pop();
  }

  fn characters(&mut self, event: CharactersEvent<'_>) {
    if self.failure.is_some() {
      return;
    }
    // Coalesce here: a run of text may arrive as several calls, and the data model wants adjacent character data as a
    // single text node. `append_text` extends the open node's last child when it is already text.
    let parent = *self.open.last().expect("the document is always open");
    if let Err(error) = self.doc.append_text(parent, event.text) {
      self.failure = Some(error);
    }
  }

  fn cdata(&mut self, event: CdataEvent<'_>) {
    if self.failure.is_some() {
      return;
    }
    let node = self.doc.create_cdata_section(event.text);
    self.append(node);
  }

  fn comment(&mut self, event: CommentEvent<'_>) {
    if self.failure.is_some() {
      return;
    }
    let node = self.doc.create_comment(event.text);
    self.append(node);
  }

  fn processing_instruction(&mut self, event: ProcessingInstructionEvent<'_>) {
    if self.failure.is_some() {
      return;
    }
    match self.doc.create_processing_instruction(event.target, event.data) {
      Ok(node) => self.append(node),
      Err(error) => self.failure = Some(error),
    }
  }

  fn doctype(&mut self, event: DoctypeEvent<'_>) {
    if self.failure.is_some() {
      return;
    }
    self.ensure_document_base(event.location.system_id.as_deref());
    // Keep the DTD so a later start tag can mark the attributes it declares `ID`.
    self.dtd = Some(event.dtd.clone());
    if let Some(name) = event.name {
      match self.doc.create_document_type(name, event.public_id, event.system_id) {
        Ok(node) => self.append(node),
        Err(error) => self.failure = Some(error),
      }
    }
  }

  fn should_continue(&self) -> bool {
    self.failure.is_none()
  }
}
