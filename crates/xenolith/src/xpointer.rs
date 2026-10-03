//! XPointer: A mechanism for selecting parts of an XML resource.
//!
//! XPointer (XML Pointer Language) is a language for an addressing scheme used to point to specific locations,
//! elements, or positions within an XML document. [`XPointer`] represents a parsed pointer string. Because it is
//! immutable, a parsed pointer can be shared among any number of [`XPointerFilter`] instances.
//!
//! [`XPointer`] functions as a filter within an event pipeline. Specifically, it allows events corresponding to the
//! element identified by the pointer, and its entire subtree, to pass through, while discarding all other events in
//! the document. The events that pass through are enclosed by [`StartDocument`](EventRef::StartDocument) and
//! [`EndDocument`](EventRef::EndDocument), but they form a document fragment rather than a complete document;
//! consequently, no re-verification of XML well-formedness is performed.
//!
//! For XPointers that do not require a tree structure, this filter performs the selection process *directly* on the
//! event stream. If a tree structure is required to select nodes, the selected nodes are emitted immediately before
//! the [`EndDocument`](EventRef::EndDocument) event.
//!
//! Matching against IDs is performed based on the `xml:id` attribute or attributes defined as type `ID` in the
//! document type declaration. The [`Doctype`](EventRef::Doctype) event is used to read these declarations; however,
//! the `Doctype` event itself is not passed on to subsequent processing stages unless explicitly specified via
//! [`with_doctype`](XPointerFilter::with_doctype).
//!
//! [`XIncludeTransformer`](crate::xinclude::XIncludeTransformer) uses this filter for the `xpointer` attribute of
//! `xi:include`.
//!
//! ```
//! use xenolith::Location;
//! use xenolith::event::{EventCursor, EventProducer};
//! use xenolith::io::StreamSource;
//! use xenolith::io::write::XmlWriter;
//! use xenolith::xpointer::{XPointer, XPointerFilter};
//!
//! // Select the second child element of the document element.
//! let pointer = XPointer::parse("element(/1/2)", Location::new())?;
//! let mut writer = XmlWriter::new(Vec::new());
//! {
//!   let mut filter = XPointerFilter::new(&pointer).add_consumer(&mut writer);
//!   StreamSource::new("<doc><a/><b>two</b></doc>".as_bytes()).add_consumer(&mut filter).emit()?;
//! }
//! assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<b>two</b>");
//! # Ok::<(), xenolith::Error>(())
//! ```
//!
//! # Supported Pointer Formats
//!
//! - A Shorthand Pointer points to the element with the corresponding ID (§3.2). It behaves identically to an
//!   `element()` pointer specifying that `NCName`.
//! - The `element()` scheme points to an element using a single ID, a sequence of child positions (e.g., `/1/2`), or
//!   an ID followed by a child positions. Child positions count only child *elements*; `/1` refers to the first
//!   top-level element.
//! - The `xmlns()` scheme is accepted but does not itself point to anything.
//! - PointerParts with other SchemeNames — including `xpointer()` — are skipped. This adheres to the
//!   [XPointer Framework] specifications regarding schemes not supported by the implementation.
//!
//! # Errors and Limitations
//!
//! - [`XPointer::parse`] returns an error for invalid XPointer syntax.
//! - An error is returned at [`StartDocument`](EventRef::StartDocument) if no supported PointerPart exists.
//! - If multiple PointerParts are specified, they are evaluated from left to right, and unsupported PointerParts are
//!   ignored; the result of the first valid PointerPart is applied.
//! - If the element pointed to by the pointer does not exist, [`Error::XPointer`] is returned at
//!   [`EndDocument`](EventRef::EndDocument) (in accordance with the [XPointer Framework] specification).
//!
//! # Specifications
//!
//! - [XPointer Framework] — W3C Recommendation, March 25, 2003
//! - [XPointer `element()` Scheme] — W3C Recommendation, March 25, 2003
//! - [XPointer `xmlns()` Scheme] — W3C Recommendation, March 25, 2003
//!
//! [XPointer Framework]: https://www.w3.org/TR/2003/REC-xptr-framework-20030325/
//! [XPointer `element()` Scheme]: https://www.w3.org/TR/2003/REC-xptr-element-20030325/
//! [XPointer `xmlns()` Scheme]: https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/

