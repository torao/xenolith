//! Evaluating an expression tree against a tree of nodes.

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_xdm::{Model, NodeKind};

use crate::ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
use crate::context::{Context, normalize};
use crate::value::Value;
use crate::{axis, functions};

/// Evaluates an expression in a context.
pub(crate) fn eval<M: Model>(expr: &Expr, context: &Context<'_, M>) -> Result<Value<M::Node>> {
  match expr {
    Expr::Literal(value) => Ok(Value::String(value.clone())),
    Expr::Number(value) => Ok(Value::Number(*value)),
    Expr::Negate(inner) => Ok(Value::Number(-eval(inner, context)?.number(context.model))),
    Expr::Binary { op, left, right } => binary(*op, left, right, context),
    Expr::Path(path) => Ok(Value::NodeSet(eval_path(path, context)?)),
    Expr::Filter { expr, predicates } => {
      let nodes = node_set(eval(expr, context)?, "a predicate can only filter a node-set")?;
      Ok(Value::NodeSet(apply_predicates(nodes, predicates, context)?))
    }
    Expr::Variable { prefix, local } => variable(prefix.as_deref(), local, context),
    Expr::Function { prefix, local, arguments } => {
      let mut values = Vec::with_capacity(arguments.len());
      for argument in arguments {
        values.push(eval(argument, context)?);
      }
      functions::call(prefix.as_deref(), local, values, context)
    }
  }
}

/// Looks up a variable, resolving the prefix of its name first.
fn variable<M: Model>(prefix: Option<&str>, local: &str, context: &Context<'_, M>) -> Result<Value<M::Node>> {
  let namespace = match prefix {
    Some(prefix) => Some(resolve_prefix(context, prefix)?),
    None => None,
  };
  match context.variables.get(namespace.as_deref(), local) {
    Some(value) => Ok(value.clone()),
    None => {
      let name = prefix.map_or_else(|| local.to_owned(), |prefix| format!("{prefix}:{local}"));
      Err(error(format!("the variable \"${name}\" is not bound in this evaluation")))
    }
  }
}

// --- Operators --------------------------------------------------------------------------------

fn binary<M: Model>(op: BinaryOp, left: &Expr, right: &Expr, context: &Context<'_, M>) -> Result<Value<M::Node>> {
  // `or` and `and` stop as soon as the answer is settled.
  match op {
    BinaryOp::Or => {
      if eval(left, context)?.boolean() {
        return Ok(Value::Boolean(true));
      }
      return Ok(Value::Boolean(eval(right, context)?.boolean()));
    }
    BinaryOp::And => {
      if !eval(left, context)?.boolean() {
        return Ok(Value::Boolean(false));
      }
      return Ok(Value::Boolean(eval(right, context)?.boolean()));
    }
    _ => {}
  }

  let left = eval(left, context)?;
  let right = eval(right, context)?;
  match op {
    BinaryOp::Union => {
      let mut nodes = node_set(left, "a union joins node-sets")?;
      nodes.extend(node_set(right, "a union joins node-sets")?);
      normalize(context.model, &mut nodes);
      Ok(Value::NodeSet(nodes))
    }
    BinaryOp::Equal
    | BinaryOp::NotEqual
    | BinaryOp::Less
    | BinaryOp::LessEqual
    | BinaryOp::Greater
    | BinaryOp::GreaterEqual => Ok(Value::Boolean(compare(op, &left, &right, context.model))),
    _ => {
      let (a, b) = (left.number(context.model), right.number(context.model));
      Ok(Value::Number(match op {
        BinaryOp::Add => a + b,
        BinaryOp::Subtract => a - b,
        BinaryOp::Multiply => a * b,
        BinaryOp::Divide => a / b,
        // `mod` is the remainder of a truncating division, which is what Rust's `%` gives.
        BinaryOp::Modulo => a % b,
        _ => unreachable!("the other operators are handled above"),
      }))
    }
  }
}

/// Compares two values (XPath 1.0 §3.4).
///
/// A node-set compares by its members: the result is true if *any* node makes it true. Which
/// conversion the other side goes through depends on the operator — the relational operators
/// always compare numbers, while `=` and `!=` compare booleans, numbers or strings depending on
/// what they are given.
fn compare<M: Model>(op: BinaryOp, left: &Value<M::Node>, right: &Value<M::Node>, model: &M) -> bool {
  match (left, right) {
    (Value::NodeSet(a), Value::NodeSet(b)) => {
      let equality = is_equality(op);
      a.iter().any(|left| {
        let left = model.string_value(*left);
        b.iter().any(|right| {
          let right = model.string_value(*right);
          if equality { compare_strings(op, &left, &right) } else { compare_numbers(op, &left, &right) }
        })
      })
    }
    (Value::NodeSet(nodes), other) => node_set_compare(op, nodes, other, model),
    (other, Value::NodeSet(nodes)) => node_set_compare(flip(op), nodes, other, model),
    _ => scalar_compare(op, left, right, model),
  }
}

