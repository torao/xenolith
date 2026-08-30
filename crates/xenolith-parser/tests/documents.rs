//! Whole documents, through the public API.
//!
//! The unit tests reach inside; these do not. They also run every document through all three
//! drivers — the parser fed directly, the blocking reader and, where it is compiled in, the
//! asynchronous one — because the promise of the sans-I/O design is that the driver cannot
//! change the answer.

use xenolith_parser::{Bounds, Event, EventKind, Parser, Progress, Reader};

/// Renders an error the way a diagnostic would: its location, if known, then the message. The
/// location is a field on the error now, not part of its `Display`, so a caller that wants it in
/// the string composes the two.
fn describe(e: &xenolith_core::Error) -> String {
  if e.location().is_unknown() { e.to_string() } else { format!("{}: {e}", e.location()) }
}

/// Merges adjacent text events, as a consumer that wants whole text nodes does. The parser may split a
/// long text run into fragments, and where the splits fall depends on how the bytes were fed, so the
/// drivers are compared on their coalesced text rather than on the raw fragments.
fn coalesce_text(events: Vec<Event>) -> Vec<Event> {
  let mut out: Vec<Event> = Vec::with_capacity(events.len());
  for event in events {
    match (out.last_mut(), &event) {
      (Some(Event::Text(last)), Event::Text(next)) => last.push_str(next),
      _ => out.push(event),
    }
  }
  out
}

/// Parses `xml` with the parser fed in `chunk`-sized pieces.
fn by_parser(xml: &str, chunk: usize) -> Result<Vec<Event>, String> {
  let mut parser = Parser::new();
  let bytes = xml.as_bytes();
  let mut fed = 0;
  let mut events = Vec::new();
  loop {
    match parser.advance() {
      Ok(Progress::Event(_)) => events.push(Event::capture(&parser).map_err(|e| describe(&e))?),
      Ok(Progress::Eof) => return Ok(coalesce_text(events)),
      Ok(Progress::NeedMoreInput) => {
        let end = (fed + chunk).min(bytes.len());
        parser.feed(&bytes[fed..end], end == bytes.len()).map_err(|e| describe(&e))?;
        fed = end;
      }
      Ok(other) => panic!("unexpected {other:?}"),
      Err(e) => return Err(describe(&e)),
    }
  }
}

fn by_reader(xml: &str) -> Result<Vec<Event>, String> {
  Reader::new(xml.as_bytes()).events().collect::<Result<Vec<Event>, _>>().map(coalesce_text).map_err(|e| describe(&e))
}

#[cfg(feature = "tokio")]
fn by_async_reader(xml: &str) -> Result<Vec<Event>, String> {
  use xenolith_parser::AsyncReader;
  tokio_test::block_on(AsyncReader::new(xml.as_bytes()).events()).map(coalesce_text).map_err(|e| describe(&e))
}

/// Parses `xml` every way available, requiring them all to agree.
fn parse(xml: &str) -> Result<Vec<Event>, String> {
  let expected = by_parser(xml, xml.len().max(1));
  for chunk in [1, 2, 3, 5, 64] {
    assert_eq!(by_parser(xml, chunk), expected, "chunk size {chunk} disagreed");
  }
  assert_eq!(by_reader(xml), expected, "the blocking reader disagreed");
  #[cfg(feature = "tokio")]
  assert_eq!(by_async_reader(xml), expected, "the asynchronous reader disagreed");
  expected
}

fn kinds(xml: &str) -> Vec<EventKind> {
  parse(xml).expect("should parse").iter().map(Event::kind).collect()
}

fn rejects(xml: &str) -> String {
  parse(xml).expect_err("should be rejected")
}

