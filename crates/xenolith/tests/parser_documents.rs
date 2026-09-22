//! Whole documents, through the public API.
//!
//! The unit tests reach inside; these do not. They also run every document through all three
//! drivers — the parser fed directly, the blocking reader and, where it is compiled in, the
//! asynchronous one — because the promise of the sans-I/O design is that the driver cannot
//! change the answer.
//!
//! An event borrows the parser, so a document is compared as the lines each event renders to
//! rather than as a collection of events. That is also what makes the comparison readable when a
//! driver does disagree.

use std::io::Read;

use xenolith::event::strict::StrictXmlValidator;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::{Parser, ParserConfig, Progress, StreamSource, TokenRef};

/// Renders an error the way a diagnostic would: its location, if known, then the message. The
/// location is a field on the error now, not part of its `Display`, so a caller that wants it in
/// the string composes the two.
fn describe(e: &xenolith::Error) -> String {
  if e.location().is_unknown() { e.to_string() } else { format!("{}: {e}", e.location()) }
}

/// `{namespace}local`, so namespace resolution is visible in what is compared.
fn expanded(namespace: Option<&str>, local: &str) -> String {
  match namespace {
    Some(namespace) => format!("{{{namespace}}}{local}"),
    None => local.to_owned(),
  }
}

/// Renders the parser's current event as one line, or `None` when there is no current event.
///
/// Every driver is compared on these lines: the same document must render the same way whichever
/// one drove the parser.
fn render(parser: &Parser) -> Option<String> {
  Some(match parser.token_ref()? {
    TokenRef::XmlDeclaration { version, .. } => format!("?xml {version}"),
    TokenRef::Doctype(_) => "!doctype".to_owned(),
    TokenRef::StartElement { attributes, .. } => {
      let mut line = format!("<{}", expanded(parser.namespace_uri(), parser.local_name()));
      for attribute in attributes.iter() {
        line.push_str(&format!(" {}={}", expanded(attribute.namespace, attribute.local), attribute.value));
      }
      line.push('>');
      line
    }
    TokenRef::EndElement { .. } => format!("</{}>", expanded(parser.namespace_uri(), parser.local_name())),
    TokenRef::Text(text) => format!("t:{text}"),
    TokenRef::CData(text) => format!("c:{text}"),
    TokenRef::Comment(text) => format!("!:{text}"),
    TokenRef::ProcessingInstruction { target, data, .. } => format!("?{target} {data}"),
    // The enum is non-exhaustive, so a kind added later renders as itself rather than being passed over in silence.
    other => format!("{other:?}"),
  })
}

/// The kind of event a rendered line came from, for the tests that compare a document's shape.
fn kind_of(line: &str) -> &'static str {
  if line.starts_with("?xml") {
    "xmldecl"
  } else if line.starts_with("!doctype") {
    "doctype"
  } else if line.starts_with("</") {
    "end"
  } else if line.starts_with('<') {
    "start"
  } else if line.starts_with("t:") {
    "text"
  } else if line.starts_with("c:") {
    "cdata"
  } else if line.starts_with("!:") {
    "comment"
  } else {
    "pi"
  }
}

/// Merges adjacent text lines, as a consumer that wants whole text nodes does. The parser may split a
/// long text run into fragments, and where the splits fall depends on how the bytes were fed, so the
/// drivers are compared on their coalesced text rather than on the raw fragments.
fn coalesce_text(lines: Vec<String>) -> Vec<String> {
  let mut out: Vec<String> = Vec::with_capacity(lines.len());
  for line in lines {
    match out.last_mut() {
      Some(last) if last.starts_with("t:") && line.starts_with("t:") => last.push_str(&line["t:".len()..]),
      _ => out.push(line),
    }
  }
  out
}

/// The text a `t:` line carries.
fn text_of(line: &str) -> Option<&str> {
  line.strip_prefix("t:")
}

