//! The XML and the text declaration placed at the beginning of a document or an external entity.
//!
//! Both start with `<?xml` and end with `?>`, and include pseudo-attributes. Document readers and DTD readers
//! encounter them at the start of the reading process. Since scanning occurs at this point, the reader does not need
//! to copy and retain their contents.
//!
//! Errors occurring here do not include positional information. The caller, being aware of the position it was
//! reading, appends that information.
//!
//! These refer to the XML declaration and text declaration placed at the beginning of a document or an external entity.

use crate::chars;
use crate::error::{Error, Result};

/// Scan results for detecting the leading text declaration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextDecl {
  /// The entity does not begin with a text declaration.
  None,
  /// The entity begins with a text declaration of the specified byte length, but the reader skips it.
  Present(usize),
  /// The reader requires additional input because the input read so far is insufficient to make a determination.
  NeedMore,
}

/// Reads a single `name = "value"` pair from an XML declaration or text declaration and returns the name, the value,
/// and the remainder of the string.
///
/// `decl` is the name used to refer to the declaration in error messages, since they all share this pseudo-attribute
/// syntax.
///
/// # Errors
///
/// [`Error::WellFormedness`] if a pseudo-attribute lacks an `=`, or if its value is unquoted or unterminated.
///
/// # Examples
///
/// ```
/// use xenolith::io::decl::pseudo_attribute;
///
/// // The tail comes back so the caller can read the next one from it.
/// let (name, value, tail) = pseudo_attribute(" version=\"1.0\" encoding=\"UTF-8\"", "XML declaration")?;
/// assert_eq!((name, value), ("version", "1.0"));
/// assert_eq!(tail, " encoding=\"UTF-8\"");
///
/// // Either quote may be used, and the value ends at the matching one.
/// let (_, value, _) = pseudo_attribute("encoding='UTF-8'", "text declaration")?;
/// assert_eq!(value, "UTF-8");
///
/// // `decl` is what the message calls the enclosing declaration.
/// let error = pseudo_attribute("version 1.0", "XML declaration").unwrap_err();
/// assert!(error.to_string().contains("the XML declaration is missing an \"=\""));
/// # Ok::<(), xenolith::Error>(())
/// ```
pub fn pseudo_attribute<'t>(rest: &'t str, decl: &str) -> Result<(&'t str, &'t str, &'t str)> {
  let malformed = |what: &str| Error::well_formedness(format!("the {decl} {what}"));
  let rest = rest.trim_start_matches(chars::is_whitespace);
  let name_len = rest.find(|c: char| c == '=' || chars::is_whitespace(c)).unwrap_or(rest.len());
  let (name, rest) = rest.split_at(name_len);
  let rest = rest.trim_start_matches(chars::is_whitespace);
  let rest = rest.strip_prefix('=').ok_or_else(|| malformed("is missing an \"=\""))?;
  let rest = rest.trim_start_matches(chars::is_whitespace);
  let quote =
    rest.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| malformed("has an unquoted value"))?;
  let rest = &rest[quote.len_utf8()..];
  let end = rest.find(quote).ok_or_else(|| malformed("has an unterminated value"))?;
  Ok((name, &rest[..end], &rest[end + quote.len_utf8()..]))
}

