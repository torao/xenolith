//! A streaming XML writer that outputs received events sequentially.

#[cfg(test)]
mod test;

use std::io;

use crate::error::{Error, Result};
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, DoctypeEventRef, EndElementEventRef, EventHandler, EventRef,
  ProcessingInstructionEventRef, StartElementEventRef,
};
use crate::io::encoding::{Encoder, Utf8Encoder, encoder_for};
use crate::name::lexical;

use crate::io::write::escape::{push_attribute, push_cdata, push_text};

/// Writes received events as XML text to an [`io::Write`] destination.
///
/// As this implements [`EventHandler`], it can accept input from any source that generates document events. Since it
/// writes on an event-by-event basis, it generates output without needing to hold the entire document in memory. It
/// also employs shorthand notation (e.g., `<a/>`) when an element has no content between its start and end tags, and
/// performs appropriate escaping when writing text or attribute values.
///
/// This component sits at the end of the processing chain; it performs the writing operation but does not pass any
/// notifications to subsequent stages. By placing a handler such as
/// [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator), which validates events, before this writer,
/// the application can detect invalid XML and halt processing before any output is written.
///
/// # Principle of separation of responsibilities
///
/// The [`XmlWriter`] outputs most information exactly as it appears in the input events. Validation tasks should
/// instead be handled by validators placed upstream of the `XmlWriter`, such as
/// [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator) or
/// [`ValidatorSet`](crate::event::validate::ValidatorSet). This ensures that invalid XML is rejected before any actual
/// byte data is written.
///
/// # Encoding
/// <a name="encoding"></a>
///
/// The default output encoding is UTF-8. Applications can specify a different encoding using
/// [`with_encoding`](Self::with_encoding). If the application wish to use an encoding name in the XML declaration that
/// differs from the actual encoding used, it can specify the encoding via
/// [`with_declared_encoding`](Self::with_declared_encoding).
///
/// Characters that cannot be represented in the output encoding are converted into character references when they
/// appear in character data or attribute values (a character reference is a markup construct interpreted as the
/// original character upon reading). In other contexts, character references are not treated as markup, so the use of
/// such characters is rejected.
///
/// UTF-16 output is not supported. This is because the WHATWG Encoding Standard does not provide a UTF-16 encoder. If
/// UTF-16 output is required, first declare `UTF-16` using [`with_declared_encoding`](Self::with_declared_encoding)
/// and write the document in UTF-8; then, convert the resulting byte sequence to UTF-16 by prepending a Byte Order
/// Mark (BOM) and save the result. The final example below demonstrates this procedure.
///
/// # Examples
///
/// When generating output from an application using the StAX-style:
///
/// ```
/// use xenolith::event::EventSource;
/// use xenolith::io::write::{WriterSource, XmlWriter};
///
/// let mut w = XmlWriter::new(Vec::new());
/// {
///   let mut doc = WriterSource::new().with_handler(&mut w);
///   doc.write_start_element("greeting")?;
///   doc.write_attribute("xml:lang", "en")?;
///   doc.write_characters("Hello & welcome")?;
///   doc.write_end_element()?;
///   doc.end_document()?;
/// }
/// let out = String::from_utf8(w.into_inner()).unwrap();
/// assert_eq!(out, "<greeting xml:lang=\"en\">Hello &amp; welcome</greeting>");
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// Outputting an existing document:
///
/// ```
/// use xenolith::event::{EventCursor, EventSource};
/// use xenolith::dom::DomSource;
/// use xenolith::dom::build::DomBuilder;
/// use xenolith::io::StreamSource;
/// use xenolith::io::write::XmlWriter;
///
/// let mut builder = DomBuilder::new();
/// StreamSource::new("<a x='1'><b>t &amp; u</b></a>".as_bytes()).with_handler(&mut builder).emit()?;
/// let doc = builder.into_document();
///
/// let mut writer = XmlWriter::new(Vec::new());
/// DomSource::new(&doc).with_handler(&mut writer).emit()?;
/// assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<a x=\"1\"><b>t &amp; u</b></a>");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Writing in one encoding while declaring a different encoding for readers that recognize the encoding by a different
/// name:
///
/// ```
/// # #[cfg(feature = "encodings")] {
/// use xenolith::event::EventSource;
/// use xenolith::io::write::{WriterSource, XmlWriter};
///
/// let mut w = XmlWriter::new(Vec::new()).with_encoding("windows-31j")?;
/// let mut w = w.with_declared_encoding("Shift_JIS").with_declaration(None);
/// {
///   let mut doc = WriterSource::new().with_handler(&mut w);
///   doc.write_start_element("a")?;
///   doc.write_end_element()?;
///   doc.end_document()?;
/// }
/// let out = w.into_inner();
/// assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?>"));
/// # }
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// Producing UTF-16, which has no encoder of its own: write UTF-8, declare `UTF-16`, then convert the whole document.
///
/// ```
/// use xenolith::event::EventSource;
/// use xenolith::io::write::{WriterSource, XmlWriter};
///
/// let mut w = XmlWriter::new(Vec::new()).with_declared_encoding("UTF-16").with_declaration(None);
/// {
///   let mut doc = WriterSource::new().with_handler(&mut w);
///   doc.write_start_element("greeting")?;
///   doc.write_characters("hi")?;
///   doc.write_end_element()?;
///   doc.end_document()?;
/// }
/// let utf8 = w.into_inner();
/// assert!(utf8.starts_with(b"<?xml version=\"1.0\" encoding=\"UTF-16\"?>"));
///
/// // Convert the whole document to UTF-16LE, opening it with the byte order mark.
/// let text = std::str::from_utf8(&utf8).unwrap();
/// let mut out = vec![0xFF, 0xFE];
/// for unit in text.encode_utf16() {
///   out.extend_from_slice(&unit.to_le_bytes());
/// }
/// assert!(out.starts_with(&[0xFF, 0xFE]));
/// # Ok::<(), xenolith::Error>(())
/// ```
///
#[derive(Debug)]
pub struct XmlWriter<W> {
  out: W,
  /// Turns what has been escaped into the bytes that leave.
  encoder: Box<dyn Encoder>,
  /// The name the XML declaration gives, when it differs from the encoder's own.
  declared: Option<String>,
  /// The `standalone` value of the XML declaration to write when the document starts, or `None` to write none.
  declaration: Option<Option<bool>>,

