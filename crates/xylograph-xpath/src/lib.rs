//! XPath 1.0 for xylograph: reading an expression into a tree.
//!
//! [`parse`] turns the text of an expression into an [`Expr`], the tree the evaluator will walk.
//! Parsing is the whole of this crate for now; evaluation arrives in a later phase (see
//! `ROADMAP.md`, Phase 4).
//!
//! # What the tree looks like
//!
//! XPath is written with abbreviations — `//`, `.`, `..`, `@`, and a step with no axis — and the
//! parser expands every one of them, so the tree holds a single plain form: each step is an axis,
//! a node test and its predicates. Printing a tree writes that form back as valid XPath, with
//! binary expressions parenthesized so the precedence the parser settled on is visible.
//!
//! ```
//! use xylograph_xpath::parse;
//!
//! let expr = parse("//book[@lang='en']/title")?;
//! assert_eq!(
//!   expr.to_string(),
//!   "/descendant-or-self::node()/child::book[(attribute::lang = 'en')]/child::title"
//! );
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! # Evaluating
//!
//! [`evaluate`] runs a tree against a document, seen through the [data model](xylograph_xdm).
//! The result is a [`Value`]: a node-set, a boolean, a number or a string, converted between
//! those as the operators demand.
//!
//! ```
//! use xylograph_dom::build;
//! use xylograph_xdm::DomModel;
//! use xylograph_xpath::{Value, evaluate, parse};
//!
//! let doc = build::parse("<list><item>a</item><item>b</item></list>".as_bytes())?;
//! let model = DomModel::new(&doc);
//! let expr = parse("//item[2]")?;
//!
//! let Value::NodeSet(nodes) = evaluate(&expr, &model, model.root_node())? else {
//!   panic!("a path yields a node-set")
//! };
//! assert_eq!(nodes.len(), 1);
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! The core function library is not complete yet: only the functions a predicate needs —
//! `position`, `last`, `count`, `not`, `true` and `false` — are available, and the rest arrive
//! in the next phase.

pub mod ast;
mod axis;
mod context;
mod eval;
mod functions;
mod lexer;
mod parser;
mod value;

pub use ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
pub use context::{Context, Environment};
pub use value::{Value, number_to_string, string_to_number};

use xylograph_core::error::Result;
use xylograph_xdm::Model;

/// Parses an XPath 1.0 expression.
///
/// # Errors
///
/// Returns [`ErrorKind::XPath`](xylograph_core::ErrorKind::XPath) if the expression cannot be
/// read, with the position in the expression that could not be made sense of.
///
/// # Examples
///
/// ```
/// use xylograph_xpath::parse;
///
/// assert_eq!(parse("a/b")?.to_string(), "child::a/child::b");
/// assert!(parse("a/").is_err());
/// # Ok::<(), xylograph_core::Error>(())
/// ```
pub fn parse(expression: &str) -> Result<Expr> {
  let tokens = lexer::tokenize(expression)?;
  parser::parse(&tokens, expression.len())
}

/// Evaluates an expression with `node` as the context node, and nothing bound.
///
/// Use [`evaluate_with`] when the expression names a variable or uses a namespace prefix.
///
/// # Errors
///
/// Returns [`ErrorKind::XPath`](xylograph_core::ErrorKind::XPath) if the expression asks for
/// something the context cannot give — an unbound variable or prefix, a function that is not
/// available — or applies an operator to a value of the wrong type.
pub fn evaluate<M: Model>(expr: &Expr, model: &M, node: M::Node) -> Result<Value<M::Node>> {
  let environment = Environment::new();
  evaluate_with(expr, model, node, &environment)
}

/// Evaluates an expression with `node` as the context node and `environment` in scope.
///
/// # Errors
///
/// As [`evaluate`].
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::DomModel;
/// use xylograph_xpath::{Environment, Value, evaluate_with, parse};
///
/// let doc = build::parse("<a><b>x</b></a>".as_bytes())?;
/// let model = DomModel::new(&doc);
/// let environment = Environment::new().with_variable("want", Value::String("x".to_owned()));
///
/// let expr = parse("//b[. = $want]")?;
/// let value = evaluate_with(&expr, &model, model.root_node(), &environment)?;
/// assert!(value.boolean(), "the b element was found");
/// # Ok::<(), xylograph_core::Error>(())
/// ```
pub fn evaluate_with<M: Model>(
  expr: &Expr,
  model: &M,
  node: M::Node,
  environment: &Environment<M::Node>,
) -> Result<Value<M::Node>> {
  let context = Context::new(model, node, environment);
  eval::eval(expr, &context)
}
