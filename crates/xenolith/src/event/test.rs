use super::*;
use crate::attr::{AttributeList, AttributeRef};
use crate::error::Error;

/// An attribute list with nothing in it, so a test event can carry no attributes.
struct NoAttributes;

impl AttributeList for NoAttributes {
  fn len(&self) -> usize {
    0
  }

  fn get(&self, _index: usize) -> Option<AttributeRef<'_>> {
    None
  }
}

/// What a [`TinySource`] reports, in order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
  Start,
  OpenA,
  Text,
  OpenB,
  CloseB,
  CloseA,
  End,
  Done,
}

/// A tiny cursor over `<a>hi<b/></a>`, so the vocabulary can be tested without a parser.
struct TinySource<'h> {
  empty: NoAttributes,
  step: Step,
  dispatch: Dispatcher<'h>,
  /// Whether the run ended because every consumer finished early.
  stopped: bool,
  /// Whether the consumers have been told how the run ended.
  told: bool,
}

impl<'h> TinySource<'h> {
  fn new() -> Self {
    Self { empty: NoAttributes, step: Step::Start, dispatch: Dispatcher::new(), stopped: false, told: false }
  }
}

impl<'h> EventProducer<'h> for TinySource<'h> {
  fn add_consumer(mut self, handler: &'h mut dyn EventConsumer) -> Self {
    self.dispatch = self.dispatch.add_consumer(handler);
    self
  }
}

impl<'h> EventCursor<'h> for TinySource<'h> {
  fn next(&mut self) -> Result<Option<EventRef<'_>>> {
    let (step, next) = match self.step {
      Step::Start => (Step::Start, Step::OpenA),
      Step::OpenA => (Step::OpenA, Step::Text),
      Step::Text => (Step::Text, Step::OpenB),
      Step::OpenB => (Step::OpenB, Step::CloseB),
      Step::CloseB => (Step::CloseB, Step::CloseA),
      Step::CloseA => (Step::CloseA, Step::End),
      Step::End => (Step::End, Step::Done),
      Step::Done => {
        if !self.told {
          self.told = true;
          self.dispatch.finish(if self.stopped { Outcome::Stopped } else { Outcome::Completed });
        }
        return Ok(None);
      }
    };
    self.step = next;
    let element = |local| {
      EventRef::StartElement(StartElementEventRef::new(
        None,
        local,
        None,
        Attributes::new(&self.empty),
        XmlSpace::Default,
        None,
        None,
        Location::unknown(),
      ))
    };
    let event = match step {
      Step::Start => EventRef::StartDocument,
      Step::OpenA => element("a"),
      Step::Text => EventRef::Characters(CharactersEventRef::new("hi", Location::unknown())),
      Step::OpenB => element("b"),
      Step::CloseB => EventRef::EndElement(EndElementEventRef::new(None, "b", None, Location::unknown())),
      Step::CloseA => EventRef::EndElement(EndElementEventRef::new(None, "a", None, Location::unknown())),
      Step::End => EventRef::EndDocument,
      Step::Done => return Ok(None),
    };
    match self.dispatch.consume(&event) {
      // A consumer that finishes at `EndDocument` has not cut the run short.
      Ok(Flow::Break(_)) if step != Step::End => {
        self.step = Step::Done;
        self.stopped = true;
      }
      Ok(_) => {}
      Err(error) => {
        self.step = Step::Done;
        self.told = true;
        return Err(self.dispatch.fail(error));
      }
    }
    Ok(Some(event))
  }
}

#[derive(Default)]
struct Counts {
  elements: usize,
  text: usize,
}

impl EventConsumer for Counts {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    match event {
      EventRef::StartElement(_) => self.elements += 1,
      EventRef::Characters(event) => self.text += event.text.len(),
      _ => {}
    }
    Ok(Flow::Continue(0))
  }
}

/// Refuses the first start element it is given.
struct Refuse;

impl EventConsumer for Refuse {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    match event {
      EventRef::StartElement(_) => Err(Error::internal("refused")),
      _ => Ok(Flow::Continue(0)),
    }
  }
}

#[test]
fn emit_drives_a_consumer() {
  let mut counts = Counts::default();
  TinySource::new().add_consumer(&mut counts).emit().unwrap();
  assert_eq!((counts.elements, counts.text), (2, 2));
}

