use super::*;

fn parse(subset: &str) -> Result<(Dtd, NamePool)> {
  let mut pool = NamePool::new();
  let dtd = parse_subset(subset, &mut pool, Location::unknown())?;
  Ok((dtd, pool))
}

fn general(subset: &str, name: &str) -> GeneralEntity {
  let (dtd, mut pool) = parse(subset).expect("parses");
  dtd.general_entity(pool.intern(name)).expect("declared").clone()
}

#[test]
fn reads_internal_general_entities() {
  assert_eq!(
    general("<!ENTITY greeting \"hello\">", "greeting"),
    GeneralEntity::Internal { value: "hello".to_owned() }
  );
}

#[test]
fn expands_character_references_but_keeps_general_ones_in_entity_values() {
  // &#60; becomes '<' now; &other; stays for expansion on use.
  assert_eq!(
    general("<!ENTITY e \"a&#60;b&other;c\">", "e"),
    GeneralEntity::Internal { value: "a<b&other;c".to_owned() }
  );
}

#[test]
fn reads_external_and_unparsed_entities() {
  let (dtd, mut pool) = parse("<!ENTITY logo SYSTEM \"logo.png\" NDATA png>").unwrap();
  let logo = dtd.general_entity(pool.intern("logo")).unwrap().clone();
  let GeneralEntity::Unparsed { system_id, notation, .. } = logo else { panic!("expected an unparsed entity") };
  assert_eq!(system_id, "logo.png");
  assert_eq!(pool.resolve(notation), "png");

  assert!(matches!(
    general("<!ENTITY chap SYSTEM \"chap1.xml\">", "chap"),
    GeneralEntity::External { system_id, .. } if system_id == "chap1.xml"
  ));
  assert!(matches!(
    general("<!ENTITY chap PUBLIC \"-//x//y\" \"chap1.xml\">", "chap"),
    GeneralEntity::External { public_id: Some(p), .. } if p == "-//x//y"
  ));
}

#[test]
fn expands_internal_parameter_entities_between_declarations() {
  let subset = "<!ENTITY % common \"<!ENTITY shared 'value'>\"> %common;";
  assert_eq!(general(subset, "shared"), GeneralEntity::Internal { value: "value".to_owned() });
}

#[test]
fn reads_element_and_attlist_declarations() {
  let subset = "<!ELEMENT note (to, from, body)>\
                <!ATTLIST note id ID #REQUIRED priority (high|low) \"low\" lang CDATA #IMPLIED>";
  let (dtd, mut pool) = parse(subset).unwrap();
  let note = pool.intern("note");
  let (to, from, body) = (pool.intern("to"), pool.intern("from"), pool.intern("body"));
  assert_eq!(
    dtd.content_spec(note),
    Some(&ContentSpec::Children(ContentParticle::Seq(
      vec![
        ContentParticle::Name(to, Occurs::Once),
        ContentParticle::Name(from, Occurs::Once),
        ContentParticle::Name(body, Occurs::Once),
      ],
      Occurs::Once,
    )))
  );

  let attlist = dtd.attlist(note).expect("has an attlist");
  assert_eq!(attlist.len(), 3);
  assert_eq!(attlist[0].name, pool.intern("id"));
  assert_eq!(attlist[0].att_type, AttType::Id);
  assert_eq!(attlist[0].default, DefaultDecl::Required);
  assert_eq!(attlist[1].att_type, AttType::Enumeration(vec![pool.intern("high"), pool.intern("low")]));
  assert_eq!(attlist[1].default, DefaultDecl::Default("low".to_owned()));
  assert_eq!(attlist[2].default, DefaultDecl::Implied);
}

#[test]
fn tokenized_defaults_are_whitespace_collapsed() {
  let (dtd, mut pool) = parse("<!ATTLIST e refs IDREFS \"  a   b  \">").unwrap();
  let attlist = dtd.attlist(pool.intern("e")).unwrap();
  assert_eq!(attlist[0].default, DefaultDecl::Default("a b".to_owned()));
}

