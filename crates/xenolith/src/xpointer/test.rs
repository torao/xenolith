use super::*;
use crate::event::{EventCursor, EventProducer};
use crate::io::StreamSource;
use crate::io::write::XmlWriter;

/// Reads `xml` through a filter for `pointer` and writes what comes out the other side.
fn select(pointer: &str, xml: &str) -> Result<String> {
  let pointer = XPointer::parse(pointer, Location::new())?;
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut writer);
    StreamSource::new(xml.as_bytes()).add_consumer(&mut filter).emit()?;
  }
  Ok(String::from_utf8(writer.into_inner()).expect("UTF-8"))
}

const DOC: &str = "<doc><a>one</a><!--c--><b><x/><y>two</y></b>tail</doc>";

#[test]
fn a_child_sequence_counts_element_children_from_the_document() {
  assert_eq!(select("element(/1)", DOC).unwrap(), DOC);
  assert_eq!(select("element(/1/1)", DOC).unwrap(), "<a>one</a>");
  // The comment and the text between the elements are not counted.
  assert_eq!(select("element(/1/2/2)", DOC).unwrap(), "<y>two</y>");
}

#[test]
fn what_is_outside_the_selected_element_is_dropped_and_what_is_inside_is_kept() {
  let xml = "<?pi before?><doc><b>in<!--kept--><?kept?><![CDATA[c]]></b>out<!--dropped--></doc>";
  assert_eq!(select("element(/1/1)", xml).unwrap(), "<b>in<!--kept--><?kept?><![CDATA[c]]></b>");
}

#[test]
fn a_shorthand_pointer_identifies_the_element_with_an_xml_id() {
  let xml = "<doc><p xml:id='a'>one</p><p xml:id='b'>two</p></doc>";
  assert!(select("b", xml).unwrap().ends_with(">two</p>"));
}

#[test]
fn an_id_may_be_declared_in_the_document_type() {
  let xml = "<!DOCTYPE doc [<!ATTLIST p key ID #IMPLIED>]><doc><p key='a'>one</p><p key='b'>two</p></doc>";
  assert_eq!(select("b", xml).unwrap(), "<p key=\"b\">two</p>");
  assert_eq!(select("element(a)", xml).unwrap(), "<p key=\"a\">one</p>");
}

#[test]
fn an_id_may_start_a_child_sequence() {
  let xml = "<doc><s xml:id='s'><t>one</t><t>two</t></s><t>outside</t></doc>";
  assert_eq!(select("element(s/2)", xml).unwrap(), "<t>two</t>");
  // Nothing after the element with the ID is counted from it.
  let error = select("element(s/3)", xml).expect_err("s has two children");
  assert!(matches!(error, Error::XPointer { .. }), "{error:?}");
}

#[test]
fn a_pointer_that_identifies_nothing_is_an_error_at_the_end_of_the_document() {
  for pointer in ["missing", "element(/1/9)", "element(/2)", "element(missing/1)"] {
    let error = select(pointer, DOC).expect_err(pointer);
    assert!(matches!(error, Error::XPointer { .. }), "{pointer}: {error:?}");
    assert!(error.message().contains("identifies no element"), "{pointer}: {error}");
  }
}

#[test]
fn parts_that_identify_nothing_are_skipped() {
  // An unsupported scheme, xmlns() and element() data that is not of that scheme's syntax all identify nothing,
  // so the one element() part that can be evaluated decides.
  for pointer in ["xpointer(/doc) element(/1/1)", "xmlns(p=urn:p)element(/1/1)", "element(/0) element(/1/1)"] {
    assert_eq!(select(pointer, DOC).unwrap(), "<a>one</a>", "{pointer}");
  }
}

#[test]
fn a_pointer_with_no_part_it_can_evaluate_is_refused_at_the_start() {
  let error = select("xpointer(/doc)", DOC).expect_err("no part this implementation evaluates");
  assert!(error.message().contains("no part"), "{error}");
}

