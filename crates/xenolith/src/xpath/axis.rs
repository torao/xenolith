//! Traversal for the thirteen XPath axes.
//!
//! This module visits the nodes of each axis in its XPath axis order: forward axes use document order, and reverse
//! axes (`ancestor`, `ancestor-or-self`, `preceding`, and `preceding-sibling`) use reverse document order. Predicates
//! count positions in that order (XPath 1.0 §2.4), so callers can assign positions as they receive the nodes.

use crate::error::Result;
use crate::xpath::ast::Axis;
use crate::xpath::model::{Model, NodeKind};

/// Passes the nodes selected by `axis` from `node` to `visit`, in XPath axis order, stopping at the first error it
/// returns.
///
/// This function passes each selected node to `visit`. Callers can test nodes as they arrive and return an error to
/// stop traversal early. Some [`Model`] methods return vectors of nodes, but `walk` does not build an additional list
/// containing the full axis result. Its callback interface also lets this function combine traversals, as it does for
/// `ancestor-or-self` and `following`.
pub(crate) fn walk<M: Model>(
  model: &M,
  node: M::Node,
  axis: Axis,
  visit: &mut impl FnMut(M::Node) -> Result<()>,
) -> Result<()> {
  match axis {
    Axis::SelfAxis => visit(node),
    Axis::Child => model.children(node).into_iter().try_for_each(visit),
    Axis::Parent => model.parent(node).into_iter().try_for_each(visit),
    Axis::Attribute => model.attributes(node).into_iter().try_for_each(visit),
    Axis::Namespace => model.namespaces(node).into_iter().try_for_each(visit),
    Axis::Descendant => descendants(model, node, visit),
    Axis::DescendantOrSelf => {
      visit(node)?;
      descendants(model, node, visit)
    }
    Axis::Ancestor => ancestors(model, node, visit),
    Axis::AncestorOrSelf => {
      // Reverse document order, so the node itself comes before its ancestors.
      visit(node)?;
      ancestors(model, node, visit)
    }
    Axis::FollowingSibling => following_siblings(model, node).into_iter().try_for_each(visit),
    Axis::PrecedingSibling => preceding_siblings(model, node).into_iter().try_for_each(visit),
    Axis::Following => following(model, node, visit),
    Axis::Preceding => preceding(model, node, visit),
  }
}

/// Passes all descendants of `node` to `visit` in document order.
///
/// This function uses an explicit stack instead of recursion because a constructed document can have arbitrary depth.
/// It pushes each node's children in reverse order, so it visits the first child first and visits each child's
/// descendants before moving to the next sibling. This follows the same traversal strategy as
/// [`dom`](crate::dom#traversal).
fn descendants<M: Model>(model: &M, node: M::Node, visit: &mut impl FnMut(M::Node) -> Result<()>) -> Result<()> {
  let mut pending: Vec<M::Node> = model.children(node).into_iter().rev().collect();
  while let Some(next) = pending.pop() {
    visit(next)?;
    pending.extend(model.children(next).into_iter().rev());
  }
  Ok(())
}

/// Passes a node's ancestors to `visit`, starting with its parent and proceeding outward in reverse document order.
fn ancestors<M: Model>(model: &M, node: M::Node, visit: &mut impl FnMut(M::Node) -> Result<()>) -> Result<()> {
  let mut current = model.parent(node);
  while let Some(ancestor) = current {
    visit(ancestor)?;
    current = model.parent(ancestor);
  }
  Ok(())
}

/// Returns a node's parent's children and the node's index among them, if it is one of those children.
///
/// Attribute and namespace nodes have a parent but are not children in the XPath data model, so this function returns
/// `None` for them and the sibling axes select no nodes.
fn siblings_and_index<M: Model>(model: &M, node: M::Node) -> Option<(Vec<M::Node>, usize)> {
  let siblings = model.children(model.parent(node)?);
  let index = siblings.iter().position(|sibling| *sibling == node)?;
  Some((siblings, index))
}

/// The siblings after `node`, in document order.
fn following_siblings<M: Model>(model: &M, node: M::Node) -> Vec<M::Node> {
  let Some((mut siblings, index)) = siblings_and_index(model, node) else { return Vec::new() };
  siblings.split_off(index + 1)
}

/// The siblings before `node`, nearest first: reverse document order.
fn preceding_siblings<M: Model>(model: &M, node: M::Node) -> Vec<M::Node> {
  let Some((mut siblings, index)) = siblings_and_index(model, node) else { return Vec::new() };
  siblings.truncate(index);
  siblings.reverse();
  siblings
}

/// Passes the nodes after `node` in document order to `visit`, excluding its descendants and all attribute and
/// namespace nodes.
///
/// This function moves from `node` toward the root, passing each node's following siblings and their descendants to
/// `visit`. Following siblings at a lower level precede following siblings at a higher level in document order, so the
/// traversal emits nodes in document order without sorting. For an attribute or namespace node, the XPath following
/// axis also includes the owning element's descendants; this function passes those descendants before moving to the
/// element.
fn following<M: Model>(model: &M, node: M::Node, visit: &mut impl FnMut(M::Node) -> Result<()>) -> Result<()> {
  let mut current = node;
  if matches!(model.kind(node), NodeKind::Attribute | NodeKind::Namespace) {
    if let Some(element) = model.parent(node) {
      descendants(model, element, visit)?;
      current = element;
    }
  }
  loop {
    for sibling in following_siblings(model, current) {
      visit(sibling)?;
      descendants(model, sibling, visit)?;
    }
    match model.parent(current) {
      Some(parent) => current = parent,
      None => return Ok(()),
    }
  }
}

/// Passes the nodes before `node` in reverse document order to `visit`, excluding its ancestors and all attribute and
/// namespace nodes.
///
/// This function moves from `node` toward the root, visiting each node's preceding siblings from nearest to farthest.
/// For each sibling, it passes the sibling's descendants in reverse document order, then passes the sibling itself.
/// Preceding sibling subtrees at a lower level occur later in document order than those at a higher level, so this
/// traversal emits the axis in reverse document order without sorting. The function visits siblings only, so it never
/// passes an ancestor.
fn preceding<M: Model>(model: &M, node: M::Node, visit: &mut impl FnMut(M::Node) -> Result<()>) -> Result<()> {
  let mut current = node;
  loop {
    for sibling in preceding_siblings(model, current) {
      let mut subtree = Vec::new();
      descendants(model, sibling, &mut |descendant| {
        subtree.push(descendant);
        Ok(())
      })?;
      subtree.into_iter().rev().try_for_each(&mut *visit)?;
      visit(sibling)?;
    }
    match model.parent(current) {
      Some(parent) => current = parent,
      None => return Ok(()),
    }
  }
}