#[test]
fn a_source_feeds_every_consumer_installed_on_it() {
  let mut first = Counts::default();
  let mut second = Counts::default();
  TinySource::new().add_consumer(&mut first).add_consumer(&mut second).emit().unwrap();
  assert_eq!(first.elements, 2);
  assert_eq!(second.elements, 2);
}

#[test]
fn a_refusal_stops_the_dispatcher_where_it_happened() {
  // Fail-fast: the consumer after the one that refused never sees the event it rejected.
  let mut refuse = Refuse;
  let mut after = Counts::default();
  let error = TinySource::new().add_consumer(&mut refuse).add_consumer(&mut after).emit().unwrap_err();
  assert!(error.to_string().contains("refused"), "{error}");
  assert_eq!(after.elements, 0, "the handler behind the refusal saw no start element");
}

#[test]
fn a_consumer_that_has_read_enough_ends_the_run_without_an_error() {
  /// Stops as soon as it has seen one element.
  #[derive(Default)]
  struct First {
    names: Vec<String>,
  }
  impl EventConsumer for First {
    fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
      if let EventRef::StartElement(event) = event {
        self.names.push(event.local.to_owned());
      }
      Ok(if self.names.is_empty() { Flow::Continue(0) } else { Flow::Break(0) })
    }
  }

  let mut first = First::default();
  TinySource::new().add_consumer(&mut first).emit().expect("stopping early is not an error");
  assert_eq!(first.names.len(), 1, "only the first start element is seen");
}

#[test]
fn pulling_gives_the_same_events_the_consumers_were_given() {
  // The two shapes are one run: `next` notifies the installed consumers before it returns the event.
  let mut counts = Counts::default();
  let mut pulled = 0;
  {
    let mut source = TinySource::new().add_consumer(&mut counts);
    while source.next().unwrap().is_some() {
      pulled += 1;
    }
  }
  assert_eq!(pulled, 7, "start, three opens and closes between them, end");
  assert_eq!(counts.elements, 2, "the handler saw the same run");
}

#[test]
fn iterating_gives_the_pulled_events_as_owned_values() {
  let mut counts = Counts::default();
  let mut source = TinySource::new().add_consumer(&mut counts);
  let events: Vec<Event> = source.events().collect::<Result<_>>().unwrap();
  let events: Vec<String> = events.iter().map(|event| render(&event.as_event_ref())).collect();

  let mut pulled = Vec::new();
  let mut again = TinySource::new();
  while let Some(event) = again.next().unwrap() {
    pulled.push(render(&event));
  }
  assert_eq!(events, pulled);
  drop(source);
  assert_eq!(counts.elements, 2, "the installed handler saw the events as they were iterated");
}

#[test]
fn iteration_ends_after_the_first_error() {
  let mut refuse = Refuse;
  let mut source = TinySource::new().add_consumer(&mut refuse);
  let mut events = source.events();
  assert!(matches!(events.next(), Some(Ok(Event::StartDocument))));
  assert!(matches!(events.next(), Some(Err(_))), "the refused start element is reported as the error");
  assert!(events.next().is_none(), "nothing is pulled after the error");
  assert!(events.next().is_none());
}

#[test]
fn a_caller_that_did_not_build_the_source_drives_it_with_a_consumer_of_its_own() {
  /// Stands for a wrapper that is handed a producer and assembles its own consumer out of locals. It cannot install
  /// that consumer, since the producer's `'h` was fixed by whoever built it, so it drives the cursor itself.
  fn run<'h>(source: &mut impl EventCursor<'h>) -> Result<usize> {
    let mut mine = Counts::default();
    while let Some(event) = source.next()? {
      if mine.consume(&event)?.is_break() {
        break;
      }
    }
    Ok(mine.elements)
  }

  let mut installed = Counts::default();
  let mut source = TinySource::new().add_consumer(&mut installed);
  assert_eq!(run(&mut source).unwrap(), 2);
  drop(source);
  assert_eq!(installed.elements, 2, "the installed handler saw the same run");
}

/// Records the events it is given by kind, and finishes after `limit` start elements when it has one.
#[derive(Default)]
struct Recorder {
  seen: Vec<&'static str>,
  starts: usize,
  limit: Option<usize>,
  resets: bool,
}

impl Recorder {
  fn stopping_after(limit: usize) -> Self {
    Self { limit: Some(limit), ..Self::default() }
  }

