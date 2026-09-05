//! Assembling a DTD by hand and reading one back, without a document parser anywhere in sight.
//!
//! A DTD reached this way is the same value the parser produces, so a schema can be written in code, read from a
//! subset, or read from one and then extended.

use xenolith_core::error::Location;
use xenolith_core::name::NamePool;
use xenolith_dtd::{AttDef, AttType, ContentParticle, ContentSpec, DefaultDecl, ExternalId, GeneralEntity, Occurs};
use xenolith_dtd::{Dtd, parse_subset};

#[test]
fn a_dtd_assembled_by_hand_reads_back_the_way_a_parsed_one_does() {
  let mut pool = NamePool::new();
  let note = pool.intern("note");
  let body = pool.intern("body");
  let id = pool.intern("id");

  let mut dtd = Dtd::default();
  assert!(dtd.declare_element(note, ContentSpec::Children(ContentParticle::Name(body, Occurs::Once))));
  assert!(dtd.declare_element(body, ContentSpec::Mixed(Vec::new())));
  dtd.declare_attributes(note, [AttDef { name: id, att_type: AttType::Id, default: DefaultDecl::Implied }]);

  assert!(dtd.has_element(note));
  assert!(matches!(dtd.content_spec(body), Some(ContentSpec::Mixed(names)) if names.is_empty()));
  let defs = dtd.attlist(note).expect("the attributes just declared");
  assert_eq!(defs.len(), 1);
  assert_eq!(defs[0].name, id);
}

#[test]
fn a_second_declaration_of_a_name_leaves_the_first_standing() {
  // XML makes a repeated <!ELEMENT> or <!NOTATION> an error. The first declaration is the one that counts, and the
  // return says the second was refused.
  let mut pool = NamePool::new();
  let a = pool.intern("a");
  let gif = pool.intern("gif");

  let mut dtd = Dtd::default();
  assert!(dtd.declare_element(a, ContentSpec::Empty));
  assert!(!dtd.declare_element(a, ContentSpec::Any), "the repeat is refused");
  assert!(matches!(dtd.content_spec(a), Some(ContentSpec::Empty)), "the first declaration stands");

  let first = ExternalId { public_id: None, system_id: Some("first.gif".to_owned()) };
  let second = ExternalId { public_id: None, system_id: Some("second.gif".to_owned()) };
  assert!(dtd.declare_notation(gif, first));
  assert!(!dtd.declare_notation(gif, second));
  assert_eq!(dtd.notation(gif).and_then(|id| id.system_id.as_deref()), Some("first.gif"));
}

#[test]
fn attribute_declarations_accumulate() {
  // XML allows several <!ATTLIST> for one element; they add up rather than replace.
  let mut pool = NamePool::new();
  let e = pool.intern("e");
  let (one, two) = (pool.intern("one"), pool.intern("two"));

  let mut dtd = Dtd::default();
  dtd.declare_attributes(e, [AttDef { name: one, att_type: AttType::Cdata, default: DefaultDecl::Implied }]);
  dtd.declare_attributes(e, [AttDef { name: two, att_type: AttType::Cdata, default: DefaultDecl::Implied }]);

  let names: Vec<_> = dtd.attlist(e).expect("declared").iter().map(|d| d.name).collect();
  assert_eq!(names, [one, two], "in the order they were declared");
}

#[test]
fn every_kind_of_declaration_can_be_walked() {
  let subset = "<!ELEMENT note (#PCDATA)>\
                <!ATTLIST note id ID #IMPLIED>\
                <!ENTITY who \"world\">\
                <!ENTITY % pe \"<!ELEMENT extra EMPTY>\">\
                <!NOTATION gif SYSTEM \"image/gif\">";
  let mut pool = NamePool::new();
  let dtd = parse_subset(subset, &mut pool, Location::unknown()).expect("a well-formed subset");

  let names = |ids: Vec<xenolith_core::name::NameId>| {
    let mut out: Vec<String> = ids.iter().map(|&id| pool.resolve(id).to_owned()).collect();
    out.sort();
    out
  };

  assert_eq!(names(dtd.elements().map(|(n, _)| n).collect()), ["note"]);
  assert_eq!(names(dtd.attlists().map(|(n, _)| n).collect()), ["note"]);
  assert_eq!(names(dtd.general_entities().map(|(n, _)| n).collect()), ["who"]);
  assert_eq!(names(dtd.parameter_entities().map(|(n, _)| n).collect()), ["pe"]);
  assert_eq!(names(dtd.notations().map(|(n, _)| n).collect()), ["gif"]);

  let who = pool.get("who").expect("interned while parsing");
  assert!(matches!(dtd.general_entity(who), Some(GeneralEntity::Internal { value }) if value == "world"));
  assert!(dtd.parameter_entity(pool.get("pe").expect("interned")).is_some());
}

#[test]
fn a_parsed_dtd_can_be_extended_by_hand() {
  let mut pool = NamePool::new();
  let mut dtd = parse_subset("<!ELEMENT a EMPTY>", &mut pool, Location::unknown()).expect("well-formed");
  let b = pool.intern("b");

  assert!(dtd.declare_element(b, ContentSpec::Any));
  assert!(dtd.has_element(pool.get("a").expect("interned while parsing")));
  assert!(dtd.has_element(b), "what was read and what was added sit in the same DTD");
}
