//! Evaluating an expression tree against a tree of nodes.

use std::ptr;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::xpath::ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
use crate::xpath::model::{Model, NodeKind, make_node_set};
use crate::xpath::object::{Object, string_to_number};
use crate::xpath::state::EvalState;
use crate::xpath::{XPath, axis, functions, object};

/// Evaluates an expression in a context.
///
/// Each match arm delegates its work to a helper. Recursive evaluation then keeps this function's stack frame small,
/// especially in debug builds where the compiler may reserve space for locals from every match arm.
pub(crate) fn eval<M: Model>(expr: &Expr, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  match expr {
    Expr::Literal(value) => Ok(Object::String(value.clone())),
    Expr::Number(value) => Ok(Object::Number(*value)),
    Expr::Negate(inner) => negate(inner, state),
    Expr::Binary { op, left, right, at } => binary(*op, left, right, *at, state),
    Expr::Path(path) => path_object(path, state),
    Expr::Filter { expr, predicates, at } => filter_object(expr, predicates, *at, state),
    Expr::Variable { prefix, local, at } => variable(prefix.as_deref(), local, *at, state),
    Expr::Function { prefix, local, arguments, at } => call(prefix.as_deref(), local, arguments, *at, state),
  }
}

/// Evaluates `-inner`.
fn negate<M: Model>(inner: &Expr, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  Ok(Object::Number(-eval(inner, state)?.number(state.model)))
}

/// Evaluates a path to its node-set.
fn path_object<M: Model>(path: &Path, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  Ok(Object::NodeSet(eval_path(path, state)?))
}

/// Evaluates `expr` as a node-set and applies `predicates` to that set. `at` is the byte offset used to locate a
/// node-set type error.
fn filter_object<M: Model>(
  expr: &Expr,
  predicates: &[Expr],
  at: usize,
  state: &EvalState<'_, M>,
) -> Result<Object<M::Node>> {
  let nodes =
    node_set(eval(expr, state)?, "a predicate can only filter a node-set").map_err(|e| state.locate(e, at))?;
  Ok(Object::NodeSet(apply_predicates(nodes, predicates, state)?))
}

/// Evaluates the arguments of a function call, whose name is at `at`, then calls it.
fn call<M: Model>(
  prefix: Option<&str>,
  local: &str,
  arguments: &[Expr],
  at: usize,
  state: &EvalState<'_, M>,
) -> Result<Object<M::Node>> {
  let mut values = Vec::with_capacity(arguments.len());
  for argument in arguments {
    values.push(eval(argument, state)?);
  }
  functions::call(prefix, local, values, state).map_err(|error| state.locate(error, at))
}

/// Returns the value of a variable, whose reference is at `at`: the value passed to the evaluation, or else the value
/// of the context's definition, as an XSLT local variable hides a top-level one. The parser has checked the prefix of
/// its name already.
fn variable<M: Model>(
  prefix: Option<&str>,
  local: &str,
  at: usize,
  state: &EvalState<'_, M>,
) -> Result<Object<M::Node>> {
  let name = || prefix.map_or_else(|| local.to_owned(), |prefix| format!("{prefix}:{local}"));
  let namespace = match prefix {
    Some(prefix) => resolve_prefix(prefix, state).map_err(|e| state.locate(e, at))?,
    None => "",
  };
  if let Some(value) = state.variables.get(namespace, local) {
    return remembered(ptr::from_ref(value).cast(), state, || {
      object(value, state.model).ok_or_else(|| {
        let message =
          format!("the variable \"${}\" holds a node that is not in the XPath tree of the document", name());
        state.locate(error(message), at)
      })
    });
  }
  if let Some(definition) = state.context.variable(namespace, local) {
    return remembered(Arc::as_ptr(definition).cast(), state, || defined(&definition.xpath, state));
  }
  Err(state.locate(error(format!("the variable \"${}\" is not bound in this evaluation", name())), at))
}

/// Evaluates the definition of a variable, with the document node as the context node, as XSLT evaluates a top-level
/// variable. An error is located in the definition, at its start if nowhere else.
fn defined<M: Model>(definition: &XPath, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  let root = state.model.root(state.node);
  let inner = EvalState::new(state.model, root, definition, state.variables, state.values);
  eval(&definition.expr, &inner)
    .map_err(|error| if error.location().is_unknown() { error.at(definition.location.clone()) } else { error })
}

