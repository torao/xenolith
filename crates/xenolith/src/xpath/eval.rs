//! Evaluating an expression tree against a tree of nodes.

use crate::error::{Error, Result};
use crate::xpath::ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
use crate::xpath::context::{Context, namespace_of};
use crate::xpath::model::{Model, NodeKind, make_node_set};
use crate::xpath::object::{Object, string_to_number};
use crate::xpath::{axis, functions};

/// Evaluates an expression in a context.
///
/// Each match arm delegates its work to a helper. Recursive evaluation then keeps this function's stack frame small,
/// especially in debug builds where the compiler may reserve space for locals from every match arm.
pub(crate) fn eval<M: Model>(expr: &Expr, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  match expr {
    Expr::Literal(value) => Ok(Object::String(value.clone())),
    Expr::Number(value) => Ok(Object::Number(*value)),
    Expr::Negate(inner) => negate(inner, context),
    Expr::Binary { op, left, right, at } => binary(*op, left, right, *at, context),
    Expr::Path(path) => path_object(path, context),
    Expr::Filter { expr, predicates, at } => filter_object(expr, predicates, *at, context),
    Expr::Variable { prefix, local, at } => Err(context.locate(variable(prefix.as_deref(), local), *at)),
    Expr::Function { prefix, local, arguments, at } => call(prefix.as_deref(), local, arguments, *at, context),
  }
}

/// Evaluates `-inner`.
fn negate<M: Model>(inner: &Expr, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  Ok(Object::Number(-eval(inner, context)?.number(context.model)))
}

/// Evaluates a path to its node-set.
fn path_object<M: Model>(path: &Path, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  Ok(Object::NodeSet(eval_path(path, context)?))
}

/// Evaluates `expr` as a node-set and applies `predicates` to that set. `at` is the byte offset used to locate a
/// node-set type error.
fn filter_object<M: Model>(
  expr: &Expr,
  predicates: &[Expr],
  at: usize,
  context: &Context<'_, M>,
) -> Result<Object<M::Node>> {
  let nodes =
    node_set(eval(expr, context)?, "a predicate can only filter a node-set").map_err(|e| context.locate(e, at))?;
  Ok(Object::NodeSet(apply_predicates(nodes, predicates, context)?))
}

/// Evaluates the arguments of a function call, whose name is at `at`, then calls it.
fn call<M: Model>(
  prefix: Option<&str>,
  local: &str,
  arguments: &[Expr],
  at: usize,
  context: &Context<'_, M>,
) -> Result<Object<M::Node>> {
  let mut values = Vec::with_capacity(arguments.len());
  for argument in arguments {
    values.push(eval(argument, context)?);
  }
  functions::call(prefix, local, values, context).map_err(|error| context.locate(error, at))
}

/// The error of a reference to a variable. No variable can be bound yet, so a reference to one is always an error;
/// the parser has checked the prefix of its name already.
fn variable(prefix: Option<&str>, local: &str) -> Error {
  let name = prefix.map_or_else(|| local.to_owned(), |prefix| format!("{prefix}:{local}"));
  error(format!("the variable \"${name}\" is not bound in this evaluation"))
}

// --- Operators --------------------------------------------------------------------------------

/// Evaluates a binary expression, whose operator is at `at`. Each kind of operator is evaluated in a function of its
/// own, as [`eval`] explains.
fn binary<M: Model>(
  op: BinaryOp,
  left: &Expr,
  right: &Expr,
  at: usize,
  context: &Context<'_, M>,
) -> Result<Object<M::Node>> {
  match op {
    BinaryOp::Or => or(left, right, context),
    BinaryOp::And => and(left, right, context),
    BinaryOp::Union => union(left, right, at, context),
    BinaryOp::Equal
    | BinaryOp::NotEqual
    | BinaryOp::Less
    | BinaryOp::LessEqual
    | BinaryOp::Greater
    | BinaryOp::GreaterEqual => comparison(op, left, right, context),
    BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => {
      arithmetic(op, left, right, context)
    }
  }
}

/// Evaluates `left or right`, which stops as soon as the answer is settled.
fn or<M: Model>(left: &Expr, right: &Expr, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  if eval(left, context)?.boolean() {
    return Ok(Object::Boolean(true));
  }
  Ok(Object::Boolean(eval(right, context)?.boolean()))
}