  /// Makes this recorder count afresh at each `StartDocument`, as a consumer meant to be used again does.
  fn resetting(mut self) -> Self {
    self.resets = true;
    self
  }
}

impl EventConsumer for Recorder {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    if self.resets && matches!(event, EventRef::StartDocument) {
      self.starts = 0;
    }
    if matches!(event, EventRef::StartElement(_)) {
      self.starts += 1;
    }
    self.seen.push(match event {
      EventRef::StartDocument => "start-document",
      EventRef::StartElement(_) => "start",
      EventRef::Characters(_) => "text",
      EventRef::EndElement(_) => "end",
      EventRef::EndDocument => "end-document",
      _ => "other",
    });
    Ok(if self.limit.is_none_or(|limit| self.starts < limit) { Flow::Continue(0) } else { Flow::Break(0) })
  }
}

/// Every event `TinySource` reports, in order.
const WHOLE_RUN: [&str; 7] = ["start-document", "start", "text", "start", "end", "end", "end-document"];

#[test]
fn a_consumer_that_finishes_leaves_the_others_running_to_the_end() {
  let mut early = Recorder::stopping_after(1);
  let mut full = Recorder::default();
  TinySource::new().add_consumer(&mut early).add_consumer(&mut full).emit().unwrap();

  assert_eq!(early.seen, ["start-document", "start"], "nothing after the element it finished at");
  assert_eq!(full.seen, WHOLE_RUN, "the handler beside it read to the end, its end included");
}

#[test]
fn the_run_ends_early_once_every_consumer_has_finished() {
  let mut one = Recorder::stopping_after(1);
  let mut two = Recorder::stopping_after(2);
  TinySource::new().add_consumer(&mut one).add_consumer(&mut two).emit().expect("finishing early is not an error");

  assert_eq!(one.seen, ["start-document", "start"]);
  assert_eq!(two.seen, ["start-document", "start", "text", "start"], "the source stopped at the last one to finish");
}

#[test]
fn a_consumer_that_changes_its_mind_stays_out_for_the_rest_of_the_run() {
  /// Answers `Break` at its second event and `Continue` at any other, so a dispatcher that handed it another event
  /// would take it back.
  #[derive(Default)]
  struct Flicker {
    events: usize,
  }
  impl EventConsumer for Flicker {
    fn consume(&mut self, _event: &EventRef<'_>) -> Result<Flow> {
      self.events += 1;
      Ok(if self.events == 2 { Flow::Break(0) } else { Flow::Continue(0) })
    }
  }

  let mut flicker = Flicker::default();
  let mut full = Recorder::default();
  TinySource::new().add_consumer(&mut flicker).add_consumer(&mut full).emit().unwrap();

  // Coming back mid-document would hand it an end element whose start it never saw.
  assert_eq!(flicker.events, 2, "it said no once and was given nothing more");
  assert_eq!(full.seen, WHOLE_RUN);
}

#[test]
fn a_dispatcher_with_no_consumers_never_ends_the_run() {
  assert!(!Dispatcher::new().is_stopped());

  // So a source with nothing installed still reads to the end, and finds what is wrong with the document.
  let error = crate::io::StreamSource::new("<a><b></a>".as_bytes()).emit().unwrap_err();
  assert!(matches!(error, Error::WellFormedness { .. }), "{error:?}");
}

#[test]
fn a_nested_dispatcher_finishes_once_all_of_its_own_consumers_have() {
  let mut early = Recorder::stopping_after(1);
  let mut late = Recorder::stopping_after(2);
  let mut beside = Recorder::default();
  {
    let mut inner = Dispatcher::new().add_consumer(&mut early).add_consumer(&mut late);
    TinySource::new().add_consumer(&mut inner).add_consumer(&mut beside).emit().unwrap();
  }

  assert_eq!(early.seen, ["start-document", "start"]);
  assert_eq!(late.seen, ["start-document", "start", "text", "start"], "the inner dispatch fed the one still running");
  assert_eq!(beside.seen, WHOLE_RUN, "and the handler beside it read to the end");
}

