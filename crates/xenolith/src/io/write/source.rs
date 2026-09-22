//! An application-driven [`EventSource`].
//!
//! [`WriterSource`] converts StAX-style write API calls into events. How these events are processed depends on the
//! composed handlers; for example, [`XmlWriter`](crate::io::write::XmlWriter) writes them out as XML text, while
//! [`DomBuilder`](crate::dom::build::DomBuilder) constructs a tree from them.

#[cfg(test)]
mod test;

use crate::attr::{AttributeList, AttributeRef, Attributes};
use crate::chars;
use crate::dtd::model::Dtd;
use crate::error::{Error, Location, Result};
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, Dispatch, DoctypeEventRef, EndElementEventRef, EventHandler,
  EventRef, EventSource, Outcome, ProcessingInstructionEventRef, StartElementEventRef, XmlSpace,
};
use crate::name::NamePool;

/// The element name and namespace specified by the caller.
#[derive(Debug)]
struct WrittenName {
  prefix: Option<String>,
  local: String,
  namespace: Option<String>,
}

/// One of the attributes of the start tag being written.
#[derive(Debug)]
struct WrittenAttribute {
  name: WrittenName,
  value: String,
}

/// The attributes of the start tag being written (`AttributeList`). This allows the event to hold attributes in a
/// format similar to how the parser handles them.
#[derive(Debug, Default)]
struct WrittenAttributes(Vec<WrittenAttribute>);

impl AttributeList for WrittenAttributes {
  fn len(&self) -> usize {
    self.0.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    let attribute = self.0.get(index)?;
    Some(AttributeRef {
      prefix: attribute.name.prefix.as_deref(),
      local: &attribute.name.local,
      namespace: attribute.name.namespace.as_deref(),
      value: &attribute.value,
      // Nothing was read from a document, so there is no position to report.
      location: Location::unknown(),
      value_location: Location::unknown(),
    })
  }
}

/// Where a [`WriterSource`] has reached. Once the run is over, how it ended decides what a later write does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
  /// Nothing written yet; the first write reports [`StartDocument`](EventRef::StartDocument).
  Before,
  /// The document is being written.
  Writing,
  /// Every handler finished early. Nothing is listening, which is not the caller's mistake, so a later write is
  /// ignored and returns `Ok`.
  Stopped,
  /// [`end_document`](WriterSource::end_document) closed the document, so a later write is the caller's mistake.
  Ended,
  /// A handler refused an event, and that call has already returned the error. A later write is refused too, rather
  /// than silently accepted.
  Failed,
}

