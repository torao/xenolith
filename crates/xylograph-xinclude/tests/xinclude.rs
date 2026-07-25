//! XInclude expansion: inclusion, recursion, text, fallback, loops, limits, and base URI fixup.

use std::collections::HashMap;

use xylograph_core::{Error, ErrorKind};
use xylograph_dom::build;
use xylograph_serialize::Serializer;
use xylograph_xinclude::{Loader, XInclude};

/// A loader over an in-memory map of URI to bytes.
struct Files(HashMap<String, Vec<u8>>);

impl Files {
  fn new(entries: &[(&str, &str)]) -> Self {
    Self(entries.iter().map(|(uri, body)| ((*uri).to_owned(), body.as_bytes().to_vec())).collect())
  }
}

impl Loader for Files {
  fn load(&mut self, uri: &str) -> Result<Vec<u8>, Error> {
    self.0.get(uri).cloned().ok_or_else(|| Error::new(ErrorKind::Io, format!("no resource at {uri}")))
  }
}

const NS: &str = " xmlns:xi='http://www.w3.org/2001/XInclude'";
/// The same declaration as it serializes back (double-quoted). It stays on the element that
/// declared it — XInclude replaces the include elements, it does not prune namespace
/// declarations.
const XI: &str = " xmlns:xi=\"http://www.w3.org/2001/XInclude\"";

/// Parses `main` at `file:///d/doc.xml`, expands it with `files`, and serializes the root.
fn run(main: &str, files: &[(&str, &str)], xi: XInclude) -> Result<String, Error> {
  let mut doc = build::parse_with_system_id(main.as_bytes(), "file:///d/doc.xml").expect("well-formed");
  let mut loader = Files::new(files);
  xi.expand(&mut doc, &mut loader)?;
  Ok(Serializer::new().to_string(&doc, doc.document_element().unwrap()))
}

fn expand(main: &str, files: &[(&str, &str)]) -> String {
  run(main, files, XInclude::new().with_base_fixup(false)).expect("expands")
}

#[test]
fn includes_an_xml_resource() {
  let out = expand(&format!("<doc{NS}><xi:include href='part.xml'/></doc>"), &[("file:///d/part.xml", "<p>hi</p>")]);
  assert_eq!(out, format!("<doc{XI}><p>hi</p></doc>"));
}

#[test]
fn resolves_href_against_the_base_uri() {
  // The document's base is file:///d/doc.xml, so sub/part.xml is file:///d/sub/part.xml.
  let out = expand(&format!("<doc{NS}><xi:include href='sub/part.xml'/></doc>"), &[("file:///d/sub/part.xml", "<p/>")]);
  assert_eq!(out, format!("<doc{XI}><p/></doc>"));
}

#[test]
fn expands_includes_within_included_resources() {
  let out = expand(
    &format!("<doc{NS}><xi:include href='a.xml'/></doc>"),
    &[("file:///d/a.xml", &format!("<a{NS}><xi:include href='b.xml'/></a>")), ("file:///d/b.xml", "<b/>")],
  );
  assert_eq!(out, format!("<doc{XI}><a{XI}><b/></a></doc>"));
}

#[test]
fn includes_text() {
  let out = expand(
    &format!("<doc{NS}><xi:include href='note.txt' parse='text'/></doc>"),
    &[("file:///d/note.txt", "a < b & c")],
  );
  // The included text is escaped as character data.
  assert_eq!(out, format!("<doc{XI}>a &lt; b &amp; c</doc>"));
}

#[test]
fn uses_the_fallback_when_the_resource_is_missing() {
  let out = expand(
    &format!("<doc{NS}><xi:include href='gone.xml'><xi:fallback><p>default</p></xi:fallback></xi:include></doc>"),
    &[],
  );
  assert_eq!(out, format!("<doc{XI}><p>default</p></doc>"));
}

#[test]
fn a_missing_resource_with_no_fallback_is_fatal() {
  let error = run(&format!("<doc{NS}><xi:include href='gone.xml'/></doc>"), &[], XInclude::new()).unwrap_err();
  assert_eq!(error.kind(), ErrorKind::XInclude);
}