  /// The names of the elements currently open, outermost first.
  open: Vec<String>,
  /// Whether a start tag is open and still accepting attributes (its `>` unwritten).
  pending: bool,
  /// Reused buffer for escaping, so a write does not allocate. A write takes it out with `mem::take`, which leaves a
  /// `String` of no capacity behind, and puts it back when it is done: that is what returns the allocation, since the
  /// `clear` before the next use keeps only the capacity of what the field holds.
  scratch: String,
  /// Reused buffer for the encoded bytes, for the same reason.
  bytes: Vec<u8>,
}

impl<W: io::Write> XmlWriter<W> {
  /// Create a writer on `out`.
  ///
  pub fn new(out: W) -> Self {
    Self {
      out,
      encoder: Box::new(Utf8Encoder::new()),
      declared: None,
      declaration: None,
      open: Vec::new(),
      pending: false,
      scratch: String::new(),
      bytes: Vec::new(),
    }
  }

  /// Writes the actual output using the specified `label` (encoding name).
  ///
  /// This `label` is also used for the encoding name within the XML declaration, unless a different name is specified
  /// via [`with_declared_encoding`](Self::with_declared_encoding).
  ///
  /// A character that cannot be represented in the chosen encoding is converted into a character reference when it
  /// appears in a character data or an attribute value. In other contexts, such as names, comments, processing
  /// instructions, or CDATA sections, writing is rejected because character references cannot be treated as markup
  /// there.
  ///
  /// # Errors
  ///
  /// If a `label` is specified for which no corresponding encoding exists, or for which writing is not supported in
  /// the current build, an [`Error::Encoding`](crate::error::Error::Encoding) occurs. UTF-16 is one such encoding;
  /// while it can be read, it cannot be written. Refer to the [Encoding](#encoding) section for information on how to
  /// output in UTF-16 despite this limitation.
  pub fn with_encoding(mut self, label: &str) -> Result<Self> {
    self.encoder = encoder_for(label)?;
    if self.declared.is_none() {
      self.declared = Some(label.to_owned());
    }
    Ok(self)
  }

  /// Sets the encoding name in the XML declaration to a label that differs from the actual output encoding.
  ///
  /// These two are usually identical, and using [`with_encoding`](Self::with_encoding) maintains this consistency. A
  /// scenario requiring different values is when the reader expects a name different from the one used by the writer
  /// (e.g., treating output written as `windows-31j` as `Shift_JIS`).
  ///
  /// No validation is performed to ensure these two names correspond correctly. Specifying a name that does not match
  /// the actual byte sequence may render the document unreadable. This feature is intended for cases where the caller
  /// knows the name expected by the reader but cannot use it directly due to technical limitations.
  #[must_use]
  pub fn with_declared_encoding(mut self, label: &str) -> Self {
    self.declared = Some(label.to_owned());
    self
  }

