//! Walking the thirteen axes.
//!
//! Each axis is returned in **axis order**: document order for a forward axis, reverse document
//! order for a reverse one (`ancestor`, `ancestor-or-self`, `preceding`, `preceding-sibling`).
//! That is the order predicates count positions in (XPath 1.0 §2.4), so a caller numbers the
//! nodes as they come and never has to ask which direction the axis runs.

use xylogue_xdm::Model;

use crate::ast::Axis;
use crate::context::normalize;

/// The nodes an axis reaches from `node`, in axis order.
pub(crate) fn nodes<M: Model>(model: &M, node: M::Node, axis: Axis) -> Vec<M::Node> {
  match axis {
    Axis::SelfAxis => vec![node],
    Axis::Child => model.children(node),
    Axis::Parent => model.parent(node).into_iter().collect(),
    Axis::Attribute => model.attributes(node),
    Axis::Namespace => model.namespaces(node),
    Axis::Descendant => {
      let mut result = Vec::new();
      push_descendants(model, node, &mut result);
      result
    }
    Axis::DescendantOrSelf => {
      let mut result = vec![node];
      push_descendants(model, node, &mut result);
      result
    }
    Axis::Ancestor => ancestors(model, node),
    Axis::AncestorOrSelf => {
      // Reverse document order, so the node itself comes before its ancestors.
      let mut result = vec![node];
      result.extend(ancestors(model, node));
      result
    }
    Axis::FollowingSibling => {
      let (siblings, index) = siblings_and_index(model, node);
      index.map(|index| siblings[index + 1..].to_vec()).unwrap_or_default()
    }
    Axis::PrecedingSibling => {
      let (siblings, index) = siblings_and_index(model, node);
      index
        .map(|index| {
          let mut before = siblings[..index].to_vec();
          before.reverse();
          before
        })
        .unwrap_or_default()
    }
    Axis::Following => following(model, node),
    Axis::Preceding => preceding(model, node),
  }
}

/// The descendants of a node, in document order.
fn push_descendants<M: Model>(model: &M, node: M::Node, out: &mut Vec<M::Node>) {
  for child in model.children(node) {
    out.push(child);
    push_descendants(model, child, out);
  }
}

/// The ancestors of a node, nearest first — reverse document order.
fn ancestors<M: Model>(model: &M, node: M::Node) -> Vec<M::Node> {
  let mut result = Vec::new();
  let mut current = model.parent(node);
  while let Some(ancestor) = current {
    result.push(ancestor);
    current = model.parent(ancestor);
  }
  result
}

/// The children of a node's parent, and where the node sits among them.
///
/// An attribute or namespace node has a parent but is not one of its children, so its index is
/// `None` — which is why neither has siblings.
fn siblings_and_index<M: Model>(model: &M, node: M::Node) -> (Vec<M::Node>, Option<usize>) {
  let Some(parent) = model.parent(node) else { return (Vec::new(), None) };
  let siblings = model.children(parent);
  let index = siblings.iter().position(|sibling| *sibling == node);
  (siblings, index)
}

/// Everything after the node in document order, bar its descendants — and, since they are never
/// children, bar attribute and namespace nodes.
fn following<M: Model>(model: &M, node: M::Node) -> Vec<M::Node> {
  let mut result = Vec::new();
  let mut current = Some(node);
  while let Some(step) = current {
    for sibling in nodes(model, step, Axis::FollowingSibling) {
      result.push(sibling);
      push_descendants(model, sibling, &mut result);
    }
    current = model.parent(step);
  }
  // Gathered from the node upwards, so the levels arrive out of order.
  normalize(model, &mut result);
  result
}

/// Everything before the node in document order, bar its ancestors — in reverse document order,
/// since `preceding` is a reverse axis.
fn preceding<M: Model>(model: &M, node: M::Node) -> Vec<M::Node> {
  let mut result = Vec::new();
  let mut current = Some(node);
  while let Some(step) = current {
    // Only the siblings before each ancestor, so the ancestors themselves stay out.
    for sibling in nodes(model, step, Axis::PrecedingSibling) {
      result.push(sibling);
      push_descendants(model, sibling, &mut result);
    }
    current = model.parent(step);
  }
  normalize(model, &mut result);
  result.reverse();
  result
}
