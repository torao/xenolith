//! End-to-end DTD validation, through the public `validate` entry point.

use xylogue_validate::validate;

/// Validates `xml`, returning the validity error messages (empty when valid).
fn errors(xml: &str) -> Vec<String> {
  let report = validate(xml.as_bytes()).expect("well-formed");
  report.errors().iter().map(ToString::to_string).collect()
}

fn is_valid(xml: &str) -> bool {
  validate(xml.as_bytes()).expect("well-formed").is_valid()
}

#[test]
fn a_document_that_follows_its_dtd_is_valid() {
  let xml = "<!DOCTYPE a [\
             <!ELEMENT a (b, c*)>\
             <!ELEMENT b (#PCDATA)>\
             <!ELEMENT c EMPTY>\
             ]><a><b>text</b><c/><c/></a>";
  assert!(is_valid(xml), "{:?}", errors(xml));
}

#[test]
fn the_root_must_be_the_declared_element() {
  let xml = "<!DOCTYPE a [<!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><b/>";
  assert!(errors(xml).iter().any(|e| e.contains("root")), "{:?}", errors(xml));
}

#[test]
fn an_undeclared_element_is_invalid() {
  let xml = "<!DOCTYPE a [<!ELEMENT a ANY>]><a><undeclared/></a>";
  assert!(errors(xml).iter().any(|e| e.contains("undeclared") && e.contains("not declared")));
}

#[test]
fn content_models_are_matched() {
  // (b, c): missing c.
  let bad = "<!DOCTYPE a [<!ELEMENT a (b, c)><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><a><b/></a>";
  assert!(errors(bad).iter().any(|e| e.contains("incomplete")), "{:?}", errors(bad));

  // wrong order.
  let disorder = "<!DOCTYPE a [<!ELEMENT a (b, c)><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><a><c/><b/></a>";
  assert!(!errors(disorder).is_empty());

  // an element not allowed here.
  let intruder = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><a><c/></a>";
  assert!(errors(intruder).iter().any(|e| e.contains("expected")));
}

#[test]
fn empty_and_mixed_content_are_checked() {
  let empty_has_child = "<!DOCTYPE a [<!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><a><b/></a>";
  assert!(errors(empty_has_child).iter().any(|e| e.contains("EMPTY")));

  let element_content_has_text = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY>]><a>text<b/></a>";
  assert!(errors(element_content_has_text).iter().any(|e| e.contains("character data")));

  let mixed_ok = "<!DOCTYPE a [<!ELEMENT a (#PCDATA|b)*><!ELEMENT b EMPTY>]><a>x<b/>y</a>";
  assert!(is_valid(mixed_ok), "{:?}", errors(mixed_ok));

  let mixed_bad = "<!DOCTYPE a [<!ELEMENT a (#PCDATA|b)*><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><a><c/></a>";
  assert!(errors(mixed_bad).iter().any(|e| e.contains("mixed content")));
}

#[test]
fn attributes_are_checked_against_their_declarations() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a EMPTY>\
             <!ATTLIST a id ID #REQUIRED kind (big|small) #IMPLIED fixed CDATA #FIXED 'k'>]>";

  assert!(is_valid(&format!("{dtd}<a id='x1' kind='big' fixed='k'/>")));

  // Missing #REQUIRED.
  assert!(errors(&format!("{dtd}<a kind='big'/>")).iter().any(|e| e.contains("required")));
  // Undeclared attribute.
  assert!(errors(&format!("{dtd}<a id='x' other='1'/>")).iter().any(|e| e.contains("not declared")));
  // Enumeration outside the list.
  assert!(errors(&format!("{dtd}<a id='x' kind='huge'/>")).iter().any(|e| e.contains("not one of")));
  // #FIXED given a different value.
  assert!(errors(&format!("{dtd}<a id='x' fixed='other'/>")).iter().any(|e| e.contains("FIXED")));
}

#[test]
fn ids_are_unique_and_idrefs_resolve() {
  let dtd = "<!DOCTYPE a [<!ELEMENT a (b*)><!ELEMENT b EMPTY>\
             <!ATTLIST b id ID #IMPLIED ref IDREF #IMPLIED>]>";

  assert!(is_valid(&format!("{dtd}<a><b id='x'/><b ref='x'/></a>")));
  // Duplicate ID.
  assert!(errors(&format!("{dtd}<a><b id='x'/><b id='x'/></a>")).iter().any(|e| e.contains("more than once")));
  // Dangling IDREF.
  assert!(errors(&format!("{dtd}<a><b ref='missing'/></a>")).iter().any(|e| e.contains("matches no ID")));
}

#[test]
fn a_nondeterministic_content_model_is_reported() {
  // (a, b) | (a, c): ambiguous on the leading `a`.
  let xml = "<!DOCTYPE r [<!ELEMENT r ((a,b)|(a,c))><!ELEMENT a EMPTY><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]>\
             <r><a/><b/></r>";
  assert!(errors(xml).iter().any(|e| e.contains("not deterministic")), "{:?}", errors(xml));
}

#[test]
fn a_document_without_a_doctype_has_no_dtd_to_validate() {
  let report = validate("<a><b/></a>".as_bytes()).expect("well-formed");
  assert!(!report.had_dtd());
  assert!(!report.is_valid(), "no DTD means not validated");
  assert!(report.errors().is_empty());
}

#[test]
fn well_formedness_errors_are_still_fatal() {
  // The validator layer does not soften a well-formedness error into a report.
  assert!(validate("<!DOCTYPE a [<!ELEMENT a EMPTY>]><a></b>".as_bytes()).is_err());
}