  /// Configure the output to include an XML declaration at the beginning of the document. If a value is specified for
  /// the `standalone` attribute, that value is also included in the output. Since the XML declaration is not part of
  /// the document content, it is not reported from the source. If this method is not called, the output begins with
  /// the first markup.
  #[must_use]
  pub fn with_declaration(mut self, standalone: Option<bool>) -> Self {
    self.declaration = Some(standalone);
    self
  }

  /// Returns the underlying writer.
  ///
  /// This call disposes of the instance. The caller is responsible for ensuring the consistency of the start and end
  /// operations. The written content is returned in its current state, regardless of whether the operation is complete.
  pub fn into_inner(self) -> W {
    self.out
  }

  /// Outputs `text` as-is, rejecting any parts that cannot be represented in the encoding.
  fn put(&mut self, text: &str) -> Result<()> {
    self.bytes.clear();
    self.encoder.encode(text, &mut self.bytes)?;
    self.out.write_all(&self.bytes)?;
    Ok(())
  }

  /// Outputs `text` to the output, writing character references for characters that cannot be represented in the
  /// encoding.
  fn put_with_references(&mut self, text: &str) -> Result<()> {
    self.bytes.clear();
    self.encoder.encode_with_references(text, &mut self.bytes)?;
    self.out.write_all(&self.bytes)?;
    Ok(())
  }

  /// Outputs ASCII markup, which every encoding this writes holds.
  fn put_markup(&mut self, markup: &str) -> Result<()> {
    self.put(markup)
  }

  /// Writes the content output at the beginning of the document, including the XML declaration, if required.
  fn start_document(&mut self) -> Result<()> {
    // Resets the internal execution state.
    self.open.clear();
    self.pending = false;

    match self.declaration {
      Some(standalone) => self.xml_declaration(standalone),
      None => Ok(()),
    }
  }

  /// Writes `<?xml version="1.0" encoding="..."?>`. Optionally includes the `standalone` attribute.
  fn xml_declaration(&mut self, standalone: Option<bool>) -> Result<()> {
    let declared = self.declared.clone().unwrap_or_else(|| self.encoder.encoding().to_owned());
    self.put_markup("<?xml version=\"1.0\" encoding=\"")?;
    self.put(&declared)?;
    self.put_markup("\"")?;
    if let Some(standalone) = standalone {
      self.put_markup(if standalone { " standalone=\"yes\"" } else { " standalone=\"no\"" })?;
    }
    self.put_markup("?>")
  }

  /// Writes a document type declaration, which a source reports once, before the root element.
  fn doctype(&mut self, event: &DoctypeEventRef<'_>) -> Result<()> {
    let Some(name) = event.name else { return Ok(()) };
    if !self.open.is_empty() {
      return Err(Error::well_formedness("a document type declaration may not appear inside an element"));
    }
    self.put_markup("<!DOCTYPE ")?;
    self.put(name)?;
    match (event.public_id, event.system_id) {
      (Some(public), Some(system)) => {
        self.put_markup(" PUBLIC \"")?;
        self.put(public)?;
        self.put_markup("\" \"")?;
        self.put(system)?;
        self.put_markup("\"")?;
      }
      (None, Some(system)) => {
        self.put_markup(" SYSTEM \"")?;
        self.put(system)?;
        self.put_markup("\"")?;
      }
      _ => {}
    }
    self.put_markup(">")
  }

  /// Closes the innermost open element. This results in `</name>` or, if there is no content yet, `/>`.
  fn end_element(&mut self, event: &EndElementEventRef<'_>) -> Result<()> {
    let Some(name) = self.open.pop() else {
      let ended = lexical(event.prefix, event.local);
      return Err(Error::well_formedness(format!("an end element </{ended}> with no element open")));
    };
    if !names(&name, event.prefix, event.local) {
      // Only the message needs the name the event carries, so it is built here and nowhere else.
      let ended = lexical(event.prefix, event.local);
      return Err(Error::well_formedness(format!("mismatched end tag: expected </{name}>, found </{ended}>")));
    }
    if self.pending {
      // Nothing was written inside: collapse to an empty element.
      self.pending = false;
      self.put_markup("/>")
    } else {
      self.put_markup("</")?;
      self.put(&name)?;
      self.put_markup(">")
    }
  }

  /// Writes character data that has undergone escape processing.
  fn characters(&mut self, event: &CharactersEventRef<'_>) -> Result<()> {
    self.close_start_tag()?;
    self.scratch.clear();
    push_text(&mut self.scratch, event.text);
    let escaped = std::mem::take(&mut self.scratch);
    let written = self.put_with_references(&escaped);
    self.scratch = escaped;
    written
  }

