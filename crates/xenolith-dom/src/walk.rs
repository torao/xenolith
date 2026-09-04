//! Walking a subtree: the cursor behind every traversal in the crate.

use crate::node::NodeSlot;
use crate::{Document, NodeId};

/// Which side of a node a [`Walk`] is reporting.
///
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Visit {
  /// The walk has reached the node, before any of its children.
  Enter,
  /// The walk has passed the node's children and is leaving it.
  Leave,
}

/// An iterator to walk over a subtree in document order, reporting every node twice: once entering it, once leaving it.
///
/// This is created by [`Document::walk`]. It yields `(`[`Visit`]`, `[`NodeId`]`)`, and the two sides of a node always
/// pair up and nest properly, so a walk of `<a><b/></a>` reports `<a>`, `<b>`, `</b>`, `</a>`. Keeping the
/// [`Enter`](Visit::Enter) items alone gives preorder, the order the nodes appear in the document. The walk stays
/// inside the subtree it started in and ends once it leaves that node, so it never reports a following sibling of the
/// start.
///
/// One that only reads nodes, counting or gathering text, keeps the `Enter` items. One that has to close what it
/// opened, a serializer or an event source, acts on `Leave` as well. This two-way reporting suits either kind of
/// consumer.
///
/// This implementation follows links between nodes in the document rather than recursing, so it keeps one fixed-size cursor and doesn't consume heap or stack memory, regardless of the tree's shape. Nesting depth is therefore safe to
/// leave to the input, which matters for a document from an untrusted source (see [traversal depth](crate#traversal)).
/// Being an iterator, it also leaves control flow to the caller, so stopping partway is an ordinary `break`.
///
/// # Examples
///
/// Gathering the node names in document order. A walk reports every kind of node, so the text inside the items turns
/// up too, under the name the W3C DOM specification gives character data:
///
/// ```
/// use xenolith_dom::{Document, Visit};
///
/// // <ul><li>one</li><li>two</li></ul>
/// let mut doc = Document::new();
/// let root = doc.create_element("ul")?;
/// for word in ["one", "two"] {
///   let item = doc.create_element("li")?;
///   let text = doc.create_text_node(word);
///   doc.append_child(item, text)?;
///   doc.append_child(root, item)?;
/// }
/// doc.append_child(doc.document_node(), root)?;
///
/// let names: Vec<String> =
///     doc.walk(root).filter(|(visit, _)| *visit == Visit::Enter).map(|(_, node)| doc.node_name(node)).collect();
/// assert_eq!(names, ["ul", "li", "#text", "li", "#text"]);
/// # Ok::<(), xenolith_dom::DomException>(())
/// ```
///
pub struct Walk<'a> {
  doc: &'a Document,
  /// The node the walk started from. Leaving it ends the walk, which is what keeps the walk inside its subtree.
  start: NodeId,
  /// What the next call reports, or `None` once the walk is done.
  next: Option<(Visit, NodeId)>,
}

impl std::fmt::Debug for Walk<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Walk").field("start", &self.start).field("next", &self.next).finish_non_exhaustive()
  }
}

impl<'a> Walk<'a> {
  /// Creates a walk over the subtree rooted at `start`, which it reports first.
  pub(crate) fn new(doc: &'a Document, start: NodeId) -> Self {
    Self { doc, start, next: Some((Visit::Enter, start)) }
  }

  fn slot(&self, id: NodeId) -> &NodeSlot {
    self.doc.slot(id)
  }
}

impl Iterator for Walk<'_> {
  type Item = (Visit, NodeId);

  fn next(&mut self) -> Option<Self::Item> {
    let current = self.next?;
    self.next = match current {
      // Entering a node, descend to its first child. A node with none is left straight away.
      (Visit::Enter, node) => match self.slot(node).first_child {
        Some(child) => Some((Visit::Enter, child)),
        None => Some((Visit::Leave, node)),
      },
      // Leaving the node the walk started from ends it. Otherwise take the next sibling, and where there is none,
      // leave the parent, which every node below the start has.
      (Visit::Leave, node) if node != self.start => match self.slot(node).next_sibling {
        Some(sibling) => Some((Visit::Enter, sibling)),
        None => self.slot(node).parent.map(|parent| (Visit::Leave, parent)),
      },
      (Visit::Leave, _) => None,
    };
    Some(current)
  }
}

// The walk reports nothing more once it is done, so an adapter can rely on that.

impl std::iter::FusedIterator for Walk<'_> {}

#[cfg(test)]
mod tests {
  use super::*;

  /// The walk as a compact string, so the order and the nesting are both readable.
  fn trace(doc: &Document, start: NodeId) -> String {
    doc
      .walk(start)
      .map(|(visit, node)| {
        let name = doc.node_name(node);
        match visit {
          Visit::Enter => format!("<{name}>"),
          Visit::Leave => format!("</{name}>"),
        }
      })
      .collect()
  }

  #[test]
  fn reports_both_sides_of_every_node_in_order() {
    let mut doc = Document::new();
    let r = doc.create_element("r").unwrap();
    let a = doc.create_text_node("a");
    let b = doc.create_element("b").unwrap();
    let c = doc.create_element("c").unwrap();
    doc.append_child(r, a).unwrap();
    doc.append_child(r, b).unwrap();
    doc.append_child(b, c).unwrap();
    doc.append_child(doc.document_node(), r).unwrap();

    assert_eq!(trace(&doc, r), "<r><#text></#text><b><c></c></b></r>");
  }

  #[test]
  fn a_leaf_is_entered_and_left() {
    // The element is never attached, so this also covers walking a subtree that hangs outside the tree.
    let mut doc = Document::new();
    let a = doc.create_element("a").unwrap();
    assert_eq!(trace(&doc, a), "<a></a>");
  }

  #[test]
  fn the_walk_stays_inside_the_subtree_it_started_from() {
    // `b` has a following sibling, which the walk must not reach when it starts at `b`.
    let mut doc = Document::new();
    let r = doc.create_element("r").unwrap();
    let b = doc.create_element("b").unwrap();
    let after = doc.create_element("after").unwrap();
    doc.append_child(r, b).unwrap();
    doc.append_child(r, after).unwrap();
    doc.append_child(doc.document_node(), r).unwrap();

    assert_eq!(trace(&doc, b), "<b></b>", "the walk ends on leaving its own start");
  }

  #[test]
  fn a_finished_walk_stays_finished() {
    let mut doc = Document::new();
    let a = doc.create_element("a").unwrap();
    let mut walk = doc.walk(a);
    assert_eq!(walk.by_ref().count(), 2, "entering and leaving the one node");
    assert_eq!(walk.next(), None);
    assert_eq!(walk.next(), None, "it reports nothing more, as a fused iterator");
  }

  #[test]
  #[should_panic(expected = "another document")]
  fn walking_with_a_node_from_another_document_panics() {
    let a = Document::new();
    let mut b = Document::new();
    let in_b = b.create_element("b").unwrap();
    // The check is at the call, not part-way through the walk.
    let _ = a.walk(in_b);
  }
}