#[test]
fn of_several_parts_the_first_that_selects_anything_decides() {
  // Framework §3.3: the parts are tried in order, and the first that identifies something is the result.
  for (pointer, expected) in [
    // The first part selects, so the second is not used.
    ("element(/1/1) element(/1/2)", "<a>one</a>"),
    // The first part selects nothing, so what the second selected, pending meanwhile, comes before the end.
    ("element(/1/9) element(/1/2)", "<b><x/><y>two</y></b>"),
    // What the second part selected came first in the document; the first part's choice still decides.
    ("element(/1/2) element(/1/1)", "<b><x/><y>two</y></b>"),
    // A later part selected first and was dropped when an earlier one selected.
    ("element(/1/9) element(/1/2) element(/1/1)", "<b><x/><y>two</y></b>"),
    // Only the last part selects anything.
    ("element(/1/9) element(/1/8) element(/1/1)", "<a>one</a>"),
  ] {
    assert_eq!(select(pointer, DOC).unwrap(), expected, "{pointer}");
  }
  let error = select("element(/1/9) element(/1/8)", DOC).expect_err("no part selects anything");
  assert!(error.message().contains("identifies no element"), "{error}");
}

#[test]
fn a_pending_selection_is_framed_like_one_passed_on_as_it_came() {
  let pointer = XPointer::parse("element(/1/9) element(/1/2/2)", Location::new()).unwrap();
  let mut kinds = Kinds::default();
  {
    let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut kinds);
    StreamSource::new(DOC.as_bytes()).add_consumer(&mut filter).emit().unwrap();
  }
  assert_eq!(kinds.0, ["start-document", "start", "other", "end", "end-document"]);
}

#[test]
fn a_string_that_is_not_a_pointer_is_refused_by_parse() {
  for pointer in ["", "1a", "element(/1", "element(/1))", "foo(^a)", "1x(a)", "a b", "element(/1) junk"] {
    let error = XPointer::parse(pointer, Location::new()).expect_err(pointer);
    assert!(matches!(error, Error::XPointer { .. }), "{pointer:?}: {error:?}");
  }
}

#[test]
fn escaped_and_balanced_parentheses_belong_to_the_scheme_data() {
  // Neither is an element() part, so both are skipped; what is checked is that they parse.
  assert!(XPointer::parse("foo(a^(b^)c^^) element(/1)", Location::new()).is_ok());
  assert!(XPointer::parse("foo(f(x)) element(/1)", Location::new()).is_ok());
}

#[test]
fn a_pointer_parses_through_from_str_as_well() {
  let pointer: XPointer = "element(/1/2)".parse().unwrap();
  assert_eq!(pointer, XPointer::parse("element(/1/2)", Location::new()).unwrap());

  // A string that stands alone starts at line 1, column 1, so an error says which character of it is at fault.
  let error = "element(/1) foo(^a)".parse::<XPointer>().expect_err("a circumflex that escapes nothing");
  assert_eq!((error.location().line, error.location().column, error.location().offset), (1, 17, 16));
}

#[test]
fn one_parsed_pointer_serves_several_filters_and_runs() {
  let pointer = XPointer::parse("element(/1/1)", Location::new()).unwrap();
  let mut first = XmlWriter::new(Vec::new());
  let mut second = XmlWriter::new(Vec::new());
  {
    let mut a = XPointerFilter::new(&pointer).add_consumer(&mut first);
    let mut b = XPointerFilter::new(&pointer).add_consumer(&mut second);
    StreamSource::new("<r><one/></r>".as_bytes()).add_consumer(&mut a).emit().unwrap();
    StreamSource::new("<r><two/></r>".as_bytes()).add_consumer(&mut b).emit().unwrap();
    // The same filter again: the state of the first run is gone.
    StreamSource::new("<r><three/></r>".as_bytes()).add_consumer(&mut a).emit().unwrap();
  }
  assert_eq!(String::from_utf8(first.into_inner()).unwrap(), "<one/><three/>");
  assert_eq!(String::from_utf8(second.into_inner()).unwrap(), "<two/>");
}

/// Records the kinds of the events that reach it.
#[derive(Default)]
struct Kinds(Vec<&'static str>);

impl EventConsumer for Kinds {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    self.0.push(match event {
      EventRef::StartDocument => "start-document",
      EventRef::EndDocument => "end-document",
      EventRef::Doctype(_) => "doctype",
      EventRef::StartElement(_) => "start",
      EventRef::EndElement(_) => "end",
      _ => "other",
    });
    Ok(Flow::Continue(0))
  }
}

