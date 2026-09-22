use super::*;

#[test]
fn display_prefixes_the_kind_and_location_stays_a_field() {
  let e = Error::well_formedness("mismatched end tag")
    .at(Location { line: 12, column: 5, ..Location::unknown() }.with_system_id("file:///doc.xml"));
  // The location is not in the message; it is read from the field.
  assert_eq!(e.to_string(), "not well-formed: mismatched end tag");
  assert_eq!((e.location().line, e.location().column), (12, 5));
  assert_eq!(e.location().system_id.as_deref(), Some("file:///doc.xml"));
}

#[test]
fn a_location_is_filled_in_only_where_the_error_has_none() {
  let known = Location { line: 7, column: 2, ..Location::unknown() }.with_system_id("file:///caller.xml");

  // An error raised without a position takes the caller's.
  let filled = Error::resolver(std::io::Error::other("the catalog is locked")).or_at(known.clone());
  assert_eq!(filled.location(), &known);

  // One that already says where it happened keeps saying it, even where only the entity is named.
  let named = Error::resolver(std::io::Error::other("x")).at(Location::unknown().with_system_id("file:///cat.ent"));
  let kept = named.or_at(known);
  assert_eq!(kept.location().system_id.as_deref(), Some("file:///cat.ent"));
  assert_eq!((kept.location().line, kept.location().column), (0, 0));
}

#[test]
fn a_kind_without_a_location_reports_an_unknown_one() {
  let e = Error::internal("unreachable");
  assert_eq!(e.to_string(), "internal error: unreachable; this is a bug in xenolith, please report it");
  assert!(e.location().is_unknown());
}

#[test]
fn a_location_that_is_not_known_cannot_be_advanced() {
  // A line feed is what would otherwise turn line 0 into line 1, and a location no reader established into one it
  // claims to have read.
  let mut at = Location::unknown();
  at.advance('a');
  at.advance('\n');
  at.advance('b');
  assert!(at.is_unknown(), "{at:?}");
  assert_eq!(at.to_string(), "<unknown>");
  assert_eq!((at.line, at.column, at.offset), (0, 0, 0));

  // A location that does have a position advances as it always did, from where a reader starts.
  let mut at = Location::new();
  at.advance('a');
  at.advance('\n');
  at.advance('b');
  assert_eq!((at.line, at.column, at.offset), (2, 2, 3));
}