/// Compares a node-set against a value that is not one.
fn node_set_compare<M: Model>(op: BinaryOp, nodes: &[M::Node], other: &Value<M::Node>, model: &M) -> bool {
  // For `=` and `!=` a boolean makes the node-set a boolean too; every other case looks at the
  // nodes one at a time.
  if is_equality(op) {
    if let Value::Boolean(other) = other {
      let present = !nodes.is_empty();
      return if op == BinaryOp::Equal { present == *other } else { present != *other };
    }
  }
  let other_string = other.string(model);
  nodes.iter().any(|node| {
    let value = model.string_value(*node);
    match (is_equality(op), other) {
      (true, Value::String(text)) => compare_strings(op, &value, text),
      // A number, or any relational comparison, is settled as numbers.
      _ => compare_numbers(op, &value, &other_string),
    }
  })
}

/// Compares two values, neither of which is a node-set.
fn scalar_compare<M: Model>(op: BinaryOp, left: &Value<M::Node>, right: &Value<M::Node>, model: &M) -> bool {
  if !is_equality(op) {
    return numbers(op, left.number(model), right.number(model));
  }
  let equal = if matches!(left, Value::Boolean(_)) || matches!(right, Value::Boolean(_)) {
    left.boolean() == right.boolean()
  } else if matches!(left, Value::Number(_)) || matches!(right, Value::Number(_)) {
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
  let (a, b) = (crate::value::string_to_number(left), crate::value::string_to_number(right));
  if is_equality(op) {
    return if op == BinaryOp::Equal { a == b } else { a != b };
  }
  numbers(op, a, b)
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
    PathStart::Expr(expr) => node_set(eval(expr, context)?, "a path can only continue from a node-set")?,
  };
  for step in &path.steps {
    nodes = eval_step(step, &nodes, context)?;
  }
  Ok(nodes)
}

/// Applies one step to every node a path has reached so far.
pub(crate) fn eval_step<M: Model>(step: &Step, from: &[M::Node], context: &Context<'_, M>) -> Result<Vec<M::Node>> {
  let mut result = Vec::new();
  for node in from {
    // The axis is walked in its own order, since that is the order predicates count in.
    let mut selected = Vec::new();
    for candidate in axis::nodes(context.model, *node, step.axis) {
      if matches(context, candidate, step.axis, &step.node_test)? {
        selected.push(candidate);
      }
    }
    for predicate in &step.predicates {
      selected = filter(selected, predicate, context)?;
    }
    result.extend(selected);
  }
  // The step's result is a node-set, however the axis reached it.
  normalize(context.model, &mut result);
  Ok(result)
}

/// Applies the predicates of a filter expression, whose nodes are in document order.
fn apply_predicates<M: Model>(
  mut nodes: Vec<M::Node>,
  predicates: &[Expr],
  context: &Context<'_, M>,
) -> Result<Vec<M::Node>> {
  normalize(context.model, &mut nodes);
  for predicate in predicates {
    nodes = filter(nodes, predicate, context)?;
  }
  Ok(nodes)
}

/// Keeps the nodes a predicate holds for, each evaluated as the context node.
///
/// A predicate that yields a number is a test on the position (XPath 1.0 §3.3): `[2]` keeps the
/// second node. Anything else is taken as a boolean.
fn filter<M: Model>(nodes: Vec<M::Node>, predicate: &Expr, context: &Context<'_, M>) -> Result<Vec<M::Node>> {
  let size = nodes.len();
  let mut kept = Vec::new();
  for (index, node) in nodes.into_iter().enumerate() {
    let position = index + 1;
    let inner = context.at(node, position, size);
    let keep = match eval(predicate, &inner)? {
      Value::Number(value) => value == position as f64,
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
    // A name test also restricts to the axis's principal node type, so `@*` is attributes only
    // and `*` on any other axis is elements only.
    NodeTest::Name(_) if kind != principal_kind(axis) => false,
    NodeTest::Name(NameTest::Any) => true,
    NodeTest::Name(NameTest::Namespace(prefix)) => {
      let namespace = resolve_prefix(context, prefix)?;
      name_of().is_some_and(|name| name.namespace.as_deref() == Some(namespace.as_str()))
    }
    NodeTest::Name(NameTest::Name { prefix, local }) => {
      let namespace = match prefix {
        Some(prefix) => Some(resolve_prefix(context, prefix)?),
        None => None,
      };
      name_of().is_some_and(|name| name.namespace == namespace && name.local == *local)
    }
  })
}

/// The node type a name test selects on an axis: attributes on `attribute`, namespace nodes on
/// `namespace`, elements everywhere else.
const fn principal_kind(axis: Axis) -> NodeKind {
  match axis {
    Axis::Attribute => NodeKind::Attribute,
    Axis::Namespace => NodeKind::Namespace,
    _ => NodeKind::Element,
  }
}

/// The namespace a prefix in the expression stands for.
fn resolve_prefix<M: Model>(context: &Context<'_, M>, prefix: &str) -> Result<String> {
  match context.namespaces.get(prefix) {
    Some(namespace) => Ok(namespace.to_owned()),
    None => {
      Err(error(format!("the prefix \"{prefix}\" is not bound; bind it with XPath::with_namespace before compiling")))
    }
  }
}

/// Takes the nodes out of a value, or says what was found instead.
fn node_set<N>(value: Value<N>, wanted: &str) -> Result<Vec<N>> {
  match value {
    Value::NodeSet(nodes) => Ok(nodes),
    other => Err(error(format!("{wanted}, but found {}", other.type_name()))),
  }
}

fn error(message: impl Into<String>) -> Error {
  Error::new(ErrorKind::XPath, message.into())
}