#[test]
fn a_validator_does_not_keep_the_run_going_once_every_consumer_has_finished() {
  use crate::event::validate::{Ended, ValidatorSet};

  let mut validation = ValidatorSet::new();
  let mut early = Recorder::stopping_after(1);
  {
    let mut lane = Dispatcher::new().add_validator(&mut validation).add_consumer(&mut early);
    TinySource::new().add_consumer(&mut lane).emit().expect("finishing early is not an error");
  }

  assert_eq!(early.seen, ["start-document", "start"]);
  assert_eq!(validation.report().ended(), Some(Ended::Stopped));
}

/// A validator that reports an error for each element named `b`.
#[derive(Default)]
struct FaultsB {
  errors: Vec<validate::ValidityError>,
}

impl EventConsumer for FaultsB {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    match event {
      EventRef::StartDocument => self.errors.clear(),
      EventRef::StartElement(start) if start.local == "b" => {
        self.errors.push(validate::ValidityError::new("b is not allowed", start.location.clone()));
        return Ok(Flow::Continue(1));
      }
      _ => {}
    }
    Ok(Flow::Continue(0))
  }
}

impl Validator for FaultsB {
  fn errors(&self) -> std::borrow::Cow<'_, [validate::ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_consumer(&mut self) -> &mut dyn EventConsumer {
    self
  }
}

#[test]
fn the_maximum_of_a_dispatcher_counts_the_errors_found_in_a_dispatcher_nested_in_it() {
  let mut faults = FaultsB::default();
  let mut behind = Recorder::default();
  let error = {
    let mut inner = Dispatcher::new().add_validator(&mut faults);
    let mut outer = Dispatcher::new().with_max_errors(Some(0)).add_consumer(&mut inner).add_consumer(&mut behind);
    TinySource::new().add_consumer(&mut outer).emit().expect_err("the first error exceeds the maximum")
  };

  assert!(matches!(error, Error::Validity { .. }), "{error:?}");
  assert_eq!(faults.errors.len(), 1, "the error itself stays in the validator");
  assert_eq!(behind.seen, ["start-document", "start", "text"], "the handler behind never saw the element b");
}

#[test]
fn a_dispatcher_with_no_maximum_reads_through_the_errors() {
  let mut faults = FaultsB::default();
  {
    let mut lane = Dispatcher::new().with_max_errors(None).add_validator(&mut faults);
    TinySource::new().add_consumer(&mut lane).emit().expect("validity errors do not end the run");
  }
  assert_eq!(faults.errors.len(), 1);
}

/// A validator that reports the largest count there is at every start element, and keeps no error.
struct Overstates;

impl EventConsumer for Overstates {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    Ok(Flow::Continue(if matches!(event, EventRef::StartElement(_)) { usize::MAX } else { 0 }))
  }
}

impl Validator for Overstates {
  fn errors(&self) -> std::borrow::Cow<'_, [validate::ValidityError]> {
    std::borrow::Cow::Borrowed(&[])
  }

  fn as_consumer(&mut self) -> &mut dyn EventConsumer {
    self
  }
}

#[test]
fn a_count_too_large_to_add_up_neither_panics_nor_slips_past_the_maximum() {
  // With no maximum, the counts of two elements and two validators are added up without overflowing.
  let (mut one, mut two) = (Overstates, Overstates);
  {
    let mut lane = Dispatcher::new().add_validator(&mut one).add_validator(&mut two);
    TinySource::new().add_consumer(&mut lane).emit().expect("validity errors do not end the run");
  }

  // Summed in a nested dispatcher and passed up, the count still exceeds the maximum of the dispatcher above it.
  let (mut one, mut two) = (Overstates, Overstates);
  let error = {
    let mut inner = Dispatcher::new().add_validator(&mut one).add_validator(&mut two);
    let mut outer = Dispatcher::new().with_max_errors(Some(usize::MAX - 1)).add_consumer(&mut inner);
    TinySource::new().add_consumer(&mut outer).emit().expect_err("more errors than the maximum")
  };
  assert!(matches!(error, Error::Validity { .. }), "{error:?}");

  // A validator set given the same counts, with no errors to show for them, still stops with an error.
  let mut validation = validate::ValidatorSet::new()
    .add_validator(Box::new(Overstates))
    .add_validator(Box::new(Overstates))
    .with_max_errors(Some(usize::MAX - 1));
  let error = TinySource::new().add_consumer(&mut validation).emit().expect_err("more errors than the maximum");
  assert!(matches!(error, Error::Validity { .. }), "{error:?}");
}

