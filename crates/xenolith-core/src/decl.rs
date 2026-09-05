//! The XML and text declarations that open a document or an external entity.
//!
//! Both are written as `<?xml` followed by pseudo-attributes and `?>`, and both a document reader and a DTD reader
//! meet one at the head of what they read. Scanning happens here, so neither has to carry a copy.
//!
//! Errors raised here carry no location. The caller knows where it was reading and adds one.
//!
//! The XML and text declarations that open a document or an external entity.
//!

use crate::chars;
use crate::error::{Error, Result};

/// Scan results for detecting the leading text declaration.
///
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextDecl {
  /// The entity does not begin with a text declaration.
  ///
  None,
  /// The entity begins with a text declaration of this byte length, but the reader skips it.
  ///
  Present(usize),
  /// The reader requests additional input because the input read so far is insufficient to make a determination.
  ///
  NeedMore,
}

/// Reads one `name = "value"` of an XML or text declaration, returning the name, the value, and what follows.
///
/// `decl` is how the enclosing declaration is called in the error message, since both share this pseudo-attribute
/// syntax.
///
/// # Errors
///
/// [`Error::WellFormedness`] if the pseudo-attribute is missing its `=`, or its value is unquoted or unterminated.
///
/// # Examples
///
/// ```
/// use xenolith_core::decl::pseudo_attribute;
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
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
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

/// Measures a leading text declaration on an external entity, without consuming it.
///
/// `TextDecl ::= '<?xml' VersionInfo? EncodingDecl S? '?>'`: the encoding is required, the version optional and, if
/// present, first, and there is no standalone, which is where it differs from the XML declaration. The stream has
/// already read it to choose the encoding; this checks its shape and reports how many bytes to skip so it doesn't
/// reach the reader as a processing instruction.
///
/// `last` is true once the entity has been read to its end. While it is false and the declaration is not yet complete,
/// [`TextDecl::NeedMore`] asks for more input.
///
/// # Errors
///
/// [`Error::WellFormedness`] if the declaration is not closed, gives no encoding, repeats a pseudo-attribute, orders
/// version after encoding, or carries one that belongs to the XML declaration alone.
///
/// # Examples
///
/// ```
/// use xenolith_core::decl::{TextDecl, text_declaration_span};
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
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
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

/// Consumes a leading text declaration from a stream that has been read to its end.
///
/// # Errors
///
/// As [`text_declaration_span`].
///
/// # Examples
///
/// ```
/// use xenolith_core::decl::strip_text_declaration;
/// use xenolith_core::stream::CharStream;
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
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
pub fn strip_text_declaration(stream: &mut crate::stream::CharStream) -> Result<()> {
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