#[test]
fn a_cdata_default_keeps_its_whitespace() {
  let (dtd, mut pool) = parse("<!ATTLIST e note CDATA \"  spaced  out  \">").unwrap();
  let attlist = dtd.attlist(pool.intern("e")).unwrap();
  // Literal whitespace still normalizes to spaces, but runs are not collapsed for CDATA.
  assert_eq!(attlist[0].default, DefaultDecl::Default("  spaced  out  ".to_owned()));
}

#[test]
fn reads_notation_types_and_declarations() {
  let subset = "<!NOTATION png SYSTEM \"image/png\"><!ATTLIST img type NOTATION (png) #IMPLIED>";
  let (dtd, mut pool) = parse(subset).unwrap();
  let attlist = dtd.attlist(pool.intern("img")).unwrap();
  assert_eq!(attlist[0].att_type, AttType::Notation(vec![pool.intern("png")]));
}

#[test]
fn comments_and_processing_instructions_are_skipped() {
  let subset = "<!-- a comment --><?pi data?><!ENTITY e \"v\">";
  assert_eq!(general(subset, "e"), GeneralEntity::Internal { value: "v".to_owned() });
}

#[test]
fn keyword_scanning_does_not_confuse_prefixes() {
  // ID must not swallow the start of IDREF, nor ENTITY the start of ENTITIES.
  let (dtd, mut pool) = parse("<!ATTLIST e a IDREF #IMPLIED b ENTITIES #IMPLIED>").unwrap();
  let attlist = dtd.attlist(pool.intern("e")).unwrap();
  assert_eq!(attlist[0].att_type, AttType::IdRef);
  assert_eq!(attlist[1].att_type, AttType::Entities);
}

#[test]
fn parses_content_models_and_rejects_malformed_ones() {
  // Every well-formed shape is accepted.
  for spec in ["EMPTY", "ANY", "(#PCDATA)", "(#PCDATA|a|b)*", "(a)", "(a,b,c)", "(a|b|c)", "(a?,(b|c)+)*", "(a,b)?"] {
    assert!(parse(&format!("<!ELEMENT e {spec}>")).is_ok(), "{spec} should parse");
  }
  // Malformed content models are rejected.
  for spec in ["(a,b|c)", "(a,)", "(|a)", "(a b)", "(#PCDATA|a)", "(a", "()", "(#PCDATA,a)*"] {
    assert!(parse(&format!("<!ELEMENT e {spec}>")).is_err(), "{spec} should be rejected");
  }
}

#[test]
fn rejects_malformed_declarations() {
  assert!(parse("<!ENTITY>").is_err());
  assert!(parse("<!ELEMENT e>").is_err(), "no content spec");
  assert!(parse("<!ATTLIST e a>").is_err(), "no type");
  assert!(parse("<!WRONG e>").is_err());
  assert!(parse("<!ENTITY e \"unclosed>").is_err());
  assert!(parse("<!ENTITY % p \"v\"> %undeclared;").is_err());
}

#[test]
fn rejects_a_parameter_entity_reference_inside_a_declaration_in_the_internal_subset() {
  // WFC: PEs in Internal Subset.
  assert!(parse("<!ENTITY % p \"CDATA\"><!ATTLIST e a %p; #IMPLIED>").is_err());
}

#[test]
fn a_dtd_comment_may_not_contain_double_hyphen() {
  // XML 1.0 §2.5: the body may not contain "--", nor end with "-" before its "-->".
  assert!(parse("<!ENTITY e \"v\"><!-- a -- b -->").is_err(), "\"--\" in the body");
  assert!(parse("<!ENTITY e \"v\"><!--a--->").is_err(), "body ends with \"-\"");
  // A well-formed comment is still skipped.
  assert!(parse("<!ENTITY e \"v\"><!-- a - b -->").is_ok());
}