#[test]
fn a_dispatcher_handed_the_next_document_starts_it_with_every_consumer() {
  let mut resetting = Recorder::stopping_after(1).resetting();
  let mut full = Recorder::default();
  {
    let mut dispatch = Dispatcher::new().add_consumer(&mut resetting).add_consumer(&mut full);
    TinySource::new().add_consumer(&mut dispatch).emit().unwrap();
    TinySource::new().add_consumer(&mut dispatch).emit().unwrap();
  }

  assert_eq!(resetting.seen, ["start-document", "start", "start-document", "start"], "each document from its start");
  assert_eq!(full.seen, [WHOLE_RUN, WHOLE_RUN].concat());
}

#[test]
fn a_consumer_that_does_not_reset_is_given_only_the_next_start_document() {
  let mut stale = Recorder::stopping_after(1);
  let mut full = Recorder::default();
  {
    let mut dispatch = Dispatcher::new().add_consumer(&mut stale).add_consumer(&mut full);
    TinySource::new().add_consumer(&mut dispatch).emit().unwrap();
    TinySource::new().add_consumer(&mut dispatch).emit().unwrap();
  }

  // Still finished from the first document, it answers `false` again once it has been handed the second's start.
  assert_eq!(stale.seen, ["start-document", "start", "start-document"]);
  assert_eq!(full.seen, [WHOLE_RUN, WHOLE_RUN].concat());
}

/// Records how each run it took part in ended, and can stop at its first element or refuse the end of the document.
#[derive(Default)]
struct Ends {
  stop_at_start: bool,
  refuse_end: bool,
  started: bool,
  seen_end: bool,
  outcomes: Vec<String>,
}

impl EventConsumer for Ends {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    match event {
      EventRef::StartElement(_) => self.started = true,
      EventRef::EndDocument => {
        self.seen_end = true;
        if self.refuse_end {
          return Err(Error::internal("refused at the end"));
        }
      }
      _ => {}
    }
    Ok(if self.stop_at_start && self.started { Flow::Break(0) } else { Flow::Continue(0) })
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    self.outcomes.push(match outcome {
      Outcome::Completed => "completed".to_owned(),
      Outcome::Stopped => "stopped".to_owned(),
      Outcome::Failed(error) => format!("failed: {}", error.message()),
      Outcome::Abandoned => "abandoned".to_owned(),
    });
  }
}

#[test]
fn a_run_dropped_part_way_is_abandoned() {
  let mut ends = Ends::default();
  {
    let mut source = crate::io::StreamSource::new("<a><b/></a>".as_bytes()).add_consumer(&mut ends);
    source.next().unwrap(); // StartDocument
    source.next().unwrap(); // <a>
  }
  assert_eq!(ends.outcomes, ["abandoned"]);

  // A source never pulled has started no run, so there is nothing to tell.
  let mut untouched = Ends::default();
  drop(crate::io::StreamSource::new("<a/>".as_bytes()).add_consumer(&mut untouched));
  assert!(untouched.outcomes.is_empty());
}

#[test]
fn every_consumer_is_told_the_same_outcome_whichever_one_ended_the_run() {
  let mut a = Ends { stop_at_start: true, ..Ends::default() };
  let mut b = Ends { refuse_end: true, ..Ends::default() };
  let mut c = Ends::default();
  let error = TinySource::new().add_consumer(&mut a).add_consumer(&mut b).add_consumer(&mut c).emit().unwrap_err();

  // `b` refused `EndDocument`: `a` had finished before it and `c` never saw it, and neither has to guess from that.
  assert!(b.seen_end && !c.seen_end);
  let told = format!("failed: {}", error.message());
  for ends in [&a, &b, &c] {
    assert_eq!(ends.outcomes, [told.as_str()], "told once, the same as the others");
  }
}

#[test]
fn a_run_that_reaches_its_end_is_completed_and_told_once() {
  let mut ends = Ends::default();
  {
    let mut source = TinySource::new().add_consumer(&mut ends);
    source.emit().unwrap();
    assert!(source.next().unwrap().is_none());
  }
  assert_eq!(ends.outcomes, ["completed"]);
}

