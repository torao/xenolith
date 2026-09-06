//! A streaming XML writer equivalent to StAX's `XMLStreamWriter`.
//!
//! The contract a caller needs is on [`XmlWriter`] itself. What is here is for whoever changes this file: the writer
//! holds a start tag open until something forces it shut, which is what lets an attribute follow the element it
//! belongs to and what lets an element with nothing inside collapse to `<a/>`. Every method that writes content calls
//! [`close_start_tag`](XmlWriter::close_start_tag) first, so that rule lives in one place.
//!

use std::io;

use xenolith_core::encoding::{Encoder, Utf8Encoder, encoder_for};
use xenolith_core::error::Result;

use crate::escape::{push_attribute, push_cdata, push_text};

/// Writes XML to an [`io::Write`] from a sequence of calls.
///
/// It is driven call by call, so it produces output without ever holding the whole document. It tracks open elements,
/// so one that starts and ends with nothing between collapses to `<a/>`, and it escapes text and attribute values as
/// it writes them. The [`Serializer`](crate::Serializer) takes a different shape: it walks a tree that already exists.
///
/// The caller manages namespaces. Write an `xmlns` declaration as an ordinary attribute.
///
/// # What is not checked
///
/// An element or attribute name, a comment's text, and a processing instruction's data reach the output as they were
/// given, so a name that is not a `Name`, a comment holding `--`, or data holding `?>` all produce output that will
/// not parse. Text, attribute values, and CDATA sections are the parts the writer rewrites to keep their meaning.
/// Check whatever comes from outside before writing it.
///
/// To have the output checked against a schema as it is written, use `ValidatingWriter`, which the `validate` feature
/// provides. It wraps this writer, holds each start tag back until its attributes are in, and refuses a write the
/// schema forbids rather than emitting it. It checks the schema, so the names, comments, and processing instructions
/// above are still the caller's to get right.
///
/// # Encoding
///
/// UTF-8 unless [`with_encoding`](Self::with_encoding) asks for another, and the XML declaration gives whichever it
/// is. [`with_declared_encoding`](Self::with_declared_encoding) separates the two where a reader knows the encoding
/// by a different name.
///
/// A character the encoding cannot hold becomes a character reference in character data and in an attribute value,
/// where a reference is markup that reads back as the character. Anywhere else it is refused, because no reference is
/// markup there.
///
/// UTF-16 output isn't available, although a decoder can read a UTF-16 entity because it's built in. The WHATWG
/// Encoding Standard defines no UTF-16 encoder, and this writer follows it. To produce UTF-16, write the document in
/// UTF-8, declare `UTF-16` with [`with_declared_encoding`](Self::with_declared_encoding), and convert the finished
/// bytes, opening them with a byte order mark. The last example below does this.
///
/// # Examples
///
/// ```
/// use xenolith_serialize::XmlWriter;
///
/// let mut w = XmlWriter::new(Vec::new());
/// w.write_start_element("greeting")?;
/// w.write_attribute("xml:lang", "en")?;
/// w.write_characters("Hello & welcome")?;
/// w.write_end_element()?;
///
/// let out = String::from_utf8(w.into_inner()).unwrap();
/// assert_eq!(out, "<greeting xml:lang=\"en\">Hello &amp; welcome</greeting>");
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// Writing one encoding and declaring another, for a reader that knows it by that name:
///
/// ```
/// # #[cfg(feature = "encodings")] {
/// use xenolith_serialize::XmlWriter;
///
/// let mut w = XmlWriter::new(Vec::new()).with_encoding("windows-31j")?.with_declared_encoding("Shift_JIS");
/// w.write_declaration(None)?;
/// w.write_start_element("a")?;
/// w.write_end_element()?;
///
/// let out = w.into_inner();
/// assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?>"));
/// # }
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// Producing UTF-16, which has no encoder of its own: write UTF-8, declare `UTF-16`, then convert the whole document.
///
/// ```
/// use xenolith_serialize::XmlWriter;
///
/// let mut w = XmlWriter::new(Vec::new()).with_declared_encoding("UTF-16");
/// w.write_declaration(None)?;
/// w.write_start_element("greeting")?;
/// w.write_characters("hi")?;
/// w.write_end_element()?;
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
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
#[derive(Debug)]
pub struct XmlWriter<W> {
  out: W,
  /// Turns what has been escaped into the bytes that leave.
  encoder: Box<dyn Encoder>,
  /// The name the XML declaration gives, when it differs from the encoder's own.
  declared: Option<String>,
  /// The names of the elements currently open, outermost first.
  open: Vec<String>,
  /// Whether a start tag is open and still accepting attributes (its `>` unwritten).
  pending: bool,
  /// Reused buffer for escaping, so a write does not allocate.
  scratch: String,
  /// Reused buffer for the encoded bytes, for the same reason.
  bytes: Vec<u8>,
}