/// Parses `xml` with the parser fed in `chunk`-sized pieces.
fn by_parser(xml: &str, chunk: usize) -> Result<Vec<String>, String> {
  let mut parser = Parser::new();
  let bytes = xml.as_bytes();
  let mut fed = 0;
  let mut lines = Vec::new();
  loop {
    match parser.advance() {
      Ok(Progress::Token(_)) => lines.extend(render(&parser)),
      Ok(Progress::Eof) => return Ok(coalesce_text(lines)),
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

/// Drives a prepared source, so a test can set a resolver or a bound first.
fn by_source<R: Read>(mut source: StreamSource<'_, R>) -> Result<Vec<String>, String> {
  let mut lines = Vec::new();
  loop {
    match source.advance() {
      Ok(Some(_)) => lines.extend(render(source.parser())),
      Ok(None) => return Ok(coalesce_text(lines)),
      Err(e) => return Err(describe(&e)),
    }
  }
}

fn by_reader(xml: &str) -> Result<Vec<String>, String> {
  by_source(StreamSource::new(xml.as_bytes()))
}

#[cfg(feature = "tokio")]
fn by_async_reader(xml: &str) -> Result<Vec<String>, String> {
  use xenolith::io::AsyncReader;
  tokio_test::block_on(async {
    let mut reader = AsyncReader::new(xml.as_bytes());
    let mut lines = Vec::new();
    loop {
      match reader.advance().await {
        Ok(Some(_)) => lines.extend(render(reader.parser())),
        Ok(None) => return Ok(coalesce_text(lines)),
        Err(e) => return Err(describe(&e)),
      }
    }
  })
}

/// Parses `xml` every way available, requiring them all to agree, then judges it by the strict validator.
///
/// The lexical constraints, a repeated attribute, `--` in a comment, the shape of a name, are the validator's to judge
/// rather than the parser's, so a document counts as accepted only when both are content.
fn parse(xml: &str) -> Result<Vec<String>, String> {
  let expected = by_parser(xml, xml.len().max(1));
  for chunk in [1, 2, 3, 5, 64] {
    assert_eq!(by_parser(xml, chunk), expected, "chunk size {chunk} disagreed");
  }
  assert_eq!(by_reader(xml), expected, "the blocking reader disagreed");
  #[cfg(feature = "tokio")]
  assert_eq!(by_async_reader(xml), expected, "the asynchronous reader disagreed");
  let lines = expected?;
  let mut strict = StrictXmlValidator::new();
  StreamSource::new(xml.as_bytes()).with_handler(&mut strict).emit().map_err(|e| describe(&e))?;
  Ok(lines)
}

fn kinds(xml: &str) -> Vec<&'static str> {
  parse(xml).expect("should parse").iter().map(|line| kind_of(line)).collect()
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
  let expected = ["doctype", "start", "start", "end", "end"];

  for before in ["", "\n", "   ", "\n\n\n", "\t", "<?xml version=\"1.0\"?>\n", "<!--c-->\n", "\n<!--c-->\n"] {
    let xml = format!("{before}{doctype}<r><a/></r>");
    let found: Vec<&str> = kinds(&xml).into_iter().filter(|kind| !matches!(*kind, "xmldecl" | "comment")).collect();
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
  let lines = parse(xml).expect("should parse");

  let starts = lines.iter().filter(|line| kind_of(line) == "start").count();
  let ends = lines.iter().filter(|line| kind_of(line) == "end").count();
  assert_eq!(starts, 9);
  assert_eq!(starts, ends, "every element is closed");

  // Namespaces, including the default one and a prefixed one. A rendered name carries its namespace in braces.
  assert!(
    lines.iter().filter(|line| kind_of(line) == "start").all(|line| line.starts_with("<{")),
    "every element is in a namespace: {lines:?}"
  );

  // A character reference inside preserved whitespace.
  let summary = lines.iter().filter_map(|line| text_of(line)).find(|text| text.contains("Volumes"));
  assert_eq!(summary, Some("  Volumes 1\u{2013}4A,\n  boxed set.  "), "&#8211; is U+2013");

  assert!(lines.iter().any(|line| kind_of(line) == "cdata"));
  assert!(lines.iter().any(|line| kind_of(line) == "pi"));
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
  let lines = parse("<a>&lt;&gt;&amp;&apos;&quot;&#65;&#x42;&#x1F600;</a>").expect("should parse");
  assert_eq!(text_of(&lines[1]), Some("<>&'\"AB\u{1F600}"));
}

#[test]
fn an_empty_document_body() {
  assert_eq!(kinds("<a/>"), ["start", "end"]);
  assert_eq!(kinds("<a></a>"), ["start", "end"]);
}

#[test]
fn utf8_beyond_the_basic_plane() {
  let lines = parse("<🎌 attr='🎏'>🎐</🎌>").expect("astral characters are valid in names");
  assert_eq!(text_of(&lines[1]), Some("🎐"));
}

#[test]
fn a_byte_order_mark_is_not_content() {
  let lines = parse("\u{FEFF}<a/>").expect("should parse");
  assert_eq!(lines.len(), 2);
  assert_eq!(kind_of(&lines[0]), "start");
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

impl xenolith::io::resolve::UriResolver for MapResolver {
  fn resolve(
    &mut self,
    request: &xenolith::io::resolve::EntityRequest,
  ) -> Result<Option<Box<dyn std::io::Read>>, xenolith::Error> {
    let entry = self.0.get(request.system_id()).map(|b| b.to_vec());
    Ok(entry.map(|b| Box::new(std::io::Cursor::new(b)) as Box<dyn std::io::Read>))
  }
}

fn parse_with(xml: &str, files: &[(&'static str, &'static [u8])]) -> Result<Vec<String>, String> {
  let resolver = MapResolver(files.iter().copied().collect());
  by_source(StreamSource::new(xml.as_bytes()).with_resolver(resolver))
}

#[test]
fn an_external_subset_declares_entities_and_defaults() {
  // The external subset supplies the entity and the attribute default the document relies on.
  let dtd: &[u8] = b"<!ELEMENT doc (#PCDATA)>\n<!ATTLIST doc lang CDATA 'en'>\n<!ENTITY greeting 'hello'>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc>&greeting;</doc>";
  let lines = parse_with(xml, &[("doc.dtd", dtd)]).expect("should parse");
  assert!(lines[1].contains("lang=en"), "the default is supplied: {:?}", lines[1]);
  assert_eq!(text_of(&lines[2]), Some("hello"));
}

#[test]
fn a_parameter_entity_parameterizes_a_declaration() {
  // `%e;` stands for the whole attribute definition, as real DTDs do.
  let dtd: &[u8] = b"<!ELEMENT doc (#PCDATA)>\n<!ENTITY % e 'a1 CDATA \"v1\"'>\n<!ATTLIST doc %e;>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc/>";
  let lines = parse_with(xml, &[("doc.dtd", dtd)]).expect("should parse");
  assert!(lines[1].contains("a1=v1"), "{:?}", lines[1]);
}

#[test]
fn an_external_parameter_entity_is_fetched_and_spliced() {
  let outer: &[u8] = b"<!ELEMENT doc EMPTY>\n<!ENTITY % inner SYSTEM 'inner.dtd'>\n%inner;";
  let inner: &[u8] = b"<!ATTLIST doc a CDATA 'defaulted'>";
  let xml = "<!DOCTYPE doc SYSTEM 'outer.dtd'><doc/>";
  let lines = parse_with(xml, &[("outer.dtd", outer), ("inner.dtd", inner)]).expect("should parse");
  assert!(lines[1].contains("a=defaulted"), "{:?}", lines[1]);
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
fn a_standalone_document_may_not_depend_on_an_external_pe_in_the_internal_subset() {
  // The entity is declared in the internal subset's text, but that text came from an external parameter entity.
  let ent: &[u8] = b"<!ENTITY e 'x'>";
  let xml =
    "<?xml version='1.0' standalone='yes'?><!DOCTYPE doc [<!ENTITY % ext SYSTEM 'ext.ent'>%ext;]><doc>&e;</doc>";
  let message = parse_with(xml, &[("ext.ent", ent)]).expect_err("standalone violation");
  assert!(message.contains("standalone"), "{message}");

  // Without standalone="yes" the same document is fine.
  let xml = "<!DOCTYPE doc [<!ENTITY % ext SYSTEM 'ext.ent'>%ext;]><doc>&e;</doc>";
  let lines = parse_with(xml, &[("ext.ent", ent)]).expect("should parse");
  assert_eq!(text_of(&lines[2]), Some("x"));
}

/// A resolver over an in-memory map of resolved URI to bytes, for the tests of relative system identifiers.
struct ResolvedMapResolver(std::collections::HashMap<&'static str, &'static [u8]>);

impl xenolith::io::resolve::UriResolver for ResolvedMapResolver {
  fn resolve(
    &mut self,
    request: &xenolith::io::resolve::EntityRequest,
  ) -> Result<Option<Box<dyn std::io::Read>>, xenolith::Error> {
    let entry = request.resolved_uri().and_then(|uri| self.0.get(uri.as_str()).map(|b| b.to_vec()));
    Ok(entry.map(|b| Box::new(std::io::Cursor::new(b)) as Box<dyn std::io::Read>))
  }
}

#[test]
fn an_external_entity_is_resolved_against_the_dtd_that_declared_it() {
  // `chap.xml` is relative to the DTD in `dtd/`, not to the document.
  let files: [(&'static str, &'static [u8]); 2] = [
    ("file:///doc/dtd/doc.dtd", b"<!ELEMENT doc (#PCDATA)><!ENTITY chap SYSTEM 'chap.xml'>"),
    ("file:///doc/dtd/chap.xml", b"from the DTD's directory"),
  ];
  let xml = "<!DOCTYPE doc SYSTEM 'dtd/doc.dtd'><doc>&chap;</doc>";
  let source = StreamSource::with_system_id(xml.as_bytes(), "file:///doc/main.xml")
    .with_resolver(ResolvedMapResolver(files.into_iter().collect()));
  let lines = by_source(source).expect("should parse");
  assert_eq!(text_of(&lines[2]), Some("from the DTD's directory"));
}

#[test]
fn a_conditional_section_includes_or_ignores() {
  let dtd: &[u8] = b"<![INCLUDE[<!ELEMENT doc (#PCDATA)>]]>\n<![IGNORE[<!ELEMENT doc EMPTY> ]]>";
  let xml = "<!DOCTYPE doc SYSTEM 'doc.dtd'><doc>text</doc>";
  let lines = parse_with(xml, &[("doc.dtd", dtd)]).expect("should parse");
  assert_eq!(text_of(&lines[2]), Some("text"));
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
fn a_tokenized_default_is_normalized_after_its_references_are_expanded() {
  let xml = "<!DOCTYPE a [<!ENTITY sp '  x  '><!ATTLIST a t NMTOKENS '&sp; y'>]><a/>";
  let lines = parse(xml).expect("should parse");
  assert!(lines[1].contains("t=x y"), "{:?}", lines[1]);
}

#[test]
fn an_error_in_the_internal_subset_is_located_in_the_document() {
  let message = rejects("<?xml version='1.0'?>\n<!DOCTYPE a [\n<!ELEMENT a FOO>]><a/>");
  assert!(message.starts_with("<unknown>:3:13:"), "{message}");
}

#[test]
fn attribute_defaults_and_types_come_from_the_dtd() {
  let xml = "<!DOCTYPE a [\
             <!ATTLIST a lang CDATA \"en\" id ID #IMPLIED tokens NMTOKENS #IMPLIED>\
             ]><a id=\"x\" tokens=\"  one   two  \"/>";
  let lines = parse(xml).expect("should parse");

  // The default is supplied for the absent `lang`, and a tokenized value has its whitespace collapsed; a CDATA value
  // would not.
  assert!(lines[1].contains("lang=en"), "{:?}", lines[1]);
  assert!(lines[1].contains("tokens=one two"), "{:?}", lines[1]);
}

#[test]
fn entities_may_nest_and_be_reused() {
  let xml = "<!DOCTYPE a [<!ENTITY inner 'x'><!ENTITY outer '&inner;&inner;'>]><a>&outer;&outer;</a>";
  let lines = parse(xml).expect("should parse");
  assert_eq!(text_of(&lines[2]), Some("xxxx"));
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
  // The entity is declared, but after the attribute list that refers to it in a default.
  let xml = "<!DOCTYPE a [<!ATTLIST a x CDATA '&e;'><!ENTITY e 'v'>]><a/>";
  assert!(rejects(xml).contains("before it is declared"));
  // Declared first, it is fine and the default expands.
  let ok = "<!DOCTYPE a [<!ENTITY e 'v'><!ATTLIST a x CDATA '&e;'>]><a/>";
  let lines = parse(ok).expect("should parse");
  assert!(lines[1].contains("x=v"), "{:?}", lines[1]);
}

#[test]
fn a_declared_entity_expands_in_content_and_attributes() {
  // A general entity may carry markup into content, and text into an attribute.
  let xml = "<!DOCTYPE a [<!ENTITY e 'as <b>bold</b>'><!ENTITY t 'plain'>]><a x='&t;'>it is &e;</a>";
  let lines = parse(xml).expect("should parse");
  let kinds: Vec<&str> = lines.iter().map(|line| kind_of(line)).collect();
  assert_eq!(
    kinds,
    [
      "doctype", "start", // a
      "text",  // "it is as " — text coalesces across the entity boundary
      "start", // b, from inside the entity
      "text",  // "bold"
      "end",   // b
      "end",   // a
    ]
  );
  assert!(lines[1].contains("x=plain"), "{:?}", lines[1]);
  assert_eq!(text_of(&lines[2]), Some("it is as "));
}

#[test]
fn an_expansion_bomb_is_refused_rather_than_expanded() {
  // The billion-laughs shape: each level refers to the one below it ten times. Expanding it fully
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
  assert_eq!(text_of(&unix[1]), Some("one\ntwo"));
}

#[test]
fn positions_survive_multi_byte_characters() {
  let message = rejects("<a>日本語のテキスト &bad</a>");
  // Columns count characters, not bytes: "<a>" is 3, the Japanese 8, the space 1, so the
  // reference begins at column 13 and not at byte 27. The document was given no system
  // identifier, so the position is all the location has to report.
  assert!(message.contains(":1:13:"), "{message}");
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
  let run = whole.iter().find_map(|line| text_of(line)).expect("a text event");
  assert_eq!(run, text.as_str());
}

#[test]
fn a_bound_rejects_an_oversized_token_but_the_default_does_not() {
  // A large comment is a single markup token. The generous default cap lets this 20 KB comment through;
  // a tighter application-set bound rejects it.
  let xml = format!("<a><!-- {} --></a>", "c".repeat(20_000));
  assert!(by_reader(&xml).is_ok(), "20 KB is well under the default comment bound");

  let mut config = ParserConfig::default();
  config.limits.tokens.max_comment = Some(4096);
  let message = by_source(StreamSource::new(xml.as_bytes()).with_config(config)).expect_err("over the bound");
  assert!(message.contains("limits.tokens.max_comment"), "{message}");
}

#[test]
fn the_element_depth_limit_comes_from_the_config() {
  let xml = "<a><b><c/></b></a>";
  let mut config = ParserConfig::default();
  config.limits.document.max_element_depth = Some(2);
  let message = by_source(StreamSource::new(xml.as_bytes()).with_config(config)).expect_err("three deep");
  assert!(message.contains("limits.document.max_element_depth"), "{message}");

  // Every limit removed: the same document parses.
  let mut config = ParserConfig::default();
  config.limits = xenolith::io::Limits::unlimited();
  assert!(by_source(StreamSource::new(xml.as_bytes()).with_config(config)).is_ok());
}

#[test]
fn a_forbidden_sequence_is_caught_even_when_text_is_fragmented() {
  // The "]]>" falls at the end of a long run, so its "]]" and ">" can land in different fragments; the
  // scanner must still not let it slip across a fragment boundary.
  let xml = format!("<a>{}]]></a>", "x".repeat(30_000));
  let err = by_parser(&xml, 500).expect_err("]]> is forbidden in text");
  assert!(err.contains("]]>"), "{err}");
}
