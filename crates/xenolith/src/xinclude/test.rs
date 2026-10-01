use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;

use super::*;
use crate::dom::build::DomBuilder;
use crate::io::write::XmlWriter;

/// A resolver over a map of absolute URIs, standing in for a filesystem or a catalogue.
///
/// It records what it was asked for, so a test can say not only what came out but what was fetched to get there.
#[derive(Default)]
struct Map {
  files: HashMap<String, Vec<u8>>,
  asked: RefCell<Vec<String>>,
}

impl Map {
  fn with(files: &[(&str, &str)]) -> Self {
    let files = files.iter().map(|(uri, body)| ((*uri).to_owned(), (*body).as_bytes().to_vec())).collect();
    Self { files, asked: RefCell::default() }
  }
}

impl UriResolver for Map {
  fn resolve(&self, request: &EntityRequest) -> Result<Option<Box<dyn Read>>> {
    let uri = request.resolved_uri().unwrap_or_else(|| request.system_id().to_owned());
    self.asked.borrow_mut().push(uri.clone());
    Ok(self.files.get(&uri).map(|body| Box::new(std::io::Cursor::new(body.clone())) as Box<dyn Read>))
  }
}

/// Reads `xml` through the transform and writes what comes out the other side.
fn written(xml: &str, resolver: &Map) -> Result<String> {
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut include = XIncludeTransform::new().with_resolver(resolver).with_handler(&mut writer);
    StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml").with_handler(&mut include).emit()?;
  }
  Ok(String::from_utf8(writer.into_inner()).expect("UTF-8"))
}

/// As `written`, without the base URI fixup, for the tests that are about what is included rather than about what
/// the inclusion writes onto it.
fn plain(xml: &str, resolver: &Map) -> Result<String> {
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut include = XIncludeTransform::new().with_resolver(resolver).with_xml_base(false).with_handler(&mut writer);
    StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml").with_handler(&mut include).emit()?;
  }
  Ok(String::from_utf8(writer.into_inner()).expect("UTF-8"))
}

/// The document with an `xmlns:xi` declaration on its root, as a caller would write it.
fn doc(content: &str) -> String {
  format!("<doc xmlns:xi=\"{XINCLUDE_NS}\">{content}</doc>")
}

#[test]
fn an_inclusion_takes_the_place_of_the_element() {
  let map = Map::with(&[("file:///doc/part.xml", "<p>included</p>")]);
  let out = written(&doc("<xi:include href='part.xml'/>"), &map).expect("included");
  // The element it brings in says where it came from, which is what the base URI fixup is for.
  let expected =
    "<doc xmlns:xi=\"http://www.w3.org/2001/XInclude\"><p xml:base=\"file:///doc/part.xml\">included</p></doc>";
  assert_eq!(out, expected);
  // The href resolved against the document's own URI, not against the process's working directory.
  assert_eq!(map.asked.borrow().as_slice(), ["file:///doc/part.xml"]);
}

#[test]
fn what_is_around_the_element_is_passed_on_untouched() {
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let out = plain(&doc("before<xi:include href='part.xml'/>after"), &map).expect("included");
  assert!(out.contains("before<p/>after"), "{out}");
}

#[test]
fn the_included_document_keeps_its_own_markup_but_not_its_prolog() {
  let part = "<?xml version='1.0'?><!DOCTYPE p [<!ELEMENT p (#PCDATA)>]><!--c--><p>text</p>";
  let map = Map::with(&[("file:///doc/part.xml", part)]);
  let out = plain(&doc("<xi:include href='part.xml'/>"), &map).expect("included");
  // The comment before the root element is content; the declaration and the document type are not.
  assert!(out.contains("<!--c--><p>text</p>"), "{out}");
  assert!(!out.contains("DOCTYPE"), "{out}");
  assert!(!out.contains("<?xml"), "{out}");
}

#[test]
fn an_inclusion_may_include_further_documents() {
  let map = Map::with(&[
    ("file:///doc/one.xml", "<one><xi:include href='two.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></one>"),
    ("file:///doc/two.xml", "<two/>"),
  ]);
  let out = plain(&doc("<xi:include href='one.xml'/>"), &map).expect("included");
  assert!(out.contains("<one><two/></one>"), "{out}");
  assert_eq!(map.asked.borrow().as_slice(), ["file:///doc/one.xml", "file:///doc/two.xml"]);
}

#[test]
fn a_resource_that_includes_itself_is_refused() {
  let loop_xml = "<a><xi:include href='loop.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></a>";
  let map = Map::with(&[("file:///doc/loop.xml", loop_xml)]);
  let error = written(&doc("<xi:include href='loop.xml'/>"), &map).expect_err("a loop");
  assert!(error.message().contains("circular reference"), "{error}");
  assert!(matches!(error, Error::XInclude { .. }), "{error}");
}