#[cfg(test)]
mod test;

mod element;

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::chars;
use crate::error::{Error, Location, Result};
use crate::event::{Dispatcher, Event, EventConsumer, EventCursor, EventProducer, EventRef, Flow, Outcome};
use crate::xpointer::element::ElementScheme;

/// A parsed XPointer.
///
/// Create one from a string using [`parse`](Self::parse) (or [`str::parse`]). The instance itself is stateless and can
/// be shared across threads. You can access the original pointer string via [`as_str`](Self::as_str).
#[derive(Clone)]
pub struct XPointer {
  /// The specified pointer.
  source: String,
  /// The location pointed to by the first character of the pointer (if an error occurs in a filter that performs a
  /// selection based on that character).
  location: Location,
  /// The result of sequentially parsing the PointerPart of the schemes supported by this implementation. It contains
  /// either the scheme or a parsing error.
  parts: Vec<ParsedPart>,
}

/// One pointer part of a supported scheme as its scheme parsed it: the data, or the error that says why the data does
/// not follow the scheme's syntax. The error is shared so that the pointer can be cloned.
type ParsedPart = std::result::Result<Arc<dyn SchemeData>, Arc<Error>>;

/// This is the scheme data for a specific `PointerPart`, as parsed by a scheme. Each scheme supported by the
/// implementation provides an implementation of this.
///
/// This data is immutable and is shared among all filters that make selections based on the pointer. Information that
/// must be persisted across executions is held by the [`SchemeSelector`], and each execution begins using this data.
trait SchemeData: fmt::Debug + Send + Sync {
  /// Creates a selector for the execution of a sequence of events.
  fn selector(&self) -> Box<dyn SchemeSelector + '_>;
}

/// The state of a specific PointerPart within a sequence of event processing.
///
/// Selectors perform selection using one or both of two methods. A streaming selector makes a determination within
/// [`filter`](Self::filter) each time an event arrives (i.e., on an event-by-event basis). In contrast, a batch
/// selector that requires the entire document to make a determination — such as XSLT — retains the loaded content (for
/// example, as a constructed DOM tree), passes nothing through the `filter`, and returns all selection results via
/// [`drain`](Self::drain). These results are output by the filter immediately before the
/// [`EndDocument`](EventRef::EndDocument) event.
trait SchemeSelector {
  /// Evaluates the `event`, updates internal state if necessary, and returns whether to pass the event through.
  fn filter(&mut self, event: &EventRef<'_>) -> bool;

  /// Returns all events held internally by the selector, forwarding them to the next stage just before the
  /// [`EndDocument`](EventRef::EndDocument), or `None` if the streaming selector has already passed the selection
  /// through or the selector selected nothing.
  fn drain(&mut self) -> Option<Box<dyn EventCursor<'_> + '_>> {
    None
  }
}

impl XPointer {
  /// Parses the pointer string.
  ///
  /// `location` specifies the starting character position of the `pointer`. For example, in the case of an `xpointer`
  /// attribute, it represents the position within the document where that value appears. If the pointer is defined in
  /// isolation, specify [`Location::new`]. This allows the system to indicate the character column where an error
  /// occurred. Error positions, both for this method and for filters selected by this pointer, are determined by
  /// counting characters in the `pointer` starting from `location`. If the pointer contains no character references or
  /// entity references, the calculated position corresponds to the actual position in the document.
  ///
  /// # Errors
  ///
  /// If the string does not conform to either the XPointer Framework's Shorthand nor SchemeBase pointer syntax, an
  /// [`Error::XPointer`] is returned, indicating the position of the erroneous character; this includes unescaped
  /// circumflex characters (`^`) and unbalanced parentheses.
  ///
  /// Unrecognized schemes or scheme-specific syntax errors are not reported here. An error is reported when a selector
  /// is referenced using a pointer that lacks a valid PointerPart.
  pub fn parse(pointer: &str, location: Location) -> Result<Self> {
    let parts = if chars::is_ncname(pointer) {
      // Framework §3.2: a shorthand pointer identifies the element with that ID, as element(NCName) does.
      let data: Arc<dyn SchemeData> = Arc::new(ElementScheme::shorthand(pointer));
      vec![Ok(data)]
    } else {
      parse_scheme_based(pointer, &location)?
    };
    Ok(Self { source: pointer.to_owned(), location, parts })
  }