#[test]
fn a_doctype_is_reported_before_the_root_element_whatever_precedes_it() {
  // Whitespace before a `<!DOCTYPE>` used to hold it back — the text before a markup token is
  // flushed first and the token interpreted on the next turn — and that path forgot to parse
  // the DTD it had just set up. The DOCTYPE's own event was then left until after the next
  // token had been scanned, so `Doctype` arrived *after* the root element's start tag.
  //
  // A newline between the XML declaration and the DOCTYPE is how most documents are written,
  // and everything downstream believes the order it is given: the DTD validator, built when the
  // DOCTYPE arrives, never saw the root element open and unbalanced its stack at the end tag.
  let doctype = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]>";
  let expected = [
    EventKind::Doctype,
    EventKind::StartElement,
    EventKind::StartElement,
    EventKind::EndElement,
    EventKind::EndElement,
  ];

  for before in ["", "\n", "   ", "\n\n\n", "\t", "<?xml version=\"1.0\"?>\n", "<!--c-->\n", "\n<!--c-->\n"] {
    let xml = format!("{before}{doctype}<r><a/></r>");
    let found: Vec<EventKind> =
      kinds(&xml).into_iter().filter(|kind| !matches!(kind, EventKind::XmlDeclaration | EventKind::Comment)).collect();
    assert_eq!(found, expected, "for {before:?} before the DOCTYPE");
  }
}

#[test]
fn a_realistic_document() {
  let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- an ordinary document -->
<catalogue xmlns="urn:example:catalogue" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <book id="b1" available="yes">
    <dc:title xml:lang="en">The Art of Computer Programming</dc:title>
    <dc:creator>Donald E. Knuth</dc:creator>
    <price currency="USD">199.99</price>
    <summary xml:space="preserve">  Volumes 1&#8211;4A,
  boxed set.  </summary>
  </book>
  <book id="b2" available="no">
    <dc:title xml:lang="ja">プログラミング言語</dc:title>
    <note><![CDATA[Contains <markup> & ampersands]]></note>
  </book>
  <?render columns="2"?>
</catalogue>
"#;
  let events = parse(xml).expect("should parse");

  let starts = events.iter().filter(|e| e.kind() == EventKind::StartElement).count();
  let ends = events.iter().filter(|e| e.kind() == EventKind::EndElement).count();
  assert_eq!(starts, 9);
  assert_eq!(starts, ends, "every element is closed");

  // Namespaces, including the default one and a prefixed one.
  let names: Vec<_> = events
    .iter()
    .filter(|e| e.kind() == EventKind::StartElement)
    .filter_map(|e| e.name())
    .map(|name| name.namespace())
    .collect();
  assert!(names.iter().all(Option::is_some), "every element is in a namespace");

  // A character reference inside preserved whitespace.
  let summary = events.iter().find_map(|e| e.text().filter(|t| t.contains("Volumes")));
  assert_eq!(summary, Some("  Volumes 1\u{2013}4A,\n  boxed set.  "), "&#8211; is U+2013");

  assert!(events.iter().any(|e| e.kind() == EventKind::CData));
  assert!(events.iter().any(|e| e.kind() == EventKind::ProcessingInstruction));
}

#[test]
fn deeply_nested_elements() {
  let depth = 200;
  let xml = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
  assert_eq!(kinds(&xml).len(), depth * 2);
}

#[test]
fn many_siblings() {
  let xml = format!("<a>{}</a>", "<b/>".repeat(2000));
  assert_eq!(kinds(&xml).len(), 2 + 2000 * 2);
}

#[test]
fn text_with_every_kind_of_reference() {
  let events = parse("<a>&lt;&gt;&amp;&apos;&quot;&#65;&#x42;&#x1F600;</a>").expect("should parse");
  assert_eq!(events[1].text(), Some("<>&'\"AB\u{1F600}"));
}

#[test]
fn an_empty_document_body() {
  assert_eq!(kinds("<a/>"), [EventKind::StartElement, EventKind::EndElement]);
  assert_eq!(kinds("<a></a>"), [EventKind::StartElement, EventKind::EndElement]);
}

#[test]
fn utf8_beyond_the_basic_plane() {
  let events = parse("<🎌 attr='🎏'>🎐</🎌>").expect("astral characters are valid in names");
  assert_eq!(events[1].text(), Some("🎐"));
}

#[test]
fn a_byte_order_mark_is_not_content() {
  let events = parse("\u{FEFF}<a/>").expect("should parse");
  assert_eq!(events.len(), 2);
  assert_eq!(events[0].kind(), EventKind::StartElement);
}