#[test]
fn inclusion_deeper_than_the_limit_is_refused() {
  // Each document includes the next, so the chain is as deep as the files allow.
  let files: Vec<(String, String)> = (0..6)
    .map(|i| {
      let uri = format!("file:///doc/{i}.xml");
      let body = format!("<n><xi:include href='{}.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></n>", i + 1);
      (uri, body)
    })
    .collect();
  let borrowed: Vec<(&str, &str)> = files.iter().map(|(u, b)| (u.as_str(), b.as_str())).collect();
  let map = Map::with(&borrowed);

  let mut writer = XmlWriter::new(Vec::new());
  let error = {
    let limits = Limits { max_depth: Some(3), ..Limits::default() };
    let mut include = XIncludeTransform::new().with_resolver(&map).with_limits(limits).with_handler(&mut writer);
    StreamSource::with_system_id(doc("<xi:include href='0.xml'/>").as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect_err("deeper than the limit")
  };
  assert!(error.message().contains("max_depth"), "{error}");
}

#[test]
fn more_inclusions_than_the_limit_are_refused() {
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let mut writer = XmlWriter::new(Vec::new());
  let error = {
    let limits = Limits { max_includes: Some(2), ..Limits::default() };
    let mut include = XIncludeTransform::new().with_resolver(&map).with_limits(limits).with_handler(&mut writer);
    let xml = doc("<xi:include href='part.xml'/><xi:include href='part.xml'/><xi:include href='part.xml'/>");
    StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect_err("more than the limit")
  };
  assert!(error.message().contains("max_includes"), "{error}");
}

#[test]
fn an_include_without_a_resolver_is_refused_rather_than_dropped() {
  let mut writer = XmlWriter::new(Vec::new());
  let error = {
    let mut include = XIncludeTransform::new().with_handler(&mut writer);
    StreamSource::with_system_id(doc("<xi:include href='part.xml'/>").as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect_err("nothing may be fetched without a resolver")
  };
  assert!(error.message().contains("with_resolver"), "{error}");
}

#[test]
fn a_resource_the_resolver_does_not_have_is_refused() {
  let map = Map::with(&[]);
  let error = written(&doc("<xi:include href='part.xml'/>"), &map).expect_err("nothing to include");
  assert!(error.message().contains("rejected"), "{error}");
}

#[test]
fn text_is_included_as_character_data() {
  let map = Map::with(&[("file:///doc/part.txt", "1 < 2 & 3")]);
  let out = written(&doc("<xi:include href='part.txt' parse='text'/>"), &map).expect("included");
  // What the resource holds is characters, so the writer escapes them rather than taking them for markup.
  assert!(out.contains("1 &lt; 2 &amp; 3"), "{out}");
}

/// Records the character data that reaches it, one entry per event, so a test can see how a run was split.
#[derive(Default)]
struct Texts(Vec<String>);

impl EventHandler for Texts {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if let EventRef::Characters(text) = event {
      self.0.push(text.text.to_owned());
    }
    Ok(())
  }
}

/// The character data an inclusion of `body` reports, with `text_fragment_len` set to `fragment`.
fn fragments(body: &str, fragment: usize) -> Vec<String> {
  let map = Map::with(&[("file:///doc/part.txt", body)]);
  let mut texts = Texts::default();
  {
    let config = ParserConfig { text_fragment_len: fragment, ..ParserConfig::default() };
    let mut include = XIncludeTransform::new().with_resolver(&map).with_config(config).with_handler(&mut texts);
    let xml = doc("<xi:include href='part.txt' parse='text'/>");
    StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect("included");
  }
  texts.0
}

#[test]
fn a_text_resource_is_reported_in_fragments_rather_than_whole() {
  // `ParserConfig::text_fragment_len` bounds what is held at once, as it does for the character data of a document,
  // so a resource of any size costs a fragment rather than its own length.
  assert_eq!(fragments("0123456789abcdef", 4), ["0123", "4567", "89ab", "cdef"]);
  // What is left when the resource ends goes out however short it is.
  assert_eq!(fragments("0123456", 4), ["0123", "456"]);
  assert!(fragments("", 4).is_empty(), "an empty resource has no character data to report");
}

#[test]
fn a_fragment_ends_at_a_character_boundary() {
  // The fragments are reported as `&str` and are one run of characters between them, so no character is cut in half:
  // only one of these three bytes each fits in a fragment of four.
  assert_eq!(fragments("日本語", 4), ["日", "本", "語"]);
  // One character longer than a whole fragment is reported on its own rather than held for ever.
  assert_eq!(fragments("日本語", 1), ["日", "本", "語"]);
}

#[test]
fn what_the_element_says_is_checked() {
  // The vocabulary is the schema's business, and the first fault stops the run before the transform fetches anything.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  for (content, expected) in [
    ("<xi:include href='part.xml' parse='both'/>", "neither"),
    ("<xi:include href='part.xml#section'/>", "fragment identifier"),
    ("<xi:include href='part.xml' accept='text/xml&#10;X: y'/>", "HTTP header"),
    ("<xi:fallback/>", "only as the direct child"),
  ] {
    let error = written(&doc(content), &map).expect_err(content);
    assert!(error.message().contains(expected), "{content}: {error}");
    assert!(matches!(error, Error::Validity { .. }), "{content}: {error}");
  }

  // What the transform refuses is what it cannot carry out, which the document is within its rights to ask for.
  let error = written(&doc("<xi:include xpointer='q'/>"), &map).expect_err("nothing to fetch");
  assert!(error.message().contains("href"), "{error}");
  assert!(matches!(error, Error::XInclude { .. }), "{error}");
}

