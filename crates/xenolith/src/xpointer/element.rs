//! The `element` scheme. It also evaluates Shorthand of Pointers.

use std::collections::{HashMap, HashSet};

use crate::chars;
use crate::dtd::model::AttType;
use crate::event::{DoctypeEventRef, EventRef, StartElementEventRef};
use crate::name::XML_NS_URI;
use crate::xpointer::{SchemeData, SchemeSelector};

/// The `element` scheme: An element ID, a sequence of child element occurrence numbers, or an element ID followed by a
/// sequence of child element occurrence numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ElementScheme {
  /// The ID of the element serving as the starting point for the child element sequence. `None` if starting from the
  /// document root.
  id: Option<String>,
  /// The sequence of child element occurrence numbers. Each number specifies the position of the element relative to
  /// its parent, using 1-based indexing.
  steps: Vec<usize>,
}

impl ElementScheme {
  /// The Shorthand of Pointer with the ID `id`.
  pub(super) fn shorthand(id: &str) -> Self {
    Self { id: Some(id.to_owned()), steps: Vec::new() }
  }

  /// Parses the SchemeData of the specified `element`. If `data` does not conform to the Scheme syntax, it returns
  /// the reason and the byte offset within `data` where the issue occurred.
  pub(super) fn parse(data: &str) -> Result<Self, (String, usize)> {
    if data.is_empty() {
      return Err(("element() needs an NCName, a child sequence, or both".to_owned(), 0));
    }
    let slash = data.find('/').unwrap_or(data.len());
    let id = match &data[..slash] {
      "" => None,
      id if chars::is_ncname(id) => Some(id.to_owned()),
      id => return Err((format!("{id:?} in element() is not an NCName"), 0)),
    };
    let mut steps = Vec::new();
    let mut at = slash + 1;
    if slash < data.len() {
      for step in data[slash + 1..].split('/') {
        // §3: each step is a positive integer with no leading zero.
        let position = step.parse().ok().filter(|_| !step.starts_with('0') && step.bytes().all(|b| b.is_ascii_digit()));
        let Some(position) = position else {
          return Err((format!("{step:?} in the child sequence of element() is not a positive integer"), at));
        };
        steps.push(position);
        at += step.len() + 1;
      }
    }
    Ok(Self { id, steps })
  }
}

impl SchemeData for ElementScheme {
  fn selector(&self) -> Box<dyn SchemeSelector + '_> {
    Box::new(ElementSelector {
      scheme: self,
      ids: HashMap::new(),
      counts: vec![0],
      anchor: None,
      selected: None,
      searching: true,
    })
  }
}

/// The state of the `element()` component during a single filtering execution; selects events within the stream.
struct ElementSelector<'s> {
  /// The scheme of the target to be selected.
  scheme: &'s ElementScheme,
  /// The attributes declared as `ID` in the document's DTD for the element's qualified name.
  ids: HashMap<String, HashSet<String>>,
  /// For each open element and for the document, how many child elements it has had so far. The last entry is the
  /// innermost. The entries before the last are also the 1-based position of each open element among its siblings,
  /// from the top-level element inward.
  counts: Vec<usize>,
  /// The depth of the element with the ID while it is open.
  anchor: Option<usize>,
  /// The depth of the selected element while its events are selected.
  selected: Option<usize>,
  /// Whether something can still be selected; false once an element has been selected, or the element with the ID has
  /// closed.
  searching: bool,
}

impl ElementSelector<'_> {
  /// Records attributes declared as `ID` in the DTD.
  fn read_doctype(&mut self, event: &DoctypeEventRef<'_>) {
    for (element, definitions) in event.dtd.attlists() {
      let names: HashSet<String> = definitions
        .iter()
        .filter(|definition| matches!(definition.att_type, AttType::Id))
        .map(|definition| event.pool.resolve(definition.name).to_owned())
        .collect();
      if !names.is_empty() {
        self.ids.insert(event.pool.resolve(element).to_owned(), names);
      }
    }
  }

  /// True if the element has an attribute of ID-type. Either an `xml:id` attribute or an attribute declared as `ID` in
  /// the DTD.
  fn has_id(&self, start: &StartElementEventRef<'_>, id: &str) -> bool {
    let declared = if self.ids.is_empty() { None } else { self.ids.get(&start.lexical()) };
    start.attributes.iter().any(|attr| {
      let is_id = (attr.namespace == Some(XML_NS_URI) && attr.local == "id")
        || declared.is_some_and(|names| names.contains(&attr.lexical()));
      // xml:id §4 normalizes the value as a tokenized ID, which the parser may not have done.
      is_id && attr.value.split(chars::is_whitespace).filter(|token| !token.is_empty()).eq(std::iter::once(id))
    })
  }

  /// Records the starting element and returns whether it is selected by the pointer. That is, whether it is the
  /// element itself or an element contained within it.
  fn start_element(&mut self, start: &StartElementEventRef<'_>) -> bool {
    let depth = self.counts.len().saturating_sub(1);
    // The document's own count exists from the start and is never removed by an end element.
    if let Some(count) = self.counts.last_mut() {
      *count += 1;
    }
    self.counts.push(0);
    if !self.searching {
      return self.selected.is_some();
    }

    let base = match &self.scheme.id {
      None => 0,
      Some(id) => match self.anchor {
        Some(anchor) => anchor + 1,
        None if self.has_id(start, id) => {
          self.anchor = Some(depth);
          depth + 1
        }
        None => return false,
      },
    };
    let target =
      depth + 1 == base + self.scheme.steps.len() && self.counts.get(base..=depth) == Some(&self.scheme.steps[..]);
    if target {
      // Set the internal state to indicate that the element pointed to by the pointer has been found.
      self.searching = false;
      self.selected = Some(depth);
    }
    target
  }

  /// Records the ending element and returns whether it is selected by the pointer
  fn end_element(&mut self) -> bool {
    let depth = self.counts.len().saturating_sub(2);
    // Even if the end element appears without a corresponding start element within the event sequence being processed,
    // the count for the document itself is maintained.
    if self.counts.len() > 1 {
      self.counts.pop();
    }
    if self.anchor == Some(depth) {
      // When an element with the anchor ID is detected but the element pointed to is not found before exiting the
      // anchor element, the internal state is set to prevent any further selection. IDs are unique, so it is assumed
      // that no subsequent elements with the same anchor ID exist.
      self.anchor = None;
      self.searching = false;
    }
    match self.selected {
      Some(selected) if selected == depth => {
        self.selected = None;
        true
      }
      Some(_) => true,
      None => false,
    }
  }
}

impl SchemeSelector for ElementSelector<'_> {
  fn filter(&mut self, event: &EventRef<'_>) -> bool {
    match event {
      EventRef::Doctype(doctype) => {
        self.read_doctype(doctype);
        false
      }
      EventRef::StartElement(start) => self.start_element(start),
      EventRef::EndElement(_) => self.end_element(),
      EventRef::StartDocument | EventRef::EndDocument => false,
      _ => self.selected.is_some(),
    }
  }
}