  /// The pointer string exactly as specified.
  #[must_use]
  pub fn as_str(&self) -> &str {
    &self.source
  }

  /// Returns errors for PointerParts that do not conform to the scheme's syntax, in the order they appear. If all
  /// PointerParts for supported schemes are parsed correctly, nothing is returned. Each error is an
  /// [`Error::XPointer`] containing the position of the character that violates the syntax.
  ///
  /// According to the XPointer specification, such pointer parts are treated as pointing to nothing, and the pointer
  /// makes its selection using the other PointerParts. Note that PointerParts for schemes not supported by this
  /// implementation are skipped in accordance with the XPointer Framework specification and are therefore not included
  /// in the errors.
  pub fn errors(&self) -> impl Iterator<Item = &Error> + '_ {
    self.parts.iter().filter_map(|part| part.as_ref().err().map(|error| &**error))
  }

  /// Generates the selectors corresponding to each parsed PointerPart. Returns an error if no PointerParts are
  /// available.
  fn candidates(&self) -> Result<Vec<Candidate<'_>>> {
    let candidates: Vec<Candidate<'_>> = self
      .parts
      .iter()
      .filter_map(|part| part.as_ref().ok())
      .map(|data| Candidate { selector: data.selector(), pending: Vec::new(), pending_chars: 0 })
      .collect();
    if !candidates.is_empty() {
      return Ok(candidates);
    }
    // Every part of a supported scheme has data its scheme refused; the first one says why.
    Err(match self.errors().next() {
      // The first error is returned as information if parsing fails for all PointerParts specified as the Pointer.
      Some(error) => Error::xpointer(format!(
        "the pointer {:?} has no part this implementation can evaluate: {}",
        self.source,
        error.message()
      ))
      .at(error.location().clone()),
      // The case where the Pointer contains only unsupported PointerParts.
      None => Error::xpointer(format!(
        "the pointer {:?} has no part of a scheme this implementation supports, so it identifies nothing",
        self.source
      ))
      .at(self.location.clone()),
    })
  }
}

/// A single PointerPart: its selector and pending events.
///
/// Each candidate has its own buffer of pending events, but only one actually has pending events. The first candidate
/// does not put the selection into a pending state; once a candidate begins the selection process, the filter discards
/// all subsequent candidates. Therefore, only the last candidate (not the first) can potentially have pending events.
struct Candidate<'p> {
  selector: Box<dyn SchemeSelector + 'p>,
  pending: Vec<Event>,
  /// The characters of the events in `pending`, counted as [`XPointerFilter::with_max_pending_chars`] describes.
  pending_chars: usize,
}

/// Passes the `event` to the currently executing PointerPart instances, in order up to the one that selects it, and
/// return whether the event should be immediately forwarded to subsequent consumers. If a part initiates a selection,
/// subsequent parts and any pending events they hold are discarded. The approximate number of characters in pending
/// events cannot exceed `max_pending_chars`.
fn select(candidates: &mut Vec<Candidate<'_>>, event: &EventRef<'_>, max_pending_chars: Option<usize>) -> Result<bool> {
  // These events are merely notified to the selector; the selection results are ignored.
  let notice_only = matches!(event, EventRef::StartDocument | EventRef::EndDocument | EventRef::Doctype(_));
  for (index, candidate) in candidates.iter_mut().enumerate() {
    if !candidate.selector.filter(event) || notice_only {
      continue;
    }
    if index > 0 {
      candidate.pending_chars = candidate.pending_chars.saturating_add(chars_of(event));
      if let Some(max) = max_pending_chars.filter(|&max| candidate.pending_chars > max) {
        let message = format!(
          "the pending events of a later part of the pointer exceed {max} characters; raise \
           XPointerFilter::with_max_pending_chars"
        );
        return Err(Error::limit(message).at(event.location().cloned().unwrap_or_else(Location::unknown)));
      }
      candidate.pending.push(Event::from(event));
    }
    candidates.truncate(index + 1);
    return Ok(index == 0);
  }
  Ok(false)
}