#[test]
fn what_is_wrong_but_could_still_be_carried_out_is_refused_as_well() {
  // An encoding on an XML resource says nothing the resource does not say itself, so the inclusion could go ahead;
  // the transform judges its input with `XIncludeSchema`, which is stricter than §3.1 here as it documents.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let content = "<xi:include href='part.xml' encoding='UTF-8'/>";
  let error = plain(&doc(content), &map).expect_err("stricter than the specification");
  assert!(matches!(error, Error::Validity { .. }), "{error}");
}

#[test]
fn the_content_of_an_include_element_is_passed_over() {
  // It is where a fallback goes, which is not used yet; either way it is not part of the document.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let out = plain(&doc("<xi:include href='part.xml'><x>ignored</x></xi:include>"), &map).expect("included");
  assert!(out.contains("<p/>"), "{out}");
  assert!(!out.contains("ignored"), "{out}");
}

#[test]
fn the_tree_built_from_the_events_holds_the_inclusion() {
  // The transform is a stage, so what is downstream decides what becomes of the events: here a tree rather than text.
  let map = Map::with(&[("file:///doc/part.xml", "<p>text</p>")]);
  let mut builder = DomBuilder::new();
  {
    let mut include = XIncludeTransform::new().with_resolver(&map).with_handler(&mut builder);
    StreamSource::with_system_id(doc("<xi:include href='part.xml'/>").as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect("included");
  }
  let tree = builder.into_document();
  let root = tree.document_element().expect("a root element");
  assert_eq!(tree.node_name(root), "doc");
  let included = tree.first_child(root).expect("the included element");
  assert_eq!(tree.node_name(included), "p");
  assert_eq!(tree.text_content(included), "text");
}

#[test]
fn an_included_document_resolves_its_own_external_entities() {
  // The resolver is lent to the parser reading the resource as well, so an entity the resource declares is fetched
  // through the same one.
  let part = "<!DOCTYPE p [<!ENTITY e SYSTEM 'entity.txt'>]><p>&e;</p>";
  let map = Map::with(&[("file:///doc/part.xml", part), ("file:///doc/entity.txt", "from the entity")]);
  let out = plain(&doc("<xi:include href='part.xml'/>"), &map).expect("included");
  assert!(out.contains("<p>from the entity</p>"), "{out}");
  assert_eq!(map.asked.borrow().as_slice(), ["file:///doc/part.xml", "file:///doc/entity.txt"]);
}

#[test]
fn a_relative_href_resolves_against_xml_base() {
  let map = Map::with(&[("file:///doc/parts/part.xml", "<p/>")]);
  let out = plain(&doc("<section xml:base='parts/'><xi:include href='part.xml'/></section>"), &map).expect("included");
  assert!(out.contains("<p/>"), "{out}");
  assert_eq!(map.asked.borrow().as_slice(), ["file:///doc/parts/part.xml"]);
}

#[test]
fn the_layout_of_an_included_document_is_not_included() {
  // §4.2.1: what a document contributes is its children, and the whitespace around its root element is not one of
  // them; its comments are, so they stay. The text the including document writes around the element stays too.
  let part = "<?xml version='1.0'?>\n<!-- part -->\n<p/>\n";
  let map = Map::with(&[("file:///doc/part.xml", part)]);
  let out = plain(&doc("[<xi:include href='part.xml'/>]"), &map).expect("included");
  assert!(out.contains("[<!-- part --><p/>]"), "{out}");

  // Text included as text is the inclusion itself, whitespace and all.
  let map = Map::with(&[("file:///doc/part.txt", "\n  indented\n")]);
  let out = plain(&doc("[<xi:include href='part.txt' parse='text'/>]"), &map).expect("included");
  assert!(out.contains("[\n  indented\n]"), "{out}");
}

#[test]
fn a_resource_that_cannot_be_fetched_falls_back_to_the_child() {
  // §4.3: a resource error is what a fallback is for, and its content stands in for the element.
  let map = Map::with(&[]);
  let out = written(&doc("<xi:include href='gone.xml'><xi:fallback><p>instead</p></xi:fallback></xi:include>"), &map)
    .expect("the fallback stood in");
  assert!(out.contains("<p>instead</p>"), "{out}");
}

#[test]
fn an_empty_fallback_includes_nothing() {
  // §4.3: "If the xi:fallback element is empty, the xi:include element is removed from the result."
  let map = Map::with(&[]);
  let out = written(&doc("a<xi:include href='gone.xml'><xi:fallback/></xi:include>b"), &map).expect("nothing");
  assert!(out.contains(">ab<"), "{out}");
}

#[test]
fn a_fallback_may_include_further_documents() {
  let map = Map::with(&[("file:///doc/other.xml", "<other/>")]);
  let content = "<xi:include href='gone.xml'><xi:fallback><xi:include href='other.xml'/></xi:fallback></xi:include>";
  let out = plain(&doc(content), &map).expect("the fallback included");
  assert!(out.contains("<other/>"), "{out}");
  assert_eq!(map.asked.borrow().as_slice(), ["file:///doc/gone.xml", "file:///doc/other.xml"]);
}

#[test]
fn a_failure_inside_a_fallback_is_reported_when_it_has_no_fallback_of_its_own() {
  let map = Map::with(&[]);
  let content =
    "<xi:include href='gone.xml'><xi:fallback><xi:include href='also-gone.xml'/></xi:fallback></xi:include>";
  let error = written(&doc(content), &map).expect_err("nothing to fall back on");
  assert!(error.message().contains("also-gone.xml"), "{error}");
}

#[test]
fn what_is_beside_the_fallback_is_not_content() {
  // Only the fallback stands in for the resource; anything else inside the xi:include is passed over either way.
  let map = Map::with(&[]);
  let content = "<xi:include href='gone.xml'>loose<x/><xi:fallback>used</xi:fallback>more</xi:include>";
  let out = written(&doc(content), &map).expect("the fallback stood in");
  assert!(out.contains(">used<"), "{out}");
  for ignored in ["loose", "<x/>", "more"] {
    assert!(!out.contains(ignored), "{ignored} reached the output: {out}");
  }
}

#[test]
fn a_second_fallback_is_refused() {
  let map = Map::with(&[]);
  let content = "<xi:include href='gone.xml'><xi:fallback>one</xi:fallback><xi:fallback>two</xi:fallback></xi:include>";
  let error = written(&doc(content), &map).expect_err("only one fallback");
  assert!(error.message().contains("only one xi:fallback"), "{error}");
}

#[test]
fn a_resource_that_is_not_well_formed_is_fatal_and_no_fallback_stands_in() {
  // §4.3 says so outright: "Resources that contain non-well-formed XML result in a fatal error, not a resource error."
  let map = Map::with(&[("file:///doc/broken.xml", "<a></b>")]);
  let content = "<xi:include href='broken.xml'><xi:fallback><p>unused</p></xi:fallback></xi:include>";
  let error = written(&doc(content), &map).expect_err("not well-formed");
  assert!(error.message().contains("does not close"), "{error}");
  assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
}

#[test]
fn a_fallback_is_used_when_no_resolver_can_be_asked() {
  // A resource nothing may fetch is a resource error like any other, so a document that says what to do instead is
  // read without one.
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut include = XIncludeTransform::new().with_handler(&mut writer);
    let xml = doc("<xi:include href='part.xml'><xi:fallback><p>instead</p></xi:fallback></xi:include>");
    StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect("the fallback stood in");
  }
  let out = String::from_utf8(writer.into_inner()).expect("UTF-8");
  assert!(out.contains("<p>instead</p>"), "{out}");
}