#[test]
fn a_deeply_nested_content_model_is_rejected_not_overflowed() {
  // Run on a generous stack so the test itself cannot overflow while proving that the parser
  // fails cleanly, rather than overflowing, once the nesting bound is passed.
  std::thread::Builder::new()
    .stack_size(16 * 1024 * 1024)
    .spawn(|| {
      let n = MAX_CONTENT_DEPTH + 8;
      let deep = format!("<!ELEMENT e {}a{}>", "(".repeat(n), ")".repeat(n));
      assert!(parse(&deep).is_err(), "a model nested past the bound is rejected");
      // A model within the bound still parses.
      let ok = format!("<!ELEMENT e {}a{}>", "(".repeat(8), ")".repeat(8));
      assert!(parse(&ok).is_ok());
    })
    .unwrap()
    .join()
    .unwrap();
}

/// The text of the entity `pe` from `entities`, keyed by system identifier, located at the start of a resource of that
/// identifier.
fn answer(entities: &[(&str, &str)], pe: &ExternalPe) -> Option<(String, Location)> {
  let (id, text) = entities.iter().find(|(id, _)| *id == pe.system_id)?;
  Some(((*text).to_owned(), Location::new().with_system_id(*id)))
}

/// Assembles `internal` as the internal subset of the document `doc`, answering each external parameter entity from
/// `entities`.
fn assemble(internal: &str, entities: &[(&str, &str)]) -> Result<(Dtd, NamePool)> {
  let mut pool = NamePool::new();
  let mut assembly = DtdAssembly::with_internal_subset(internal, Location::new().with_system_id("doc"));
  let dtd = assembly.complete(&mut pool, |pe| Ok(answer(entities, pe)))?;
  Ok((dtd, pool))
}

#[test]
fn declarations_from_an_external_pe_in_the_internal_subset_are_external() {
  let internal = "<!ENTITY before 'b'><!ENTITY % ext SYSTEM 'urn:ext'>%ext;<!ENTITY after 'a'>";
  let (dtd, mut pool) = assemble(internal, &[("urn:ext", "<!ENTITY inside 'i'><!ATTLIST e x CDATA 'd'>")]).unwrap();

  assert!(dtd.general_entity_is_external(pool.intern("inside")), "declared in the external parameter entity");
  assert!(dtd.attlist_is_external(pool.intern("e")), "declared in the external parameter entity");
  assert!(!dtd.general_entity_is_external(pool.intern("before")), "declared in the internal subset");
  assert!(!dtd.general_entity_is_external(pool.intern("after")), "declared in the internal subset");
}

#[test]
fn an_external_pe_in_the_internal_subset_is_read_by_external_subset_rules() {
  // A parameter entity reference inside a declaration and a conditional section, both allowed only in external text.
  // `%model;` is spliced into the external text before the conditional section, which must still be read as external.
  let internal = "<!ENTITY % model 'EMPTY'><!ENTITY % ext SYSTEM 'urn:ext'>%ext;";
  let external = "<!ELEMENT a %model;><![INCLUDE[<!ELEMENT b EMPTY>]]>";
  let (dtd, mut pool) = assemble(internal, &[("urn:ext", external)]).unwrap();

  assert_eq!(dtd.content_spec(pool.intern("a")), Some(&ContentSpec::Empty));
  assert!(dtd.has_element(pool.intern("b")));
}

#[test]
fn the_internal_subset_after_an_external_pe_keeps_internal_subset_rules() {
  let internal = "<!ENTITY % ext SYSTEM 'urn:ext'>%ext;<![INCLUDE[<!ELEMENT b EMPTY>]]>";
  let error = assemble(internal, &[("urn:ext", "<!ELEMENT a EMPTY>")]).unwrap_err();
  assert!(error.message().contains("internal subset"), "{error}");
}