/// Calculates an approximate value for the number of characters held by `event`. This includes names, attribute names
/// and values, character data, comments, as well as processing instruction targets and data.
fn chars_of(event: &EventRef<'_>) -> usize {
  let name =
    |prefix: Option<&str>, local: &str| prefix.map_or(0, |prefix| prefix.chars().count() + 1) + local.chars().count();
  match event {
    EventRef::StartElement(start) => {
      let attributes: usize =
        start.attributes.iter().map(|attr| name(attr.prefix, attr.local) + attr.value.chars().count()).sum();
      name(start.prefix, start.local) + attributes
    }
    EventRef::EndElement(end) => name(end.prefix, end.local),
    EventRef::Characters(text) => text.text.chars().count(),
    EventRef::Cdata(text) => text.text.chars().count(),
    EventRef::Comment(comment) => comment.text.chars().count(),
    EventRef::ProcessingInstruction(pi) => pi.target.chars().count() + pi.data.chars().count(),
    EventRef::StartDocument | EventRef::EndDocument | EventRef::Doctype(_) => 0,
  }
}

/// Retrieves events that were selected but deferred when the end of the document was reached without any events having
/// been sent to the subsequent stage.
fn deferred_selected_events<'c>(
  candidates: &'c mut [Candidate<'_>],
) -> (&'c [Event], Option<Box<dyn EventCursor<'c> + 'c>>) {
  for candidate in candidates.iter_mut() {
    // No pending events remaining for the PointerPart which internally maintains state based on events. It returns all
    // pending events upon a call to drain.
    let cursor = candidate.selector.drain();
    if !candidate.pending.is_empty() || cursor.is_some() {
      return (&candidate.pending, cursor);
    }
  }
  (&[], None)
}

impl fmt::Debug for XPointer {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("XPointer").field("pointer", &self.source).field("location", &self.location).finish()
  }
}

impl PartialEq for XPointer {
  /// Two pointers are equal when they were written the same and at the same location.
  fn eq(&self, other: &Self) -> bool {
    self.source == other.source && self.location == other.location
  }
}

impl Eq for XPointer {}

impl FromStr for XPointer {
  type Err = Error;

  /// It parses the `pointer` in the same way as [`parse`](XPointer::parse). Since it treats the input as a standalone
  /// string and counts from [`Location::new`] (line 1, column 1), errors indicate the column position within the string.
  fn from_str(pointer: &str) -> Result<Self> {
    Self::parse(pointer, Location::new())
  }
}

/// Creates the corresponding [`SchemeData`] from the `SchemeName '(' SchemeData ')'` of a `PointerPart`. If the
/// `SchemeName` is `xmlns` or unsupported, it results in `None` and is skipped in accordance with the framework
/// specification (§3.3).
fn parse_scheme_data(scheme: &str, data: &str) -> Option<std::result::Result<Arc<dyn SchemeData>, (String, usize)>> {
  match scheme {
    "element" => Some(ElementScheme::parse(data).map(|data| Arc::new(data) as Arc<dyn SchemeData>)),
    _ => None,
  }
}

/// Parses a SchemeBase and returns PointerParts. Each PointerPart is separated by zero or more whitespace characters
/// (Framework §3.3). `location` is the starting position of the `pointer`.
fn parse_scheme_based(pointer: &str, location: &Location) -> Result<Vec<ParsedPart>> {
  // `byte` is an offset into `pointer`, and the error is located at the character there.
  let invalid = |why: &str, byte: usize| {
    Error::xpointer(format!("{pointer:?} is not an XPointer: {why}")).at(located(location, pointer, byte))
  };
  let mut parts = Vec::new();
  let mut rest = pointer.trim_start_matches(chars::is_whitespace);
  if rest.is_empty() {
    return Err(invalid("it has no pointer part", 0));
  }
  while !rest.is_empty() {
    let start = pointer.len() - rest.len();
    let Some(open) = rest.find('(') else {
      return Err(invalid("a pointer part has no scheme data in parentheses", start));
    };
    let scheme = &rest[..open];
    if !is_qname(scheme) {
      return Err(invalid(&format!("{scheme:?} is not a scheme name"), start));
    }
    let data_start = start + open + 1;
    let (data, after) = read_scheme_data(&rest[open + 1..])
      .map_err(|(why, at)| invalid(why, at.map_or(start + open, |at| data_start + at)))?;
    match parse_scheme_data(scheme, &data) {
      Some(Ok(data)) => parts.push(Ok(data)),
      // Located by counting into the data as it was read; an escape before the fault would shift it by one, but the
      // schemes supported here allow no escaped character in their data anyway.
      Some(Err((why, at))) => {
        parts.push(Err(Arc::new(Error::xpointer(why).at(located(location, pointer, data_start + at)))));
      }
      None => {}
    }
    rest = after.trim_start_matches(chars::is_whitespace);
  }
  Ok(parts)
}

