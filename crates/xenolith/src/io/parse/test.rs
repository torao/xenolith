use super::*;

/// Renders one event compactly, so tests can assert on a whole document at once.
fn render(parser: &Parser, event: TokenRef) -> String {
  match event {
    TokenRef::XmlDeclaration { version, encoding, standalone } => {
      let mut s = format!("?xml {version}");
      if let Some(encoding) = encoding {
        s.push_str(&format!(" {encoding}"));
      }
      if let Some(standalone) = standalone {
        s.push_str(if standalone { " standalone" } else { " not-standalone" });
      }
      s
    }
    TokenRef::Doctype(text) => format!("!doctype {text}"),
    TokenRef::StartElement { name, attributes, .. } => {
      let mut s = format!("<{}", qualified(parser, name));
      for attribute in attributes.iter() {
        s.push_str(&format!(" {}={}", expanded(attribute.namespace, attribute.local), attribute.value));
      }
      s.push('>');
      s
    }
    TokenRef::EndElement { name } => format!("</{}>", qualified(parser, name)),
    TokenRef::Text(text) => format!("t:{text}"),
    TokenRef::CData(text) => format!("c:{text}"),
    TokenRef::Comment(text) => format!("!:{text}"),
    TokenRef::ProcessingInstruction { target, data, .. } => format!("?{target} {data}"),
  }
}

/// The character data of the current text, CDATA, or comment event, for the tests that collect a run by hand.
fn text_of(parser: &Parser) -> &str {
  parser.token_ref().and_then(|e| e.text()).expect("the current event is character data")
}

/// `{namespace}local`, so namespace resolution is visible in the trace.
fn qualified(parser: &Parser, name: QName) -> String {
  match name.namespace() {
    Some(ns) => format!("{{{}}}{}", parser.pool().resolve(ns), parser.pool().resolve(name.local())),
    None => parser.pool().resolve(name.local()).to_owned(),
  }
}

/// `{namespace}local` for a name already resolved to its parts, as an attribute view lends it.
fn expanded(namespace: Option<&str>, local: &str) -> String {
  match namespace {
    Some(namespace) => format!("{{{namespace}}}{local}"),
    None => local.to_owned(),
  }
}

/// Parses `xml` fed in chunks of `chunk` bytes, returning the rendered events.
fn trace_in_chunks(xml: &str, chunk: usize) -> Result<Vec<String>> {
  let mut parser = Parser::with_document(Entity::document(CharStream::with_encoding("UTF-8")?));
  let bytes = xml.as_bytes();
  let mut fed = 0;
  let mut events = Vec::new();
  loop {
    match parser.advance()? {
      Progress::Token(_) => events.push(render(&parser, parser.token_ref().expect("a current event"))),
      Progress::Eof => return Ok(events),
      Progress::NeedMoreInput => {
        assert!(fed <= bytes.len(), "requested input after everything was fed");
        let end = (fed + chunk).min(bytes.len());
        parser.feed(&bytes[fed..end], end == bytes.len())?;
        fed = end;
      }
      // These unit tests use no external entities.
      Progress::NeedEntity => parser.decline_entity()?,
    }
  }
}

fn trace(xml: &str) -> Result<Vec<String>> {
  let all = trace_in_chunks(xml, xml.len().max(1))?;
  // Every document must parse identically however the input is split; this is the property
  // the resumable design exists for, so it is checked on every case rather than once.
  for chunk in [1, 2, 3, 7] {
    let split = trace_in_chunks(xml, chunk).unwrap_or_else(|e| panic!("failed at chunk size {chunk}: {e}"));
    assert_eq!(split, all, "chunk size {chunk} changed the result");
  }
  Ok(all)
}

fn error(xml: &str) -> Error {
  let whole = trace_in_chunks(xml, xml.len().max(1)).expect_err("should fail");
  for chunk in [1, 2, 3, 7] {
    let split = trace_in_chunks(xml, chunk).expect_err("should fail whatever the chunk size");
    assert_eq!(
      std::mem::discriminant(&split),
      std::mem::discriminant(&whole),
      "chunk size {chunk} changed the error kind"
    );
  }
  whole
}