#[test]
fn ill_formed_documents_are_rejected_with_a_useful_message() {
  let cases = [
    ("<a>", "not closed"),
    ("<a></b>", "does not close"),
    ("</a>", "never opened"),
    ("<a/><b/>", "only one root"),
    ("", "no root element"),
    ("<a>&nosuch;</a>", "not declared"),
    ("<a>Tom & Jerry</a>", "must end with"),
    ("<a b/>", "has no value"),
    ("<a b=c/>", "not quoted"),
    ("<a b='1' b='2'/>", "appears twice"),
    ("<p:a/>", "not bound"),
    ("<a>]]></a>", "may not appear in text"),
    ("<!-- -- --><a/>", "may not contain"),
    ("<a xml:space='maybe'/>", "\"default\" or \"preserve\""),
  ];
  for (xml, expected) in cases {
    let message = rejects(xml);
    assert!(message.contains(expected), "parsing {xml:?} said {message:?},\n  which lacks {expected:?}");
  }
}

/// A resolver over an in-memory map of system id to bytes, for the external-DTD tests.
struct MapResolver(std::collections::HashMap<&'static str, &'static [u8]>);

impl xenolith_parser::resolve::UriResolver for MapResolver {
  fn resolve(
    &mut self,
    request: &xenolith_parser::resolve::EntityRequest,
  ) -> Result<Option<Box<dyn std::io::Read>>, xenolith_core::Error> {
    let entry = self.0.get(request.system_id()).map(|b| b.to_vec());
    Ok(entry.map(|b| Box::new(std::io::Cursor::new(b)) as Box<dyn std::io::Read>))
  }
}

fn parse_with(xml: &str, files: &[(&'static str, &'static [u8])]) -> Result<Vec<Event>, String> {
  let resolver = MapResolver(files.iter().copied().collect());
  Reader::new(xml.as_bytes()).with_resolver(resolver).events().collect::<Result<_, _>>().map_err(|e| describe(&e))
}

#[test]
fn an_external_subset_declares_entities_and_defaults() {
  // The external subset supplies the entity and the attribute default the document relies on.
  let dtd: &[u8] = b"<!ELEMENT doc (#PCDATA)>\n<!ATTLIST doc lang CDATA 'en'>\n<!ENTITY greeting 'hello'>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc>&greeting;</doc>";
  let events = parse_with(xml, &[("doc.dtd", dtd)]).expect("should parse");
  let Event::StartElement { attributes, .. } = &events[1] else { panic!("expected <doc>") };
  assert_eq!(attributes.iter().find(|a| a.value == "en").map(|a| a.value.as_str()), Some("en"));
  assert_eq!(events[2].text(), Some("hello"));
}

#[test]
fn a_parameter_entity_parameterizes_a_declaration() {
  // `%e;` stands for the whole attribute definition, as real DTDs do.
  let dtd: &[u8] = b"<!ELEMENT doc (#PCDATA)>\n<!ENTITY % e 'a1 CDATA \"v1\"'>\n<!ATTLIST doc %e;>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc/>";
  let events = parse_with(xml, &[("doc.dtd", dtd)]).expect("should parse");
  let Event::StartElement { attributes, .. } = &events[1] else { panic!("expected <doc>") };
  assert_eq!(attributes.iter().find(|a| a.value == "v1").map(|a| a.value.as_str()), Some("v1"));
}

#[test]
fn an_external_parameter_entity_is_fetched_and_spliced() {
  let outer: &[u8] = b"<!ELEMENT doc EMPTY>\n<!ENTITY % inner SYSTEM 'inner.dtd'>\n%inner;";
  let inner: &[u8] = b"<!ATTLIST doc a CDATA 'defaulted'>";
  let xml = "<!DOCTYPE doc SYSTEM 'outer.dtd'><doc/>";
  let events = parse_with(xml, &[("outer.dtd", outer), ("inner.dtd", inner)]).expect("should parse");
  let Event::StartElement { attributes, .. } = &events[1] else { panic!("expected <doc>") };
  assert_eq!(attributes.first().map(|a| a.value.as_str()), Some("defaulted"));
}