impl<W: io::Write> XmlWriter<W> {
  /// Creates a writer over `out`.
  ///
  pub fn new(out: W) -> Self {
    Self {
      out,
      encoder: Box::new(Utf8Encoder::new()),
      declared: None,
      open: Vec::new(),
      pending: false,
      scratch: String::new(),
      bytes: Vec::new(),
    }
  }

  /// Writes the output in `label` rather than UTF-8.
  ///
  /// The XML declaration also gives `label`, unless [`with_declared_encoding`](Self::with_declared_encoding) sets a
  /// different name.
  ///
  /// A character the encoding cannot hold becomes a character reference in character data and in an attribute value.
  /// Anywhere else, a name, a comment, a processing instruction, or a CDATA section, no reference is markup, so the
  /// write is refused instead.
  ///
  /// # Errors
  ///
  /// [`Error::Encoding`](xenolith_core::error::Error::Encoding) for a label no encoding answers to, or one this build
  /// cannot write. UTF-16 is among those: it can be read but not written. The [Encoding](#encoding) section shows how
  /// to produce UTF-16 output anyway.
  ///
  pub fn with_encoding(mut self, label: &str) -> Result<Self> {
    self.encoder = encoder_for(label)?;
    self.declared = Some(label.to_owned());
    Ok(self)
  }

  /// Gives the XML declaration a different encoding name from the one the bytes are written in.
  ///
  /// The two are usually the same, and [`with_encoding`](Self::with_encoding) keeps them so. They part where a reader
  /// is expected to know one name for what the writer knows by another, `Shift_JIS` for output written as
  /// `windows-31j` among them.
  ///
  /// Nothing checks that the two agree. A name that doesn't describe the bytes makes the document unreadable, so this
  /// is for cases where the caller knows the reader's vocabulary.
  ///
  #[must_use]
  pub fn with_declared_encoding(mut self, label: &str) -> Self {
    self.declared = Some(label.to_owned());
    self
  }

  /// Appends `text` to the output as it stands, refusing what the encoding cannot hold.
  ///
  fn put(&mut self, text: &str) -> Result<()> {
    self.bytes.clear();
    self.encoder.encode(text, &mut self.bytes)?;
    self.out.write_all(&self.bytes)?;
    Ok(())
  }

  /// Appends `text` to the output, writing a character reference for what the encoding cannot hold.
  ///
  fn put_with_references(&mut self, text: &str) -> Result<()> {
    self.bytes.clear();
    self.encoder.encode_with_references(text, &mut self.bytes)?;
    self.out.write_all(&self.bytes)?;
    Ok(())
  }

  /// Appends ASCII markup, which every encoding this writes holds.
  ///
  fn put_markup(&mut self, markup: &str) -> Result<()> {
    self.put(markup)
  }

  /// Writes `<?xml version="1.0" encoding="..."?>`, optionally with a `standalone` value.
  ///
  /// Call it first, before any element. The encoding it uses is the one set by
  /// [`with_declared_encoding`](Self::with_declared_encoding); otherwise, it uses the encoding the bytes are written
  /// in.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  ///
  pub fn write_declaration(&mut self, standalone: Option<bool>) -> Result<()> {
    let declared = self.declared.clone().unwrap_or_else(|| self.encoder.encoding().to_owned());
    self.put_markup("<?xml version=\"1.0\" encoding=\"")?;
    self.put(&declared)?;
    self.put_markup("\"")?;
    if let Some(standalone) = standalone {
      self.put_markup(if standalone { " standalone=\"yes\"" } else { " standalone=\"no\"" })?;
    }
    self.put_markup("?>")
  }