#[test]
fn a_run_every_consumer_finished_early_is_stopped() {
  let mut one = Ends { stop_at_start: true, ..Ends::default() };
  let mut two = Ends { stop_at_start: true, ..Ends::default() };
  TinySource::new().add_consumer(&mut one).add_consumer(&mut two).emit().unwrap();
  assert_eq!(one.outcomes, ["stopped"]);
  assert_eq!(two.outcomes, ["stopped"]);
}

#[test]
fn an_error_in_the_input_is_told_to_the_consumers_as_well() {
  let mut early = Ends { stop_at_start: true, ..Ends::default() };
  let mut full = Ends::default();
  let error = crate::io::StreamSource::new("<a><b></a>".as_bytes())
    .add_consumer(&mut early)
    .add_consumer(&mut full)
    .emit()
    .unwrap_err();
  let told = format!("failed: {}", error.message());
  assert_eq!(early.outcomes, [told.as_str()]);
  assert_eq!(full.outcomes, [told.as_str()]);
}

#[test]
fn the_other_sources_and_the_consumers_that_wrap_others_pass_the_outcome_on() {
  use crate::dom::DomSource;
  use crate::event::validate::{Ended, ValidatorSet};
  use crate::io::write::XmlWriter;

  let doc = crate::Reader::new().document("<a><b/></a>".as_bytes()).unwrap();
  let mut beside_writer = Ends::default();
  let mut writer = XmlWriter::new(Vec::new());
  DomSource::new(&doc).add_consumer(&mut writer).add_consumer(&mut beside_writer).emit().unwrap();
  assert_eq!(beside_writer.outcomes, ["completed"]);

  let mut validation = ValidatorSet::new();
  TinySource::new().add_consumer(&mut validation).emit().unwrap();
  assert_eq!(validation.report().ended(), Some(Ended::Completed));
}

/// Every field of an event written out, so an event can be compared with one made by another route.
fn render(event: &EventRef<'_>) -> String {
  match event {
    EventRef::StartDocument => "start-document".to_owned(),
    EventRef::EndDocument => "end-document".to_owned(),
    EventRef::StartElement(e) => {
      let attributes: Vec<_> =
        e.attributes.iter().map(|a| (a.prefix, a.local, a.namespace, a.value, a.declares_namespace())).collect();
      format!(
        "start {:?} {:?} {:?} {attributes:?} {:?} {:?} {:?} {:?}",
        e.prefix, e.local, e.namespace, e.xml_space, e.xml_lang, e.base_uri, e.location
      )
    }
    EventRef::EndElement(e) => format!("end {:?} {:?} {:?} {:?}", e.prefix, e.local, e.namespace, e.location),
    EventRef::Characters(e) => format!("text {:?} {:?}", e.text, e.location),
    EventRef::Cdata(e) => format!("cdata {:?} {:?}", e.text, e.location),
    EventRef::Comment(e) => format!("comment {:?} {:?}", e.text, e.location),
    EventRef::ProcessingInstruction(e) => {
      format!("pi {:?} {:?} {:?} {:?}", e.target, e.data, e.data_location, e.location)
    }
    EventRef::Doctype(e) => format!("doctype {:?} {:?} {:?} {:?}", e.name, e.public_id, e.system_id, e.location),
  }
}

#[test]
fn an_owned_event_lends_back_the_event_it_was_copied_from() {
  let attributes = vec![
    Attribute {
      prefix: Some("p".into()),
      local: "x".into(),
      namespace: Some("urn:p".into()),
      value: "1".into(),
      location: Location { line: 3, column: 10, offset: 23, ..Location::unknown() },
      value_location: Location { line: 3, column: 14, offset: 27, ..Location::unknown() },
    },
    Attribute {
      prefix: Some("xmlns".into()),
      local: "p".into(),
      namespace: Some(crate::name::XMLNS_NS_URI.into()),
      value: "urn:p".into(),
      location: Location::unknown(),
      value_location: Location::unknown(),
    },
  ];
  let at = Location { line: 3, column: 7, offset: 20, ..Location::unknown() }.with_system_id("file:///d.xml");
  let data_at = Location { line: 3, column: 12, offset: 25, ..Location::unknown() }.with_system_id("file:///d.xml");
  let events = [
    EventRef::StartDocument,
    EventRef::StartElement(StartElementEventRef::new(
      Some("p"),
      "a",
      Some("urn:p"),
      Attributes::new(&attributes),
      XmlSpace::Preserve,
      Some("en"),
      Some("file:///d.xml"),
      at.clone(),
    )),
    EventRef::Characters(CharactersEventRef::new("hi", at.clone())),
    EventRef::Cdata(CdataEventRef::new(" a<b ", at.clone())),
    EventRef::Comment(CommentEventRef::new(" note ", at.clone())),
    EventRef::ProcessingInstruction(ProcessingInstructionEventRef::new("php", "echo 1; ", data_at, at.clone())),
    EventRef::EndElement(EndElementEventRef::new(Some("p"), "a", Some("urn:p"), at)),
    EventRef::EndDocument,
  ];

  for borrowed in &events {
    let owned = Event::from(borrowed);
    assert_eq!(render(&owned.as_event_ref()), render(borrowed));
  }
}

