//! A streaming XML writer: the StAX `XMLStreamWriter` shape.
//!
//! Where the [`Serializer`](crate::Serializer) walks a tree that already exists, an [`XmlWriter`]
//! is driven call by call, so output can be produced without ever holding the whole document.
//! It tracks the open elements, so an element started and ended with nothing between becomes
//! `<a/>`, and it escapes text and attribute values for you. Namespaces are the caller's to
//! manage: write an `xmlns` declaration as an ordinary attribute.
//!
//! # Examples
//!
//! ```
//! use xenolith_serialize::XmlWriter;
//!
//! let mut w = XmlWriter::new(Vec::new());
//! w.write_start_element("greeting")?;
//! w.write_attribute("xml:lang", "en")?;
//! w.write_characters("Hello & welcome")?;
//! w.write_end_element()?;
//! let out = String::from_utf8(w.into_inner()).unwrap();
//! assert_eq!(out, "<greeting xml:lang=\"en\">Hello &amp; welcome</greeting>");
//! # Ok::<(), std::io::Error>(())
//! ```

use std::io;

use crate::escape::{push_attribute, push_cdata, push_text};

/// Writes XML to an [`io::Write`] from a sequence of calls.
#[derive(Debug)]
pub struct XmlWriter<W> {
  out: W,
  /// The names of the elements currently open, outermost first.
  open: Vec<String>,
  /// Whether a start tag is open and still accepting attributes (its `>` unwritten).
  pending: bool,
  /// Reused buffer for escaping, so a write does not allocate.
  scratch: String,
}

impl<W: io::Write> XmlWriter<W> {
  /// Creates a writer over `out`.
  pub fn new(out: W) -> Self {
    Self { out, open: Vec::new(), pending: false, scratch: String::new() }
  }

  /// Writes `<?xml version="1.0" encoding="UTF-8"?>`, optionally with a `standalone` value.
  ///
  /// Call it first, before any element.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  pub fn write_declaration(&mut self, standalone: Option<bool>) -> io::Result<()> {
    self.out.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"")?;
    if let Some(standalone) = standalone {
      self.out.write_all(if standalone { b" standalone=\"yes\"" } else { b" standalone=\"no\"" })?;
    }
    self.out.write_all(b"?>")
  }

  /// Opens an element with `name`. Attributes may follow until the next content or end.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  pub fn write_start_element(&mut self, name: &str) -> io::Result<()> {
    self.close_start_tag()?;
    self.out.write_all(b"<")?;
    self.out.write_all(name.as_bytes())?;
    self.open.push(name.to_owned());
    self.pending = true;
    Ok(())
  }

  /// Writes an attribute on the element just started.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  ///
  /// # Panics
  ///
  /// If no start tag is open — an attribute has to belong to an element.
  pub fn write_attribute(&mut self, name: &str, value: &str) -> io::Result<()> {
    assert!(self.pending, "write_attribute must follow write_start_element, before any content");
    self.out.write_all(b" ")?;
    self.out.write_all(name.as_bytes())?;
    self.out.write_all(b"=\"")?;
    self.scratch.clear();
    push_attribute(&mut self.scratch, value);
    self.out.write_all(self.scratch.as_bytes())?;
    self.out.write_all(b"\"")
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
  pub fn write_end_element(&mut self) -> io::Result<()> {
    let name = self.open.pop().expect("write_end_element with no open element");
    if self.pending {
      // Nothing was written inside: collapse to an empty element.
      self.pending = false;
      self.out.write_all(b"/>")
    } else {
      self.out.write_all(b"</")?;
      self.out.write_all(name.as_bytes())?;
      self.out.write_all(b">")
    }
  }

  /// Writes character data, escaped.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  pub fn write_characters(&mut self, text: &str) -> io::Result<()> {
    self.close_start_tag()?;
    self.scratch.clear();
    push_text(&mut self.scratch, text);
    self.out.write_all(self.scratch.as_bytes())
  }

  /// Writes a CDATA section, splitting any `]]>` so it cannot close early.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  pub fn write_cdata(&mut self, text: &str) -> io::Result<()> {
    self.close_start_tag()?;
    self.scratch.clear();
    push_cdata(&mut self.scratch, text);
    self.out.write_all(self.scratch.as_bytes())
  }

  /// Writes a comment.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  pub fn write_comment(&mut self, text: &str) -> io::Result<()> {
    self.close_start_tag()?;
    self.out.write_all(b"<!--")?;
    self.out.write_all(text.as_bytes())?;
    self.out.write_all(b"-->")
  }

  /// Writes a processing instruction.
  ///
  /// # Errors
  ///
  /// Propagates any error from the underlying writer.
  pub fn write_processing_instruction(&mut self, target: &str, data: &str) -> io::Result<()> {
    self.close_start_tag()?;
    self.out.write_all(b"<?")?;
    self.out.write_all(target.as_bytes())?;
    if !data.is_empty() {
      self.out.write_all(b" ")?;
      self.out.write_all(data.as_bytes())?;
    }
    self.out.write_all(b"?>")
  }

  /// The number of elements currently open.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len()
  }

  /// Returns the underlying writer, discarding the writer's own state.
  ///
  /// It is the caller's business to have balanced every start with an end; whatever has been
  /// written is returned as-is.
  pub fn into_inner(self) -> W {
    self.out
  }

  /// Closes an open start tag with `>` if one is pending.
  fn close_start_tag(&mut self) -> io::Result<()> {
    if self.pending {
      self.pending = false;
      self.out.write_all(b">")?;
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn written(build: impl FnOnce(&mut XmlWriter<Vec<u8>>) -> io::Result<()>) -> String {
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
