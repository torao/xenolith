//! `http://exslt.org/set` — node-set operations beyond union.
//!
//! XPath 1.0 has `|`, and that is all: no difference, no intersection. This module is what EXSLT
//! adds. Every one of them answers with a node-set, which means document order with each node
//! once, however the arguments were arranged.
//!
//! Two notions of sameness appear here and they are not the same notion. `set:difference`,
//! `set:intersection` and `set:has-same-node` compare nodes by *identity* — the same node, not a
//! node that says the same thing. `set:distinct` compares by *string value*, which is what makes
//! it useful for grouping. Getting those the wrong way round would give answers that look
//! plausible and are wrong, so each says which it uses.
//!
//! # Specifications
//!
//! - [`exslt:sets`](http://exslt.org/set/index.html)

use std::collections::HashSet;

use xenolith_xdm::Model;
use xenolith_xpath::{Context, Functions, Value};

use crate::support::{arity, in_document_order, nodes};

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/set";

/// Adds this module's functions.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>) -> Functions<M> {
  functions
    .with(NAMESPACE, "difference", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("set:difference", &arguments, 2, Some(2))?;
      let first = nodes::<M>("set:difference", &arguments[0])?;
      let second: HashSet<M::Node> = nodes::<M>("set:difference", &arguments[1])?.into_iter().collect();
      let kept = first.into_iter().filter(|node| !second.contains(node)).collect();
      Ok(Value::NodeSet(in_document_order(kept, context)))
    })
    .with(NAMESPACE, "intersection", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("set:intersection", &arguments, 2, Some(2))?;
      let first = nodes::<M>("set:intersection", &arguments[0])?;
      let second: HashSet<M::Node> = nodes::<M>("set:intersection", &arguments[1])?.into_iter().collect();
      let kept = first.into_iter().filter(|node| second.contains(node)).collect();
      Ok(Value::NodeSet(in_document_order(kept, context)))
    })
    .with(NAMESPACE, "distinct", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("set:distinct", &arguments, 1, Some(1))?;
      // By string value, not identity: this is the one that groups.
      let mut seen: HashSet<String> = HashSet::new();
      let mut kept = Vec::new();
      // In document order first, so that of several nodes saying the same thing it is the first
      // that is kept — which is what makes the answer depend on the tree and not on the walk.
      for node in in_document_order(nodes::<M>("set:distinct", &arguments[0])?, context) {
        if seen.insert(context.model.string_value(node)) {
          kept.push(node);
        }
      }
      Ok(Value::NodeSet(kept))
    })
    .with(NAMESPACE, "has-same-node", |arguments: Vec<Value<M::Node>>, _: &Context<'_, M>| {
      arity("set:has-same-node", &arguments, 2, Some(2))?;
      let first: HashSet<M::Node> = nodes::<M>("set:has-same-node", &arguments[0])?.into_iter().collect();
      let second = nodes::<M>("set:has-same-node", &arguments[1])?;
      Ok(Value::Boolean(second.iter().any(|node| first.contains(node))))
    })
    .with(NAMESPACE, "leading", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("set:leading", &arguments, 2, Some(2))?;
      Ok(Value::NodeSet(either_side::<M>(&arguments, context, Side::Before)?))
    })
    .with(NAMESPACE, "trailing", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("set:trailing", &arguments, 2, Some(2))?;
      Ok(Value::NodeSet(either_side::<M>(&arguments, context, Side::After)?))
    })
}

/// Which side of the mark is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
  Before,
  After,
}

/// The nodes of the first set that come before, or after, the first node of the second.
///
/// EXSLT says an empty second node-set means there is nothing to be before or after, and the
/// answer is empty either way.
fn either_side<M: Model>(
  arguments: &[Value<M::Node>],
  context: &Context<'_, M>,
  side: Side,
) -> xenolith_core::error::Result<Vec<M::Node>> {
  let name = if side == Side::Before { "set:leading" } else { "set:trailing" };
  let first = in_document_order(nodes::<M>(name, &arguments[0])?, context);
  let second = in_document_order(nodes::<M>(name, &arguments[1])?, context);
  let Some(mark) = second.first() else { return Ok(Vec::new()) };

  let kept = first
    .into_iter()
    .filter(|node| {
      let order = context.model.document_order(*node, *mark);
      match side {
        Side::Before => order == std::cmp::Ordering::Less,
        Side::After => order == std::cmp::Ordering::Greater,
      }
    })
    .collect();
  Ok(kept)
}
