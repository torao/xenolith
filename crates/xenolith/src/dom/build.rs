//! Builds a [`Document`] from an event sequence generated during processes such as XML parsing.
//!
//! [`DomBuilder`] is a type of [`EventHandler`] that builds a DOM tree based on events generated from an arbitrary
//! event source.

use crate::dtd::model::{AttType, Dtd};
use crate::error::Error;
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, DoctypeEventRef, EndElementEventRef, EventHandler, EventRef,
  ProcessingInstructionEventRef, StartElementEventRef,
};
use crate::name::{NamePool, XML_NS_URI};

use crate::dom::{Document, DomException, NodeId};

/// An [`EventHandler`] that builds a [`Document`] from received events.
///
/// It can be attached to an [`EventSource`](crate::event::EventSource), such as a
/// [`StreamSource`](crate::io::StreamSource), and the built document can be retrieved via
/// [`into_document`](Self::into_document) after all events have been received.
///
/// The builder marks attributes of type ID, which are either `xml:id` attributes if [`DomBuilder::with_xml_id`] is
/// enabled, or attributes declared as `ID`-type in the DTD. This allows
/// [`get_element_by_id`](Document::get_element_by_id) to be used for these attributes. The builder also records the
/// base URI for each element. This is resolved using `xml:base` and the document's system identifier, and can be
/// retrieved via [`base_uri`](Document::base_uri).
///
/// Since [`DomBuilder`] is an [`EventHandler`], it can operate in a single parsing pass alongside other handlers. In
/// many cases, a [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator) should be placed upstream to ensure
/// that the constructed DOM is well-formed XML, and if validity is also required, it is appropriate to include another
/// [`ValidatorSet`](crate::event::validate::ValidatorSet).
///
/// # Examples
///
/// A build driven by an XML byte stream via the parser's [`emit`](crate::event::EventCursor::emit):
///
/// ```
/// use xenolith::event::{EventCursor, EventSource};
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::io::StreamSource;
///
/// let mut builder = DomBuilder::new();
/// StreamSource::new("<doc><p>Hello</p></doc>".as_bytes()).with_handler(&mut builder).emit()?;
/// let doc = builder.into_document();
/// let root = doc.document_element().unwrap();
/// assert_eq!(doc.node_name(root), "doc");
/// assert_eq!(doc.text_content(root), "Hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Validation against an internal DTD and DOM construction are performed in a single pass. Add this using
/// [`with_schema`](crate::event::validate::ValidatorSet::with_schema), either instead of or in conjunction with
/// `validating_dtd`:
///
/// ```
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::event::validate::ValidatorSet;
/// use xenolith::event::{Dispatch, EventCursor, EventSource};
/// use xenolith::io::StreamSource;
///
/// let xml = "<!DOCTYPE r [<!ELEMENT r (item+)><!ELEMENT item (#PCDATA)>]><r><item>hi</item></r>";
/// let mut validation = ValidatorSet::new().validating_dtd(true);
/// let mut builder = DomBuilder::new();
/// {
///   let mut lane = Dispatch::new().with_handler(&mut validation).with_handler(&mut builder);
///   StreamSource::new(xml.as_bytes()).with_handler(&mut lane).emit()?;
/// }
/// assert!(validation.report().is_valid());
///
/// let doc = builder.into_document();
/// assert_eq!(doc.node_name(doc.document_element().unwrap()), "r");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct DomBuilder {
  doc: Document,
  /// The open nodes (ordered from outermost to innermost). The document is always positioned at the bottom.
  open: Vec<NodeId>,
  /// Whether the document's base URI has been recorded. This occurs only once, upon the first event.
  document_base_set: bool,
  /// The DTD obtained from the `DOCTYPE` event; used to identify attributes declared as `ID`s.
  dtd: Option<Dtd>,
  /// The pool storing names found within the DTD. Since the DTD manages these names as keys and events carry names as
  /// text, this pool is maintained alongside the DTD.
  dtd_pool: Option<NamePool>,
  /// Whether to treat `xml:id` attributes as IDs.
  xml_id: bool,
  /// Whether any processing has taken place since this builder was created or reset.
  fresh: bool,
}