#[test]
fn nested_external_pes_in_the_internal_subset_are_external() {
  // The inner entity is referenced from the outer one's text, which is itself inside the internal subset.
  let internal = "<!ENTITY % outer SYSTEM 'urn:outer'>%outer;<!ENTITY last 'l'>";
  let outer = "<!ENTITY % inner SYSTEM 'urn:inner'>%inner;<!ENTITY middle 'm'>";
  let (dtd, mut pool) = assemble(internal, &[("urn:outer", outer), ("urn:inner", "<!ENTITY deepest 'd'>")]).unwrap();

  assert!(dtd.general_entity_is_external(pool.intern("deepest")));
  assert!(dtd.general_entity_is_external(pool.intern("middle")), "after the inner entity, still in the outer one");
  assert!(!dtd.general_entity_is_external(pool.intern("last")));
}

#[test]
fn shifting_regions_follows_a_splice() {
  let mut regions = vec![0..4, 10..20, 30..40];
  let shift_all = |regions: &mut Vec<Range<usize>>, range: Range<usize>, new_len: usize| {
    for region in regions.iter_mut() {
      shift(region, &range, new_len);
    }
  };
  // Replace 12..15 (inside the second region) with 8 bytes: +5.
  shift_all(&mut regions, 12..15, 8);
  assert_eq!(regions, [0..4, 10..25, 35..45]);
  // Replace 5..9 (between regions) with 1 byte: -3.
  shift_all(&mut regions, 5..9, 1);
  assert_eq!(regions, [0..4, 7..22, 32..42]);
}

/// Assembles `external` as the external subset `ext.dtd` alone, answering each external parameter entity from
/// `entities`.
fn assemble_external(external: &str, entities: &[(&str, &str)]) -> Result<(Dtd, NamePool)> {
  let mut assembly = DtdAssembly::new();
  assembly.add_external_subset(external, Location::new().with_system_id("ext.dtd"));
  let mut pool = NamePool::new();
  let dtd = assembly.complete(&mut pool, |pe| Ok(answer(entities, pe)))?;
  Ok((dtd, pool))
}

const STRADDLES: &str = "begins in one parameter entity and ends in another";

#[test]
fn a_declaration_may_not_straddle_the_end_of_an_external_pe() {
  let entities = [("urn:p", "<!ELEMENT a")];
  let error = assemble_external("<!ENTITY % p SYSTEM 'urn:p'>%p; EMPTY>", &entities).unwrap_err();
  assert!(error.message().contains(STRADDLES), "in the external subset: {error}");

  let error = assemble("<!ENTITY % p SYSTEM 'urn:p'>%p; EMPTY>", &entities).unwrap_err();
  assert!(error.message().contains(STRADDLES), "in the internal subset: {error}");
}

#[test]
fn an_internal_pe_expanded_before_a_pause_is_still_checked_after_it() {
  // `%i;` is expanded in the first pass, which then stops at `%e;` inside the declaration it opened. The second pass
  // reads that declaration again, and must still know where `%i;`'s text ends.
  let external = "<!ENTITY % i '<!ELEMENT a'><!ENTITY % e SYSTEM 'urn:e'>%i; %e;>";
  let error = assemble_external(external, &[("urn:e", "EMPTY")]).unwrap_err();
  assert!(error.message().contains(STRADDLES), "{error}");
}

#[test]
fn a_declaration_in_an_include_section_may_not_straddle_a_pe() {
  let error = parse_external("<!ENTITY % p '<!ELEMENT a'><![INCLUDE[%p; EMPTY>]]>").unwrap_err();
  assert!(error.message().contains(STRADDLES), "{error}");
}

#[test]
fn an_external_pe_inside_a_declaration_is_properly_nested() {
  let (dtd, mut pool) =
    assemble_external("<!ENTITY % m SYSTEM 'urn:m'><!ELEMENT a %m;>", &[("urn:m", "EMPTY")]).unwrap();
  assert_eq!(dtd.content_spec(pool.intern("a")), Some(&ContentSpec::Empty));
}