/// Parses the text declaration at the beginning of an external entity without consuming its content.
///
/// `TextDecl ::= '<?xml' VersionInfo? EncodingDecl S? '?>'`: An encoding specification is mandatory, whereas the
/// version is optional (and must appear first if specified); a standalone declaration is not included. This
/// distinguishes it from an XML declaration. Although the stream has already read this section to identify the
/// encoding, this process validates the format and reports the number of bytes to skip so that it is not passed to the
/// reader as a "processing instruction."
///
/// `last` becomes `true` when reading up to the end of the entity is complete. While `last` is `false` and the
/// declaration parsing is incomplete, [`TextDecl::NeedMore`] is returned to request further input.
///
/// # Errors
///
/// [`Error::WellFormedness`] if the declaration is not closed, the encoding is not specified, pseudo-attributes are
/// duplicated, the version is specified after the encoding, or attributes permitted only in an XML declaration are
/// included.
///
/// # Examples
///
/// ```
/// use xenolith::io::decl::{TextDecl, text_declaration_span};
///
/// // The span covers `<?xml ... ?>`, so a reader steps over exactly that much.
/// let head = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!ELEMENT a EMPTY>";
/// assert_eq!(text_declaration_span(head, true)?, TextDecl::Present(38));
/// assert_eq!(&head[38..], "<!ELEMENT a EMPTY>");
///
/// // An entity that opens with content has none.
/// assert_eq!(text_declaration_span("<!ELEMENT a EMPTY>", true)?, TextDecl::None);
/// // `<?xmlfoo?>` is a processing instruction: a declaration has whitespace after `<?xml`.
/// assert_eq!(text_declaration_span("<?xmlfoo?>", true)?, TextDecl::None);
///
/// // Still reading, and too little to tell one from the other.
/// assert_eq!(text_declaration_span("<?xm", false)?, TextDecl::NeedMore);
///
/// // A text declaration must give an encoding, which is where it differs from the XML declaration.
/// assert!(text_declaration_span("<?xml version=\"1.0\"?>", true).is_err());
/// # Ok::<(), xenolith::Error>(())
/// ```
pub fn text_declaration_span(rem: &str, last: bool) -> Result<TextDecl> {
  const HEAD: &str = "<?xml";
  // Too little read to tell `<?xml` from a shorter prefix, or its following character apart.
  if !last && rem.len() <= HEAD.len() && HEAD.starts_with(rem) {
    return Ok(TextDecl::NeedMore);
  }
  let Some(after) = rem.strip_prefix(HEAD) else { return Ok(TextDecl::None) };
  // `<?xmlfoo` is not a declaration; a real one is followed by whitespace.
  match after.chars().next() {
    None if !last => return Ok(TextDecl::NeedMore),
    Some(c) if chars::is_whitespace(c) => {}
    _ => return Ok(TextDecl::None),
  }
  let malformed = |what: &str| Error::well_formedness(format!("the text declaration {what}"));
  let Some(end) = rem.find("?>") else {
    return if last { Err(malformed("is not closed by \"?>\"")) } else { Ok(TextDecl::NeedMore) };
  };
  let mut rest = &rem[HEAD.len()..end];
  let mut seen: Vec<&str> = Vec::new();
  while !rest.trim_start_matches(chars::is_whitespace).is_empty() {
    let (name, _value, tail) = pseudo_attribute(rest, "text declaration")?;
    // Dispatch on the name first, so a misplaced or repeated version or encoding is told apart from a name that is not
    // a pseudo-attribute at all.
    if seen.contains(&name) {
      return Err(malformed(&format!("has more than one {name}")));
    }
    match name {
      // `TextDecl ::= '<?xml' VersionInfo? EncodingDecl S? '?>'`, so version, when present, comes before encoding.
      "version" if seen.is_empty() => {}
      "version" => return Err(malformed("has version after encoding")),
      "encoding" => {}
      "standalone" => return Err(malformed("may not have a standalone declaration")),
      other => return Err(malformed(&format!("has {other:?}, which is not one of version or encoding"))),
    }
    seen.push(name);
    rest = tail;
  }
  if !seen.contains(&"encoding") {
    return Err(malformed("has no encoding"));
  }
  Ok(TextDecl::Present(end + 2))
}

/// Reads and consumes the leading text declaration from a stream that has been read to the end.
///
/// # Errors
///
/// As [`text_declaration_span`].
///
/// # Examples
///
/// ```
/// use xenolith::io::decl::strip_text_declaration;
/// use xenolith::io::stream::CharStream;
///
/// let mut stream = CharStream::new();
/// stream.feed(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><!ELEMENT a EMPTY>", true)?;
/// strip_text_declaration(&mut stream)?;
/// assert_eq!(stream.remainder(), "<!ELEMENT a EMPTY>");
///
/// // An entity with no declaration is left as it stands.
/// let mut plain = CharStream::new();
/// plain.feed(b"<!ELEMENT a EMPTY>", true)?;
/// strip_text_declaration(&mut plain)?;
/// assert_eq!(plain.remainder(), "<!ELEMENT a EMPTY>");
/// # Ok::<(), xenolith::Error>(())
/// ```
///
pub fn strip_text_declaration(stream: &mut crate::io::stream::CharStream) -> Result<()> {
  let at = stream.location();
  let len = match text_declaration_span(stream.remainder(), true).map_err(|e| e.at(at))? {
    TextDecl::Present(len) => len,
    TextDecl::None => return Ok(()),
    TextDecl::NeedMore => {
      return Err(Error::internal("a completed entity still requested more of its text declaration"));
    }
  };
  stream.advance(len);
  Ok(())
}
