use super::*;
use crate::attr::{Attribute, Attributes};
use crate::event::XmlSpace;

/// A name split as the lenient parser splits one: at the first colon, when both sides are non-empty.
fn parts(lexical: &str) -> (Option<&str>, &str) {
  match lexical.split_once(':') {
    Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => (Some(prefix), local),
    _ => (None, lexical),
  }
}

/// An attribute written as `lexical="value"` in `namespace`.
fn attribute(lexical: &str, namespace: Option<&str>, value: &str) -> Attribute {
  let (prefix, local) = parts(lexical);
  Attribute {
    prefix: prefix.map(ToOwned::to_owned),
    local: local.to_owned(),
    namespace: namespace.map(ToOwned::to_owned),
    value: value.to_owned(),
    location: Location::unknown(),
    value_location: Location::unknown(),
  }
}

/// A start element named `lexical` in `namespace`, with `attributes`.
fn start<'a>(lexical: &'a str, namespace: Option<&'a str>, attributes: &'a Vec<Attribute>) -> EventRef<'a> {
  let (prefix, local) = parts(lexical);
  EventRef::StartElement(StartElementEventRef::new(
    prefix,
    local,
    namespace,
    Attributes::new(attributes),
    XmlSpace::Default,
    None,
    None,
    Location::unknown(),
  ))
}

/// An end element named `lexical` in `namespace`.
fn end<'a>(lexical: &'a str, namespace: Option<&'a str>) -> EventRef<'a> {
  let (prefix, local) = parts(lexical);
  EventRef::EndElement(EndElementEventRef::new(prefix, local, namespace, Location::unknown()))
}

fn text(text: &str) -> EventRef<'_> {
  EventRef::Characters(CharactersEventRef::new(text, Location::unknown()))
}

fn cdata(text: &str) -> EventRef<'_> {
  EventRef::Cdata(CdataEventRef::new(text, Location::unknown()))
}

fn comment(text: &str) -> EventRef<'_> {
  EventRef::Comment(CommentEventRef::new(text, Location::unknown()))
}

fn pi<'a>(target: &'a str, data: &'a str) -> EventRef<'a> {
  EventRef::ProcessingInstruction(ProcessingInstructionEventRef::new(
    target,
    data,
    Location::unknown(),
    Location::unknown(),
  ))
}