/// Every fault the validator finds in `xml`, in the order it found them.
fn faults(xml: &str) -> Vec<String> {
  use crate::event::validate::Schema as _;
  let mut validation = XIncludeSchema.validator();
  StreamSource::new(xml.as_bytes()).with_handler(validation.as_event_handler()).emit().expect("well-formed");
  validation.errors().iter().map(|error| error.message().to_owned()).collect()
}

#[test]
fn what_a_fallback_that_is_passed_over_holds_is_refused_all_the_same() {
  // §3.2: "apparent fatal errors caused by the presence, absence, or content of elements and attributes inside the
  // xi:fallback element must not be reported in xi:fallback elements that are ignored." This build does not meet it:
  // the vocabulary is judged before anyone knows whether the fallback will be used, so what it holds is refused even
  // though the resource is there.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let content = "<xi:include href='part.xml'>\
    <xi:fallback><xi:include href='x' parse='both'/><xi:fallback/></xi:fallback>\
    </xi:include>";
  let error = plain(&doc(content), &map).expect_err("judged before the fallback is known to be ignored");
  assert!(matches!(error, Error::Validity { .. }), "{error}");
  assert_eq!(map.asked.borrow().as_slice(), ["file:///doc/part.xml"], "nothing in the fallback was fetched");
}