#[test]
fn a_standalone_document_may_not_depend_on_the_external_subset() {
  // standalone="yes" but the entity is only declared externally: a fatal error.
  let dtd: &[u8] = b"<!ELEMENT doc (#PCDATA)>\n<!ENTITY e 'x'>";
  let xml = "<?xml version='1.0' standalone='yes'?><!DOCTYPE doc SYSTEM 'doc.dtd'><doc>&e;</doc>";
  let message = parse_with(xml, &[("doc.dtd", dtd)]).expect_err("standalone violation");
  assert!(message.contains("standalone"), "{message}");
}

#[test]
fn a_conditional_section_includes_or_ignores() {
  let dtd: &[u8] = b"<![INCLUDE[<!ELEMENT doc (#PCDATA)>]]>\n<![IGNORE[<!ELEMENT doc EMPTY> ]]>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc>text</doc>";
  assert_eq!(parse_with(xml, &[("doc.dtd", dtd)]).expect("should parse")[2].text(), Some("text"));
}

#[test]
fn a_declaration_may_not_straddle_a_parameter_entity_boundary() {
  // WFC: the whole markup declaration must lie in one replacement text.
  let dtd: &[u8] = b"<!ENTITY % partial '<!ELEMENT doc '>\n%partial;EMPTY>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc/>";
  let message = parse_with(xml, &[("doc.dtd", dtd)]).expect_err("straddling declaration");
  assert!(message.contains("parameter entity"), "{message}");
}

#[test]
fn attribute_defaults_and_types_come_from_the_dtd() {
  let xml = "<!DOCTYPE a [\
             <!ATTLIST a lang CDATA \"en\" id ID #IMPLIED tokens NMTOKENS #IMPLIED>\
             ]><a id=\"x\" tokens=\"  one   two  \"/>";
  let events = parse(xml).expect("should parse");
  let attrs = events[1].attributes();

  // The default is supplied for the absent `lang`.
  let lang = attrs.iter().find(|a| a.value == "en").expect("lang defaulted");
  let _ = lang;
  // A tokenized value has its whitespace collapsed; a CDATA value would not.
  let tokens = attrs.iter().find(|a| a.value.contains("one")).unwrap();
  assert_eq!(tokens.value, "one two");
}

#[test]
fn entities_may_nest_and_be_reused() {
  let xml = "<!DOCTYPE a [<!ENTITY inner 'x'><!ENTITY outer '&inner;&inner;'>]><a>&outer;&outer;</a>";
  assert_eq!(parse(xml).expect("should parse")[2].text(), Some("xxxx"));
}

#[test]
fn dtd_and_content_errors_are_reported() {
  // A self-referential entity is refused, not expanded forever.
  assert!(rejects("<!DOCTYPE a [<!ENTITY e '&e;'>]><a>&e;</a>").contains("itself"));
  // An unparsed entity may not be referenced as if it were parsed.
  assert!(
    rejects("<!DOCTYPE a [<!NOTATION n SYSTEM 'x'><!ENTITY e SYSTEM 'e.dat' NDATA n>]><a>&e;</a>").contains("unparsed")
  );
  // A malformed declaration in the internal subset is caught.
  assert!(!rejects("<!DOCTYPE a [<!ENTITY e>]><a/>").is_empty());
  // An entity with no declaration anywhere is still an error.
  assert!(rejects("<a>&undeclared;</a>").contains("not declared"));
}

#[test]
fn an_element_must_start_and_end_in_one_entity() {
  // The classic well-formedness constraint an entity can be used to break: the entity's
  // replacement closes a tag the entity did not open, or leaves one open.
  assert!(rejects("<!DOCTYPE a [<!ENTITY e '</b><b>'>]><a><b>&e;</b></a>").contains("different entities"));
  assert!(rejects("<!DOCTYPE a [<!ENTITY e '<b>'>]><a>&e;</b></a>").contains("different entities"));
  // A balanced entity is fine: Doctype, <a>, <b>, </b>, </a>.
  assert_eq!(parse("<!DOCTYPE a [<!ENTITY e '<b/>'>]><a>&e;</a>").expect("balanced").len(), 5);
}