  /// Writes a CDATA section. If the content contains `]]>`, it is split to prevent the section from being closed
  /// prematurely.
  fn cdata(&mut self, event: &CdataEventRef<'_>) -> Result<()> {
    self.close_start_tag()?;
    self.scratch.clear();
    push_cdata(&mut self.scratch, event.text);
    let escaped = std::mem::take(&mut self.scratch);
    // A CDATA section admits no reference, so what the encoding cannot hold is refused rather than rewritten.
    let written = self.put(&escaped);
    self.scratch = escaped;
    written
  }

  /// Writes a comment by placing `text` directly between `<!--` and `-->`.
  fn comment(&mut self, event: &CommentEventRef<'_>) -> Result<()> {
    self.close_start_tag()?;
    self.put_markup("<!--")?;
    self.put(event.text)?;
    self.put_markup("-->")
  }

  /// Writes a processing instructions using `target` and `data` as they are.
  fn processing_instruction(&mut self, event: &ProcessingInstructionEventRef<'_>) -> Result<()> {
    self.close_start_tag()?;
    self.put_markup("<?")?;
    self.put(event.target)?;
    if !event.data.is_empty() {
      self.put_markup(" ")?;
      self.put(event.data)?;
    }
    self.put_markup("?>")
  }

  /// Closes an open start tag with `>` if one is pending.
  ///
  fn close_start_tag(&mut self) -> Result<()> {
    if self.pending {
      self.pending = false;
      self.put_markup(">")?;
    }
    Ok(())
  }

  /// Outputs the name in lexical form (`prefix:local` or `local`) instead of converting it to a string.
  fn put_name(&mut self, prefix: Option<&str>, local: &str) -> Result<()> {
    if let Some(prefix) = prefix {
      self.put(prefix)?;
      self.put_markup(":")?;
    }
    self.put(local)
  }

  /// Opens an element, and attributes are written in the order reported by the event. By leaving the start tag open,
  /// elements with no content can be written using the abbreviated syntax (empty-element tag), such as `<a/>`.
  fn start_element(&mut self, event: &StartElementEventRef<'_>) -> Result<()> {
    self.close_start_tag()?;
    self.put_markup("<")?;
    self.put_name(event.prefix, event.local)?;
    // The name is kept for the end tag, which is the one name this writer has to hold on to.
    self.open.push(lexical(event.prefix, event.local));
    self.pending = true;
    for attribute in event.attributes.iter() {
      self.put_markup(" ")?;
      self.put_name(attribute.prefix, attribute.local)?;
      self.put_markup("=\"")?;
      self.scratch.clear();
      push_attribute(&mut self.scratch, attribute.value);
      let escaped = std::mem::take(&mut self.scratch);
      let written = self.put_with_references(&escaped);
      self.scratch = escaped;
      written?;
      self.put_markup("\"")?;
    }
    Ok(())
  }
}

/// A writer acting as the output destination for events. It writes out each event as it arrives. It functions as a
/// write target for any [`EventSource`](crate::event::EventSource).
///
/// # Errors
///
/// In addition to errors returned by the underlying writer, a [`WellFormedness`](Error::WellFormedness) error occurs
/// if an event cannot be written at the current position. Specific examples include encountering an end element when
/// no element is open, encountering an end element with a name different from that of the innermost element, or
/// including a DOCTYPE declaration inside an element. Since these operations are rejected at the moment the write is
/// attempted, the offending data is never output as bytes.
impl<W: io::Write> EventHandler for XmlWriter<W> {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    match event {
      // A document's beginning and end are not markup. What the start puts out is the declaration, which no event
      // carries, and the end puts out nothing at all.
      EventRef::StartDocument => self.start_document()?,
      EventRef::EndDocument => {}
      EventRef::StartElement(event) => self.start_element(event)?,
      EventRef::EndElement(event) => self.end_element(event)?,
      EventRef::Characters(event) => self.characters(event)?,
      EventRef::Cdata(event) => self.cdata(event)?,
      EventRef::Comment(event) => self.comment(event)?,
      EventRef::ProcessingInstruction(event) => self.processing_instruction(event)?,
      EventRef::Doctype(event) => self.doctype(event)?,
    }
    Ok(())
  }
}

/// Compare whether the lexical representation `name` matches the name composed of `prefix` and `local`, without
/// actually constructing that form.
fn names(name: &str, prefix: Option<&str>, local: &str) -> bool {
  match prefix {
    Some(prefix) => name.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(':')).is_some_and(|rest| rest == local),
    None => name == local,
  }
}