impl Default for DomBuilder {
  fn default() -> Self {
    Self::new()
  }
}

impl DomBuilder {
  /// Creates a builder for constructing a document.
  #[must_use]
  pub fn new() -> Self {
    let doc = Document::new();
    let document_node = doc.document_node();
    Self {
      doc,
      open: vec![document_node],
      document_base_set: false,
      dtd: None,
      dtd_pool: None,
      xml_id: true,
      fresh: true,
    }
  }

  /// Specifies whether to treat `xml:id` attributes as IDs, allowing the corresponding elements to be retrieved via
  /// [`get_element_by_id`](Document::get_element_by_id). This is `true` by default.
  ///
  /// If disabled, attributes named `xml:id` are treated as ordinary attributes unless declared as `ID` in a DTD. This
  /// is the standard behavior for processors that do not recognize `xml:id`. Since this builder does not reference the
  /// parser-side [`Extensions::xml_id`](crate::io::Extensions::xml_id), callers who have disabled `xml:id` on the
  /// parser must also disable it here.
  #[must_use]
  pub fn with_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// The built document.
  ///
  /// The resulting document is returned in its current state, regardless of whether processing completed successfully
  /// or the outcome of the validation.
  #[must_use]
  pub fn into_document(self) -> Document {
    self.doc
  }

  /// Saves the document's base URI exactly once, based on the system identifier reported by the first event.
  fn ensure_document_base(&mut self, system_id: Option<&str>) {
    if !self.document_base_set {
      self.document_base_set = true;
      self.doc.set_document_base(system_id);
    }
  }

  /// Creates an element for the start tag, including attributes, the base URI, and the ID mark.
  fn build_element(&mut self, event: &StartElementEventRef<'_>) -> std::result::Result<NodeId, DomException> {
    let lexical = event.lexical();
    let element = self.doc.create_element_ns(event.namespace, &lexical)?;

    for attr in event.attributes.iter() {
      let name = attr.lexical();
      match attr.namespace {
        Some(namespace) => self.doc.set_attribute_ns(element, Some(namespace), &name, attr.value)?,
        None => self.doc.set_attribute(element, &name, attr.value)?,
      }
    }

    self.doc.set_element_base(element, event.base_uri);
    self.mark_id_attributes(element, event);
    Ok(element)
  }

  /// Marks an element's attribute as being of type ID. This applies to `xml:id` (unless disabled via
  /// [`with_xml_id`](Self::with_xml_id)) and any attribute declared as `ID` in the DTD. This enables these elements to
  /// be retrieved using [`Document::get_element_by_id`].
  fn mark_id_attributes(&mut self, element: NodeId, event: &StartElementEventRef<'_>) {
    if self.xml_id {
      for attr in event.attributes.iter() {
        if attr.namespace == Some(XML_NS_URI) && attr.local == "id" {
          let name = attr.lexical();
          let _ = self.doc.set_id_attribute(element, &name, true);
        }
      }
    }

    // The DTD is keyed by the names interned in the pool it was parsed in, so the element is looked up by the form it
    // was written in.
    let Some(dtd) = &self.dtd else { return };
    let Some(pool) = &self.dtd_pool else { return };
    let Some(element_id) = pool.get(&event.lexical()) else { return };
    let Some(defs) = dtd.attlist(element_id) else { return };
    for def in defs {
      if matches!(def.att_type, AttType::Id) {
        let name = pool.resolve(def.name).to_owned();
        if self.doc.has_attribute(element, &name) {
          let _ = self.doc.set_id_attribute(element, &name, true);
        }
      }
    }
  }

  /// Resets the document's build state.
  fn reset(&mut self) {
    self.doc = Document::new();
    self.open.clear();
    self.open.push(self.doc.document_node());
    self.document_base_set = false;
    self.dtd = None;
    self.dtd_pool = None;
    self.fresh = true;
  }