/// The location advanced by `byte` bytes from `text` starting at `start`.
fn located(start: &Location, text: &str, byte: usize) -> Location {
  let mut at = start.clone();
  for c in text[..byte].chars() {
    at.advance(c);
  }
  at
}

/// Reads SchemeData from `text`. As specified in Framework § 3.1, it unescapes `^(`, `^)`, and `^^`, generates a
/// string up to the closing parenthesis, and returns it along with the remaining text. If reading fails, it returns an
/// error containing the reason and the position.
fn read_scheme_data(text: &str) -> std::result::Result<(String, &str), (&'static str, Option<usize>)> {
  let mut data = String::new();
  let mut depth = 0usize;
  let mut chars = text.char_indices();
  while let Some((at, c)) = chars.next() {
    match c {
      '^' => match chars.next() {
        Some((_, escaped @ ('(' | ')' | '^'))) => data.push(escaped),
        _ => return Err(("a circumflex escapes only a parenthesis or another circumflex", Some(at))),
      },
      '(' => {
        depth += 1;
        data.push(c);
      }
      ')' if depth == 0 => return Ok((data, &text[at + 1..])),
      ')' => {
        depth -= 1;
        data.push(c);
      }
      _ => data.push(c),
    }
  }
  Err(("a parenthesis is not closed", None))
}

/// True if the `name` is a `QName` (an `NCName` or two `NCNames` joined by a colon).
fn is_qname(name: &str) -> bool {
  match name.split_once(':') {
    Some((prefix, local)) => chars::is_ncname(prefix) && chars::is_ncname(local),
    None => chars::is_ncname(name),
  }
}

/// The default value for [`XPointerFilter::with_max_pending_chars`]: 16 Mi characters.
pub(crate) const DEFAULT_MAX_PENDING_CHARS: usize = 16 * 1024 * 1024;

/// Forwards only the elements selected by the [`XPointer`] and their descendants from the incoming event stream. This
/// functions as an [`XPointer`]-based filter within the event pipeline.
///
/// The filter merely borrows the [`XPointer`]; consequently, any number of filters can be created from a single parsed
/// pointer. Since each filter initializes itself upon receiving a [`StartDocument`](EventRef::StartDocument) event, it
/// can be reused across multiple documents. Please refer to the [module documentation](self) for details on supported
/// formats and potential errors.
pub struct XPointerFilter<'p, 'h> {
  /// The pointer serving as the selection criterion.
  pointer: &'p XPointer,
  /// The consumer to which selected events are sent.
  dispatcher: Dispatcher<'h>,
  /// True if `Doctype` events are to be passed downstream.
  doctype: bool,
  /// The maximum total number of characters for events that can be buffered by this filter (`None` for unlimited).
  max_pending_chars: Option<usize>,
  /// The selectors corresponding to the parsed XPointer's PointerParts (in order of appearance). The XPointer
  /// framework attempts selectors in the order they appear until a valid selection is made (§3.3). In an event
  /// streaming model, events selected by subsequent selectors must be buffered in case earlier selectors ultimately
  /// select nothing. If the first selector completes `EndDocument` without having sent any events, the buffered events
  /// from the second and subsequent selectors are then sent. Once a selector selects an event, subsequent selectors
  /// are no longer needed and are discarded along with their buffered events.
  candidates: Option<Vec<Candidate<'p>>>,
  /// True if events are being sent downstream.
  found: bool,
}

impl fmt::Debug for XPointerFilter<'_, '_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("XPointerFilter")
      .field("pointer", &self.pointer.source)
      .field("doctype", &self.doctype)
      .field("max_pending_chars", &self.max_pending_chars)
      .field("found", &self.found)
      .finish_non_exhaustive()
  }
}

