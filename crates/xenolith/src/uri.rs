//! URI references and reference resolution (RFC 3986).
//!
//! Since the computation of the base URI affects the behavior of `xml:base`, the `href` attribute in XInclude, and the
//! `document()` function in XSLT, reference resolution must strictly comply with Section 5.3 of RFC 3986 rather than
//! merely generally.
//!

#[cfg(test)]
mod test;

use std::fmt;

use crate::error::{Error, Result};

/// A parsed URI reference: absolute URI, relative reference, or same-document reference.
///
/// In accordance with RFC 3986, each component is retained in its URL-encoded form for resolution purposes.
///
/// Empty segments within the path (e.g., `a//b/c`) are treated as empty directories, unlike the behavior of permissive
/// libraries found in some languages. Across all [`UriReference`] functionality, empty segments are processed
/// according to the definitions in RFC 3986 (§3.3 Paths, §5.2.4 Removal of Dot Segments).
///
/// # Examples
///
/// ```
/// use xenolith::UriReference;
///
/// let uri = UriReference::parse("https://example.org/docs/main.xml?v=1#intro")?;
/// assert_eq!(uri.scheme(), Some("https"));
/// assert_eq!(uri.authority(), Some("example.org"));
/// assert_eq!(uri.path(), "/docs/main.xml");
/// assert_eq!(uri.query(), Some("v=1"));
/// assert_eq!(uri.fragment(), Some("intro"));
/// assert!(uri.is_absolute());
/// # Ok::<(), xenolith::Error>(())
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
  /// To ensure compliance with the URI specification, construction using [`new`](Self::new) follows the same procedure
  /// as [`parse`](Self::parse). The scheme is stored in lowercase, and each component is assumed to be already
  /// URL-encoded.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Uri`] if the components do not form a valid URI reference. Specifically, this occurs when the
  /// URI reference contains characters that are not permitted in a URI, a schema outside `ALPHA *( ALPHA / DIGIT /
  /// "+" / "-" / "." )`, or a component that contains a delimiter belonging to a subsequent segment (such as `/`
  /// within the authority).
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::UriReference;
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
  /// # Ok::<(), xenolith::Error>(())
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

  /// The normal form of this URI, so that two URIs that differ only in how they are written compare equal.
  ///
  /// This is the syntax-based normalization of RFC 3986 §6.2.2 — the scheme and the host are lower-cased, the hex
  /// digits of a percent-encoded octet are upper-cased, an octet that stands for an unreserved character is written as
  /// that character, and `.` and `..` are removed from a hierarchical path — together with what §6.2.3 allows a
  /// processor to do for the schemes it knows: a port that is the scheme's default is dropped, and an empty path
  /// becomes `/`. [`SCHEMES`] holds that knowledge, and a scheme is added to it by writing one more row.
  ///
  /// What is left alone: `.` and `..` in a relative reference, which say where the reference points and are removed
  /// only by [`resolve`](Self::resolve); a percent sequence that is not `%` followed by two hex digits, since nothing
  /// here can say what it was meant to be; and everything a scheme knows that is not in [`SCHEMES`], `file://localhost`
  /// standing for `file://` (RFC 8089) among it.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::UriReference;
  ///
  /// let normalized = |s: &str| -> Result<String, xenolith::Error> {
  ///   Ok(UriReference::parse(s)?.normalize().to_string())
  /// };
  ///
  /// // The scheme and the host are case-insensitive, the default port is implied, and an empty path is the root.
  /// assert_eq!(normalized("HTTP://Example.ORG:80")?, "http://example.org/");
  /// assert_eq!(normalized("https://EXAMPLE.org:443/a")?, "https://example.org/a");
  /// // `%7E` is `~`, and the hex digits of an octet that has to stay encoded are upper case.
  /// assert_eq!(normalized("http://example.org/%7Euser/a%2fb")?, "http://example.org/~user/a%2Fb");
  /// // A hierarchical path loses its dot segments; a relative reference keeps them.
  /// assert_eq!(normalized("http://example.org/a/./b/../c")?, "http://example.org/a/c");
  /// assert_eq!(normalized("../a/./b")?, "../a/./b");
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  #[must_use]
  pub fn normalize(&self) -> Self {
    // §6.2.2.1: the scheme is case-insensitive. `parse` lower-cases it already; one built by hand may not have.
    let scheme = self.scheme.as_ref().map(|scheme| scheme.to_ascii_lowercase());
    let rules = scheme.as_deref().and_then(SchemeRules::of);
    let authority = self.authority.as_ref().map(|authority| normalize_authority(authority, rules));

    // §6.2.2.2 before §6.2.2.3, which is the order the RFC lists them in and the order that matters: `%2E` is `.`,
    // so an encoded dot segment becomes a dot segment first and is then removed.
    let mut path = normalize_percent(&self.path);
    // §6.2.2.3 removes the dot segments of a hierarchical path. In a relative reference they say where the reference
    // points, and `resolve` is what removes them.
    if scheme.is_some() && (authority.is_some() || path.starts_with('/')) {
      path = remove_dot_segments(&path);
    }
    // §6.2.3: for a scheme whose authority names a hierarchy, an empty path is the same as `/`.
    if path.is_empty() && authority.is_some() && rules.is_some_and(|rules| rules.empty_path_is_root) {
      path = "/".to_owned();
    }

    Self {
      scheme,
      authority,
      path,
      query: self.query.as_deref().map(normalize_percent),
      fragment: self.fragment.as_deref().map(normalize_percent),
    }
  }

  /// Resolves `reference` using this URI as the base (RFC 3986 §5.3).
  ///
  /// The base should be abosolute. Otherwise, the result may retain relative.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::UriReference;
  ///
  /// let base = UriReference::parse("http://example.org/a/b/c.xml")?;
  /// let resolved = |r: &str| -> Result<String, xenolith::Error> {
  ///   Ok(base.resolve(&UriReference::parse(r)?).to_string())
  /// };
  ///
  /// assert_eq!(resolved("d.xml")?, "http://example.org/a/b/d.xml");
  /// assert_eq!(resolved("../d.xml")?, "http://example.org/a/d.xml");
  /// assert_eq!(resolved("/d.xml")?, "http://example.org/d.xml");
  /// assert_eq!(resolved("#frag")?, "http://example.org/a/b/c.xml#frag");
  /// assert_eq!(resolved("urn:other")?, "urn:other"); // already absolute
  /// # Ok::<(), xenolith::Error>(())
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
  /// use xenolith::UriReference;
  ///
  /// let base = UriReference::parse("http://example.org/a/b/c.xml")?;
  /// let relative = |t: &str| -> Result<String, xenolith::Error> {
  ///   Ok(base.relativize(&UriReference::parse(t)?).to_string())
  /// };
  ///
  /// assert_eq!(relative("http://example.org/a/b/d.xml")?, "d.xml");
  /// assert_eq!(relative("http://example.org/a/e.xml")?, "../e.xml");
  /// assert_eq!(relative("http://example.org/a/b/c.xml#s")?, "c.xml#s");
  /// assert_eq!(relative("http://other.example/x")?, "http://other.example/x"); // other authority
  /// # Ok::<(), xenolith::Error>(())
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
/// use xenolith::uri;
///
/// assert_eq!(uri::resolve("file:///docs/main.xml", "inc/part.xml")?, "file:///docs/inc/part.xml");
/// # Ok::<(), xenolith::Error>(())
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