/// A component that allows an application to perform write operations as an [`EventSource`]. It corresponds to the
/// `XMLStreamWriter` API in StAX.
///
/// Each method call triggers a notification of the corresponding event to the registered handler. The data written by
/// the application is passed to the handler for processing, such as outputting XML text, building a tree, performing
/// schema validation, or executing multiple such operations concurrently via [`Dispatch`]. This source merely
/// generates application-driven events; it does not perform output operations itself.
///
/// This is a push-based mechanism. Processing begins with the initial write or a call to
/// [`start_document`](Self::start_document) and concludes with [`end_document`](Self::end_document). When
/// [`end_document`](Self::end_document) is called, the [`EndDocument`](EventRef::EndDocument) event is signaled, and
/// the handler is notified that processing has [`Completed`](crate::event::Outcome::Completed). Conversely, if the
/// source is dropped without this call being made, the handler is notified that processing has been
/// [`Abandoned`](crate::event::Outcome::Abandoned).
///
/// # Caller's responsibility
///
/// The start tag is buffered until all attributes are collected, and all attributes are conveyed in a single
/// [`StartElement`](EventRef::StartElement) event. A side effect of this is that the timing of write rejections is
/// affected; specifically, attributes that a handler cannot accept are reported not at the call that wrote the
/// attribute itself, but at the call that finalizes the start tag.
///
/// Namespace management is the caller's responsibility. When specifying a namespace via
/// [`write_start_element_ns`](Self::write_start_element_ns), any required `xmlns` declarations must be written as
/// standard attributes. This event source does not perform checks such as whether names are valid `Name`s, whether
/// tags are properly balanced, whether prefixes are declared, or whether there is a single root element. Xenolith
/// adheres to a principle of separation of concerns where validation is performed by a validator, and the event source
/// performs only the minimum validation necessary for its operation. Handlers requiring such checks must place a
/// validator, such as [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator), upstream in the processing
/// pipeline.
///
/// You cannot perform write operations after the run has finished. Attempting to write to the source after calling
/// [`end_document`](Self::end_document) or when an error has already occurred will result in an error. Conversely, if
/// all handlers have terminated early, it will continue to return `Ok` without reporting anything.
///
/// Calls not permitted by the API, such as specifying attributes when no start tag is open, or using an end element
/// when no element is currently open, will also result in an error.
///
/// A written document has no source position, so every [`Location`] is [`unknown`](Location::unknown). A start element
/// reports [`XmlSpace::default`], no language, and no base URI.
///
/// # Examples
///
/// Writing a document straight out as XML text:
///
/// ```
/// use xenolith::event::EventSource;
/// use xenolith::io::write::{WriterSource, XmlWriter};
///
/// let mut writer = XmlWriter::new(Vec::new());
/// {
///   let mut doc = WriterSource::new().with_handler(&mut writer);
///   doc.write_start_element("greeting")?;
///   doc.write_attribute("xml:lang", "en")?;
///   doc.write_characters("Hello & welcome")?;
///   doc.write_end_element()?;
///   doc.end_document()?;
/// }
/// let out = String::from_utf8(writer.into_inner()).unwrap();
/// assert_eq!(out, "<greeting xml:lang=\"en\">Hello &amp; welcome</greeting>");
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// The same calls building a tree instead, because the handler is what decides where the events go:
///
/// ```
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::event::EventSource;
/// use xenolith::io::write::WriterSource;
///
/// let mut builder = DomBuilder::new();
/// {
///   let mut doc = WriterSource::new().with_handler(&mut builder);
///   doc.write_start_element_ns(Some("urn:example"), "e:note")?;
///   doc.write_attribute("xmlns:e", "urn:example")?;
///   doc.write_characters("hi")?;
///   doc.write_end_element()?;
///   doc.end_document()?;
/// }
/// let doc = builder.into_document();
/// let root = doc.document_element().unwrap();
/// assert_eq!(doc.node_name(root), "e:note");
/// assert_eq!(doc.namespace_uri(root), Some("urn:example"));
/// assert_eq!(doc.text_content(root), "hi");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct WriterSource<'h> {
  /// The handlers this source was built with, which every event reaches. Its own [`Drop`] is what tells them the run
  /// was [`Abandoned`](Outcome::Abandoned) when this source is dropped part way, so there is no `Drop` here.
  dispatch: Dispatch<'h>,
  /// The elements currently open, outermost first, so an end element reports the name its start element did.
  open: Vec<WrittenName>,
  /// The start tag whose attributes are still being collected, held until the first content or the end.
  pending: Option<WrittenName>,
  /// The attributes of that start tag, so the event carries them all at once.
  attributes: WrittenAttributes,
  /// Where the run has reached.
  step: Step,
}

impl Default for WriterSource<'_> {
  fn default() -> Self {
    Self::new()
  }
}

impl<'h> WriterSource<'h> {
  /// Creates a source with no handlers.
  #[must_use]
  pub fn new() -> Self {
    Self {
      dispatch: Dispatch::new(),
      open: Vec::new(),
      pending: None,
      attributes: WrittenAttributes::default(),
      step: Step::Before,
    }
  }