/// Returns the value remembered for `key` in this evaluation, or computes and remembers it.
fn remembered<M: Model>(
  key: *const (),
  state: &EvalState<'_, M>,
  compute: impl FnOnce() -> Result<Object<M::Node>>,
) -> Result<Object<M::Node>> {
  if let Some(value) = state.values.borrow().get(&key) {
    return Ok(value.clone());
  }
  // Not borrowed while computing: a definition may refer to the variables defined before it.
  let value = compute()?;
  state.values.borrow_mut().insert(key, value.clone());
  Ok(value)
}

// --- Operators --------------------------------------------------------------------------------

/// Evaluates a binary expression, whose operator is at `at`. Each kind of operator is evaluated in a function of its
/// own, as [`eval`] explains.
fn binary<M: Model>(
  op: BinaryOp,
  left: &Expr,
  right: &Expr,
  at: usize,
  state: &EvalState<'_, M>,
) -> Result<Object<M::Node>> {
  match op {
    BinaryOp::Or => or(left, right, state),
    BinaryOp::And => and(left, right, state),
    BinaryOp::Union => union(left, right, at, state),
    BinaryOp::Equal
    | BinaryOp::NotEqual
    | BinaryOp::Less
    | BinaryOp::LessEqual
    | BinaryOp::Greater
    | BinaryOp::GreaterEqual => comparison(op, left, right, state),
    BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => {
      arithmetic(op, left, right, state)
    }
  }
}

/// Evaluates `left or right`, which stops as soon as the answer is settled.
fn or<M: Model>(left: &Expr, right: &Expr, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  if eval(left, state)?.boolean() {
    return Ok(Object::Boolean(true));
  }
  Ok(Object::Boolean(eval(right, state)?.boolean()))
}

/// Evaluates `left and right`, which stops as soon as the answer is settled.
fn and<M: Model>(left: &Expr, right: &Expr, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  if !eval(left, state)?.boolean() {
    return Ok(Object::Boolean(false));
  }
  Ok(Object::Boolean(eval(right, state)?.boolean()))
}

/// Evaluates `left | right`, whose `|` is at `at`.
fn union<M: Model>(left: &Expr, right: &Expr, at: usize, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  let locate = |error| state.locate(error, at);
  let mut nodes = node_set(eval(left, state)?, "a union joins node-sets").map_err(locate)?;
  nodes.extend(node_set(eval(right, state)?, "a union joins node-sets").map_err(locate)?);
  make_node_set(state.model, &mut nodes);
  Ok(Object::NodeSet(nodes))
}

/// Evaluates a comparison.
fn comparison<M: Model>(op: BinaryOp, left: &Expr, right: &Expr, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  let left = eval(left, state)?;
  let right = eval(right, state)?;
  Ok(Object::Boolean(compare(op, &left, &right, state.model)))
}

/// Evaluates an arithmetic operator.
fn arithmetic<M: Model>(op: BinaryOp, left: &Expr, right: &Expr, state: &EvalState<'_, M>) -> Result<Object<M::Node>> {
  let a = eval(left, state)?.number(state.model);
  let b = eval(right, state)?.number(state.model);
  Ok(Object::Number(match op {
    BinaryOp::Add => a + b,
    BinaryOp::Subtract => a - b,
    BinaryOp::Multiply => a * b,
    BinaryOp::Divide => a / b,
    // `mod` is the remainder of a truncating division, which is what Rust's `%` gives.
    BinaryOp::Modulo => a % b,
    _ => unreachable!("only the arithmetic operators reach here"),
  }))
}

/// Compares two objects (XPath 1.0 §3.4).
///
/// When an operand is a node-set, the comparison checks its members and succeeds if any applicable pair satisfies the
/// operator. Relational operators compare numbers; `=` and `!=` choose boolean, numeric, or string comparison according
/// to the operand types.
fn compare<M: Model>(op: BinaryOp, left: &Object<M::Node>, right: &Object<M::Node>, model: &M) -> bool {
  match (left, right) {
    (Object::NodeSet(a), Object::NodeSet(b)) => {
      let equality = is_equality(op);
      a.iter().any(|left| {
        let left = model.string_value(*left);
        b.iter().any(|right| {
          let right = model.string_value(*right);
          if equality { compare_strings(op, &left, &right) } else { compare_numbers(op, &left, &right) }
        })
      })
    }
    (Object::NodeSet(nodes), other) => node_set_compare(op, nodes, other, model),
    (other, Object::NodeSet(nodes)) => node_set_compare(flip(op), nodes, other, model),
    _ => scalar_compare(op, left, right, model),
  }
}