  /// Closes the innermost open element.
  ///
  /// # Errors
  ///
  /// If there are no open elements, or if the event specifies an element other than the innermost open element.
  fn end_element(&mut self, event: &EndElementEventRef<'_>) -> crate::Result<()> {
    // The document sits at the bottom of the stack and is not an element, so a stack of one is nothing open.
    let innermost = if self.open.len() > 1 { self.open.last().copied() } else { None };
    let ended = event.lexical();
    let Some(element) = innermost else {
      return Err(Error::well_formedness(format!("an end element </{ended}> with no element open")));
    };
    let open = self.doc.node_name(element);
    if open != ended {
      return Err(Error::well_formedness(format!("mismatched end tag: expected </{open}>, found </{ended}>")));
    }
    self.open.pop();
    Ok(())
  }

  /// Appends a node under the currently open node.
  fn append(&mut self, node: NodeId) -> crate::Result<()> {
    let parent = *self.open.last().expect("the document is always open");
    self.doc.append_child(parent, node)?;
    Ok(())
  }

  /// Begins a run: a document that follows another starts from a document of its own.
  fn start_document(&mut self) {
    // The first run is already in the document this builder was made with, so nothing is put back and nothing is
    // thrown away.
    if !self.fresh {
      self.reset();
    }
  }

  /// Opens an element under the currently open node, with its attributes, base URI, and ID marks.
  fn start_element(&mut self, event: &StartElementEventRef<'_>) -> crate::Result<()> {
    self.ensure_document_base(event.location.system_id.as_deref());
    let element = self.build_element(event)?;
    self.append(element)?;
    self.open.push(element);
    Ok(())
  }

  /// Appends character data, joined to the text it follows.
  fn characters(&mut self, event: &CharactersEventRef<'_>) -> crate::Result<()> {
    // Coalesce here: a run of text may arrive as several events, and the data model wants adjacent character data as a
    // single text node. `append_text` extends the open node's last child when it is already text.
    let parent = *self.open.last().expect("the document is always open");
    self.doc.append_text(parent, event.text)?;
    Ok(())
  }

  /// Appends a CDATA section, which the data model keeps apart from the text around it.
  fn cdata(&mut self, event: &CdataEventRef<'_>) -> crate::Result<()> {
    let node = self.doc.create_cdata_section(event.text);
    self.append(node)
  }

  /// Appends a comment.
  fn comment(&mut self, event: &CommentEventRef<'_>) -> crate::Result<()> {
    let node = self.doc.create_comment(event.text);
    self.append(node)
  }

  /// Appends a processing instruction.
  fn processing_instruction(&mut self, event: &ProcessingInstructionEventRef<'_>) -> crate::Result<()> {
    let node = self.doc.create_processing_instruction(event.target, event.data)?;
    self.append(node)
  }

  /// Appends the document type node, and keeps the DTD the event carries.
  fn doctype(&mut self, event: &DoctypeEventRef<'_>) -> crate::Result<()> {
    self.ensure_document_base(event.location.system_id.as_deref());
    // Keep the DTD, and the pool its names are interned in, so a later start tag can mark the attributes it declares
    // `ID`.
    self.dtd = Some(event.dtd.clone());
    self.dtd_pool = Some(event.pool.fork());
    // A document type declaration with no name is not a form XML has, so the event adds no node.
    let Some(name) = event.name else { return Ok(()) };
    let node = self.doc.create_document_type(name, event.public_id, event.system_id)?;
    self.append(node)
  }
}

impl EventHandler for DomBuilder {
  fn handle(&mut self, event: &EventRef<'_>) -> crate::Result<()> {
    if !matches!(event, EventRef::StartDocument) {
      self.fresh = false;
    }
    let handled = match event {
      EventRef::StartDocument => {
        self.start_document();
        Ok(())
      }
      EventRef::EndDocument => Ok(()),
      EventRef::Doctype(event) => self.doctype(event),
      EventRef::StartElement(event) => self.start_element(event),
      EventRef::EndElement(event) => self.end_element(event),
      EventRef::Characters(event) => self.characters(event),
      EventRef::Cdata(event) => self.cdata(event),
      EventRef::Comment(event) => self.comment(event),
      EventRef::ProcessingInstruction(event) => self.processing_instruction(event),
    };
    match (handled, event.location()) {
      (Err(error), Some(at)) => Err(error.or_at(at.clone())),
      (handled, _) => handled,
    }
  }
}
