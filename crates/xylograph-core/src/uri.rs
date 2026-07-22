//! URI references and reference resolution (RFC 3986).
//!
//! Base URI computation drives `xml:base`, XInclude's `href`, and XSLT's `document()`, so
//! reference resolution has to follow RFC 3986 §5.3 exactly rather than approximately.

use std::fmt;

use crate::error::{Error, ErrorKind, Result};

/// A parsed URI reference: absolute URI, relative reference, or same-document reference.
///
/// Components are stored still percent-encoded, as RFC 3986 requires for resolution.
///
/// # Examples
///
/// ```
/// use xylograph_core::UriReference;
///
/// let uri = UriReference::parse("https://example.org/docs/main.xml?v=1#intro")?;
/// assert_eq!(uri.scheme(), Some("https"));
/// assert_eq!(uri.authority(), Some("example.org"));
/// assert_eq!(uri.path(), "/docs/main.xml");
/// assert_eq!(uri.query(), Some("v=1"));
/// assert_eq!(uri.fragment(), Some("intro"));
/// assert!(uri.is_absolute());
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UriReference {
  scheme: Option<String>,
  authority: Option<String>,
  path: String,
  query: Option<String>,
  fragment: Option<String>,
}

impl UriReference {
  /// Parses a URI reference.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::Uri`] if `s` contains characters that cannot appear in a URI
  /// reference, or if it has a scheme that is not `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
  pub fn parse(s: &str) -> Result<Self> {
    if let Some(bad) = s.chars().find(|c| !is_uri_char(*c)) {
      return Err(Error::new(ErrorKind::Uri, format!("character {bad:?} is not allowed in a URI reference: {s:?}")));
    }

    let (rest, fragment) = split_once_at(s, '#');
    let (rest, query) = split_once_at(rest, '?');

    // A scheme is present only if the colon precedes any '/', '?' or '#'.
    let (scheme, rest) = match rest.find(':') {
      Some(i) if is_scheme(&rest[..i]) => (Some(rest[..i].to_ascii_lowercase()), &rest[i + 1..]),
      _ => (None, rest),
    };

    let (authority, path) = if let Some(after) = rest.strip_prefix("//") {
      let end = after.find('/').unwrap_or(after.len());
      (Some(after[..end].to_owned()), &after[end..])
    } else {
      (None, rest)
    };

    if scheme.is_none() && authority.is_none() && path.starts_with("//") {
      return Err(Error::new(ErrorKind::Uri, format!("ambiguous relative path: {s:?}")));
    }

    Ok(Self {
      scheme,
      authority,
      path: path.to_owned(),
      query: query.map(str::to_owned),
      fragment: fragment.map(str::to_owned),
    })
  }

  /// The scheme, lower-cased. `None` for a relative reference.
  #[must_use]
  pub fn scheme(&self) -> Option<&str> {
    self.scheme.as_deref()
  }

  /// The authority component, without the leading `//`.
  #[must_use]
  pub fn authority(&self) -> Option<&str> {
    self.authority.as_deref()
  }

  /// The path component, possibly empty.
  #[must_use]
  pub fn path(&self) -> &str {
    &self.path
  }

  /// The query component, without the leading `?`.
  #[must_use]
  pub fn query(&self) -> Option<&str> {
    self.query.as_deref()
  }

  /// The fragment identifier, without the leading `#`.
  ///
  /// XInclude's `xpointer` attribute is used instead of a fragment, but `document()` takes
  /// its fragment from the URI, so this has to survive resolution.
  #[must_use]
  pub fn fragment(&self) -> Option<&str> {
    self.fragment.as_deref()
  }

  /// True if a scheme is present, i.e. this is a URI rather than a relative reference.
  #[must_use]
  pub fn is_absolute(&self) -> bool {
    self.scheme.is_some()
  }

  /// Returns a copy without the fragment identifier.
  ///
  /// Two documents are the same document if their URIs differ only by fragment; `document()`
  /// depends on this to return identical nodes.
  #[must_use]
  pub fn without_fragment(&self) -> Self {
    Self { fragment: None, ..self.clone() }
  }