/// Runs `events` through one validator, returning the first refusal.
fn run(events: &[EventRef<'_>]) -> Result<()> {
  let mut strict = StrictXmlValidator::new();
  events.iter().try_for_each(|event| strict.handle(event))
}

/// Runs `events` as the content of a document with an empty root element `r` around them.
fn inside(events: &[EventRef<'_>]) -> Result<()> {
  let none = Vec::new();
  let mut document = vec![EventRef::StartDocument, start("r", None, &none)];
  document.extend(events.iter().cloned());
  document.extend([end("r", None), EventRef::EndDocument]);
  run(&document)
}

/// The message of the refusal `events` gets, which the test expects there to be.
fn refused(result: Result<()>) -> String {
  result.expect_err("the strict validator refuses this").message().to_owned()
}

#[test]
fn a_well_formed_document_passes() {
  let root = vec![attribute("xmlns", XMLNS_NS_URI.into(), "urn:d"), attribute("xmlns:p", XMLNS_NS_URI.into(), "urn:p")];
  let child = vec![
    attribute("id", None, "1"),
    attribute("p:x", Some("urn:p"), "2"),
    attribute("xml:space", Some(XML_NS_URI), "preserve"),
  ];
  let none = Vec::new();
  let events = [
    EventRef::StartDocument,
    comment(" before "),
    text("\n"),
    start("r", Some("urn:d"), &root),
    start("p:a", Some("urn:p"), &child),
    text("t"),
    cdata(" a<b "),
    end("p:a", Some("urn:p")),
    start("b", Some("urn:d"), &none),
    pi("pi", "data"),
    end("b", Some("urn:d")),
    end("r", Some("urn:d")),
    text(" "),
    comment(" after "),
    EventRef::EndDocument,
  ];
  run(&events).unwrap();
}

#[test]
fn a_qname_passes_and_a_name_that_is_not_one_is_refused_with_the_fault() {
  let none = Vec::new();
  assert!(refused(inside(&[start("1a", None, &none), end("1a", None)])).contains("may not begin a name"));
  let caret = vec![attribute("b^c", None, "v")];
  assert!(refused(inside(&[start("a", None, &caret), end("a", None)])).contains("'^'"));
  let colons = vec![attribute("p:q:r", None, "v")];
  assert!(refused(inside(&[start("a", None, &colons), end("a", None)])).contains("more than one colon"));
}

#[test]
fn a_part_that_begins_badly_is_named_as_the_part_it_is() {
  let none = Vec::new();
  let local = refused(inside(&[start("a:1b", Some("urn:a"), &none), end("a:1b", Some("urn:a"))]));
  assert!(local.contains("has a local part that starts with '1'"), "{local}");
  let prefix = refused(inside(&[start("1a:b", Some("urn:a"), &none), end("1a:b", Some("urn:a"))]));
  assert!(prefix.contains("has a prefix that starts with '1'"), "{prefix}");
  let whole = refused(inside(&[start("1a", None, &none), end("1a", None)]));
  assert!(whole.contains("\"1a\" starts with '1'"), "{whole}");
}

#[test]
fn an_attribute_prefix_that_is_not_bound_is_reported_at_the_attribute() {
  let at = Location { line: 4, column: 9, offset: 30, ..Location::unknown() };
  let unbound = vec![Attribute { location: at, ..attribute("q:x", Some("urn:q"), "1") }];
  let mut strict = StrictXmlValidator::new();
  strict.handle(&EventRef::StartDocument).unwrap();
  let error = strict.handle(&start("a", None, &unbound)).expect_err("`q` is not bound");
  assert!(error.message().contains("prefix \"q\" is not bound"), "{error}");
  assert_eq!((error.location().line, error.location().column), (4, 9), "at the attribute, not its element");
}

#[test]
fn an_attribute_given_twice_is_refused() {
  let twice = vec![attribute("x", None, "1"), attribute("y", None, "2"), attribute("x", None, "3")];
  assert!(refused(inside(&[start("a", None, &twice), end("a", None)])).contains("\"x\" appears twice"));
}

#[test]
fn a_comment_with_two_dashes_or_a_trailing_dash_is_refused() {
  assert!(refused(inside(&[comment(" a -- b ")])).contains("--"));
  assert!(refused(inside(&[comment("a-")])).contains("end with"));
  inside(&[comment(" a - b ")]).unwrap();
}

#[test]
fn a_processing_instruction_needs_a_name_that_is_not_reserved_and_data_that_does_not_end_it() {
  assert!(refused(inside(&[pi("1a", "")])).contains("processing instruction target"));
  assert!(refused(inside(&[pi("XmL", "")])).contains("reserved target"));
  assert!(refused(inside(&[pi("p", "a ?> b")])).contains("?>"));
}

#[test]
fn an_event_outside_a_document_is_refused() {
  assert!(refused(run(&[comment("c")])).contains("outside a document"));
  let none = Vec::new();
  let after = [EventRef::StartDocument, start("r", None, &none), end("r", None), EventRef::EndDocument, comment("c")];
  assert!(refused(run(&after)).contains("outside a document"));
}

#[test]
fn elements_have_to_nest() {
  let none = Vec::new();
  assert!(refused(inside(&[start("a", None, &none), end("b", None)])).contains("<a> is closed with an invalid </b>"));
  assert!(refused(run(&[EventRef::StartDocument, end("a", None)])).contains("never opened"));
  let open = [EventRef::StartDocument, start("r", None, &none), EventRef::EndDocument];
  assert!(refused(run(&open)).contains("<r> is not closed"));
}

#[test]
fn an_end_element_that_does_not_match_names_where_its_start_element_was() {
  let none = Vec::new();
  let opened = EventRef::StartElement(StartElementEventRef::new(
    None,
    "a",
    None,
    Attributes::new(&none),
    XmlSpace::Default,
    None,
    None,
    Location { line: 2, column: 3, offset: 10, ..Location::unknown() },
  ));
  let closed = EventRef::EndElement(EndElementEventRef::new(
    None,
    "b",
    None,
    Location { line: 4, column: 1, offset: 30, ..Location::unknown() },
  ));

  let mut strict = StrictXmlValidator::new();
  strict.handle(&EventRef::StartDocument).unwrap();
  strict.handle(&start("r", None, &none)).unwrap();
  strict.handle(&opened).unwrap();
  let error = strict.handle(&closed).expect_err("</b> does not close <a>");
  assert!(error.message().contains("<a> (opened at line 2, column 3) is closed with an invalid </b>"), "{error}");
  assert_eq!((error.location().line, error.location().column), (4, 1), "the error itself is at the end element");

  // A start element with no known position is named without one.
  let unknown = refused(inside(&[start("a", None, &none), end("b", None)]));
  assert!(unknown.ends_with("<a> is closed with an invalid </b>"), "{unknown}");
}

#[test]
fn an_element_left_open_is_reported_where_it_was_opened() {
  let none = Vec::new();
  let at = Location { line: 3, column: 5, offset: 40, ..Location::unknown() }.with_system_id("file:///d.xml");
  let opened = EventRef::StartElement(StartElementEventRef::new(
    None,
    "r",
    None,
    Attributes::new(&none),
    XmlSpace::Default,
    None,
    None,
    at,
  ));

  let mut strict = StrictXmlValidator::new();
  strict.handle(&EventRef::StartDocument).unwrap();
  strict.handle(&opened).unwrap();
  let error = strict.handle(&EventRef::EndDocument).expect_err("<r> was never closed");

  assert!(error.message().contains("<r> is not closed"), "{error}");
  assert_eq!((error.location().line, error.location().column), (3, 5), "where the start element was");
  assert_eq!(error.location().system_id.as_deref(), Some("file:///d.xml"));
}

#[test]
fn a_document_has_exactly_one_root_element() {
  assert!(refused(run(&[EventRef::StartDocument, comment("c"), EventRef::EndDocument])).contains("no root element"));
  let none = Vec::new();
  let two = [EventRef::StartDocument, start("a", None, &none), end("a", None), start("b", None, &none)];
  assert!(refused(run(&two)).contains("only one root element"));
}

#[test]
fn text_and_cdata_sections_outside_the_root_element_are_refused() {
  assert!(refused(run(&[EventRef::StartDocument, text(" x")])).contains("before the root element"));
  let none = Vec::new();
  let after = [EventRef::StartDocument, start("r", None, &none), end("r", None), text("x")];
  assert!(refused(run(&after)).contains("after the root element"));
  assert!(refused(run(&[EventRef::StartDocument, cdata("x")])).contains("inside the root element"));
  assert!(refused(inside(&[cdata("a]]>b")])).contains("]]>"));
}

#[test]
fn a_character_xml_does_not_allow_is_refused_where_it_is() {
  let mut strict = StrictXmlValidator::new();
  strict.handle(&EventRef::StartDocument).unwrap();
  let error =
    strict.handle(&EventRef::Comment(CommentEventRef::new("a\u{1}", Location::new()))).expect_err("U+0001 is refused");
  assert!(error.message().contains("U+0001"), "{error}");
  assert_eq!(error.location().column, 6, "after `<!--a`");
  let value = vec![attribute("x", None, "\u{FFFE}")];
  assert!(refused(inside(&[start("a", None, &value), end("a", None)])).contains("U+FFFE"));
}

#[test]
fn a_document_type_declaration_comes_once_before_the_root_element() {
  let (dtd, pool) = crate::dtd::DtdReader::new("".as_bytes()).read().expect("an empty DTD");
  let doctype = |name, public_id, system_id| {
    EventRef::Doctype(DoctypeEventRef::new(name, public_id, system_id, &dtd, &pool, Location::unknown()))
  };
  let none = Vec::new();
  let root = [start("r", None, &none), end("r", None), EventRef::EndDocument];

  let fine = doctype(Some("r"), Some("-//x//y"), Some("r.dtd"));
  run(&[&[EventRef::StartDocument, fine.clone()][..], &root].concat()).unwrap();

  let twice = [EventRef::StartDocument, fine.clone(), fine.clone()];
  assert!(refused(run(&twice)).contains("only one document type declaration"));
  let late = [EventRef::StartDocument, start("r", None, &none), fine];
  assert!(refused(run(&late)).contains("before the root element"));
  let alone = [EventRef::StartDocument, doctype(Some("r"), Some("-//x//y"), None)];
  assert!(refused(run(&alone)).contains("not followed by a system identifier"));
  let quotes = [EventRef::StartDocument, doctype(Some("r"), None, Some("a'b\"c"))];
  assert!(refused(run(&quotes)).contains("both kinds of quotation mark"));
}

#[test]
fn a_prefix_has_to_be_bound_to_the_namespace_the_event_carries() {
  let none = Vec::new();
  assert!(refused(inside(&[start("p:a", Some("urn:p"), &none), end("p:a", Some("urn:p"))])).contains("not bound"));

  let declared = vec![attribute("xmlns:p", XMLNS_NS_URI.into(), "urn:p")];
  let wrong = [start("p:a", Some("urn:q"), &declared), end("p:a", Some("urn:q"))];
  assert!(refused(inside(&wrong)).contains("its name is in namespace \"urn:p\""));

  // An unprefixed attribute is in no namespace, even with a default namespace in scope.
  let default = vec![attribute("xmlns", XMLNS_NS_URI.into(), "urn:d"), attribute("x", Some("urn:d"), "1")];
  let unprefixed = [start("a", Some("urn:d"), &default), end("a", Some("urn:d"))];
  assert!(refused(inside(&unprefixed)).contains("attribute \"x\""));
}

#[test]
fn a_binding_ends_with_the_element_that_declared_it() {
  let declared = vec![attribute("xmlns:p", XMLNS_NS_URI.into(), "urn:p")];
  let none = Vec::new();
  let events =
    [start("a", None, &declared), end("a", None), start("p:b", Some("urn:p"), &none), end("p:b", Some("urn:p"))];
  assert!(refused(inside(&events)).contains("prefix \"p\" is not bound"));
}

#[test]
fn a_declaration_follows_the_rules_for_reserved_names() {
  let refuse = |lexical, value| {
    let declaration = vec![attribute(lexical, XMLNS_NS_URI.into(), value)];
    refused(inside(&[start("a", None, &declaration), end("a", None)]))
  };
  assert!(refuse("xmlns:p", "").contains("empty namespace name"));
  assert!(refuse("xmlns:xmlns", "urn:x").contains("cannot be declared"));
  assert!(refuse("xmlns:xml", "urn:x").contains("its own namespace name"));
  assert!(refuse("xmlns", XML_NS_URI).contains("default namespace"));
}

#[test]
fn xml_space_is_default_or_preserve() {
  let space = vec![attribute("xml:space", Some(XML_NS_URI), "keep")];
  assert!(refused(inside(&[start("a", None, &space), end("a", None)])).contains("xml:space"));
}

#[test]
fn a_lexical_only_validator_checks_nothing_the_parser_checks() {
  let mut strict = StrictXmlValidator::lexical_only();
  let none = Vec::new();
  // Outside a document, unbalanced, and with an unbound prefix: all the parser's to refuse.
  for event in [comment("c"), end("a", None), start("p:b", None, &none), text("\u{1}")] {
    strict.handle(&event).unwrap();
  }
  assert!(strict.handle(&comment("a--b")).unwrap_err().message().contains("--"));
}

#[test]
fn each_start_document_starts_the_check_afresh() {
  let none = Vec::new();
  let mut strict = StrictXmlValidator::new();
  // A run that stopped inside its root element, then a whole document.
  for event in [EventRef::StartDocument, start("a", None, &none)] {
    strict.handle(&event).unwrap();
  }
  for event in [EventRef::StartDocument, start("b", None, &none), end("b", None), EventRef::EndDocument] {
    strict.handle(&event).unwrap();
  }
}