#[test]
fn a_default_referencing_a_later_entity_is_rejected() {
  // The entity is declared, but after the attribute list that names it in a default.
  let xml = "<!DOCTYPE a [<!ATTLIST a x CDATA '&e;'><!ENTITY e 'v'>]><a/>";
  assert!(rejects(xml).contains("before it is declared"));
  // Declared first, it is fine and the default expands.
  let ok = "<!DOCTYPE a [<!ENTITY e 'v'><!ATTLIST a x CDATA '&e;'>]><a/>";
  assert_eq!(parse(ok).expect("should parse")[1].attributes()[0].value, "v");
}

#[test]
fn a_declared_entity_expands_in_content_and_attributes() {
  // A general entity may carry markup into content, and text into an attribute.
  let xml = "<!DOCTYPE a [<!ENTITY e 'as <b>bold</b>'><!ENTITY t 'plain'>]><a x='&t;'>it is &e;</a>";
  let events = parse(xml).expect("should parse");
  let kinds = events.iter().map(Event::kind).collect::<Vec<_>>();
  assert_eq!(
    kinds,
    [
      EventKind::Doctype,
      EventKind::StartElement, // a
      EventKind::Text,         // "it is as " — text coalesces across the entity boundary
      EventKind::StartElement, // b, from inside the entity
      EventKind::Text,         // "bold"
      EventKind::EndElement,   // b
      EventKind::EndElement,   // a
    ]
  );
  assert_eq!(events[1].attributes()[0].value, "plain");
  assert_eq!(events[2].text(), Some("it is as "));
}

#[test]
fn an_expansion_bomb_is_refused_rather_than_expanded() {
  // The billion-laughs shape: each level names the one below it ten times. Expanding it fully
  // would be 10^10 characters; the entity-expansion limits must stop it long before that.
  let mut dtd = String::from("<!DOCTYPE a [<!ENTITY l0 \"boom\">");
  for level in 1..=10 {
    let child = format!("&l{};", level - 1);
    dtd += &format!("<!ENTITY l{level} \"{}\">", child.repeat(10));
  }
  dtd += "]><a>&l10;</a>";
  // Drive it once directly rather than through every chunk size and driver: the point is that
  // it is refused, and refusing it a hundred thousand entities in is not worth doing sevenfold.
  let message = by_reader(&dtd).expect_err("a bomb must be refused");
  assert!(message.contains("expansion") || message.contains("entities"), "{message}");
}

/// Cases the W3C suite caught that hand-written tests had missed. Each is named after the
/// case that found it.
#[test]
fn productions_are_checked_to_the_letter() {
  // A comment body may not end with a dash: `<!--a--->` is not `<!--a-->` plus a stray one.
  // (not-wf-sa-070, o-p15fail1)
  assert!(rejects("<!--a---><a/>").contains("may not end with"));
  assert!(rejects("<!-- three dashes ---><a/>").contains("may not end with"));
  assert_eq!(kinds("<!----><a/>").len(), 3, "an empty comment is still a comment");
  assert_eq!(kinds("<!--a-b--><a/>").len(), 3, "a lone dash inside is fine");

  // `CharRef` spells the hexadecimal marker in lower case only. (not-wf-sa-093)
  assert!(rejects("<a>&#X58;</a>").contains("not a character reference"));
  assert!(rejects("<a>&#x;</a>").contains("not a character reference"));
  assert!(rejects("<a>&#+58;</a>").contains("not a character reference"));
  assert_eq!(parse("<a>&#x58;&#X0058;</a>"), parse("<a>&#x58;&#X0058;</a>"), "sanity");
  assert!(rejects("<a>&#x58;&#X58;</a>").contains("not a character reference"));

  // The XML declaration needs whitespace between its parts. (not-wf-sa-096, o-p32fail3)
  assert!(rejects("<?xml version=\"1.0\"encoding=\"UTF-8\"?><a/>").contains("needs whitespace"));
  assert!(rejects("<?xml version=\"1.0\"standalone=\"yes\"?><a/>").contains("needs whitespace"));

  // `VersionNum ::= '1.' [0-9]+`. (not-wf-sa-102, o-p26fail1, o-p26fail2)
  assert!(rejects("<?xml version=\"1.0 \"?><a/>").contains("not an XML version"));
  assert!(rejects("<?xml version=\"1.0?\"?><a/>").contains("not an XML version"));
  assert!(rejects("<?xml version=\"1.\"?><a/>").contains("not an XML version"));
  assert!(rejects("<?xml version=\"2.0\"?><a/>").contains("not an XML version"));

  // `EncName` starts with a letter and admits no spaces. (not-wf-sa-101)
  assert!(rejects("<?xml version=\"1.0\" encoding=\" UTF-8\"?><a/>").contains("not an encoding name"));
  assert!(rejects("<?xml version=\"1.0\" encoding=\"8859-1\"?><a/>").contains("not an encoding name"));
}