#[test]
fn an_owned_doctype_keeps_the_dtd_after_its_source_is_gone() {
  let owned = {
    let (dtd, pool) = crate::dtd::DtdReader::new("<!ATTLIST r id ID #IMPLIED>".as_bytes()).read().expect("a DTD");
    let borrowed = EventRef::Doctype(DoctypeEventRef::new(
      Some("r"),
      Some("-//x//y"),
      Some("r.dtd"),
      &dtd,
      &pool,
      Location::unknown(),
    ));
    Event::from(&borrowed)
  };

  // The DTD and pool it was copied from have been dropped; what it lends back is its own.
  let EventRef::Doctype(back) = owned.as_event_ref() else { panic!("a doctype lends back a doctype") };
  assert_eq!((back.name, back.public_id, back.system_id), (Some("r"), Some("-//x//y"), Some("r.dtd")));
  let r = back.pool.get("r").expect("the DTD's names are in the pool it carries");
  assert!(back.dtd.attlist(r).is_some(), "the declarations are reachable through it");
}

#[test]
fn a_document_kept_as_owned_events_builds_the_same_tree_when_replayed() {
  use crate::dom::build::DomBuilder;
  use crate::dom::{Document, DomSource};
  use crate::io::StreamSource;
  use crate::io::write::XmlWriter;

  let xml = "<!DOCTYPE r [<!ATTLIST r id ID #IMPLIED>]>\
             <r id='x' xmlns:p='urn:p'><p:a>t<![CDATA[c]]></p:a><!--n--><?pi d?></r>";

  let mut direct = DomBuilder::new();
  StreamSource::new(xml.as_bytes()).add_consumer(&mut direct).emit().unwrap();
  let direct = direct.into_document();

  let mut kept = Vec::new();
  let mut source = StreamSource::new(xml.as_bytes());
  while let Some(event) = source.next().unwrap() {
    kept.push(Event::from(&event));
  }
  let mut replayed = DomBuilder::new();
  for event in &kept {
    let _ = replayed.consume(&event.as_event_ref()).unwrap();
  }
  let replayed = replayed.into_document();

  // Written back out through the writer, so the two trees are compared as the XML they produce.
  let text = |doc: &Document| {
    let mut writer = XmlWriter::new(Vec::new());
    DomSource::new(doc).add_consumer(&mut writer).emit().unwrap();
    String::from_utf8(writer.into_inner()).unwrap()
  };
  assert_eq!(text(&replayed), text(&direct));
  assert!(replayed.get_element_by_id("x").is_some(), "the DTD's ID declaration reached the replayed build too");
}

#[test]
fn owned_events_of_one_kind_are_kept_as_that_kind_and_lent_back_on_their_own() {
  use crate::io::StreamSource;

  let xml = "<r><a x='1'/><b/><a x='2'/></r>";
  let mut starts: Vec<StartElementEvent> = Vec::new();
  let mut source = StreamSource::new(xml.as_bytes());
  while let Some(event) = source.next().unwrap() {
    if let Event::StartElement(start) = Event::from(&event) {
      starts.push(start);
    }
  }

  let names: Vec<&str> = starts.iter().map(|start| start.local.as_str()).collect();
  assert_eq!(names, ["r", "a", "b", "a"]);

  // A payload is lent back by itself, in the borrowed form a function over one kind of event takes.
  let last: StartElementEventRef<'_> = starts[3].as_event_ref();
  assert_eq!(last.lexical(), "a");
  assert_eq!(last.attributes.get_by_name(None, "x").map(|a| a.value), Some("2"));
}