#[test]
fn what_a_successful_inclusion_may_hold_is_still_read() {
  // §3.1: "The appearance of more than one xi:fallback element, an xi:include element, or any other element from the
  // XInclude namespace is a fatal error." Its own children are read for that however the inclusion went, unlike what
  // its fallback holds.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  for content in [
    "<xi:include href='part.xml'><xi:fallback/><xi:fallback/></xi:include>",
    "<xi:include href='part.xml'><xi:include href='part.xml'/></xi:include>",
    "<xi:include href='part.xml'><xi:something/></xi:include>",
  ] {
    let error = written(&doc(content), &map).expect_err(content);
    assert!(matches!(error, Error::Validity { .. }), "{content}: {error}");
  }
}

#[test]
fn an_xinclude_element_the_namespace_does_not_define_is_refused_inside_a_fallback() {
  // §3.2: "It is a fatal error for the xi:fallback element to contain any elements from the XInclude namespace other
  // than xi:include."
  let map = Map::with(&[]);
  let content = "<xi:include href='gone.xml'><xi:fallback><xi:something/></xi:fallback></xi:include>";
  let error = written(&doc(content), &map).expect_err(content);
  assert!(matches!(error, Error::Validity { .. }), "{error}");
  // Outside a fallback it is an element like any other, which XInclude 1.0 does not constrain; that this build
  // refuses it anyway is what `XIncludeSchema` documents, and the transform judges its input with that schema.
  let error = written(&doc("<xi:something/>"), &map).expect_err("stricter than the specification");
  assert!(matches!(error, Error::Validity { .. }), "{error}");
}

#[test]
fn an_xpointer_is_a_resource_error_rather_than_ignored() {
  // §4.2 requires XPointer and this build has none: "An error in the XPointer is a resource error." Including the
  // whole resource where the document asked for part of it would be a silently different document, so what stands in
  // is the fallback, and without one the element is refused.
  let map = Map::with(&[("file:///doc/part.xml", "<p><q/></p>")]);
  let error = written(&doc("<xi:include href='part.xml' xpointer='q'/>"), &map).expect_err("no XPointer here");
  assert!(error.message().contains("XPointer"), "{error}");
  let content = "<xi:include href='part.xml' xpointer='q'><xi:fallback><r/></xi:fallback></xi:include>";
  let out = plain(&doc(content), &map).expect("the fallback stood in");
  assert!(out.contains("<r/>") && !out.contains("<q/>"), "{out}");
  assert!(map.asked.borrow().is_empty(), "nothing is fetched for an XPointer this build cannot follow");

  // An absent href is a resource error for another reason, which XPointer will not take away (§4.1 allows it):
  // selecting part of the document being processed is what that element asks for, and the pipeline never holds it.
  let error = written(&doc("<xi:include xpointer='q'/>"), &map).expect_err("the document itself");
  assert!(error.message().contains("href"), "{error}");
  assert!(!error.message().contains("XPointer is not yet"), "the two errors read differently: {error}");
  let out = plain(&doc("<xi:include xpointer='q'><xi:fallback><r/></xi:fallback></xi:include>"), &map)
    .expect("the fallback stood in");
  assert!(out.contains("<r/>"), "{out}");
}

#[test]
fn a_fallback_may_include_the_resource_that_failed() {
  // §4.2.7 is about resources being processed. The one that failed never was, so a fallback that takes the whole of
  // it where a part could not be selected is not a loop.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let content = "<xi:include href='part.xml' xpointer='q'><xi:fallback><xi:include href='part.xml'/></xi:fallback>\
    </xi:include>";
  let out = plain(&doc(content), &map).expect("not a loop");
  assert!(out.contains("<p/>"), "{out}");
}

#[test]
fn another_part_of_the_resource_being_processed_is_not_a_loop() {
  // §4.2.7 allows "a different part of the same local resource (same href, different xpointer)". This build cannot
  // select the part, which is a resource error, rather than a loop it would be taken for by the URI alone.
  let part = "<p xmlns:xi='http://www.w3.org/2001/XInclude'>\
    <xi:include href='part.xml' xpointer='q'><xi:fallback><r/></xi:fallback></xi:include></p>";
  let map = Map::with(&[("file:///doc/part.xml", part)]);
  let out = plain(&doc("<xi:include href='part.xml'/>"), &map).expect("not a loop");
  assert!(out.contains("<r/>"), "{out}");
}

#[test]
fn an_encoding_this_build_cannot_decode_is_a_resource_error() {
  // §4.3: "the resource is in an unsupported encoding" is among the resource errors, so a fallback stands in for it.
  let map = Map::with(&[("file:///doc/part.txt", "text")]);
  let content = "<xi:include href='part.txt' parse='text' encoding='x-unknown'><xi:fallback>instead</xi:fallback>\
    </xi:include>";
  let out = plain(&doc(content), &map).expect("the fallback stood in");
  assert!(out.contains(">instead<"), "{out}");
  let content = "<xi:include href='part.txt' parse='text' encoding='x-unknown'/>";
  written(&doc(content), &map).expect_err("nothing to fall back on");
}