  /// Writes a document type declaration for a root element called `name`.
  ///
  /// The external identifiers decide the shape: both give `PUBLIC "public" "system"`, a system identifier alone gives
  /// `SYSTEM "system"`, and neither gives `<!DOCTYPE name>`. A public identifier without a system one is not a form
  /// XML has, so write it as though neither were given.
  ///
  /// It belongs after the XML declaration and before the root element. There is no internal subset here, which is
  /// what a tree carries too, so a DTD written this way is one an external identifier refers to.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer, and refuses a name or identifier the output encoding cannot
  /// hold.
  ///
  /// # Panics
  ///
  /// If an element is already open. A document type declaration may not appear inside one.
  ///
  pub fn write_doctype(&mut self, name: &str, public_id: Option<&str>, system_id: Option<&str>) -> Result<()> {
    assert!(self.open.is_empty(), "write_doctype must come before the root element");
    self.put_markup("<!DOCTYPE ")?;
    self.put(name)?;
    match (public_id, system_id) {
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

  /// Opens an element with `name`. Attributes may follow until the next content or end.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer, and refuses a name the output encoding cannot hold.
  ///
  pub fn write_start_element(&mut self, name: &str) -> Result<()> {
    self.close_start_tag()?;
    self.put_markup("<")?;
    self.put(name)?;
    self.open.push(name.to_owned());
    self.pending = true;
    Ok(())
  }

  /// Writes an attribute on the element just started.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer, and refuses a name the output encoding cannot hold. The value
  /// is never refused: what the encoding cannot hold becomes a character reference.
  ///
  /// # Panics
  ///
  /// If no start tag is open. An attribute has to belong to an element.
  ///
  pub fn write_attribute(&mut self, name: &str, value: &str) -> Result<()> {
    assert!(self.pending, "write_attribute must follow write_start_element, before any content");
    self.put_markup(" ")?;
    self.put(name)?;
    self.put_markup("=\"")?;
    self.scratch.clear();
    push_attribute(&mut self.scratch, value);
    let escaped = std::mem::take(&mut self.scratch);
    let written = self.put_with_references(&escaped);
    self.scratch = escaped;
    written?;
    self.put_markup("\"")
  }

  /// Ends the innermost open element: `</name>`, or `/>` if it has no content yet.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  ///
  /// # Panics
  ///
  /// If no element is open.
  ///
  pub fn write_end_element(&mut self) -> Result<()> {
    let name = self.open.pop().expect("write_end_element with no open element");
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

  /// Writes character data, escaped.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer. Text is never refused: what the encoding cannot hold becomes a
  /// character reference.
  ///
  pub fn write_characters(&mut self, text: &str) -> Result<()> {
    self.close_start_tag()?;
    self.scratch.clear();
    push_text(&mut self.scratch, text);
    let escaped = std::mem::take(&mut self.scratch);
    let written = self.put_with_references(&escaped);
    self.scratch = escaped;
    written
  }

  /// Writes a CDATA section, splitting any `]]>` so it cannot close early.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer, and refuses text the output encoding cannot hold. A CDATA
  /// section admits no character reference, so there is nothing to fall back to. Use
  /// [`write_characters`](Self::write_characters) where the text may go beyond the encoding.
  ///
  pub fn write_cdata(&mut self, text: &str) -> Result<()> {
    self.close_start_tag()?;
    self.scratch.clear();
    push_cdata(&mut self.scratch, text);
    let escaped = std::mem::take(&mut self.scratch);
    // A CDATA section admits no reference, so what the encoding cannot hold is refused rather than rewritten.
    let written = self.put(&escaped);
    self.scratch = escaped;
    written
  }

  /// Writes a comment, with `text` between `<!--` and `-->` as it stands.
  ///
  /// XML forbids `--` inside a comment and a `-` immediately before the close. Neither is checked here, so text from
  /// outside is worth checking first.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer, and refuses text the output encoding cannot hold. A comment
  /// admits no character reference either.
  ///
  pub fn write_comment(&mut self, text: &str) -> Result<()> {
    self.close_start_tag()?;
    self.put_markup("<!--")?;
    self.put(text)?;
    self.put_markup("-->")
  }

  /// Writes a processing instruction, with `target` and `data` as they stand.
  ///
  /// `data` holding `?>` would close the instruction early, which is not checked here.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer, and refuses a target or data the output encoding cannot hold.
  ///
  pub fn write_processing_instruction(&mut self, target: &str, data: &str) -> Result<()> {
    self.close_start_tag()?;
    self.put_markup("<?")?;
    self.put(target)?;
    if !data.is_empty() {
      self.put_markup(" ")?;
      self.put(data)?;
    }
    self.put_markup("?>")
  }

  /// The number of elements currently open.
  ///
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len()
  }

  /// Returns the underlying writer.
  ///
  /// Balancing every start with an end is the caller's business. Whatever has been written comes back as it stands,
  /// finished or not.
  ///
  pub fn into_inner(self) -> W {
    self.out
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
}

#[cfg(test)]
mod tests {
  use super::*;

  fn written(build: impl FnOnce(&mut XmlWriter<Vec<u8>>) -> Result<()>) -> String {
    let mut w = XmlWriter::new(Vec::new());
    build(&mut w).unwrap();
    String::from_utf8(w.into_inner()).unwrap()
  }

  #[test]
  fn an_element_with_no_content_is_empty() {
    let out = written(|w| {
      w.write_start_element("a")?;
      w.write_end_element()
    });
    assert_eq!(out, "<a/>");
  }

  #[test]
  fn nests_elements_and_attributes() {
    let out = written(|w| {
      w.write_start_element("a")?;
      w.write_attribute("x", "1")?;
      w.write_start_element("b")?;
      w.write_characters("t")?;
      w.write_end_element()?;
      w.write_end_element()
    });
    assert_eq!(out, "<a x=\"1\"><b>t</b></a>");
  }

  #[test]
  fn escapes_text_and_attributes() {
    let out = written(|w| {
      w.write_start_element("a")?;
      w.write_attribute("x", "a \"b\" < c")?;
      w.write_characters("1 < 2 & 3")?;
      w.write_end_element()
    });
    assert_eq!(out, "<a x=\"a &quot;b&quot; &lt; c\">1 &lt; 2 &amp; 3</a>");
  }

  #[test]
  fn writes_the_prolog_and_leaves() {
    let out = written(|w| {
      w.write_declaration(Some(true))?;
      w.write_comment("hi")?;
      w.write_start_element("a")?;
      w.write_processing_instruction("pi", "d")?;
      w.write_cdata("<raw>]]>x")?;
      w.write_end_element()
    });
    assert_eq!(
      out,
      "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><!--hi--><a><?pi d?><![CDATA[<raw>]]]]><![CDATA[>x]]></a>"
    );
  }

  #[test]
  fn a_doctype_takes_the_shape_its_identifiers_allow() {
    let both = written(|w| w.write_doctype("html", Some("-//W3C//DTD XHTML 1.0 Strict//EN"), Some("x.dtd")));
    assert_eq!(both, "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x.dtd\">");

    let system = written(|w| w.write_doctype("note", None, Some("note.dtd")));
    assert_eq!(system, "<!DOCTYPE note SYSTEM \"note.dtd\">");

    let bare = written(|w| w.write_doctype("note", None, None));
    assert_eq!(bare, "<!DOCTYPE note>");

    // `PUBLIC` without a system identifier is not a form XML has, so the public one is left out rather than written
    // into something that will not parse.
    let lone_public = written(|w| w.write_doctype("note", Some("-//X//EN"), None));
    assert_eq!(lone_public, "<!DOCTYPE note>");
  }

  #[test]
  fn a_doctype_sits_between_the_declaration_and_the_root() {
    let out = written(|w| {
      w.write_declaration(None)?;
      w.write_doctype("note", None, Some("note.dtd"))?;
      w.write_start_element("note")?;
      w.write_end_element()
    });
    assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE note SYSTEM \"note.dtd\"><note/>");
  }

  #[test]
  #[should_panic(expected = "before the root element")]
  fn a_doctype_inside_an_element_panics() {
    let mut w = XmlWriter::new(Vec::new());
    w.write_start_element("note").unwrap();
    let _ = w.write_doctype("note", None, None);
  }

  #[test]
  fn what_is_not_escaped_reaches_the_output_as_it_stands() {
    // `XmlWriter` says a comment's text and a processing instruction's data are written as they were given. That
    // is a footgun worth pinning: both of these produce output that will not parse, and the writer allows it.
    let out = written(|w| {
      w.write_comment("a--b")?;
      w.write_processing_instruction("pi", "closes ?> early")
    });
    assert_eq!(out, "<!--a--b--><?pi closes ?> early?>");
  }

  #[test]
  fn tracks_depth() {
    let mut w = XmlWriter::new(Vec::new());
    assert_eq!(w.depth(), 0);
    w.write_start_element("a").unwrap();
    w.write_start_element("b").unwrap();
    assert_eq!(w.depth(), 2);
    w.write_end_element().unwrap();
    assert_eq!(w.depth(), 1);
  }
}