/// Drives the parser over `xml`, streaming any general entity named in `entities` in
/// `chunk`-byte pieces through `begin_entity` + `feed`, exactly as the streaming driver will.
/// A stack of byte sources mirrors the parser's entity stack: the innermost source feeds the
/// innermost entity.
fn trace_streaming_in_chunks(xml: &str, entities: &[(&str, &str)], chunk: usize) -> Result<Vec<String>> {
  let mut parser = Parser::with_document(Entity::document(CharStream::new()));
  let mut sources: Vec<(Vec<u8>, usize)> = vec![(xml.as_bytes().to_vec(), 0)];
  let mut events = Vec::new();
  loop {
    match parser.advance()? {
      Progress::Token(_) => events.push(render(&parser, parser.token_ref().expect("a current event"))),
      Progress::Eof => return Ok(events),
      Progress::NeedMoreInput => {
        let (bytes, at) = sources.last_mut().expect("a source for the innermost entity");
        let end = at.saturating_add(chunk).min(bytes.len());
        let last = end == bytes.len();
        parser.feed(&bytes[*at..end], last)?;
        *at = end;
        if last {
          sources.pop();
        }
      }
      Progress::NeedEntity => {
        let name = parser.pending_entity().and_then(|r| r.name()).expect("a named general entity").to_owned();
        match entities.iter().find(|(n, _)| *n == name) {
          Some((_, content)) => {
            parser.begin_entity()?;
            sources.push((content.as_bytes().to_vec(), 0));
          }
          None => parser.decline_entity()?,
        }
      }
    }
  }
}

/// Streams the entities and asserts the result does not depend on how the bytes are split.
fn trace_streaming(xml: &str, entities: &[(&str, &str)]) -> Result<Vec<String>> {
  let whole = trace_streaming_in_chunks(xml, entities, usize::MAX)?;
  for chunk in [1, 2, 3, 7] {
    let split =
      trace_streaming_in_chunks(xml, entities, chunk).unwrap_or_else(|e| panic!("failed at chunk size {chunk}: {e}"));
    assert_eq!(split, whole, "chunk size {chunk} changed the result");
  }
  Ok(whole)
}

#[test]
fn a_streamed_external_entity_is_read_in_chunks() {
  let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
  let events = trace_streaming(xml, &[("e", "<b>in</b>")]).unwrap();
  assert_eq!(events, ["!doctype <!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]>", "<a>", "<b>", "t:in", "</b>", "</a>"]);
}

#[test]
fn a_streamed_entity_text_declaration_is_stripped_across_feeds() {
  // The text declaration can straddle any feed boundary; it must be stepped over, not surface
  // as a processing instruction, whatever the chunk size.
  let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
  let entity = "<?xml version='1.0' encoding='UTF-8'?><b>in</b>";
  let events = trace_streaming(xml, &[("e", entity)]).unwrap();
  assert_eq!(events, ["!doctype <!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]>", "<a>", "<b>", "t:in", "</b>", "</a>"]);
}

#[test]
fn a_malformed_text_declaration_names_what_is_wrong() {
  let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
  let bad = |decl: &str| trace_streaming(xml, &[("e", &format!("{decl}<b/>"))]).unwrap_err().message().to_owned();
  // A name that is not a pseudo-attribute is reported as such, not as a misordering.
  assert!(bad("<?xml version='1.0' bogus='x'?>").contains("not one of version or encoding"));
  // A known pseudo-attribute misplaced or repeated is reported specifically.
  assert!(bad("<?xml encoding='UTF-8' version='1.0'?>").contains("version after encoding"));
  assert!(bad("<?xml encoding='UTF-8' encoding='UTF-8'?>").contains("more than one encoding"));
  assert!(bad("<?xml version='1.0' encoding='UTF-8' standalone='yes'?>").contains("standalone"));
  // The pseudo-attribute parser's own reason survives, named for the text declaration, not overwritten as "malformed".
  assert_eq!(bad("<?xml version=1.0?>"), "the text declaration has an unquoted value");
}