/// What [`UriReference::normalize`] knows about one scheme, which RFC 3986 §6.2.3 leaves to the processor.
///
/// Knowing a scheme is worth a row here where its URIs are written in more than one way for the same resource. A
/// scheme that is not listed is normalized by §6.2.2 alone, which is correct for every scheme but says less.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemeRules {
  /// The scheme, lower-cased.
  pub scheme: &'static str,
  /// The port implied when the authority carries none, which the normal form therefore drops. `None` where the
  /// scheme has no port at all.
  pub default_port: Option<&'static str>,
  /// Whether an empty path is the same as `/`, which holds where the authority names a hierarchy.
  pub empty_path_is_root: bool,
}

/// The schemes [`UriReference::normalize`] knows, in no particular order.
///
/// Adding one is a row: the scheme in lower case, the port it implies, and whether an empty path is its root.
pub const SCHEMES: &[SchemeRules] = &[
  SchemeRules { scheme: "http", default_port: Some("80"), empty_path_is_root: true },
  SchemeRules { scheme: "https", default_port: Some("443"), empty_path_is_root: true },
  // A `file` URI carries no port, and its path is a hierarchy from the root (RFC 8089).
  SchemeRules { scheme: "file", default_port: None, empty_path_is_root: true },
];

impl SchemeRules {
  /// What is known about `scheme`, which is expected in lower case, or `None` for a scheme that is not listed.
  #[must_use]
  pub fn of(scheme: &str) -> Option<&'static Self> {
    SCHEMES.iter().find(|rules| rules.scheme == scheme)
  }
}