#[test]
fn the_doctype_is_passed_on_only_when_asked() {
  let xml = "<!DOCTYPE doc><doc><a/></doc>";
  let pointer = XPointer::parse("element(/1/1)", Location::new()).unwrap();
  for (on, expected) in [
    (false, &["start-document", "start", "end", "end-document"][..]),
    (true, &["start-document", "doctype", "start", "end", "end-document"][..]),
  ] {
    let mut kinds = Kinds::default();
    {
      let mut filter = XPointerFilter::new(&pointer).with_doctype(on).add_consumer(&mut kinds);
      StreamSource::new(xml.as_bytes()).add_consumer(&mut filter).emit().unwrap();
    }
    assert_eq!(kinds.0, expected, "with_doctype({on})");
  }
}

#[test]
fn a_consumer_that_stops_stops_the_filter_too() {
  /// Stops at the first start element.
  struct First;
  impl EventConsumer for First {
    fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
      Ok(if matches!(event, EventRef::StartElement(_)) { Flow::Break(0) } else { Flow::Continue(0) })
    }
  }
  // The rest of the document is not read, so the mismatched end tag after the selected element is not found.
  let pointer = XPointer::parse("element(/1/1)", Location::new()).unwrap();
  let mut first = First;
  let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut first);
  StreamSource::new("<r><a/><b></c></r>".as_bytes()).add_consumer(&mut filter).emit().expect("stopped early");
}

/// Where a pointer written at line 3, column 10 of `file:///d.xml` begins.
fn written_at() -> Location {
  Location { line: 3, column: 10, offset: 40, ..Location::unknown() }.with_system_id("file:///d.xml")
}

#[test]
fn a_syntax_error_is_located_at_the_character_that_makes_it() {
  // (pointer, the character the error is at)
  for (pointer, at) in [
    ("element(/1) foo(^a)", 16), // the circumflex that escapes nothing
    ("element(/1) 1x(a)", 12),   // the start of the part whose scheme name is not one
    ("element(/1) junk", 12),    // the start of the part with no parentheses
    ("element(/1) foo(a", 15),   // the parenthesis that is not closed
  ] {
    let error = XPointer::parse(pointer, written_at()).expect_err(pointer);
    let location = error.location();
    assert_eq!((location.line, location.column), (3, 10 + at), "{pointer}: {error}");
    assert_eq!(location.offset, 40 + at as u64, "{pointer}");
    assert_eq!(location.system_id.as_deref(), Some("file:///d.xml"), "{pointer}");
  }
}

#[test]
fn the_errors_of_a_filter_are_located_at_the_pointer() {
  let filtered = |pointer: &XPointer| {
    let mut writer = XmlWriter::new(Vec::new());
    let mut filter = XPointerFilter::new(pointer).add_consumer(&mut writer);
    StreamSource::new(DOC.as_bytes()).add_consumer(&mut filter).emit().expect_err(pointer.as_str())
  };

  // Identifying nothing is the pointer's failure as a whole.
  let error = filtered(&XPointer::parse("missing", written_at()).unwrap());
  assert_eq!((error.location().line, error.location().column), (3, 10));
}

#[test]
fn a_shorthand_pointer_selects_as_element_with_that_name_does() {
  let xml = "<doc><p xml:id='a'>one</p><p xml:id='b'>two</p></doc>";
  assert_eq!(select("b", xml).unwrap(), select("element(b)", xml).unwrap());
}

#[test]
fn the_pointer_as_written_is_kept() {
  let written = "xmlns(p=urn:p) element(/1/2)";
  let pointer = XPointer::parse(written, Location::new()).unwrap();
  assert_eq!(pointer.as_str(), written);
  assert_eq!(pointer.to_string(), written);
  assert!(format!("{pointer:?}").contains(&format!("{written:?}")), "{pointer:?}");
}

/// A scheme that needs the whole document to decide, as XSLT does: the last child element of the document element.
/// Its selector builds a tree from every event and returns the selection as a cursor over that tree.
#[derive(Debug)]
struct LastChild;