/// Compares a node-set against an object that is not one.
fn node_set_compare<M: Model>(op: BinaryOp, nodes: &[M::Node], other: &Object<M::Node>, model: &M) -> bool {
  // For `=` and `!=` a boolean makes the node-set a boolean too; every other case looks at the nodes one at a time.
  if is_equality(op) {
    if let Object::Boolean(other) = other {
      let present = !nodes.is_empty();
      return if op == BinaryOp::Equal { present == *other } else { present != *other };
    }
  }
  let other_string = other.string(model);
  nodes.iter().any(|node| {
    let value = model.string_value(*node);
    match (is_equality(op), other) {
      (true, Object::String(text)) => compare_strings(op, &value, text),
      // A number, or any relational comparison, is settled as numbers.
      _ => compare_numbers(op, &value, &other_string),
    }
  })
}

/// Compares two objects, neither of which is a node-set.
fn scalar_compare<M: Model>(op: BinaryOp, left: &Object<M::Node>, right: &Object<M::Node>, model: &M) -> bool {
  if !is_equality(op) {
    return numbers(op, left.number(model), right.number(model));
  }
  let equal = if matches!(left, Object::Boolean(_)) || matches!(right, Object::Boolean(_)) {
    left.boolean() == right.boolean()
  } else if matches!(left, Object::Number(_)) || matches!(right, Object::Number(_)) {
    left.number(model) == right.number(model)
  } else {
    left.string(model) == right.string(model)
  };
  if op == BinaryOp::Equal { equal } else { !equal }
}

fn compare_strings(op: BinaryOp, left: &str, right: &str) -> bool {
  if op == BinaryOp::Equal { left == right } else { left != right }
}

fn compare_numbers(op: BinaryOp, left: &str, right: &str) -> bool {
  numbers(op, string_to_number(left), string_to_number(right))
}

fn numbers(op: BinaryOp, a: f64, b: f64) -> bool {
  match op {
    BinaryOp::Equal => a == b,
    BinaryOp::NotEqual => a != b,
    BinaryOp::Less => a < b,
    BinaryOp::LessEqual => a <= b,
    BinaryOp::Greater => a > b,
    BinaryOp::GreaterEqual => a >= b,
    _ => unreachable!("only the comparison operators reach here"),
  }
}

const fn is_equality(op: BinaryOp) -> bool {
  matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
}

/// The operator with its operands the other way round, for putting a node-set on the left.
const fn flip(op: BinaryOp) -> BinaryOp {
  match op {
    BinaryOp::Less => BinaryOp::Greater,
    BinaryOp::LessEqual => BinaryOp::GreaterEqual,
    BinaryOp::Greater => BinaryOp::Less,
    BinaryOp::GreaterEqual => BinaryOp::LessEqual,
    other => other,
  }
}

// --- Paths ------------------------------------------------------------------------------------

/// Walks a path, returning its node-set in document order.
fn eval_path<M: Model>(path: &Path, state: &EvalState<'_, M>) -> Result<Vec<M::Node>> {
  let mut nodes = match &path.start {
    PathStart::Root => vec![state.model.root(state.node)],
    PathStart::Context => vec![state.node],
    PathStart::Expr(expr) => node_set(eval(expr, state)?, "a path can only continue from a node-set")
      .map_err(|error| state.locate(error, path.at))?,
  };
  for step in &path.steps {
    nodes = eval_step(step, &nodes, state)?;
  }
  Ok(nodes)
}