#[test]
fn reports_the_events_of_a_small_document() {
  assert_eq!(
    trace("<?xml version='1.0' encoding='UTF-8'?>\n<!--hi--><a x='1'>text<b/></a>\n").unwrap(),
    ["?xml 1.0 UTF-8", "!:hi", "<a x=1>", "t:text", "<b>", "</b>", "</a>"]
  );
}

#[test]
fn an_empty_element_reports_a_start_and_an_end() {
  assert_eq!(trace("<a/>").unwrap(), ["<a>", "</a>"]);
  assert_eq!(trace("<a></a>").unwrap(), ["<a>", "</a>"]);
  assert_eq!(trace("<a>  </a>").unwrap(), ["<a>", "t:  ", "</a>"]);
}

#[test]
fn whitespace_outside_the_root_is_dropped_but_markup_is_not() {
  assert_eq!(trace("  <a/>\n\n<!--after-->\n<?pi data?>  ").unwrap(), ["<a>", "</a>", "!:after", "?pi data"]);
}

#[test]
fn resolves_namespaces() {
  let events = trace("<a xmlns='urn:d' xmlns:p='urn:p'><p:b q='1' p:r='2'/></a>").unwrap();
  assert_eq!(
    events,
    [
      "<{urn:d}a {http://www.w3.org/2000/xmlns/}xmlns=urn:d {http://www.w3.org/2000/xmlns/}p=urn:p>",
      "<{urn:p}b q=1 {urn:p}r=2>",
      "</{urn:p}b>",
      "</{urn:d}a>",
    ]
  );
}

#[test]
fn an_unprefixed_attribute_is_in_no_namespace() {
  let events = trace("<a xmlns='urn:d' x='1'/>").unwrap();
  assert!(events[0].contains(" x=1"), "{events:?}");
  assert!(events[0].starts_with("<{urn:d}a"), "{events:?}");
}

#[test]
fn the_default_namespace_can_be_undeclared() {
  let events = trace("<a xmlns='urn:d'><b xmlns=''/></a>").unwrap();
  assert!(events[1].starts_with("<b "), "{events:?}");
}

#[test]
fn a_namespace_declaration_leaves_scope_with_its_element() {
  assert!(matches!(error("<a><b xmlns:p='urn:p'/><p:c/></a>"), Error::Namespace { .. }));
}

#[test]
fn xml_is_always_bound() {
  let events = trace("<a xml:lang='en'/>").unwrap();
  assert!(events[0].contains("{http://www.w3.org/XML/1998/namespace}lang=en"), "{events:?}");
}

#[test]
fn expands_character_and_predefined_references() {
  assert_eq!(trace("<a>&lt;&amp;&gt;&#65;&#x42;&apos;&quot;</a>").unwrap()[1], "t:<&>AB'\"");
  assert_eq!(trace("<a b='&lt;&#65;'/>").unwrap()[0], "<a b=<A>");
}

#[test]
fn normalizes_attribute_values() {
  // Literal whitespace becomes a space; whitespace written as a reference does not.
  assert_eq!(trace("<a b='x\ty\nz'/>").unwrap()[0], "<a b=x y z>");
  assert_eq!(trace("<a b='x&#9;y'/>").unwrap()[0], "<a b=x\ty>");
}

#[test]
fn cdata_is_reported_separately_and_is_not_expanded() {
  assert_eq!(trace("<a><![CDATA[<&]]>tail</a>").unwrap(), ["<a>", "c:<&", "t:tail", "</a>"]);
}

#[test]
fn processing_instructions_keep_their_data_verbatim() {
  assert_eq!(trace("<a><?target a='1' &b;?></a>").unwrap()[1], "?target a='1' &b;");
  assert_eq!(trace("<a><?bare?></a>").unwrap()[1], "?bare ");
}