/// Normalizes one authority: `[userinfo@]host[:port]`.
///
/// §6.2.2.1 makes the host case-insensitive, while the userinfo is left as it is: it is not defined to be. §6.2.3
/// drops a port that the scheme implies, and an empty port, which stands for the same thing.
fn normalize_authority(authority: &str, rules: Option<&SchemeRules>) -> String {
  let (userinfo, host_port) = match authority.rsplit_once('@') {
    Some((userinfo, host_port)) => (Some(userinfo), host_port),
    None => (None, authority),
  };
  // The port follows the last `:`, except within an IPv6 literal, where the address itself holds them.
  let split = match host_port.rfind(']') {
    Some(end) => host_port[end..].find(':').map(|at| end + at),
    None => host_port.rfind(':'),
  };
  let (host, port) = match split {
    Some(at) => (&host_port[..at], Some(&host_port[at + 1..])),
    None => (host_port, None),
  };

  let mut out = String::with_capacity(authority.len());
  if let Some(userinfo) = userinfo {
    out.push_str(&normalize_percent(userinfo));
    out.push('@');
  }
  out.push_str(&normalize_percent(&host.to_ascii_lowercase()));
  if let Some(port) = port.filter(|port| !port.is_empty() && Some(*port) != rules.and_then(|r| r.default_port)) {
    out.push(':');
    out.push_str(port);
  }
  out
}

/// RFC 3986 §6.2.2.2: the hex digits of a percent-encoded octet are upper case, and an octet that stands for an
/// unreserved character is written as that character.
///
/// A sequence that is not `%` followed by two hex digits is left exactly as it stands, since nothing here can say what
/// it was meant to be; [`UriReference::parse`] does not refuse one either.
fn normalize_percent(s: &str) -> String {
  let bytes = s.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut at = 0;
  while at < bytes.len() {
    match (bytes[at], bytes.get(at + 1).copied().and_then(hex_digit), bytes.get(at + 2).copied().and_then(hex_digit)) {
      (b'%', Some(high), Some(low)) => {
        let octet = high << 4 | low;
        if is_unreserved(octet) {
          out.push(octet);
        } else {
          out.extend_from_slice(&[b'%', HEX[usize::from(high)], HEX[usize::from(low)]]);
        }
        at += 3;
      }
      // Any other byte stands for itself, including the bytes of a character written literally.
      _ => {
        out.push(bytes[at]);
        at += 1;
      }
    }
  }
  // Only ASCII is ever replaced, so what comes out is the text it went in as, and the fallback never runs.
  String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// The value of one hexadecimal digit, or `None` for a byte that is not one.
fn hex_digit(byte: u8) -> Option<u8> {
  match byte {
    b'0'..=b'9' => Some(byte - b'0'),
    b'a'..=b'f' => Some(byte - b'a' + 10),
    b'A'..=b'F' => Some(byte - b'A' + 10),
    _ => None,
  }
}

/// Whether `octet` stands for an `unreserved` character, which never needs to be encoded (RFC 3986 §2.3).
fn is_unreserved(octet: u8) -> bool {
  octet.is_ascii_alphanumeric() || matches!(octet, b'-' | b'.' | b'_' | b'~')
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
/// use xenolith::uri::escape_uri;
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
