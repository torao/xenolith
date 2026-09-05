//! Reading a DTD from a source of its own, apart from any document.

use std::io::Read;

use xenolith_core::Error;
use xenolith_core::resolve::{EntityRequest, UriResolver};
use xenolith_dtd::{ContentSpec, DtdReader, GeneralEntity};

#[test]
fn reads_declarations_from_a_source_of_its_own() {
  let text = "<!ELEMENT note (body)>\
              <!ELEMENT body (#PCDATA)>\
              <!ATTLIST note id ID #IMPLIED>\
              <!ENTITY who \"world\">";
  let (dtd, pool) = DtdReader::new(text.as_bytes()).read().expect("a well-formed DTD");

  let note = pool.get("note").expect("declared");
  assert!(dtd.has_element(note));
  assert!(matches!(dtd.content_spec(note), Some(ContentSpec::Children(_))));
  assert_eq!(dtd.attlist(note).map(<[_]>::len), Some(1));
  let who = pool.get("who").expect("declared");
  assert!(matches!(dtd.general_entity(who), Some(GeneralEntity::Internal { .. })));
}

#[test]
fn a_text_declaration_at_the_head_is_not_mistaken_for_content() {
  // An external entity may open with `<?xml ... ?>`. It is consumed as a text declaration, not reported as a
  // processing instruction and not left to confuse the DTD scanner.
  let text = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!ELEMENT a EMPTY>";
  let (dtd, pool) = DtdReader::new(text.as_bytes()).read().expect("a well-formed DTD");
  assert!(dtd.has_element(pool.get("a").expect("declared")));
}

#[test]
fn a_malformed_dtd_is_an_error() {
  let error = DtdReader::new("<!ELEMENT a (".as_bytes()).read().unwrap_err();
  assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
}

#[test]
fn an_external_parameter_entity_is_fetched_through_the_resolver() {
  struct Catalog;
  impl UriResolver for Catalog {
    fn resolve(&mut self, request: &EntityRequest) -> Result<Option<Box<dyn Read>>, Error> {
      if request.name() == Some("more") {
        Ok(Some(Box::new(std::io::Cursor::new(&b"<!ELEMENT extra EMPTY>"[..]))))
      } else {
        Ok(None)
      }
    }
  }

  let text = "<!ENTITY % more SYSTEM 'urn:more'>%more;<!ELEMENT a EMPTY>";
  let (dtd, pool) = DtdReader::new(text.as_bytes()).with_resolver(Catalog).read().expect("a well-formed DTD");

  assert!(dtd.has_element(pool.get("a").expect("declared here")));
  assert!(dtd.has_element(pool.get("extra").expect("declared in the fetched entity")));
}

#[test]
fn an_external_parameter_entity_without_a_resolver_is_refused() {
  // The same default a document reader takes: an external reference is not fetched unless a resolver allows it.
  let text = "<!ENTITY % more SYSTEM 'urn:more'>%more;";
  let error = DtdReader::new(text.as_bytes()).read().unwrap_err();
  assert!(matches!(error, Error::WellFormedness { .. }), "{error}");
}