#[test]
fn tracks_xml_space_and_lang_through_the_tree() {
  let mut parser = Parser::new();
  parser.feed(b"<a xml:space='preserve' xml:lang='ja'><b xml:space='default'><c/></b></a>", true).unwrap();

  let mut seen = Vec::new();
  while let Progress::Token(kind) = parser.advance().unwrap() {
    if kind == TokenKind::StartElement {
      seen.push((parser.local_name().to_owned(), parser.xml_space(), parser.xml_lang().map(str::to_owned)));
    }
  }
  assert_eq!(
    seen,
    [
      ("a".to_owned(), XmlSpace::Preserve, Some("ja".to_owned())),
      ("b".to_owned(), XmlSpace::Default, Some("ja".to_owned())),
      ("c".to_owned(), XmlSpace::Default, Some("ja".to_owned())),
    ]
  );
}

#[test]
fn computes_base_uris_from_the_system_id_and_xml_base() {
  let doc = Entity::document(CharStream::with_encoding("UTF-8").unwrap().with_system_id("file:///a/b/doc.xml"));
  let mut parser = Parser::with_document(doc);
  parser.feed(b"<a><b xml:base='../c/'><d xml:base='e.xml'/></b><f/></a>", true).unwrap();

  let mut seen = Vec::new();
  while let Progress::Token(kind) = parser.advance().unwrap() {
    if kind == TokenKind::StartElement {
      seen.push((parser.local_name().to_owned(), parser.base_uri()));
    }
  }
  assert_eq!(
    seen,
    [
      ("a".to_owned(), Some("file:///a/b/doc.xml".to_owned())),
      ("b".to_owned(), Some("file:///a/c/".to_owned())),
      ("d".to_owned(), Some("file:///a/c/e.xml".to_owned())),
      ("f".to_owned(), Some("file:///a/b/doc.xml".to_owned())),
    ]
  );
}

#[test]
fn xml_base_can_be_turned_off() {
  let doc = Entity::document(CharStream::with_encoding("UTF-8").unwrap().with_system_id("file:///doc.xml"));
  let mut parser = Parser::with_document(doc);
  parser.set_config(ParserConfig { extensions: Extensions::none(), ..ParserConfig::default() });
  parser.feed(b"<a xml:base='sub/'/>", true).unwrap();
  parser.advance().unwrap();
  assert_eq!(parser.base_uri(), None);
}

#[test]
fn normalizes_and_exposes_xml_id() {
  let mut parser = Parser::new();
  parser.feed(b"<a xml:id='  x1  '><b/></a>", true).unwrap();
  parser.advance().unwrap(); // <a>
  assert_eq!(parser.xml_id(), Some("x1"), "surrounding whitespace is collapsed away");
  assert_eq!(parser.attribute_value(Some(XML_NS_URI), "id"), Some("x1"), "the reported value is normalized too");
  parser.advance().unwrap(); // <b>
  assert_eq!(parser.xml_id(), None);
}

#[test]
fn reports_depth() {
  let mut parser = Parser::new();
  parser.feed(b"<a><b/></a>", true).unwrap();
  let mut depths = Vec::new();
  while let Progress::Token(_) = parser.advance().unwrap() {
    depths.push(parser.depth());
  }
  assert_eq!(depths, [1, 2, 1, 0]);
}

#[test]
fn keeps_the_doctype_for_later_phases() {
  let events = trace("<!DOCTYPE a [<!ENTITY e 'v'>]><a/>").unwrap();
  assert_eq!(events[0], "!doctype <!DOCTYPE a [<!ENTITY e 'v'>]>");
}

#[test]
fn rejects_content_after_the_doctype_external_id() {
  let err = error("<!DOCTYPE r SYSTEM 'a.dtd' junk><r/>");
  assert!(err.message().contains("content after the external identifier"), "{}", err.message());
  // The error points at the stray content, not at the whole declaration.
  assert_eq!(err.location().column, 28);
}