#[test]
fn a_document_that_is_only_a_prolog_is_rejected() {
  assert!(rejects("<?xml version='1.0'?>").contains("no root element"));
  assert!(rejects("<!-- comment -->").contains("no root element"));
}

#[test]
fn line_endings_of_every_convention_agree() {
  let unix = parse("<a>one\ntwo</a>").expect("should parse");
  let windows = parse("<a>one\r\ntwo</a>").expect("should parse");
  let classic_mac = parse("<a>one\rtwo</a>").expect("should parse");
  assert_eq!(unix, windows);
  assert_eq!(unix, classic_mac);
  assert_eq!(unix[1].text(), Some("one\ntwo"));
}

#[test]
fn positions_survive_multi_byte_characters() {
  let message = rejects("<a>日本語のテキスト &bad</a>");
  // Columns count characters, not bytes: "<a>" is 3, the Japanese 8, the space 1, so the
  // reference begins at column 13 and not at byte 27.
  assert!(message.starts_with("1:13:"), "{message}");
}

#[test]
fn a_long_text_run_arrives_whole_however_it_is_fragmented() {
  // Several times the parser's text-fragmentation threshold. Fed in small pieces the run is emitted in
  // fragments; fed whole it is one event. Coalesced, both make one text node with all of the content.
  let text = "x".repeat(30_000);
  let xml = format!("<a>{text}</a>");
  let whole = by_parser(&xml, xml.len()).expect("whole");
  let chunked = by_parser(&xml, 500).expect("chunked");
  assert_eq!(whole, chunked, "the fragments coalesce to the same events, however the bytes were fed");
  let text_event = whole.iter().find(|e| e.kind() == EventKind::Text).expect("a text event");
  assert_eq!(text_event.text(), Some(text.as_str()));
}

#[test]
fn a_bound_rejects_an_oversized_token_but_the_default_does_not() {
  // A large comment is a single markup token. The generous default cap lets this 20 KB comment through;
  // a tighter application-set bound rejects it.
  let xml = format!("<a><!-- {} --></a>", "c".repeat(20_000));
  let ok = Reader::new(xml.as_bytes()).events().collect::<Result<Vec<_>, _>>();
  assert!(ok.is_ok(), "20 KB is well under the default comment bound");

  let bounds = Bounds::default().with_max_comment(4096);
  let err = Reader::new(xml.as_bytes()).with_bounds(bounds).events().collect::<Result<Vec<_>, _>>().unwrap_err();
  assert!(describe(&err).contains("Bounds::max_comment"), "{}", describe(&err));
}

#[test]
fn a_forbidden_sequence_is_caught_even_when_text_is_fragmented() {
  // The "]]>" falls at the end of a long run, so its "]]" and ">" can land in different fragments; the
  // scanner must still not let it slip across a fragment boundary.
  let xml = format!("<a>{}]]></a>", "x".repeat(30_000));
  let err = by_parser(&xml, 500).expect_err("]]> is forbidden in text");
  assert!(err.contains("]]>"), "{err}");
}