/// Applies one step to every node a path has reached so far.
fn eval_step<M: Model>(step: &Step, from: &[M::Node], state: &EvalState<'_, M>) -> Result<Vec<M::Node>> {
  let mut result = Vec::new();
  for node in from {
    // The axis is walked in its own order, since that is the order predicates count in.
    let mut selected = Vec::new();
    axis::walk(state.model, *node, step.axis, &mut |candidate| {
      if matches(state, candidate, step.axis, &step.node_test)? {
        selected.push(candidate);
      }
      Ok(())
    })?;
    for predicate in &step.predicates {
      selected = filter(selected, predicate, state)?;
    }
    result.extend(selected);
  }
  // The step's result is a node-set, however the axis reached it.
  make_node_set(state.model, &mut result);
  Ok(result)
}

/// Applies the predicates of a filter expression, whose nodes are in document order.
fn apply_predicates<M: Model>(
  mut nodes: Vec<M::Node>,
  predicates: &[Expr],
  state: &EvalState<'_, M>,
) -> Result<Vec<M::Node>> {
  make_node_set(state.model, &mut nodes);
  for predicate in predicates {
    nodes = filter(nodes, predicate, state)?;
  }
  Ok(nodes)
}

/// Keeps the nodes a predicate holds for, each evaluated as the context node.
///
/// A predicate that yields a number is a test on the position (XPath 1.0 §3.3): `[2]` keeps the second node. Anything
/// else is taken as a boolean.
fn filter<M: Model>(nodes: Vec<M::Node>, predicate: &Expr, state: &EvalState<'_, M>) -> Result<Vec<M::Node>> {
  let size = nodes.len();
  let mut kept = Vec::new();
  for (index, node) in nodes.into_iter().enumerate() {
    let position = index + 1;
    let inner = state.at(node, position, size);
    let keep = match eval(predicate, &inner)? {
      Object::Number(value) => value == position as f64,
      other => other.boolean(),
    };
    if keep {
      kept.push(node);
    }
  }
  Ok(kept)
}

/// Whether a node passes a step's node test.
fn matches<M: Model>(state: &EvalState<'_, M>, node: M::Node, axis: Axis, test: &NodeTest) -> Result<bool> {
  let kind = state.model.kind(node);
  let name_of = || state.model.expanded_name(node);
  Ok(match test {
    NodeTest::Node => true,
    NodeTest::Text => kind == NodeKind::Text,
    NodeTest::Comment => kind == NodeKind::Comment,
    NodeTest::ProcessingInstruction(None) => kind == NodeKind::ProcessingInstruction,
    NodeTest::ProcessingInstruction(Some(target)) => {
      kind == NodeKind::ProcessingInstruction && name_of().is_some_and(|name| name.local == *target)
    }
    // A name test also restricts to the axis's principal node type, so `@*` is attributes only and `*` on any other
    // axis is elements only.
    NodeTest::Name(_) if kind != principal_kind(axis) => false,
    NodeTest::Name(NameTest::Any) => true,
    NodeTest::Name(NameTest::AnyLocal(prefix)) => {
      let namespace = resolve_prefix(prefix, state)?;
      name_of().is_some_and(|name| name.namespace.as_deref() == Some(namespace))
    }
    NodeTest::Name(NameTest::Exact { prefix, local }) => {
      let namespace = match prefix {
        Some(prefix) => Some(resolve_prefix(prefix, state)?),
        None => None,
      };
      name_of().is_some_and(|name| name.namespace.as_deref() == namespace && name.local == *local)
    }
  })
}

/// The node type a name test selects on an axis: attributes on `attribute`, namespace nodes on `namespace`, elements
/// everywhere else.
const fn principal_kind(axis: Axis) -> NodeKind {
  match axis {
    Axis::Attribute => NodeKind::Attribute,
    Axis::Namespace => NodeKind::Namespace,
    _ => NodeKind::Element,
  }
}

/// The namespace a prefix in the expression stands for.
fn resolve_prefix<'a, M: Model>(prefix: &str, state: &EvalState<'a, M>) -> Result<&'a str> {
  state.context.namespace_of(prefix).ok_or_else(|| error(format!("the prefix \"{prefix}\" is not bound")))
}

/// Takes the nodes out of an object, or says what was found instead.
fn node_set<N>(object: Object<N>, wanted: &str) -> Result<Vec<N>> {
  match object {
    Object::NodeSet(nodes) => Ok(nodes),
    other => Err(error(format!("{wanted}, but found {}", other.type_name()))),
  }
}

fn error(message: impl Into<String>) -> Error {
  Error::xpath(message.into())
}