#[test]
fn each_event_clears_the_previous_events_accessors() {
  let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
  let mut parser = Parser::with_document(document);
  parser.feed("<a x='1'>hi</a>".as_bytes(), true).unwrap();
  assert_eq!(parser.advance().unwrap(), Progress::Token(TokenKind::StartElement));
  assert_eq!(parser.local_name(), "a");
  assert_eq!(parser.token_ref().unwrap().attributes().len(), 1);
  // The start tag's name must not linger into the following text event; its attributes cannot, since a `Text`
  // event is not the variant that carries them.
  assert_eq!(parser.advance().unwrap(), Progress::Token(TokenKind::Text));
  assert_eq!(text_of(&parser), "hi");
  assert!(parser.token_ref().unwrap().attributes().is_empty());
  assert_eq!(parser.local_name(), "", "the start tag's name leaked into the text event");
}

#[test]
fn event_ref_gives_the_current_event_as_a_borrowed_enum() {
  let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
  let mut parser = Parser::with_document(document);
  parser.feed("<a x='1'>hi</a>".as_bytes(), true).unwrap();

  assert_eq!(parser.advance().unwrap(), Progress::Token(TokenKind::StartElement));
  let TokenRef::StartElement { name, attributes, .. } = parser.token_ref().unwrap() else {
    panic!("expected a start element");
  };
  assert_eq!(parser.pool().resolve(name.local()), "a");
  assert_eq!(attributes.len(), 1);
  assert_eq!(attributes.get(0).unwrap().value, "1");

  assert_eq!(parser.advance().unwrap(), Progress::Token(TokenKind::Text));
  assert!(matches!(parser.token_ref(), Some(TokenRef::Text("hi"))));

  // No current event before the first advance or after the last one.
  assert_eq!(parser.advance().unwrap(), Progress::Token(TokenKind::EndElement));
  assert_eq!(parser.advance().unwrap(), Progress::Eof);
  assert!(parser.token_ref().is_none());
}

#[test]
fn a_too_deep_entity_chain_in_an_attribute_names_the_path() {
  let xml = "<!DOCTYPE a [<!ENTITY e0 'x'><!ENTITY e1 '&e0;'><!ENTITY e2 '&e1;'><!ENTITY e3 '&e2;'>]><a v='&e3;'/>";
  let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
  let mut parser = Parser::with_document(document);
  let mut config = ParserConfig::default();
  config.limits.entities.max_depth = Some(2);
  parser.set_config(config);
  parser.feed(xml.as_bytes(), true).unwrap();
  let error = loop {
    match parser.advance() {
      Ok(Progress::Eof) => panic!("expected a depth-limit error"),
      Ok(Progress::NeedEntity) => parser.decline_entity().unwrap(),
      Ok(_) => {}
      Err(error) => break error,
    }
  };
  let message = error.message();
  assert!(message.contains("nested more than 2 deep"), "{message}");
  // The message traces the nesting path that tripped the limit.
  assert!(message.contains("e3 -> e2 -> e1"), "{message}");
}

#[test]
fn a_cyclic_entity_in_an_attribute_names_the_loop() {
  // `a` and `b` refer to each other, so expanding either in an attribute loops.
  let xml = "<!DOCTYPE d [<!ENTITY a '&b;'><!ENTITY b '&a;'>]><d v='&a;'/>";
  let message = error(xml).message().to_owned();
  assert!(message.contains("refers to itself"), "{message}");
  assert!(message.contains("a -> b -> a"), "{message}");
}

#[test]
fn rejects_mismatched_and_stray_end_tags() {
  assert!(error("<a></b>").message().contains("does not close"));
  assert!(error("<a/></a>").message().contains("never opened"));
  assert!(error("<a>").message().contains("not closed"));
  assert!(matches!(error("<a></a></a>"), Error::WellFormedness { .. }));
}

#[test]
fn rejects_documents_without_exactly_one_root() {
  assert!(error("").message().contains("no root element"));
  assert!(error("<!--only a comment-->").message().contains("no root element"));
  assert!(error("<a/><b/>").message().contains("only one root"));
  assert!(error("text<a/>").message().contains("before the root"));
  assert!(error("<a/>text").message().contains("after the root"));
  // The error points at the first non-whitespace character, past the leading whitespace of the run.
  assert_eq!(error("  x<a/>").location().column, 3);
  assert_eq!(error("<a/>\n\ny").location().line, 3);
}