/// Parses `external` as an external subset that references no external parameter entity.
fn parse_external(external: &str) -> Result<(Dtd, NamePool)> {
  assemble_external(external, &[])
}

#[test]
fn an_external_pe_in_an_entity_value_is_included_as_literal_data() {
  // No spaces are added around the text, and its quotes do not close the literal (XML 1.0 §4.4.5).
  let external = "<!ENTITY % q SYSTEM 'urn:q'><!ENTITY e \"[%q;]\">";
  let (dtd, mut pool) = assemble_external(external, &[("urn:q", "a\"b'c")]).unwrap();
  assert_eq!(dtd.general_entity(pool.intern("e")), Some(&GeneralEntity::Internal { value: "[a\"b'c]".to_owned() }));
}

#[test]
fn a_pe_reference_in_an_external_pe_included_in_a_literal_is_expanded() {
  let external = "<!ENTITY % i 'I'><!ENTITY % q SYSTEM 'urn:q'><!ENTITY e '[%q;]'>";
  let (dtd, mut pool) = assemble_external(external, &[("urn:q", "x%i;y")]).unwrap();
  assert_eq!(dtd.general_entity(pool.intern("e")), Some(&GeneralEntity::Internal { value: "[xIy]".to_owned() }));
}

/// The system identifier, line, and column of the error `result` failed with.
fn error_position(result: Result<(Dtd, NamePool)>) -> (Option<String>, u32, u32) {
  let error = result.expect_err("the DTD is malformed");
  let at = error.location();
  (at.system_id.as_deref().map(ToOwned::to_owned), at.line, at.column)
}

/// The 1-based column of the first `needle` in the one-line `text`.
fn column_of(text: &str, needle: &str) -> u32 {
  u32::try_from(text[..text.find(needle).expect("present")].chars().count()).unwrap() + 1
}

#[test]
fn an_error_in_the_internal_subset_has_its_line_and_column() {
  let error = parse_subset("<!ELEMENT a EMPTY>\n<!ELEMENT b FOO>", &mut NamePool::new(), Location::new()).unwrap_err();
  assert_eq!((error.location().line, error.location().column), (2, 13));
}

#[test]
fn an_error_after_an_expanded_internal_pe_counts_the_reference_as_written() {
  let subset = "<!ENTITY % p '<!ELEMENT a EMPTY>'>%p; <!ELEMENT b FOO>";
  let error = parse_subset(subset, &mut NamePool::new(), Location::new()).unwrap_err();
  assert_eq!((error.location().line, error.location().column), (1, column_of(subset, "FOO")));
}

#[test]
fn an_error_inside_an_internal_pe_is_located_at_the_reference() {
  let subset = "<!ENTITY % p '<!ELEMENT b FOO>'>\n  %p;";
  let error = parse_subset(subset, &mut NamePool::new(), Location::new()).unwrap_err();
  assert_eq!((error.location().line, error.location().column), (2, 3));
}

#[test]
fn an_error_inside_an_external_pe_is_located_in_its_resource() {
  let entity = "<!ELEMENT a EMPTY>\n<!ELEMENT b FOO>";
  let result = assemble("<!ENTITY % p SYSTEM 'urn:p'>\n%p;", &[("urn:p", entity)]);
  assert_eq!(error_position(result), (Some("urn:p".to_owned()), 2, 13));
}

#[test]
fn an_error_after_an_external_pe_is_located_in_the_text_that_referenced_it() {
  let internal = "<!ENTITY % p SYSTEM 'urn:p'>%p;<!ELEMENT b FOO>";
  let result = assemble(internal, &[("urn:p", "<!ELEMENT a EMPTY>\n\n\n")]);
  assert_eq!(error_position(result), (Some("doc".to_owned()), 1, column_of(internal, "FOO")));
}

