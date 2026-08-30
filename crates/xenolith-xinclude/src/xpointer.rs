//! XPointer, as far as XInclude uses it: selecting one element of a resource.
//!
//! Three forms are handled. A **shorthand pointer** is a bare `NCName` that names an element by
//! its ID. The **`element()` scheme** walks a child sequence — `element(id/2/1)` from the
//! element with that ID, or `element(/1/2)` from the root, each step the position of a child
//! *element*. The **`xmlns()` scheme** declares a prefix; it is parsed (a scheme-based pointer
//! may carry it) but not needed by `element()`, which addresses by position, not name.
//!
//! A scheme-based pointer is a sequence of parts tried in order; the first that finds an element
//! wins. The full `xpointer()` scheme (XPath) is not supported.

use xenolith_core::chars::is_ncname;
use xenolith_dom::{Document, NodeId, NodeType};

/// Selects the element a pointer identifies in `doc`, if any.
pub(crate) fn select(doc: &Document, xpointer: &str) -> Option<NodeId> {
  let pointer = xpointer.trim();
  // A bare NCName is a shorthand pointer: an element with that ID.
  if is_ncname(pointer) {
    return doc.get_element_by_id(pointer);
  }
  // Otherwise a scheme-based pointer: try each part, first match wins.
  for (scheme, data) in scheme_parts(pointer)? {
    // `xmlns` and any unknown scheme contribute nothing to an `element()` selection.
    if scheme == "element" {
      if let Some(node) = parse_element_scheme(&data).and_then(|element| eval_element(doc, &element)) {
        return Some(node);
      }
    }
  }
  None
}

/// The `element()` scheme's data: an optional starting ID, then a child sequence of positions.
struct ElementScheme {
  id: Option<String>,
  steps: Vec<usize>,
}

/// Splits a scheme-based pointer into `(scheme, data)` parts, honouring `^`-escaped and nested
/// parentheses in the data.
fn scheme_parts(pointer: &str) -> Option<Vec<(String, String)>> {
  let mut parts = Vec::new();
  let mut rest = pointer.trim();
  while !rest.is_empty() {
    let open = rest.find('(')?;
    let scheme = rest[..open].trim().to_owned();
    if scheme.is_empty() {
      return None;
    }
    let data = &rest[open + 1..];
    let end = scheme_data_end(data)?;
    parts.push((scheme, data[..end].to_owned()));
    rest = data[end + 1..].trim_start();
  }
  Some(parts)
}

/// The byte index of the `)` that closes scheme data starting just after its `(`.
fn scheme_data_end(data: &str) -> Option<usize> {
  let bytes = data.as_bytes();
  let mut depth = 1usize;
  let mut i = 0;
  while i < bytes.len() {
    match bytes[i] {
      // `^` escapes the next character, so an escaped parenthesis does not nest.
      b'^' => i += 1,
      b'(' => depth += 1,
      b')' => {
        depth -= 1;
        if depth == 0 {
          return Some(i);
        }
      }
      _ => {}
    }
    i += 1;
  }
  None
}

/// Parses `element()` scheme data: `NCName ChildSequence?` or a bare `ChildSequence`.
fn parse_element_scheme(data: &str) -> Option<ElementScheme> {
  let data = data.trim();
  if let Some(sequence) = data.strip_prefix('/') {
    // A child sequence from the root, e.g. `/1/2`.
    Some(ElementScheme { id: None, steps: parse_child_sequence(sequence)? })
  } else {
    // An ID, optionally followed by a child sequence, e.g. `chap/2`.
    let (id, sequence) = match data.split_once('/') {
      Some((id, sequence)) => (id, Some(sequence)),
      None => (data, None),
    };
    if !is_ncname(id) {
      return None;
    }
    let steps = match sequence {
      Some(sequence) => parse_child_sequence(sequence)?,
      None => Vec::new(),
    };
    Some(ElementScheme { id: Some(id.to_owned()), steps })
  }
}

/// Parses a `/`-separated run of positive positions (the `/` prefix already removed).
fn parse_child_sequence(sequence: &str) -> Option<Vec<usize>> {
  let mut steps = Vec::new();
  for part in sequence.split('/') {
    let position: usize = part.parse().ok()?;
    if position == 0 {
      return None;
    }
    steps.push(position);
  }
  Some(steps)
}

/// Evaluates an `element()` scheme against the document.
fn eval_element(doc: &Document, scheme: &ElementScheme) -> Option<NodeId> {
  let mut current = match &scheme.id {
    Some(id) => doc.get_element_by_id(id)?,
    None => doc.root(),
  };
  for &step in &scheme.steps {
    current = nth_child_element(doc, current, step)?;
  }
  // A pointer with only an ID lands on that element; one with a sequence lands on an element
  // too. The document root (no ID, no steps) is not a valid target.
  (doc.node_type(current) == NodeType::Element).then_some(current)
}

/// The `position`-th child *element* of `node`, counting from 1.
fn nth_child_element(doc: &Document, node: NodeId, position: usize) -> Option<NodeId> {
  doc.children(node).filter(|&child| doc.node_type(child) == NodeType::Element).nth(position - 1)
}