  /// Resolves `reference` against this URI as base (RFC 3986 §5.3).
  ///
  /// The base should be absolute; if it is not, the result may remain relative.
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_core::UriReference;
  ///
  /// let base = UriReference::parse("http://example.org/a/b/c.xml")?;
  /// let resolved = |r: &str| -> Result<String, xylograph_core::Error> {
  ///   Ok(base.resolve(&UriReference::parse(r)?).to_string())
  /// };
  ///
  /// assert_eq!(resolved("d.xml")?, "http://example.org/a/b/d.xml");
  /// assert_eq!(resolved("../d.xml")?, "http://example.org/a/d.xml");
  /// assert_eq!(resolved("/d.xml")?, "http://example.org/d.xml");
  /// assert_eq!(resolved("#frag")?, "http://example.org/a/b/c.xml#frag");
  /// assert_eq!(resolved("urn:other")?, "urn:other"); // already absolute
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  #[must_use]
  pub fn resolve(&self, reference: &Self) -> Self {
    // Strict resolution: a reference with a scheme is already absolute.
    if reference.scheme.is_some() {
      return Self { path: remove_dot_segments(&reference.path), ..reference.clone() };
    }
    if reference.authority.is_some() {
      return Self { scheme: self.scheme.clone(), path: remove_dot_segments(&reference.path), ..reference.clone() };
    }
    if reference.path.is_empty() {
      return Self {
        scheme: self.scheme.clone(),
        authority: self.authority.clone(),
        path: self.path.clone(),
        query: reference.query.clone().or_else(|| self.query.clone()),
        fragment: reference.fragment.clone(),
      };
    }
    let path = if reference.path.starts_with('/') {
      remove_dot_segments(&reference.path)
    } else {
      remove_dot_segments(&merge(self, &reference.path))
    };
    Self {
      scheme: self.scheme.clone(),
      authority: self.authority.clone(),
      path,
      query: reference.query.clone(),
      fragment: reference.fragment.clone(),
    }
  }
}

impl fmt::Display for UriReference {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Some(scheme) = &self.scheme {
      write!(f, "{scheme}:")?;
    }
    if let Some(authority) = &self.authority {
      write!(f, "//{authority}")?;
    }
    f.write_str(&self.path)?;
    if let Some(query) = &self.query {
      write!(f, "?{query}")?;
    }
    if let Some(fragment) = &self.fragment {
      write!(f, "#{fragment}")?;
    }
    Ok(())
  }
}

/// Resolves `reference` against `base`, both given as strings.
///
/// A convenience wrapper over [`UriReference::resolve`] for callers that hold no parsed URI.
///
/// # Examples
///
/// ```
/// use xylograph_core::uri;
///
/// assert_eq!(uri::resolve("file:///docs/main.xml", "inc/part.xml")?, "file:///docs/inc/part.xml");
/// # Ok::<(), xylograph_core::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`ErrorKind::Uri`] if either input fails to parse.
pub fn resolve(base: &str, reference: &str) -> Result<String> {
  let base = UriReference::parse(base)?;
  let reference = UriReference::parse(reference)?;
  Ok(base.resolve(&reference).to_string())
}

/// RFC 3986 §5.2.3, merge.
fn merge(base: &UriReference, path: &str) -> String {
  if base.authority.is_some() && base.path.is_empty() {
    format!("/{path}")
  } else {
    let keep = base.path.rfind('/').map_or(0, |i| i + 1);
    format!("{}{path}", &base.path[..keep])
  }
}

/// RFC 3986 §5.2.4, remove_dot_segments.
///
/// Written as the literal five-rule loop from the RFC. A `split('/')`-based version looks
/// tidier but silently collapses empty segments, which the RFC preserves.
fn remove_dot_segments(path: &str) -> String {
  let mut input = path;
  let mut out = String::with_capacity(path.len());

  while !input.is_empty() {
    if let Some(rest) = input.strip_prefix("../") {
      input = rest; // A
    } else if let Some(rest) = input.strip_prefix("./") {
      input = rest; // A
    } else if input.starts_with("/./") {
      input = &input[2..]; // B: "/./x" -> "/x"
    } else if input == "/." {
      input = "/"; // B
    } else if input.starts_with("/../") {
      input = &input[3..]; // C: "/../x" -> "/x"
      pop_segment(&mut out);
    } else if input == "/.." {
      input = "/"; // C
      pop_segment(&mut out);
    } else if input == "." || input == ".." {
      input = ""; // D
    } else {
      // E: move the leading segment, including its leading slash if present.
      let start = usize::from(input.starts_with('/'));
      let end = input[start..].find('/').map_or(input.len(), |i| start + i);
      out.push_str(&input[..end]);
      input = &input[end..];
    }
  }
  out
}