#[test]
fn leaves_duplicate_attributes_to_the_strict_validator() {
  // WFC: Unique Att Spec is lexical: the parser delivers both attributes, and `StrictXmlValidator` refuses the tag.
  assert_eq!(trace("<a x='1' x='2'/>").unwrap().len(), 2);
  assert_eq!(trace("<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>").unwrap().len(), 2);
  assert_eq!(trace("<a xmlns:p='u' xmlns:q='v' p:x='1' q:x='2'/>").unwrap().len(), 2);
}

#[test]
fn leaves_the_shape_of_a_name_to_the_strict_validator() {
  // A name that is not a `QName` still becomes an event. The split at the first colon is lenient, so `a:b:c` is prefix
  // `a` with local part `b:c`, and `1a` has no prefix; `StrictXmlValidator` is what refuses them.
  assert_eq!(trace("<a:b:c xmlns:a='u'/>").unwrap().len(), 2);
  assert_eq!(trace("<1a/>").unwrap().len(), 2);
  assert_eq!(trace("<a b^c='1'/>").unwrap().len(), 2);
}

#[test]
fn rejects_undeclared_prefixes() {
  assert!(matches!(error("<p:a/>"), Error::Namespace { .. }));
  assert!(matches!(error("<a p:x='1'/>"), Error::Namespace { .. }));
  assert!(matches!(error("<a xmlns:p=''/>"), Error::Namespace { .. }));
}

#[test]
fn protects_the_reserved_prefixes() {
  assert!(matches!(error("<a xmlns:xmlns='urn:x'/>"), Error::Namespace { .. }));
  assert!(matches!(error("<a xmlns:xml='urn:x'/>"), Error::Namespace { .. }));
  assert!(matches!(error("<a xmlns:p='http://www.w3.org/XML/1998/namespace'/>"), Error::Namespace { .. }));
  assert!(matches!(error("<a xmlns='http://www.w3.org/2000/xmlns/'/>"), Error::Namespace { .. }));
  // Rebinding xml to its own namespace name is allowed.
  assert_eq!(trace("<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>").unwrap().len(), 2);
}

#[test]
fn rejects_malformed_tags() {
  assert!(error("<a x/>").message().contains("no value"));
  assert!(error("<a x=1/>").message().contains("not quoted"));
  assert!(error("<a x='1'y='2'/>").message().contains("separated by whitespace"));
  assert!(error("<a b c='1'/>").message().contains("no value"));
}

#[test]
fn rejects_bad_references() {
  assert!(error("<a>&nosuch;</a>").message().contains("not declared"));
  assert!(error("<a>&#xD800;</a>").message().contains("not a character"));
  assert!(error("<a>&#0;</a>").message().contains("not a character"));
  assert!(error("<a>&amp</a>").message().contains("must end with \";\""));
  assert!(error("<a b='&'/>").message().contains("must end with \";\""));
  assert!(error("<a b='<'/>").message().contains("\"<\" may not appear"));
}

#[test]
fn rejects_the_cdata_end_sequence_in_text() {
  assert!(error("<a>]]></a>").message().contains("]]>"));
}

#[test]
fn rejects_a_misplaced_or_malformed_xml_declaration() {
  assert!(error("<a><?xml version='1.0'?></a>").message().contains("reserved"));
  assert!(error(" <?xml version='1.0'?><a/>").message().contains("reserved"));
  assert!(error("<?XML version='1.0'?><a/>").message().contains("reserved"));
  assert!(error("<?xml?><a/>").message().contains("no version"));
  assert!(error("<?xml version='2.0'?><a/>").message().contains("not an XML version"));
  assert!(error("<?xml version='1.0' standalone='maybe'?><a/>").message().contains("standalone"));
  // A misplaced version/encoding/standalone reports its position, not that the name is unknown.
  assert!(error("<?xml encoding='UTF-8' version='1.0'?><a/>").message().contains("encoding must come after version"));
  assert!(
    error("<?xml version='1.0' standalone='yes' encoding='UTF-8'?><a/>").message().contains("encoding must come")
  );
  assert!(error("<?xml standalone='yes' version='1.0'?><a/>").message().contains("standalone must come after version"));
  assert!(error("<?xml version='1.0' version='1.0'?><a/>").message().contains("more than one version"));
  assert!(
    error("<?xml version='1.0' encoding='UTF-8' encoding='UTF-8'?><a/>").message().contains("more than one encoding")
  );
  assert!(
    error("<?xml version='1.0' standalone='yes' standalone='no'?><a/>").message().contains("more than one standalone")
  );
  // A name that is not a pseudo-attribute says so, rather than blaming its position.
  assert!(error("<?xml version='1.0' encdng='UTF-8'?><a/>").message().contains("not a pseudo-attribute"));
}

