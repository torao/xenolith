//! Walking a subtree: the cursor behind every traversal in the document.

#[cfg(test)]
mod test;

use crate::dom::node::NodeSlot;
use crate::dom::{Document, NodeId};

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
/// This implementation follows links between nodes in the document rather than recursing, so it keeps one fixed-size
/// cursor and doesn't consume heap or stack memory, regardless of the tree's shape. Nesting depth is therefore safe to
/// leave to the input, which matters for a document from an untrusted source (see [traversal depth](crate#traversal)).
/// Being an iterator, it also leaves control flow to the caller, so stopping partway is an ordinary `break`.
///
/// # Examples
///
/// Gathering the node names in document order. A walk reports every kind of node, so the text inside the items turns
/// up too, under the name the W3C DOM specification gives character data:
///
/// ```
/// use xenolith::dom::{Document, Visit};
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
/// # Ok::<(), xenolith::dom::DomException>(())
/// ```
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

impl std::iter::FusedIterator for Walk<'_> {}