#[test]
fn an_error_in_the_external_subset_is_located_in_its_resource() {
  let result = assemble_external("\n\n<!ELEMENT b FOO>", &[]);
  assert_eq!(error_position(result), (Some("ext.dtd".to_owned()), 3, 13));
}

#[test]
fn a_declined_pe_is_reported_at_its_reference() {
  let result = assemble("<!ENTITY % p SYSTEM 'urn:p'>\n\n %p;", &[]);
  assert_eq!(error_position(result), (Some("doc".to_owned()), 3, 2));
}

/// Each request's system identifier and base, in the order they were made.
type Requests = std::rc::Rc<std::cell::RefCell<Vec<(String, Option<String>)>>>;

/// A resolver that serves `files` by resolved URI and records the base of each request.
struct Recording {
  files: Vec<(&'static str, &'static str)>,
  bases: Requests,
}

impl crate::io::resolve::UriResolver for Recording {
  fn resolve(&mut self, request: &crate::io::resolve::EntityRequest) -> Result<Option<Box<dyn std::io::Read>>> {
    self.bases.borrow_mut().push((request.system_id().to_owned(), request.base_uri().map(ToOwned::to_owned)));
    let uri = request.resolved_uri().unwrap_or_default();
    let text = self.files.iter().find(|(id, _)| *id == uri).map(|(_, text)| *text);
    Ok(text.map(|text| Box::new(std::io::Cursor::new(text.as_bytes().to_vec())) as Box<dyn std::io::Read>))
  }
}

#[test]
fn a_pe_system_id_is_resolved_against_the_resource_it_was_declared_in() {
  let bases = Requests::default();
  let resolver = Recording {
    files: vec![
      ("file:///dtd/sub/a.ent", "<!ENTITY % b SYSTEM 'b.ent'>%b;"),
      ("file:///dtd/sub/b.ent", "<!ELEMENT deep EMPTY>"),
    ],
    bases: std::rc::Rc::clone(&bases),
  };
  let main = "<!ENTITY % a SYSTEM 'sub/a.ent'>%a;";
  let (dtd, pool) =
    DtdReader::with_system_id(main.as_bytes(), "file:///dtd/main.dtd").with_resolver(resolver).read().unwrap();

  assert!(dtd.has_element(pool.get("deep").expect("declared in the nested entity")));
  assert_eq!(
    *bases.borrow(),
    [
      ("sub/a.ent".to_owned(), Some("file:///dtd/main.dtd".to_owned())),
      ("b.ent".to_owned(), Some("file:///dtd/sub/a.ent".to_owned())),
    ]
  );
}

#[test]
fn an_external_entity_records_the_resource_it_was_declared_in() {
  let internal = "<!ENTITY here SYSTEM 'here.xml'><!ENTITY % p SYSTEM 'urn:p'>%p;";
  let entity = "<!ENTITY there SYSTEM 'there.xml'><!NOTATION n SYSTEM 'n'><!ENTITY pic SYSTEM 'pic.png' NDATA n>";
  let (dtd, mut pool) = assemble(internal, &[("urn:p", entity)]).unwrap();

  let base_of = |entity: Option<&GeneralEntity>| match entity {
    Some(GeneralEntity::External { base, .. } | GeneralEntity::Unparsed { base, .. }) => base.clone(),
    other => panic!("not an external entity: {other:?}"),
  };
  assert_eq!(base_of(dtd.general_entity(pool.intern("here"))).as_deref(), Some("doc"));
  assert_eq!(base_of(dtd.general_entity(pool.intern("there"))).as_deref(), Some("urn:p"));
  assert_eq!(base_of(dtd.general_entity(pool.intern("pic"))).as_deref(), Some("urn:p"));
  assert!(matches!(
    dtd.parameter_entity(pool.intern("p")),
    Some(ParameterEntity::External { base: Some(base), .. }) if base == "doc"
  ));
}