/// Removes the last segment of `out`, including its preceding `/`.
fn pop_segment(out: &mut String) {
  match out.rfind('/') {
    Some(i) => out.truncate(i),
    None => out.clear(),
  }
}

fn split_once_at(s: &str, delimiter: char) -> (&str, Option<&str>) {
  match s.split_once(delimiter) {
    Some((before, after)) => (before, Some(after)),
    None => (s, None),
  }
}

fn is_scheme(s: &str) -> bool {
  let mut chars = s.chars();
  matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Characters permitted in a URI reference: `unreserved / reserved / "%"`.
fn is_uri_char(c: char) -> bool {
  c.is_ascii_alphanumeric()
    || matches!(
      c,
      '-' | '.' | '_' | '~'                        // unreserved
      | ':' | '/' | '?' | '#' | '[' | ']' | '@'    // gen-delims
      | '!' | '$' | '&' | '\'' | '(' | ')'         // sub-delims
      | '*' | '+' | ',' | ';' | '='
      | '%'
    )
}

/// Percent-encodes the characters that may not appear literally in a URI reference.
///
/// XSLT and XInclude both accept attribute values that are not strictly URIs — they may
/// contain spaces or non-ASCII characters — and require them to be escaped as UTF-8 before
/// dereferencing. Existing `%` escapes are left alone.
///
/// # Examples
///
/// ```
/// use xylograph_core::uri::escape_uri;
///
/// assert_eq!(escape_uri("my docs/a.xml"), "my%20docs/a.xml");
/// assert_eq!(escape_uri("日本語.xml"), "%E6%97%A5%E6%9C%AC%E8%AA%9E.xml");
/// assert_eq!(escape_uri("already%20escaped"), "already%20escaped");
/// ```
#[must_use]
pub fn escape_uri(s: &str) -> String {
  const HEX: &[u8; 16] = b"0123456789ABCDEF";
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    if is_uri_char(c) {
      out.push(c);
    } else {
      let mut buf = [0u8; 4];
      for byte in c.encode_utf8(&mut buf).as_bytes() {
        out.push('%');
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0F)]));
      }
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn r(base: &str, reference: &str) -> String {
    resolve(base, reference).expect("resolvable")
  }

  /// RFC 3986 §5.4.1, normal examples. The base is fixed by the RFC.
  #[test]
  fn rfc3986_normal_examples() {
    const BASE: &str = "http://a/b/c/d;p?q";
    assert_eq!(r(BASE, "g:h"), "g:h");
    assert_eq!(r(BASE, "g"), "http://a/b/c/g");
    assert_eq!(r(BASE, "./g"), "http://a/b/c/g");
    assert_eq!(r(BASE, "g/"), "http://a/b/c/g/");
    assert_eq!(r(BASE, "/g"), "http://a/g");
    assert_eq!(r(BASE, "//g"), "http://g");
    assert_eq!(r(BASE, "?y"), "http://a/b/c/d;p?y");
    assert_eq!(r(BASE, "g?y"), "http://a/b/c/g?y");
    assert_eq!(r(BASE, "#s"), "http://a/b/c/d;p?q#s");
    assert_eq!(r(BASE, "g#s"), "http://a/b/c/g#s");
    assert_eq!(r(BASE, "g?y#s"), "http://a/b/c/g?y#s");
    assert_eq!(r(BASE, ";x"), "http://a/b/c/;x");
    assert_eq!(r(BASE, "g;x"), "http://a/b/c/g;x");
    assert_eq!(r(BASE, "g;x?y#s"), "http://a/b/c/g;x?y#s");
    assert_eq!(r(BASE, ""), "http://a/b/c/d;p?q");
    assert_eq!(r(BASE, "."), "http://a/b/c/");
    assert_eq!(r(BASE, "./"), "http://a/b/c/");
    assert_eq!(r(BASE, ".."), "http://a/b/");
    assert_eq!(r(BASE, "../"), "http://a/b/");
    assert_eq!(r(BASE, "../g"), "http://a/b/g");
    assert_eq!(r(BASE, "../.."), "http://a/");
    assert_eq!(r(BASE, "../../"), "http://a/");
    assert_eq!(r(BASE, "../../g"), "http://a/g");
  }

  /// RFC 3986 §5.4.2, abnormal examples.
  #[test]
  fn rfc3986_abnormal_examples() {
    const BASE: &str = "http://a/b/c/d;p?q";
    assert_eq!(r(BASE, "../../../g"), "http://a/g");
    assert_eq!(r(BASE, "../../../../g"), "http://a/g");
    assert_eq!(r(BASE, "/./g"), "http://a/g");
    assert_eq!(r(BASE, "/../g"), "http://a/g");
    assert_eq!(r(BASE, "g."), "http://a/b/c/g.");
    assert_eq!(r(BASE, ".g"), "http://a/b/c/.g");
    assert_eq!(r(BASE, "g.."), "http://a/b/c/g..");
    assert_eq!(r(BASE, "..g"), "http://a/b/c/..g");
    assert_eq!(r(BASE, "./../g"), "http://a/b/g");
    assert_eq!(r(BASE, "./g/."), "http://a/b/c/g/");
    assert_eq!(r(BASE, "g/./h"), "http://a/b/c/g/h");
    assert_eq!(r(BASE, "g/../h"), "http://a/b/c/h");
    assert_eq!(r(BASE, "g;x=1/./y"), "http://a/b/c/g;x=1/y");
    assert_eq!(r(BASE, "g;x=1/../y"), "http://a/b/c/y");
    assert_eq!(r(BASE, "g?y/./x"), "http://a/b/c/g?y/./x");
    assert_eq!(r(BASE, "g?y/../x"), "http://a/b/c/g?y/../x");
    assert_eq!(r(BASE, "g#s/./x"), "http://a/b/c/g#s/./x");
    assert_eq!(r(BASE, "g#s/../x"), "http://a/b/c/g#s/../x");
  }

  #[test]
  fn parses_components() {
    let u = UriReference::parse("https://example.org:8443/a/b?q=1#frag").unwrap();
    assert_eq!(u.scheme(), Some("https"));
    assert_eq!(u.authority(), Some("example.org:8443"));
    assert_eq!(u.path(), "/a/b");
    assert_eq!(u.query(), Some("q=1"));
    assert_eq!(u.fragment(), Some("frag"));
    assert!(u.is_absolute());
    assert_eq!(u.without_fragment().to_string(), "https://example.org:8443/a/b?q=1");

    let rel = UriReference::parse("sub/doc.xml").unwrap();
    assert!(!rel.is_absolute());
    assert_eq!(rel.path(), "sub/doc.xml");
  }

  #[test]
  fn scheme_is_case_insensitive_and_not_confused_with_a_path() {
    assert_eq!(UriReference::parse("HTTP://x/").unwrap().scheme(), Some("http"));
    // A colon after a slash is not a scheme delimiter.
    assert_eq!(UriReference::parse("a/b:c").unwrap().scheme(), None);
    // Neither is one preceded by a non-scheme character.
    assert_eq!(UriReference::parse("1a:b").unwrap().scheme(), None);
  }

  #[test]
  fn rejects_characters_outside_the_uri_repertoire() {
    assert_eq!(UriReference::parse("http://a/ b").unwrap_err().kind(), ErrorKind::Uri);
    assert!(UriReference::parse("http://a/日本語").is_err());
  }

  #[test]
  fn file_uris_resolve_relative_documents() {
    assert_eq!(r("file:///C:/docs/main.xml", "include/part.xml"), "file:///C:/docs/include/part.xml");
    assert_eq!(r("file:///C:/docs/main.xml", "../other.xml"), "file:///C:/other.xml");
  }

  #[test]
  fn escaping_leaves_existing_escapes_and_encodes_utf8() {
    assert_eq!(escape_uri("a b"), "a%20b");
    assert_eq!(escape_uri("a%20b"), "a%20b");
    assert_eq!(escape_uri("\u{3042}"), "%E3%81%82");
    assert_eq!(escape_uri("ok/path?q=1#f"), "ok/path?q=1#f");
  }
}