  /// The number of elements currently open.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len() + usize::from(self.pending.is_some())
  }

  /// Emits the [`StartDocument`](EventRef::StartDocument) event to declare the start of document writing.
  ///
  /// This occurs automatically upon the first write operation, so application only need to call this method explicitly
  /// when initiating an execution that does not involve writing anything. Calling this method multiple times while a
  /// write operation is already in progress has no effect.
  ///
  /// # Errors
  ///
  /// Returns the value returned by the event handlers (such as when execution terminates with a "reject" status), or
  /// an error if execution has already concluded.
  pub fn start_document(&mut self) -> Result<()> {
    if !self.taking_writes()? || self.step != Step::Before {
      return Ok(());
    }
    self.step = Step::Writing;
    self.report(&EventRef::StartDocument)
  }

  /// Emits the [`Doctype`](EventRef::Doctype) event for a document type declaration named `name`. This event carries a
  /// `dtd` object and a [`NamePool`] in which each name within that DTD is interned.
  ///
  /// Even when [`Dtd::default`](crate::dtd::model::Dtd::default) is used, the declaration itself is still emitted;
  /// however, the object passed to handlers that consume the declaration, such as DTD validators or tree builders that
  /// mark `ID` attributes, is an empty DTD.
  ///
  /// # Errors
  ///
  /// Returns the value returned by the event handlers (such as when execution terminates with a "reject" status), or
  /// an error if execution has already concluded.
  pub fn write_doctype(
    &mut self,
    name: &str,
    public_id: Option<&str>,
    system_id: Option<&str>,
    dtd: &Dtd,
    pool: &NamePool,
  ) -> Result<()> {
    self.start_document()?;
    if !self.taking_writes()? {
      return Ok(());
    }
    let event =
      EventRef::Doctype(DoctypeEventRef::new(Some(name), public_id, system_id, dtd, pool, Location::unknown()));
    self.report(&event)
  }

  /// Opens an element that has a qualified name but no namespace. Attributes may follow until the next content or the
  /// end of the element.
  ///
  /// # Errors
  ///
  /// A handler for the element start event returned an error, or execution has already finished.
  pub fn write_start_element(&mut self, qualified_name: &str) -> Result<()> {
    self.write_start_element_ns(None, qualified_name)
  }

  /// Opens an element within the `namespace`; the caller declares the prefix for the qualified name.
  ///
  /// The namespace is passed to the handler as part of the event, the corresponding `xmlns` declaration must be
  /// specified as an attribute for it to be reflected in the output.
  ///
  /// # Errors
  ///
  /// As [`write_start_element`](Self::write_start_element).
  ///
  pub fn write_start_element_ns(&mut self, namespace: Option<&str>, qualified_name: &str) -> Result<()> {
    self.flush_start()?;
    if !self.taking_writes()? {
      return Ok(());
    }
    self.pending = Some(name_of(qualified_name, namespace));
    Ok(())
  }

  /// Writes an attribute that does not have a namespace to the element that has been opened.
  ///
  /// # Errors
  ///
  /// When the element to which the attribute belongs has not started, or when its scope has already ended. Note that
  /// no notification is sent to the handler in this case; consequently, any rejection by the handler would occur at
  /// the stage of the call that terminates the start tag.
  pub fn write_attribute(&mut self, qualified_name: &str, value: &str) -> Result<()> {
    self.write_attribute_ns(None, qualified_name, value)
  }

  /// Writes an attribute within the `namespace` to the element that has been opened.
  ///
  /// # Errors
  ///
  /// As [`write_attribute`](Self::write_attribute).
  pub fn write_attribute_ns(&mut self, namespace: Option<&str>, qualified_name: &str, value: &str) -> Result<()> {
    if !self.taking_writes()? {
      return Ok(());
    }
    if self.pending.is_none() {
      return Err(Error::misuse("write_attribute must follow write_start_element, before any content"));
    }
    self.attributes.0.push(WrittenAttribute { name: name_of(qualified_name, namespace), value: value.to_owned() });
    Ok(())
  }

  /// Emits character data as a [`Characters`](EventRef::Characters) event.
  ///
  /// # Errors
  ///
  /// If the event handler returns an error, that value is returned, such as when execution completes in a rejected
  /// state. Additionally, an error is returned if execution has already finished.
  pub fn write_characters(&mut self, text: &str) -> Result<()> {
    self.content(&EventRef::Characters(CharactersEventRef::new(text, Location::unknown())))
  }

  /// Emits a CDATA section as a [`Cdata`](EventRef::Cdata) event.
  ///
  /// # Errors
  ///
  /// As [`write_characters`](Self::write_characters).
  pub fn write_cdata(&mut self, text: &str) -> Result<()> {
    self.content(&EventRef::Cdata(CdataEventRef::new(text, Location::unknown())))
  }

  /// Emits a comment as a [`Comment`](EventRef::Comment) event, `text` being what lies between `<!--` and `-->`.
  ///
  /// # Errors
  ///
  /// As [`write_characters`](Self::write_characters).
  pub fn write_comment(&mut self, text: &str) -> Result<()> {
    self.content(&EventRef::Comment(CommentEventRef::new(text, Location::unknown())))
  }

  /// Emits a processing instruction as a [`ProcessingInstruction`](EventRef::ProcessingInstruction) event.
  ///
  /// # Errors
  ///
  /// As [`write_characters`](Self::write_characters).
  pub fn write_processing_instruction(&mut self, target: &str, data: &str) -> Result<()> {
    let event = ProcessingInstructionEventRef::new(target, data, Location::unknown(), Location::unknown());
    self.content(&EventRef::ProcessingInstruction(event))
  }

  /// Closes the innermost open element and emits an [`EndElement`](EventRef::EndElement) event with the name of that
  /// start element.
  ///
  /// # Errors
  ///
  /// When an error is returned by a handler for this event, or for the start element that this event terminates, when
  /// the element is not in an open state, or when processing has already concluded.
  pub fn write_end_element(&mut self) -> Result<()> {
    self.flush_start()?;
    if !self.taking_writes()? {
      return Ok(());
    }
    let Some(name) = self.open.pop() else {
      return Err(Error::misuse("write_end_element with no element open"));
    };
    let event = EventRef::EndElement(EndElementEventRef::new(
      name.prefix.as_deref(),
      &name.local,
      name.namespace.as_deref(),
      Location::unknown(),
    ));
    self.report(&event)
  }

  /// Emits the [`EndDocument`](EventRef::EndDocument) event to notify the handler that execution has
  /// [`Completed`](crate::event::Outcome::Completed).
  ///
  /// Writing to the document is complete, so any subsequent write operations will result in an error.
  ///
  /// # Errors
  ///
  /// An error returned by the handler for this event, or an indication of whether the start element remains open, or
  /// writing has already finished.
  pub fn end_document(&mut self) -> Result<()> {
    self.start_document()?;
    self.flush_start()?;
    if !self.taking_writes()? {
      return Ok(());
    }
    // Not `report`: that would end the run as stopped when a handler finishes at this event, and a handler that
    // finished here has accepted `EndDocument`, which is what completes a run. The document was written in full.
    if let Err(error) = self.dispatch.handle(&EventRef::EndDocument) {
      self.step = Step::Failed;
      return Err(self.dispatch.fail(error));
    }
    self.step = Step::Ended;
    self.dispatch.finish(Outcome::Completed);
    Ok(())
  }

  /// Emits one content event within the start tag.
  fn content(&mut self, event: &EventRef<'_>) -> Result<()> {
    self.flush_start()?;
    if !self.taking_writes()? {
      return Ok(());
    }
    self.report(event)
  }

  /// Emits a pending start tag if there is one, and begin the element's content.
  fn flush_start(&mut self) -> Result<()> {
    self.start_document()?;
    let Some(name) = self.pending.take() else { return Ok(()) };
    if !self.taking_writes()? {
      return Ok(());
    }
    let outcome = {
      let event = EventRef::StartElement(StartElementEventRef::new(
        name.prefix.as_deref(),
        &name.local,
        name.namespace.as_deref(),
        Attributes::new(&self.attributes),
        XmlSpace::default(),
        None,
        None,
        Location::unknown(),
      ));
      self.dispatch.handle(&event)
    };
    self.attributes.0.clear();
    self.open.push(name);
    self.settle(outcome)
  }

  /// Dispatches the `event` to the handlers. Execution terminates if any handler rejects it or if all handlers have
  /// finished processing.
  fn report(&mut self, event: &EventRef<'_>) -> Result<()> {
    let outcome = self.dispatch.handle(event);
    self.settle(outcome)
  }

  /// Finalizes the handler's decision regarding the event. A refusal results in the run ending as a failure, while the
  /// run ends as a stoppage if all handlers have finished. No further reports are issued for a run that has concluded.
  fn settle(&mut self, outcome: Result<()>) -> Result<()> {
    if let Err(error) = outcome {
      self.step = Step::Failed;
      return Err(self.dispatch.fail(error));
    }
    if !self.dispatch.should_continue() {
      self.step = Step::Stopped;
      self.dispatch.finish(Outcome::Stopped);
    }
    Ok(())
  }

  /// Indicates whether the current execution still accepts writes. Specifically, it rejects writes that arrive after
  /// the document has terminated or after a handler has rejected the event. If all handlers terminate early,
  /// `Ok(false)` is returned; in this case, writes are ignored because there is no recipient to accept them.
  fn taking_writes(&self) -> Result<bool> {
    match self.step {
      Step::Before | Step::Writing => Ok(true),
      Step::Stopped => Ok(false),
      Step::Ended => Err(Error::misuse("the document has ended; end_document was already called")),
      Step::Failed => Err(Error::misuse("the run has ended; a handler refused an event and it was reported then")),
    }
  }
}

impl<'h> EventSource<'h> for WriterSource<'h> {
  fn with_handler(mut self, handler: &'h mut dyn EventHandler) -> Self {
    self.dispatch = self.dispatch.with_handler(handler);
    self
  }
}

/// Splits the qualified name into the prefix and local part contained in the event; however, names that do not contain
/// a colon (or where the colon does not function as a separator) are not split and are kept as-is.
fn name_of(qualified_name: &str, namespace: Option<&str>) -> WrittenName {
  let (prefix, local) = match chars::split_qname(qualified_name) {
    Some((prefix, local)) => (prefix, local),
    None => (None, qualified_name),
  };
  WrittenName {
    prefix: prefix.map(ToOwned::to_owned),
    local: local.to_owned(),
    namespace: namespace.map(ToOwned::to_owned),
  }
}