impl SchemeData for LastChild {
  fn selector(&self) -> Box<dyn SchemeSelector + '_> {
    Box::new(LastChildSelector { builder: crate::dom::build::DomBuilder::new(), doc: None })
  }
}

struct LastChildSelector {
  builder: crate::dom::build::DomBuilder,
  /// The tree, once the document has ended.
  doc: Option<crate::dom::Document>,
}

impl SchemeSelector for LastChildSelector {
  fn filter(&mut self, event: &EventRef<'_>) -> bool {
    let _ = self.builder.consume(event);
    if matches!(event, EventRef::EndDocument) {
      self.doc = Some(std::mem::take(&mut self.builder).into_document());
    }
    false
  }

  fn drain(&mut self) -> Option<Box<dyn EventCursor<'_> + '_>> {
    use crate::dom::{DomSource, NodeType};
    let doc = self.doc.as_ref()?;
    let root = doc.document_element()?;
    let last = doc.children(root).filter(|&node| doc.node_type(node) == NodeType::ELEMENT_NODE).last()?;
    Some(Box::new(DomSource::at(doc, last)))
  }
}

/// A pointer whose one part is `LastChild`.
fn last_child() -> XPointer {
  let parts: Vec<ParsedPart> = vec![Ok(Arc::new(LastChild))];
  XPointer { source: "last-child()".to_owned(), location: Location::new(), parts }
}

#[test]
fn a_selector_that_builds_the_document_sends_its_selection_before_the_end() {
  let pointer = last_child();
  let mut kinds = Kinds::default();
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut both = Dispatcher::new().add_consumer(&mut kinds).add_consumer(&mut writer);
    let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut both);
    StreamSource::new("<doc><a/>text<b>x</b><c><d/></c>tail</doc>".as_bytes())
      .add_consumer(&mut filter)
      .emit()
      .unwrap();
  }
  assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<c><d/></c>");
  // Nothing passed while the document was read; the selection came between the start and the end.
  assert_eq!(kinds.0, ["start-document", "start", "start", "end", "end", "end-document"]);

  // A selector that identified nothing makes the filter report it, as a streaming one does.
  let mut writer = XmlWriter::new(Vec::new());
  let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut writer);
  let error = StreamSource::new("<doc>no element</doc>".as_bytes()).add_consumer(&mut filter).emit().unwrap_err();
  assert!(matches!(error, Error::XPointer { .. }), "{error:?}");
}

#[test]
fn a_pointer_whose_supported_parts_all_have_malformed_data_reports_the_first() {
  // Unsupported schemes are left out. Of the element() parts, none of which follows the scheme's syntax, the first
  // says why, at the character that breaks it.
  let pointer = "foo(x) element(/0) element(a b)";
  let error = select(pointer, DOC).expect_err(pointer);
  assert!(matches!(error, Error::XPointer { .. }), "{error:?}");
  assert!(error.message().contains("\"0\" in the child sequence"), "{error}");
  assert_eq!((error.location().line, error.location().column), (1, 17), "at the 0 of element(/0)");
}

#[test]
fn a_pointer_with_no_part_of_a_supported_scheme_says_so() {
  let error = select("foo(x) xmlns(p=urn:p)", DOC).expect_err("nothing to evaluate");
  assert!(error.message().contains("no part of a scheme this implementation supports"), "{error}");
  assert_eq!((error.location().line, error.location().column), (1, 1), "at the pointer");
}

#[test]
fn a_part_with_malformed_data_gives_way_to_one_that_parses() {
  assert_eq!(select("element(/0) element(/1/1)", DOC).unwrap(), "<a>one</a>");
  assert_eq!(select("element(/1/1) element(a b)", DOC).unwrap(), "<a>one</a>");
}

