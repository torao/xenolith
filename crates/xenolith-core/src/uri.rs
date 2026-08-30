//! URI references and reference resolution (RFC 3986).
//!
//! Since the computation of the base URI affects the behavior of `xml:base`, the `href` attribute in XInclude, and
//! `document()` function in XSLT, reference resolution must strictly comply with Section 5.3 of RFC 3986 rather than
//! merely generally.
//!

use std::fmt;

use crate::error::{Error, Result};

/// A parsed URI reference: absolute URI, relative reference, or same-document reference.
///
/// The components are still stored in URL-encoded from for resolution, as specified in RFC 3986.
///
/// Within a path, empty segments (`a//b/c`) are considered as empty directories unlike the lenient libraries in some
/// language. All features of [`UriReference`] treat empty segments as defined in RFC 3986 (§3.3 Paths, §5.2.4 Removal
/// of Dot Segments).
///
/// # Examples
///
/// ```
/// use xenolith_core::UriReference;
///
/// let uri = UriReference::parse("https://example.org/docs/main.xml?v=1#intro")?;
/// assert_eq!(uri.scheme(), Some("https"));
/// assert_eq!(uri.authority(), Some("example.org"));
/// assert_eq!(uri.path(), "/docs/main.xml");
/// assert_eq!(uri.query(), Some("v=1"));
/// assert_eq!(uri.fragment(), Some("intro"));
/// assert!(uri.is_absolute());
/// # Ok::<(), xenolith_core::Error>(())
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
  /// Builds a URI reference from its components.
  ///
  /// To comply with the URI specification, construction using [`new`](Self::new) follows the same process as
  /// [`parse`](Self::parse). Schemas are stored in lower-case, and components are assumed to be already URL-encoded.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Uri`] if the components do not form a valid URI reference. Specifically, this occurs when the URI
  /// reference contains characters that are not permitted in a URI, a schema outside
  /// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`, or a component chat contains a delimiter belonging to a subsequent
  /// segment (such as `/` within the authority).
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::UriReference;
  ///
  /// let uri = UriReference::new(Some("HTTPS"), Some("example.org"), "/docs/main.xml", Some("v=1"), Some("intro"))?;
  /// assert_eq!(uri.scheme(), Some("https")); // the scheme is lower-cased
  /// assert_eq!(uri.to_string(), "https://example.org/docs/main.xml?v=1#intro");
  ///
  /// // A relative reference has no scheme or authority.
  /// let rel = UriReference::new(None, None, "inc/part.xml", None, None)?;
  /// assert!(!rel.is_absolute());
  ///
  /// // A component that holds another's delimiter is rejected.
  /// assert!(UriReference::new(None, Some("a/b"), "", None, None).is_err());
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  pub fn new(
    scheme: Option<&str>,
    authority: Option<&str>,
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
  ) -> Result<Self> {
    let candidate = Self {
      scheme: scheme.map(str::to_ascii_lowercase),
      authority: authority.map(str::to_owned),
      path: path.to_owned(),
      query: query.map(str::to_owned),
      fragment: fragment.map(str::to_owned),
    };
    // Each component is considered valid exactly only when it matches the result of reverse-parsing its string
    // representation. In other words, if a forbidden character or an invalid schema is detected during parsing, the
    // parsing fails; if a component containing a delimiter belonging to a subsequent component is present, the equality
    // check fails.
    let rendered = candidate.to_string();
    if Self::parse(&rendered)? == candidate {
      Ok(candidate)
    } else {
      Err(Error::uri(format!(
        "these components do not form a URI reference {rendered:?}; a component may not contain delimiters using by subsequent ones (such as '/' in the authority, '?' in the path, or '#' in the query)"
      )))
    }
  }

  /// Parses a URI reference.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Uri`] if `s` contains characters that cannot appear in a URI
  /// reference, or if it has a scheme that is not `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
  ///
  pub fn parse(s: &str) -> Result<Self> {
    if let Some(bad) = s.chars().find(|c| !is_uri_char(*c)) {
      return Err(Error::uri(format!(
        "character U+{:04X} {bad:?} is not allowed in a URI reference: {s:?}",
        u32::from(bad)
      )));
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
      return Err(Error::uri(format!("ambiguous relative path: {s:?}")));
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

  /// The authority component, without the leading `//`. Generally, it will follow the format `[userinfo@]host[:port]`.
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

  /// The fragment identifier, without the leading `#`. A URI retains its fragment even after resolution.
  ///
  /// While XInclude uses the `xpointer` attribute instead of a fragment, XSLT's `document()` function uses the URI's
  /// fragment. For this reason, URIs must be structured so that the fragment is not lost after resolution.
  ///
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
  /// If the only difference between the URIs of two documents is the fragment, they are considered to be the same
  /// document. Based on this, `document()` returns the same node.
  ///
  #[must_use]
  pub fn without_fragment(&self) -> Self {
    Self { fragment: None, ..self.clone() }
  }

  /// Resolves `reference` using this URI as the base (RFC 3986 §5.3).
  ///
  /// The base should be abosolute. Otherwise, the result may retain relative.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::UriReference;
  ///
  /// let base = UriReference::parse("http://example.org/a/b/c.xml")?;
  /// let resolved = |r: &str| -> Result<String, xenolith_core::Error> {
  ///   Ok(base.resolve(&UriReference::parse(r)?).to_string())
  /// };
  ///
  /// assert_eq!(resolved("d.xml")?, "http://example.org/a/b/d.xml");
  /// assert_eq!(resolved("../d.xml")?, "http://example.org/a/d.xml");
  /// assert_eq!(resolved("/d.xml")?, "http://example.org/d.xml");
  /// assert_eq!(resolved("#frag")?, "http://example.org/a/b/c.xml#frag");
  /// assert_eq!(resolved("urn:other")?, "urn:other"); // already absolute
  /// # Ok::<(), xenolith_core::Error>(())
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

  /// Create a URI reference that returns `target` when resolved using this URI as the starting point. This is the
  /// inverse operation of [`resolve`](Self::resolve).
  ///
  /// The relative paths are built segment by segment relative to the base directory; if the two differ, `..` is output
  /// to move up a level. If the base path ends with `/`, it is treat as a directory. Otherwise, the last segment is a
  /// file and is not considered as directory. The query and fragment are retrieved from `target`.
  /// Empty path segments (a `//`) are significant per RFC 3986 and are preserved, not collapsed.
  ///
  /// If `target` cannot be converted to a relative path (because its schema or authority differs from base URI, or
  /// because the computed reference does not resolve back to `target`), the `target` is returned unchanged. Therefore,
  /// unless `target` itself contains `.` or `..` segments, the result always satisfies
  /// `base.resolve(&base.relativize(&target)) == target`.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::UriReference;
  ///
  /// let base = UriReference::parse("http://example.org/a/b/c.xml")?;
  /// let relative = |t: &str| -> Result<String, xenolith_core::Error> {
  ///   Ok(base.relativize(&UriReference::parse(t)?).to_string())
  /// };
  ///
  /// assert_eq!(relative("http://example.org/a/b/d.xml")?, "d.xml");
  /// assert_eq!(relative("http://example.org/a/e.xml")?, "../e.xml");
  /// assert_eq!(relative("http://example.org/a/b/c.xml#s")?, "c.xml#s");
  /// assert_eq!(relative("http://other.example/x")?, "http://other.example/x"); // other authority
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  #[must_use]
  pub fn relativize(&self, target: &Self) -> Self {
    // Only a target under the same scheme and authority can become a path-relative reference.
    if self.scheme != target.scheme || self.authority != target.authority {
      return target.clone();
    }
    let candidate = Self {
      scheme: None,
      authority: None,
      path: relative_path(&self.path, &target.path),
      query: target.query.clone(),
      fragment: target.fragment.clone(),
    };
    // Keep the relative reference only if it resolves back to the target exactly; otherwise the
    // segment computation did not apply, so fall back to the absolute target, which is always correct.
    if self.resolve(&candidate) == *target { candidate } else { target.clone() }
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
/// use xenolith_core::uri;
///
/// assert_eq!(uri::resolve("file:///docs/main.xml", "inc/part.xml")?, "file:///docs/inc/part.xml");
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`Error::Uri`] if either input fails to parse.
pub fn resolve(base: &str, reference: &str) -> Result<String> {
  let base = UriReference::parse(base)?;
  let reference = UriReference::parse(reference)?;
  Ok(base.resolve(&reference).to_string())
}

/// Computes the relative path from the `base_path` directory to `target_path`, and at each point where the paths
/// diverge, output a `..` segment to move up a level.
///
/// The base treats the path up to the last `/` as a directory. A segment of the base that ends with `/` is considered
/// a file. If `target_path` is that directory, the result is `..`; if the first segment is interpreted as a scheme,
/// `./` is appended as a prefix.
///
fn relative_path(base_path: &str, target_path: &str) -> String {
  let base_dir = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
  let base_dirs = dir_segments(base_dir);
  let target_segs = path_segments(target_path);
  let (last, target_dirs): (&str, &[&str]) = match target_segs.split_last() {
    Some((last, dirs)) => (last, dirs),
    None => ("", &[]),
  };
  let common = base_dirs.iter().zip(target_dirs).take_while(|(a, b)| a == b).count();

  let mut parts: Vec<&str> = vec![".."; base_dirs.len() - common];
  parts.extend_from_slice(&target_dirs[common..]);
  parts.push(last);
  let path = parts.join("/");

  if path.is_empty() {
    ".".to_owned() // the target is the base's directory itself
  } else if parts[0] != ".." && parts[0].contains(':') {
    format!("./{path}") // a leading segment holding ':' would be mistaken for a scheme
  } else {
    path
  }
}

/// The segments of `path`, with the leading `/` of an absolute path removed. Empty for `/` or "".
fn path_segments(path: &str) -> Vec<&str> {
  let path = path.strip_prefix('/').unwrap_or(path);
  if path.is_empty() { Vec::new() } else { path.split('/').collect() }
}

/// The directory segments of `base_dir`, which ends in `/`: its segments without the trailing "".
fn dir_segments(base_dir: &str) -> Vec<&str> {
  let mut segs = path_segments(base_dir);
  if segs.last().is_some_and(|s| s.is_empty()) {
    segs.pop();
  }
  segs
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
/// Written as a 5-rule loop, as described in the RFC. While using `split('/')` makes it simple, it silently
/// concatenates empty segments. In contract, the RFC preserves empty segment.
///
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

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// URL-encodes tha characters that may not appear literally in a URI reference.
///
/// # Examples
///
/// ```
/// use xenolith_core::uri::escape_uri;
///
/// assert_eq!(escape_uri("my docs/a.xml"), "my%20docs/a.xml");
/// assert_eq!(escape_uri("日本語.xml"), "%E6%97%A5%E6%9C%AC%E8%AA%9E.xml");
/// assert_eq!(escape_uri("already%20escaped"), "already%20escaped");
/// ```
#[must_use]
pub fn escape_uri(s: &str) -> String {
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
  fn new_builds_from_components_and_rejects_ill_formed_ones() {
    // Round-trips through Display and parse, with the scheme lower-cased.
    let u = UriReference::new(Some("HTTPS"), Some("example.org:8443"), "/a/b", Some("q=1"), Some("frag")).unwrap();
    assert_eq!(u, UriReference::parse("https://example.org:8443/a/b?q=1#frag").unwrap());

    // A relative reference: no scheme, no authority.
    let rel = UriReference::new(None, None, "sub/doc.xml", None, None).unwrap();
    assert_eq!(rel, UriReference::parse("sub/doc.xml").unwrap());

    // A component that carries another's delimiter, a bad scheme, and a forbidden character.
    assert!(UriReference::new(None, Some("a/b"), "", None, None).is_err()); // '/' in the authority
    assert!(UriReference::new(None, None, "a?b", None, None).is_err()); // '?' in the path
    assert!(UriReference::new(Some("1http"), None, "x", None, None).is_err()); // scheme starts with a digit
    assert!(matches!(UriReference::new(None, None, "a b", None, None).unwrap_err(), Error::Uri { .. }));
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
    assert!(matches!(UriReference::parse("http://a/ b").unwrap_err(), Error::Uri { .. }));
    assert!(UriReference::parse("http://a/日本語").is_err());
  }

  #[test]
  fn file_uris_resolve_relative_documents() {
    assert_eq!(r("file:///C:/docs/main.xml", "include/part.xml"), "file:///C:/docs/include/part.xml");
    assert_eq!(r("file:///C:/docs/main.xml", "../other.xml"), "file:///C:/other.xml");
  }

  #[test]
  fn relativize_is_the_inverse_of_resolve() {
    let base = UriReference::parse("http://example.org/a/b/c.xml").unwrap();
    let rel = |t: &str| base.relativize(&UriReference::parse(t).unwrap()).to_string();

    // Sibling, ancestor, cousin, descendant, same document, with a query and fragment kept.
    assert_eq!(rel("http://example.org/a/b/d.xml"), "d.xml");
    assert_eq!(rel("http://example.org/a/e.xml"), "../e.xml");
    assert_eq!(rel("http://example.org/x/y.xml"), "../../x/y.xml");
    assert_eq!(rel("http://example.org/a/b/sub/f.xml"), "sub/f.xml");
    assert_eq!(rel("http://example.org/a/b/c.xml"), "c.xml");
    assert_eq!(rel("http://example.org/a/b/c.xml?q=1#s"), "c.xml?q=1#s");

    // A base ending in '/' is a directory; the target directory itself becomes '../'.
    let dir = UriReference::parse("http://example.org/a/b/").unwrap();
    let dir_rel = |t: &str| dir.relativize(&UriReference::parse(t).unwrap()).to_string();
    assert_eq!(dir_rel("http://example.org/a/b/g.xml"), "g.xml");
    assert_eq!(dir_rel("http://example.org/a/"), "../");

    // A first segment that looks like a scheme is guarded with './'.
    assert_eq!(rel("http://example.org/a/b/x:y"), "./x:y");

    // A different scheme or authority cannot be made relative: the target is returned unchanged.
    assert_eq!(rel("https://example.org/a/b/d.xml"), "https://example.org/a/b/d.xml");
    assert_eq!(rel("http://other.example/a/b/d.xml"), "http://other.example/a/b/d.xml");
  }

  #[test]
  fn relativize_round_trips_through_resolve() {
    let bases =
      ["http://example.org/a/b/c.xml", "http://example.org/a/b/", "http://example.org/", "file:///docs/main.xml"];
    let targets = [
      "http://example.org/a/b/d.xml",
      "http://example.org/a/e.xml",
      "http://example.org/x/y.xml",
      "http://example.org/a/b/",
      "http://example.org/",
      "http://example.org/a/b/c.xml?q#f",
      "http://example.org/a/b/x//y.xml",
      "http://example.org/a/b//d.xml",
      "file:///docs/inc/part.xml",
      "http://other.example/x",
    ];
    for b in bases {
      let base = UriReference::parse(b).unwrap();
      for t in targets {
        let target = UriReference::parse(t).unwrap();
        let relative = base.relativize(&target);
        assert_eq!(base.resolve(&relative), target, "base={b} target={t} relative={relative}");
      }
    }
  }

  #[test]
  fn relativize_accepts_a_relative_base() {
    let base = UriReference::parse("a/b/c.xml").unwrap();
    let rel = |t: &str| base.relativize(&UriReference::parse(t).unwrap()).to_string();

    // Within the base's own directory tree the reference is relative and resolves back.
    assert_eq!(rel("a/b/d.xml"), "d.xml");
    assert_eq!(rel("a/e.xml"), "../e.xml");
    assert_eq!(rel("a/b/sub/f.xml"), "sub/f.xml");
    assert_eq!(rel("a/b/c.xml?q#f"), "c.xml?q#f");
    for t in ["a/b/d.xml", "a/e.xml", "a/b/sub/f.xml", "a/b/c.xml?q#f", "a/b/"] {
      let target = UriReference::parse(t).unwrap();
      assert_eq!(base.resolve(&base.relativize(&target)), target, "target={t}");
    }
  }

  #[test]
  fn relativize_preserves_empty_path_segments() {
    let base = UriReference::parse("http://example.org/a/b/c.xml").unwrap();
    let rel = |t: &str| base.relativize(&UriReference::parse(t).unwrap()).to_string();

    // An empty segment below the base directory is significant (RFC 3986) and is kept, and the
    // reference still resolves back exactly.
    assert_eq!(rel("http://example.org/a/b/x//y.xml"), "x//y.xml");
    let below = UriReference::parse("http://example.org/a/b/x//y.xml").unwrap();
    assert_eq!(base.resolve(&base.relativize(&below)), below);

    // An empty segment right at the base-directory boundary cannot be a path-relative reference
    // (resolution never inserts a slash at the join), so the absolute target is returned unchanged.
    assert_eq!(rel("http://example.org/a/b//d.xml"), "http://example.org/a/b//d.xml");
    let boundary = UriReference::parse("http://example.org/a/b//d.xml").unwrap();
    assert_eq!(base.resolve(&base.relativize(&boundary)), boundary);
  }

  #[test]
  fn escaping_leaves_existing_escapes_and_encodes_utf8() {
    assert_eq!(escape_uri("a b"), "a%20b");
    assert_eq!(escape_uri("a%20b"), "a%20b");
    assert_eq!(escape_uri("\u{3042}"), "%E3%81%82");
    assert_eq!(escape_uri("ok/path?q=1#f"), "ok/path?q=1#f");
  }
}