#[test]
fn reads_the_standalone_declaration() {
  assert_eq!(trace("<?xml version='1.0' standalone='yes'?><a/>").unwrap()[0], "?xml 1.0 standalone");
  assert_eq!(trace("<?xml version='1.1'?><a/>").unwrap()[0], "?xml 1.1");
}

#[test]
fn rejects_an_invalid_xml_space() {
  assert!(error("<a xml:space='maybe'/>").message().contains("xml:space"));
}

/// Every message is read by someone deciding what to do next, so each one is checked for
/// the remedy and not merely for the complaint. See the guidance in `crate::error`.
#[test]
fn messages_say_what_to_do_next() {
  let cases: [(&str, &str); 8] = [
    ("<a>&nosuch;</a>", "write \"&amp;nosuch;\""),
    ("<a>Tom & Jerry</a>", "a reference must end with \";\""),
    ("<a>]]></a>", "write \"]]&gt;\""),
    ("<a b='<'/>", "write \"&lt;\""),
    ("<p:a/>", "add an xmlns:p attribute"),
    ("<a checked/>", "checked=\"...\""),
    ("<a b=1/>", "enclose it in \" or '"),
    ("<a xml:space='maybe'/>", "\"default\" or \"preserve\""),
  ];
  for (xml, expected) in cases {
    let message = error(xml).message().to_owned();
    assert!(message.contains(expected), "parsing {xml:?} said {message:?},\n  which lacks {expected:?}");
  }
}

#[test]
fn errors_point_at_the_offending_position() {
  let at = error("<a>\n  &nosuch;\n</a>").location().clone();
  assert_eq!((at.line, at.column), (2, 3));
}

#[test]
fn attributes_can_be_looked_up_by_expanded_name() {
  let mut parser = Parser::new();
  parser.feed(b"<a xmlns:p='urn:p' x='1' p:y='2'/>", true).unwrap();
  parser.advance().unwrap();
  assert_eq!(parser.attribute_value(None, "x"), Some("1"));
  assert_eq!(parser.attribute_value(Some("urn:p"), "y"), Some("2"));
  assert_eq!(parser.attribute_value(None, "y"), None, "the prefix is not ignored");
  assert_eq!(parser.attribute_value(Some("urn:none"), "x"), None);
  assert_eq!(parser.token_ref().unwrap().attributes().len(), 3);
}

#[test]
fn text_split_across_chunks_is_still_one_event_per_run() {
  // The scanner may cut text short, so a run can arrive as several events; the
  // concatenation is what must be stable.
  let mut parser = Parser::with_document(Entity::document(CharStream::with_encoding("UTF-8").unwrap()));
  let xml = b"<a>one &amp; two</a>";
  let mut text = String::new();
  let mut fed = 0;
  loop {
    match parser.advance().unwrap() {
      Progress::Token(TokenKind::Text) => text.push_str(text_of(&parser)),
      Progress::Eof => break,
      Progress::NeedMoreInput => {
        let end = (fed + 1).min(xml.len());
        parser.feed(&xml[fed..end], end == xml.len()).unwrap();
        fed = end;
      }
      _ => {}
    }
  }
  assert_eq!(text, "one & two");
}

