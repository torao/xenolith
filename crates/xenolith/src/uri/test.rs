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

/// The normal form of `s`, as text.
fn normalized(s: &str) -> String {
  UriReference::parse(s).unwrap().normalize().to_string()
}

#[test]
fn normalization_lower_cases_what_is_case_insensitive() {
  // §6.2.2.1: the scheme and the host, and nothing else — a path and a userinfo are case-sensitive.
  assert_eq!(normalized("HTTP://Example.ORG/A/b"), "http://example.org/A/b");
  assert_eq!(normalized("http://User@Example.ORG/"), "http://User@example.org/");
  // An IPv6 literal is a host like any other.
  assert_eq!(normalized("http://[2001:DB8::1]/a"), "http://[2001:db8::1]/a");
}

#[test]
fn normalization_writes_percent_encoding_one_way() {
  // §6.2.2.2: an octet that stands for an unreserved character is written as that character, and the hex digits of
  // one that stays encoded are upper case.
  assert_eq!(normalized("http://example.org/%7Euser/a%2fb"), "http://example.org/~user/a%2Fb");
  assert_eq!(normalized("http://example.org/a?q=%7e#%7e"), "http://example.org/a?q=~#~");
  // A sequence that is not `%` followed by two hex digits is left as it stands.
  assert_eq!(normalized("http://example.org/a%zz%2"), "http://example.org/a%zz%2");
}

#[test]
fn normalization_removes_the_dot_segments_of_a_hierarchy_only() {
  assert_eq!(normalized("http://example.org/a/./b/../c"), "http://example.org/a/c");
  // `%2E` is `.`, so an encoded dot segment is removed as well (§6.2.2.2 runs before §6.2.2.3).
  assert_eq!(normalized("http://example.org/a/%2E%2E/b"), "http://example.org/b");
  // In a relative reference the dots say where the reference points; `resolve` is what removes them.
  assert_eq!(normalized("../a/./b"), "../a/./b");
  assert_eq!(normalized("urn:isbn:0451450523"), "urn:isbn:0451450523");
}

#[test]
fn normalization_applies_what_it_knows_of_a_scheme() {
  // §6.2.3, for the schemes in `SCHEMES`: the implied port is dropped, and an empty path is the root.
  assert_eq!(normalized("http://example.org:80"), "http://example.org/");
  assert_eq!(normalized("https://example.org:443/a"), "https://example.org/a");
  // An empty authority is still an authority, and a path that is not empty is kept as it was written.
  assert_eq!(normalized("file://///"), "file://///");
  assert_eq!(normalized("file://host"), "file://host/");
  // An empty port stands for the implied one, whatever the scheme.
  assert_eq!(normalized("http://example.org:/a"), "http://example.org/a");
  // A port that is not the implied one stays, and so does the port of a scheme that is not listed.
  assert_eq!(normalized("http://example.org:8080/a"), "http://example.org:8080/a");
  assert_eq!(normalized("ftp://example.org:21/a"), "ftp://example.org:21/a");
  assert_eq!(normalized("urn:example:a"), "urn:example:a", "no authority, so no root to imply");
}

#[test]
fn a_normal_form_is_its_own_normal_form() {
  for uri in ["HTTP://Example.ORG:80", "http://example.org/a/./b/../c", "http://example.org/%7Ea%2fb", "../a"] {
    let once = normalized(uri);
    assert_eq!(normalized(&once), once, "{uri}");
  }
}