/// Evaluates `left and right`, which stops as soon as the answer is settled.
fn and<M: Model>(left: &Expr, right: &Expr, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  if !eval(left, context)?.boolean() {
    return Ok(Object::Boolean(false));
  }
  Ok(Object::Boolean(eval(right, context)?.boolean()))
}

/// Evaluates `left | right`, whose `|` is at `at`.
fn union<M: Model>(left: &Expr, right: &Expr, at: usize, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  let locate = |error| context.locate(error, at);
  let mut nodes = node_set(eval(left, context)?, "a union joins node-sets").map_err(locate)?;
  nodes.extend(node_set(eval(right, context)?, "a union joins node-sets").map_err(locate)?);
  make_node_set(context.model, &mut nodes);
  Ok(Object::NodeSet(nodes))
}

/// Evaluates a comparison.
fn comparison<M: Model>(op: BinaryOp, left: &Expr, right: &Expr, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  let left = eval(left, context)?;
  let right = eval(right, context)?;
  Ok(Object::Boolean(compare(op, &left, &right, context.model)))
}

/// Evaluates an arithmetic operator.
fn arithmetic<M: Model>(op: BinaryOp, left: &Expr, right: &Expr, context: &Context<'_, M>) -> Result<Object<M::Node>> {
  let a = eval(left, context)?.number(context.model);
  let b = eval(right, context)?.number(context.model);
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
fn eval_path<M: Model>(path: &Path, context: &Context<'_, M>) -> Result<Vec<M::Node>> {
  let mut nodes = match &path.start {
    PathStart::Root => vec![context.model.root(context.node)],
    PathStart::Context => vec![context.node],
    PathStart::Expr(expr) => node_set(eval(expr, context)?, "a path can only continue from a node-set")
      .map_err(|error| context.locate(error, path.at))?,
  };
  for step in &path.steps {
    nodes = eval_step(step, &nodes, context)?;
  }
  Ok(nodes)
}

/// Applies one step to every node a path has reached so far.
fn eval_step<M: Model>(step: &Step, from: &[M::Node], context: &Context<'_, M>) -> Result<Vec<M::Node>> {
  let mut result = Vec::new();
  for node in from {
    // The axis is walked in its own order, since that is the order predicates count in.
    let mut selected = Vec::new();
    axis::walk(context.model, *node, step.axis, &mut |candidate| {
      if matches(context, candidate, step.axis, &step.node_test)? {
        selected.push(candidate);
      }
      Ok(())
    })?;
    for predicate in &step.predicates {
      selected = filter(selected, predicate, context)?;
    }
    result.extend(selected);
  }
  // The step's result is a node-set, however the axis reached it.
  make_node_set(context.model, &mut result);
  Ok(result)
}

/// Applies the predicates of a filter expression, whose nodes are in document order.
fn apply_predicates<M: Model>(
  mut nodes: Vec<M::Node>,
  predicates: &[Expr],
  context: &Context<'_, M>,
) -> Result<Vec<M::Node>> {
  make_node_set(context.model, &mut nodes);
  for predicate in predicates {
    nodes = filter(nodes, predicate, context)?;
  }
  Ok(nodes)
}

/// Keeps the nodes a predicate holds for, each evaluated as the context node.
///
/// A predicate that yields a number is a test on the position (XPath 1.0 §3.3): `[2]` keeps the second node. Anything
/// else is taken as a boolean.
fn filter<M: Model>(nodes: Vec<M::Node>, predicate: &Expr, context: &Context<'_, M>) -> Result<Vec<M::Node>> {
  let size = nodes.len();
  let mut kept = Vec::new();
  for (index, node) in nodes.into_iter().enumerate() {
    let position = index + 1;
    let inner = context.at(node, position, size);
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
fn matches<M: Model>(context: &Context<'_, M>, node: M::Node, axis: Axis, test: &NodeTest) -> Result<bool> {
  let kind = context.model.kind(node);
  let name_of = || context.model.expanded_name(node);
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
      let namespace = resolve_prefix(prefix)?;
      name_of().is_some_and(|name| name.namespace.as_deref() == Some(namespace))
    }
    NodeTest::Name(NameTest::Exact { prefix, local }) => {
      let namespace = match prefix {
        Some(prefix) => Some(resolve_prefix(prefix)?),
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
fn resolve_prefix(prefix: &str) -> Result<&'static str> {
  namespace_of(prefix).ok_or_else(|| error(format!("the prefix \"{prefix}\" is not bound")))
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