#[test]
fn the_errors_of_the_parts_can_be_read_from_the_pointer() {
  // A part of an unsupported scheme is skipped rather than an error; the malformed element() parts are, in order.
  let pointer = XPointer::parse("foo(x) element(/0) element(/1/1) element(a b)", Location::new()).unwrap();
  let errors: Vec<(String, u32)> =
    pointer.errors().map(|error| (error.message().to_owned(), error.location().column)).collect();
  assert_eq!(errors.len(), 2, "{errors:?}");
  assert!(errors[0].0.contains("\"0\" in the child sequence"), "{errors:?}");
  assert_eq!(errors[0].1, 17);
  assert!(errors[1].0.contains("\"a b\" in element() is not an NCName"), "{errors:?}");
  assert_eq!(errors[1].1, 42);

  // A pointer whose parts all parsed has none.
  assert_eq!(XPointer::parse("foo(x) element(/1/1)", Location::new()).unwrap().errors().count(), 0);
  assert_eq!(XPointer::parse("q", Location::new()).unwrap().errors().count(), 0);
}

/// A pointer of the given parts, whatever their schemes.
fn pointer_of(parts: Vec<Arc<dyn SchemeData>>) -> XPointer {
  XPointer { source: "parts".to_owned(), location: Location::new(), parts: parts.into_iter().map(Ok).collect() }
}

#[test]
fn a_part_that_holds_its_selection_itself_takes_its_turn_in_order() {
  let element = |data: &str| -> Arc<dyn SchemeData> { Arc::new(element::ElementScheme::parse(data).unwrap()) };
  // `LastChild` selects <b>, the last child of the document element, and holds it to the end.
  for (parts, expected) in [
    // It comes after a part that selects nothing.
    (vec![element("/1/9"), Arc::new(LastChild) as Arc<dyn SchemeData>], "<b><x/><y>two</y></b>"),
    // It comes first, so what the streaming part after it selected is not used.
    (vec![Arc::new(LastChild) as Arc<dyn SchemeData>, element("/1/1")], "<b><x/><y>two</y></b>"),
  ] {
    let pointer = pointer_of(parts);
    let mut writer = XmlWriter::new(Vec::new());
    {
      let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut writer);
      StreamSource::new(DOC.as_bytes()).add_consumer(&mut filter).emit().unwrap();
    }
    assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), expected);
  }
}

/// Reads `DOC` through a filter for `pointer` with `max_pending_chars`, and writes what comes out.
fn select_pending(pointer: &str, max_pending_chars: Option<usize>) -> Result<String> {
  let pointer = XPointer::parse(pointer, Location::new())?;
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut filter = XPointerFilter::new(&pointer).with_max_pending_chars(max_pending_chars).add_consumer(&mut writer);
    StreamSource::new(DOC.as_bytes()).add_consumer(&mut filter).emit()?;
  }
  Ok(String::from_utf8(writer.into_inner()).expect("UTF-8"))
}

#[test]
fn what_a_later_part_keeps_pending_is_limited_in_characters() {
  // The second part keeps <b><x/><y>two</y></b> pending: b, x, x, y, "two", y and b come to 9 characters.
  let pointer = "element(/1/9) element(/1/2)";
  assert_eq!(select_pending(pointer, Some(9)).unwrap(), "<b><x/><y>two</y></b>");
  let error = select_pending(pointer, Some(8)).expect_err("one character over");
  assert!(matches!(error, Error::Limit { .. }), "{error:?}");
  assert!(error.message().contains("with_max_pending_chars"), "{error}");
  assert_eq!(select_pending(pointer, None).unwrap(), "<b><x/><y>two</y></b>");
}

#[test]
fn what_the_first_part_selects_is_not_pending_and_not_limited() {
  assert_eq!(select_pending("element(/1/2)", Some(0)).unwrap(), "<b><x/><y>two</y></b>");
}

#[test]
fn a_prefix_and_attributes_count_toward_the_limit() {
  let xml = "<doc xmlns:p='urn:p'><p:e a='12'/></doc>";
  let pointer = XPointer::parse("element(/1/9) element(/1/1)", Location::new()).unwrap();
  // The start tag carries "p:e" (3), "a" (1) and "12" (2); the end tag carries "p:e" (3): 9 in all.
  for (max, ok) in [(Some(9), true), (Some(8), false)] {
    let mut writer = XmlWriter::new(Vec::new());
    let mut filter = XPointerFilter::new(&pointer).with_max_pending_chars(max).add_consumer(&mut writer);
    let result = StreamSource::new(xml.as_bytes()).add_consumer(&mut filter).emit();
    assert_eq!(result.is_ok(), ok, "{max:?}: {result:?}");
  }
}