#[test]
fn an_inclusion_may_not_put_more_than_one_element_where_the_document_element_goes() {
  // §4.5: "It is a fatal error to attempt to replace an xi:include element appearing as the document (top-level)
  // element in the source infoset with something other than a list of zero or more comments, zero or more processing
  // instructions, and one element."
  let map = Map::with(&[("file:///doc/part.txt", "text"), ("file:///doc/part.xml", "<p/>")]);
  let refused = |xml: &str| {
    let mut writer = XmlWriter::new(Vec::new());
    let mut include = XIncludeTransform::new().with_resolver(&map).with_handler(&mut writer);
    StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect_err("more than the document element allows")
      .message()
      .to_owned()
  };
  let xi = XINCLUDE_NS;
  // Characters where the document element goes, which `parse="text"` is what brings.
  let text = refused(&format!("<xi:include xmlns:xi='{xi}' href='part.txt' parse='text'/>"));
  assert!(text.contains("character data"), "{text}");
  // A fallback that holds two elements, where one document element is all there is room for.
  let two = format!("<xi:include xmlns:xi='{xi}' href='gone.xml'><xi:fallback><a/><b/></xi:fallback></xi:include>");
  let two = refused(&two);
  assert!(two.contains("more than one element"), "{two}");
  // An empty fallback, which leaves no document element at all: "one element" is not met either.
  let none = format!("<xi:include xmlns:xi='{xi}' href='gone.xml'><xi:fallback/></xi:include>");
  let none = refused(&none);
  assert!(none.contains("no element"), "{none}");
}

#[test]
fn the_validator_reports_an_include_with_neither_href_nor_xpointer() {
  let faults = faults(&doc("<xi:include/>"));
  assert_eq!(faults.len(), 1, "{faults:?}");
  assert!(faults[0].contains("needs an href"), "{faults:?}");
}

#[test]
fn the_validator_reports_a_fragment_identifier_in_an_href() {
  let faults = faults(&doc("<xi:include href='part.xml#section'/>"));
  assert_eq!(faults.len(), 1, "{faults:?}");
  assert!(faults[0].contains("fragment identifier"), "{faults:?}");
}

#[test]
fn the_validator_reports_an_xpointer_where_the_resource_is_text() {
  let faults = faults(&doc("<xi:include href='a' parse='text' xpointer='x'/>"));
  assert_eq!(faults.len(), 1, "{faults:?}");
  assert!(faults[0].contains("parse=\"text\""), "{faults:?}");
}

#[test]
fn the_validator_reports_a_header_value_no_header_may_carry() {
  for content in
    ["<xi:include href='a' accept='text/xml&#10;X: y'/>", "<xi:include href='a' accept-language='\u{e9}'/>"]
  {
    let faults = faults(&doc(content));
    assert_eq!(faults.len(), 1, "{content}: {faults:?}");
    assert!(faults[0].contains("HTTP header"), "{content}: {faults:?}");
  }
  // Within #x20-#x7E it is the document's business what it asks for.
  assert!(faults(&doc("<xi:include href='a' accept='text/xml' accept-language='en, fr;q=0.5'/>")).is_empty());
}

#[test]
fn the_validator_reads_the_encoding_as_a_name_rather_than_as_something_to_decode_with() {
  // An encoding name this build cannot decode with is still a name, and whether it can be decoded is the inclusion's
  // business rather than the document's.
  assert!(faults(&doc("<xi:include href='a' parse='text' encoding='Shift_JIS'/>")).is_empty());
  let faults = faults(&doc("<xi:include href='a' parse='text' encoding='8859-1'/>"));
  assert_eq!(faults.len(), 1, "{faults:?}");
  assert!(faults[0].contains("is not an encoding name"), "{faults:?}");
}

#[test]
fn the_validator_reports_an_attribute_carrying_the_xinclude_namespace() {
  // The attributes XInclude defines are in no namespace (Namespaces in XML §6.2), so a prefixed one is a different
  // name, in a namespace where XInclude defines nothing at all.
  let on_include = faults(&doc("<xi:include xi:href='part.xml'/>"));
  // The element also has no href of its own, since `xi:href` is not one, which is the second fault.
  assert_eq!(on_include.len(), 2, "{on_include:?}");
  assert!(on_include[0].contains("xi:href is not an attribute XInclude defines"), "{on_include:?}");
  assert!(on_include[1].contains("needs an href"), "{on_include:?}");

  // On a fallback it stands the same way.
  let on_fallback = faults(&doc("<xi:include href='a'><xi:fallback xi:href='b'/></xi:include>"));
  assert_eq!(on_fallback.len(), 1, "{on_fallback:?}");
  assert!(on_fallback[0].contains("is not an attribute XInclude defines"), "{on_fallback:?}");
}