#[test]
fn an_inclusion_loop_is_detected() {
  let error = run(
    &format!("<doc{NS}><xi:include href='a.xml'/></doc>"),
    &[
      ("file:///d/a.xml", &format!("<a{NS}><xi:include href='doc.xml'/></a>")),
      ("file:///d/doc.xml", &format!("<doc{NS}><xi:include href='a.xml'/></doc>")),
    ],
    XInclude::new(),
  )
  .unwrap_err();
  assert_eq!(error.kind(), ErrorKind::XInclude);
  assert!(error.message().contains("loop"), "{}", error.message());
}

#[test]
fn the_inclusion_count_is_bounded() {
  let error = run(
    &format!("<doc{NS}><xi:include href='p.xml'/><xi:include href='p.xml'/></doc>"),
    &[("file:///d/p.xml", "<p/>")],
    XInclude::new().with_max_includes(1),
  )
  .unwrap_err();
  assert_eq!(error.kind(), ErrorKind::Limit);
}

#[test]
fn xpointer_shorthand_selects_an_element_by_id() {
  let out = expand(
    &format!("<doc{NS}><xi:include href='parts.xml' xpointer='s1'/></doc>"),
    &[("file:///d/parts.xml", "<root><sec xml:id='s1'><p>a</p></sec><sec xml:id='s2'/></root>")],
  );
  assert_eq!(out, format!("<doc{XI}><sec xml:id=\"s1\"><p>a</p></sec></doc>"));
}

#[test]
fn xpointer_element_scheme_walks_child_positions() {
  // /1 is the document element; /1/2 is its second child element.
  let out = expand(
    &format!("<doc{NS}><xi:include href='parts.xml' xpointer='element(/1/2)'/></doc>"),
    &[("file:///d/parts.xml", "<root><a/><b>hit</b></root>")],
  );
  assert_eq!(out, format!("<doc{XI}><b>hit</b></doc>"));
}

#[test]
fn xpointer_element_scheme_starts_from_an_id() {
  let out = expand(
    &format!("<doc{NS}><xi:include href='parts.xml' xpointer='element(s1/1)'/></doc>"),
    &[("file:///d/parts.xml", "<root><sec xml:id='s1'><p>x</p><q/></sec></root>")],
  );
  assert_eq!(out, format!("<doc{XI}><p>x</p></doc>"));
}

#[test]
fn xpointer_can_select_from_the_same_document() {
  // No href: the xpointer selects part of the document that contains the include.
  let out = expand(&format!("<doc{NS}><data><item xml:id='i'>X</item></data><xi:include xpointer='i'/></doc>"), &[]);
  assert_eq!(out, format!("<doc{XI}><data><item xml:id=\"i\">X</item></data><item xml:id=\"i\">X</item></doc>"));
}

#[test]
fn a_missing_xpointer_target_uses_the_fallback() {
  let out = expand(
    &format!(
      "<doc{NS}><xi:include href='parts.xml' xpointer='nope'><xi:fallback>gone</xi:fallback></xi:include></doc>"
    ),
    &[("file:///d/parts.xml", "<root/>")],
  );
  assert_eq!(out, format!("<doc{XI}>gone</doc>"));
}

#[test]
fn xpointer_is_rejected_with_parse_text() {
  let error = run(
    &format!("<doc{NS}><xi:include href='n.txt' parse='text' xpointer='x'/></doc>"),
    &[("file:///d/n.txt", "hi")],
    XInclude::new(),
  )
  .unwrap_err();
  assert_eq!(error.kind(), ErrorKind::XInclude);
}

#[test]
fn base_fixup_records_the_included_resources_base() {
  // With fixup on, the included element carries xml:base so its own base is preserved.
  let out = run(
    &format!("<doc{NS}><xi:include href='sub/part.xml'/></doc>"),
    &[("file:///d/sub/part.xml", "<p/>")],
    XInclude::new(),
  )
  .unwrap();
  assert_eq!(out, format!("<doc{XI}><p xml:base=\"file:///d/sub/part.xml\"/></doc>"));
}