#[test]
fn a_long_text_run_is_delivered_in_bounded_fragments() {
  // Fed in pieces, a run longer than the fragmentation threshold must come out as more than one Text
  // event, so neither the stream nor the parser buffers the whole run. The pieces still concatenate
  // to the original.
  let mut parser = Parser::with_document(Entity::document(CharStream::with_encoding("UTF-8").unwrap()));
  let body = "x".repeat(30_000);
  let xml = format!("<a>{body}</a>");
  let bytes = xml.as_bytes();
  let mut text = String::new();
  let mut text_events = 0;
  let mut fed = 0;
  loop {
    match parser.advance().unwrap() {
      Progress::Token(TokenKind::Text) => {
        text.push_str(text_of(&parser));
        text_events += 1;
      }
      Progress::Eof => break,
      Progress::NeedMoreInput => {
        let end = (fed + 1000).min(bytes.len());
        parser.feed(&bytes[fed..end], end == bytes.len()).unwrap();
        fed = end;
      }
      _ => {}
    }
  }
  assert_eq!(text, body);
  assert!(text_events > 1, "a long run must be split into fragments, got {text_events}");
}

#[test]
fn a_document_can_be_parsed_from_a_reader_that_stalls() {
  // NeedMoreInput must be answerable with nothing at all without losing state.
  let mut parser = Parser::new();
  assert_eq!(parser.advance().unwrap(), Progress::NeedMoreInput);
  parser.feed(b"", false).unwrap();
  assert_eq!(parser.advance().unwrap(), Progress::NeedMoreInput);
  parser.feed(b"<a/>", true).unwrap();
  assert_eq!(parser.advance().unwrap(), Progress::Token(TokenKind::StartElement));
}

#[test]
fn non_utf8_documents_are_decoded_before_parsing() {
  let mut parser = Parser::new();
  let mut bytes = b"<?xml version='1.0' encoding='ISO-8859-1'?><a>".to_vec();
  bytes.push(0xE9); // e-acute in Latin-1
  bytes.extend_from_slice(b"</a>");
  parser.feed(&bytes, true).unwrap();

  let mut text = None;
  while let Progress::Token(kind) = parser.advance().unwrap() {
    if kind == TokenKind::Text {
      text = Some(text_of(&parser).to_owned());
    }
  }
  assert_eq!(text.as_deref(), Some("é"));
}

/// Drives the parser to the point where it asks for the external entity `e`, hands the entity over with `hand_over`,
/// and returns the position the parser then reports, which is the one inside that entity.
fn location_in_external_entity(hand_over: fn(&mut Parser) -> Result<()>) -> Location {
  let xml = "<!DOCTYPE a [<!ENTITY e PUBLIC \"-//x//y\" \"part.ent\">]><a>&e;</a>";
  let document = Entity::document(CharStream::new().with_system_id("file:///d/doc.xml"));
  let mut parser = Parser::with_document(document);
  let mut fed = false;
  loop {
    match parser.advance().expect("the document is well-formed") {
      Progress::NeedMoreInput => {
        assert!(!fed, "the parser asked for input it has already been given");
        parser.feed(xml.as_bytes(), true).expect("the whole document at once");
        fed = true;
      }
      Progress::NeedEntity => {
        let public_id = parser.pending_entity().expect("the reference asks for the entity").public_id();
        assert_eq!(public_id, Some("-//x//y"), "the request carries what the declaration named");
        hand_over(&mut parser).expect("the entity is handed over");
        return parser.location();
      }
      Progress::Token(_) => {}
      Progress::Eof => panic!("the entity was never requested"),
    }
  }
}

#[test]
fn a_position_inside_an_external_entity_carries_both_of_its_identifiers() {
  // The public identifier is what the declaration named the entity by, and the system identifier is where it was read
  // from, resolved against the document's own URI. Both travel with the entity, by either way of handing one over.
  for at in [
    location_in_external_entity(|parser| parser.provide_entity(b"hi")),
    location_in_external_entity(Parser::begin_entity),
  ] {
    assert_eq!(at.public_id.as_deref(), Some("-//x//y"));
    assert_eq!(at.system_id.as_deref(), Some("file:///d/part.ent"));
  }
}