#[test]
fn the_validator_passes_over_an_attribute_xinclude_does_not_define() {
  // §3.1 reserves unprefixed names for a later version and requires them to be ignored.
  assert!(faults(&doc("<xi:include href='a' fixup-xml-base='true'/>")).is_empty());
  // One in a namespace of its own is somebody else's either way.
  assert!(faults(&doc("<xi:include href='a' xml:id='x'/>")).is_empty());
}

#[test]
fn the_validator_accepts_an_xpointer_without_an_href() {
  // §3.1 allows it, and the vocabulary is what is being judged here; that this build cannot follow an xpointer is the
  // transform's business, not the document's.
  assert!(faults(&doc("<xi:include xpointer='x'/>")).is_empty());
}

#[test]
fn the_validator_reports_what_the_attributes_say() {
  for (content, expected) in [
    ("<xi:include href='a' parse='both'/>", "neither"),
    ("<xi:include href='a' encoding='UTF-8'/>", "no effect where parse=\"xml\""),
  ] {
    let faults = faults(&doc(content));
    assert_eq!(faults.len(), 1, "{content}: {faults:?}");
    assert!(faults[0].contains(expected), "{content}: {faults:?}");
  }
  // An encoding with parse="text" is what it is for.
  assert!(faults(&doc("<xi:include href='a' parse='text' encoding='UTF-8'/>")).is_empty());
}

#[test]
fn the_validator_reports_a_fallback_that_has_no_include() {
  let loose = faults(&doc("<xi:fallback/>"));
  assert_eq!(loose.len(), 1, "{loose:?}");
  assert!(loose[0].contains("only as the direct child"), "{loose:?}");

  // One inside an element that is not an include is just as loose.
  let inside = faults(&doc("<section><xi:fallback/></section>"));
  assert_eq!(inside.len(), 1, "{inside:?}");
}

#[test]
fn the_validator_reports_a_second_fallback() {
  let content = "<xi:include href='a'><xi:fallback/><xi:fallback/></xi:include>";
  let faults = faults(&doc(content));
  assert_eq!(faults.len(), 1, "{faults:?}");
  assert!(faults[0].contains("only one xi:fallback"), "{faults:?}");
}

#[test]
fn the_validator_reports_an_include_inside_an_include_outside_its_fallback() {
  let nested = faults(&doc("<xi:include href='a'><xi:include href='b'/></xi:include>"));
  assert_eq!(nested.len(), 1, "{nested:?}");
  assert!(nested[0].contains("within its xi:fallback"), "{nested:?}");

  // Inside the fallback it is allowed, which is how a fallback includes something else instead.
  let content = "<xi:include href='a'><xi:fallback><xi:include href='b'/></xi:fallback></xi:include>";
  assert!(faults(&doc(content)).is_empty());
}

#[test]
fn the_validator_leaves_alone_what_the_specification_does_not_constrain() {
  // §3.1: text, comments, processing instructions, elements outside the XInclude namespace and the descendants of any
  // child are not constrained and are ignored. The transform passes them over; the document is not at fault for them.
  assert!(faults(&doc("<xi:include href='a'><p>text</p><!--c--><?pi?>text</xi:include>")).is_empty());
  // Even an `xi:include`, so long as it is not a child: what holds it is not part of the vocabulary.
  assert!(faults(&doc("<xi:include href='a'><p><xi:include href='b'/></p></xi:include>")).is_empty());
}

#[test]
fn the_validator_reports_an_element_the_namespace_does_not_have() {
  let faults = faults(&doc("<xi:something/>"));
  assert_eq!(faults.len(), 1, "{faults:?}");
  assert!(faults[0].contains("is not an element XInclude defines"), "{faults:?}");
}

#[test]
fn the_validator_finds_every_fault_rather_than_the_first() {
  let content = "<xi:include/><xi:fallback/><xi:include href='a' parse='both'/>";
  let faults = faults(&doc(content));
  assert_eq!(faults.len(), 3, "{faults:?}");
}

#[test]
fn an_included_element_says_where_it_came_from() {
  // §4.5.5: the element keeps the base URI it had in the resource it was written in, so a relative reference inside
  // it still resolves to what its author meant.
  let map = Map::with(&[("file:///doc/parts/part.xml", "<p><a href='next.xml'/></p>")]);
  let out = written(&doc("<xi:include href='parts/part.xml'/>"), &map).expect("included");
  assert!(out.contains("<p xml:base=\"file:///doc/parts/part.xml\">"), "{out}");
}

#[test]
fn an_element_from_the_same_place_is_left_alone() {
  // Nothing differs, so nothing is written: the fixup says where an element came from, not where it is.
  let map = Map::with(&[("file:///doc/main.xml", "<p/>")]);
  let out = written(&doc("<xi:include href='main.xml'/>"), &map).expect("included");
  assert!(!out.contains("xml:base"), "{out}");
}

