//! Whole documents, through the public API.
//!
//! The unit tests reach inside; these do not. They also run every document through all three
//! drivers — the parser fed directly, the blocking reader and, where it is compiled in, the
//! asynchronous one — because the promise of the sans-I/O design is that the driver cannot
//! change the answer.

use xylograph_parser::{Event, EventKind, Parser, Progress, Reader};

/// Parses `xml` with the parser fed in `chunk`-sized pieces.
fn by_parser(xml: &str, chunk: usize) -> Result<Vec<Event>, String> {
  let mut parser = Parser::new();
  let bytes = xml.as_bytes();
  let mut fed = 0;
  let mut events = Vec::new();
  loop {
    match parser.advance() {
      Ok(Progress::Event(_)) => events.push(Event::capture(&parser)),
      Ok(Progress::Eof) => return Ok(events),
      Ok(Progress::NeedMoreInput) => {
        let end = (fed + chunk).min(bytes.len());
        parser.feed(&bytes[fed..end], end == bytes.len()).map_err(|e| e.to_string())?;
        fed = end;
      }
      Ok(other) => panic!("unexpected {other:?}"),
      Err(e) => return Err(e.to_string()),
    }
  }
}

fn by_reader(xml: &str) -> Result<Vec<Event>, String> {
  Reader::new(xml.as_bytes()).events().collect::<Result<_, _>>().map_err(|e| e.to_string())
}

#[cfg(feature = "tokio")]
fn by_async_reader(xml: &str) -> Result<Vec<Event>, String> {
  use xylograph_parser::AsyncReader;
  tokio_test::block_on(AsyncReader::new(xml.as_bytes()).events()).map_err(|e| e.to_string())
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
    ("<a>", "never closed"),
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

#[test]
fn an_expansion_bomb_is_refused_rather_than_expanded() {
  // The classic shape. Phase 1 has no entity declarations, so this is rejected at the first
  // reference; phase 2 must keep it rejected, by the expansion budget instead.
  let xml = "<!DOCTYPE a [\
             <!ENTITY x0 'boom'>\
             <!ENTITY x1 '&x0;&x0;&x0;&x0;&x0;&x0;&x0;&x0;&x0;&x0;'>\
             <!ENTITY x2 '&x1;&x1;&x1;&x1;&x1;&x1;&x1;&x1;&x1;&x1;'>\
             ]><a>&x2;</a>";
  let message = rejects(xml);
  assert!(message.contains("x2"), "{message}");
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