impl<'p> XPointerFilter<'p, '_> {
  /// Creates a filter that makes a selection based on `pointer`.
  #[must_use]
  pub fn new(pointer: &'p XPointer) -> Self {
    Self {
      pointer,
      dispatcher: Dispatcher::new(),
      doctype: false,
      max_pending_chars: Some(DEFAULT_MAX_PENDING_CHARS),
      candidates: None,
      found: false,
    }
  }

  /// Specifies whether to pass the document's [`Doctype`](EventRef::Doctype) event to the next stage. The default
  /// value is `false`. This is because the filter passes only a portion of the document, and the document type
  /// declaration does not describe that portion.
  #[must_use]
  pub fn with_doctype(mut self, on: bool) -> Self {
    self.doctype = on;
    self
  }

  /// Specifies the maximum number of characters for pending events held in memory (events sent subsequently are not
  /// counted). The default is 16 Mi characters (16,777,216 characters). If the specified limit is exceeded, the filter
  /// returns [`Error::Limit`]. Specifying `None` means no limit is applied.
  ///
  /// This filter may buffer received events before forwarding them. A typical scenario is when a subsequent pointer
  /// selects an event, even though the leading pointer — in a pointer composed of multiple `PointerPart`s — has not
  /// yet selected an event. Since it is uncertain whether the leading pointer will subsequently send an event, the
  /// selected event must be held in memory.
  ///
  /// Note that the character count is an estimate based on element names, attribute names and values, text content,
  /// and so on. This serves as a *rough guide* to the memory consumed by buffering, rather than an exact measurement.
  #[must_use]
  pub fn with_max_pending_chars(mut self, max: Option<usize>) -> Self {
    self.max_pending_chars = max;
    self
  }

  /// The value to return when not forwarding the event to the next handler.
  fn skip(&self) -> Result<Flow> {
    Ok(if self.dispatcher.is_stopped() { Flow::Break(0) } else { Flow::Continue(0) })
  }
}

impl<'h> EventProducer<'h> for XPointerFilter<'_, 'h> {
  fn add_consumer(mut self, consumer: &'h mut dyn EventConsumer) -> Self {
    self.dispatcher = self.dispatcher.add_consumer(consumer);
    self
  }
}

impl<'p> EventConsumer for XPointerFilter<'p, '_> {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    // The pointer instances outlive filters, so the selector can borrow the data from the PointerPart while the filter
    // holds it.
    let pointer: &'p XPointer = self.pointer;
    if matches!(event, EventRef::StartDocument) {
      self.candidates = None;
      self.found = false;
    }
    // Generate the selector upon the first event or the execution of `StartDocument`.
    let candidates = match &mut self.candidates {
      Some(candidates) => candidates,
      slot @ None => slot.insert(pointer.candidates()?),
    };
    let selected = select(candidates, event, self.max_pending_chars)?;
    match event {
      EventRef::StartDocument => self.dispatcher.consume(event),
      EventRef::EndDocument => {
        let mut count: usize = 0;
        let (pending, then) = deferred_selected_events(candidates);
        // Passes one event of the selection on, and returns whether the consumers have stopped.
        let mut relay = |event: &EventRef<'_>| -> Result<bool> {
          self.found = true;
          let (Flow::Continue(found) | Flow::Break(found)) = self.dispatcher.consume(event)?;
          count = count.saturating_add(found);
          Ok(self.dispatcher.is_stopped())
        };
        let mut stopped = false;
        for event in pending {
          stopped = relay(&event.as_event_ref())?;
          if stopped {
            break;
          }
        }
        if let Some(mut cursor) = then.filter(|_| !stopped) {
          while let Some(event) = cursor.next()? {
            if matches!(event, EventRef::StartDocument | EventRef::EndDocument) {
              continue;
            }
            if relay(&event)? {
              break;
            }
          }
        }
        if !self.found {
          let message = format!("the pointer {:?} identifies no element in the resource", self.pointer.source);
          return Err(Error::xpointer(message).at(self.pointer.location.clone()));
        }
        let (Flow::Continue(ended) | Flow::Break(ended)) = self.dispatcher.consume(event)?;
        let count = count.saturating_add(ended);
        Ok(if self.dispatcher.is_stopped() { Flow::Break(count) } else { Flow::Continue(count) })
      }
      EventRef::Doctype(_) if self.doctype => self.dispatcher.consume(event),
      EventRef::Doctype(_) => self.skip(),
      _ if selected => {
        self.found = true;
        self.dispatcher.consume(event)
      }
      _ => self.skip(),
    }
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    self.dispatcher.finish(outcome);
  }
}