#[test]
fn what_the_element_said_itself_is_replaced_by_what_it_means() {
  // §4.5.5: "If an xml:base attribute information item is already present, it is replaced by the new attribute." The
  // element wrote its base relative to the resource it was in; read against the base of the including document, the
  // same characters would name something else.
  let map = Map::with(&[("file:///doc/parts/part.xml", "<p xml:base='sub/'/>")]);
  let out = written(&doc("<xi:include href='parts/part.xml'/>"), &map).expect("included");
  assert!(out.contains("xml:base=\"file:///doc/parts/sub/\""), "{out}");
  assert_eq!(out.matches("xml:base").count(), 1, "replaced rather than kept beside the fixup: {out}");
}

#[test]
fn the_language_of_an_included_element_is_kept() {
  // §4.5.6: the language in effect where the element was written is part of what it means.
  let map = Map::with(&[("file:///doc/part.xml", "<p xml:lang='fr'><q/></p>")]);
  let out = written(&doc("<xi:include href='part.xml'/>"), &map).expect("included");
  // What the element said is what the fixup says, so the one it wrote is replaced by the same value.
  assert!(out.contains("xml:lang=\"fr\""), "{out}");
  assert_eq!(out.matches("xml:lang").count(), 1, "{out}");
  // The inner element inherits it as it did in the resource.
  assert!(out.contains("<q/>"), "{out}");

  // A resource whose root has no language of its own, included where another language is in effect.
  let map = Map::with(&[("file:///doc/part.xml", "<p/>")]);
  let out = written(&doc("<section xml:lang='de'><xi:include href='part.xml'/></section>"), &map).expect("included");
  // §4.5.6 reads `xml:lang=""` as no language at all, which is what the element had where it was written; without it
  // the element would read as German for having been put here.
  assert!(out.contains("xml:lang=\"\""), "{out}");
}

#[test]
fn a_language_that_differs_only_in_case_is_the_same_language() {
  // §4.5.6 compares "taking case-insensitivity into account per [IETF RFC 3066]", so there is nothing to say.
  let map = Map::with(&[("file:///doc/part.xml", "<p xml:lang='EN'/>")]);
  let out = written(&doc("<section xml:lang='en'><xi:include href='part.xml'/></section>"), &map).expect("included");
  // The element keeps the tag it wrote, since a fixup replaces only what it has something else to say about.
  assert!(out.contains("<p xml:lang=\"EN\" xml:base=\"file:///doc/part.xml\"/>"), "{out}");
}

#[test]
fn the_fixups_can_be_turned_off() {
  let map = Map::with(&[("file:///doc/parts/part.xml", "<p/>")]);
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut include = XIncludeTransform::new().with_resolver(&map).with_xml_base(false).with_handler(&mut writer);
    StreamSource::with_system_id(doc("<xi:include href='parts/part.xml'/>").as_bytes(), "file:///doc/main.xml")
      .with_handler(&mut include)
      .emit()
      .expect("included");
  }
  let out = String::from_utf8(writer.into_inner()).expect("UTF-8");
  assert!(!out.contains("xml:base"), "{out}");
}

#[test]
fn a_fixup_is_written_at_each_depth_of_inclusion() {
  let map = Map::with(&[
    (
      "file:///doc/a/one.xml",
      "<one><xi:include href='../b/two.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></one>",
    ),
    ("file:///doc/b/two.xml", "<two/>"),
  ]);
  let out = written(&doc("<xi:include href='a/one.xml'/>"), &map).expect("included");
  assert!(out.contains("<one xml:base=\"file:///doc/a/one.xml\">"), "{out}");
  assert!(out.contains("<two xml:base=\"file:///doc/b/two.xml\"/>"), "{out}");
}

#[test]
fn the_handler_behind_is_told_that_a_run_failed_inside_an_inclusion() {
  // Only the transform hears how the run ended, so a failure in the middle of an inclusion reaches the handler behind
  // it as a failure rather than as a run given up.
  #[derive(Default)]
  struct Ended(Option<&'static str>);
  impl EventHandler for Ended {
    fn handle(&mut self, _event: &EventRef<'_>) -> Result<()> {
      Ok(())
    }
    fn finish(&mut self, outcome: Outcome<'_>) {
      self.0.get_or_insert(match outcome {
        Outcome::Completed => "completed",
        Outcome::Stopped => "stopped",
        Outcome::Failed(_) => "failed",
        _ => "abandoned",
      });
    }
  }

  let map = Map::with(&[("file:///doc/broken.xml", "<a></b>")]);
  let mut ended = Ended::default();
  {
    let mut include = XIncludeTransform::new().with_resolver(&map).with_handler(&mut ended);
    let xml = doc("<xi:include href='broken.xml'/>");
    let result = StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml").with_handler(&mut include).emit();
    assert!(result.is_err(), "not well-formed");
  }
  assert_eq!(ended.0, Some("failed"));
}
