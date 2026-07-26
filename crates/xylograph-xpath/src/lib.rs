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
//! Compile an expression once, then run it against a document seen through the
//! [data model](xylograph_xdm). The result is a [`Value`] — a node-set, a boolean, a number or a
//! string — converted between those as the operators demand.
//!
//! ```
//! use xylograph_dom::build;
//! use xylograph_xdm::DomModel;
//! use xylograph_xpath::XPathExpression;
//!
//! let doc = build::parse("<list><item>a</item><item>b</item></list>".as_bytes())?;
//! let model = DomModel::new(&doc);
//!
//! let query = XPathExpression::compile("//item[2]")?;
//! let value = query.evaluate(&model, model.root_node())?;
//! assert_eq!(value.nodes().map(<[_]>::len), Some(1));
//! assert_eq!(value.string(&model), "b");
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! # Coming from Java
//!
//! The API follows `javax.xml.xpath`, so what you know there carries over:
//!
//! | `javax.xml.xpath` | here |
//! |---|---|
//! | `XPathFactory.newInstance().newXPath()` | [`XPath::new`] |
//! | `XPath.setNamespaceContext(…)` | [`XPath::with_namespace`] |
//! | `NamespaceContext` | [`Namespaces`] |
//! | `XPath.compile(String)` | [`XPath::compile`] |
//! | `XPathExpression` | [`XPathExpression`] |
//! | `XPathExpression.evaluate(item)` | [`XPathExpression::evaluate`] |
//! | `XPathVariableResolver` | [`Variables`] |
//! | `XPathConstants.NODESET` / `.STRING` / … | [`Value`] and its conversions |
//! | `XPathExpressionException` | [`ErrorKind::XPath`](xylograph_core::ErrorKind::XPath) |
//!
//! Two differences are deliberate. Java asks for the result type up front and casts; here the
//! [`Value`] says which type it is and converts on request, since XPath's conversions are what
//! the operators use anyway. And `XPathFunctionResolver` has no counterpart yet — extension
//! functions arrive with XSLT.
//!
//! [`parse`] and [`evaluate`] are the two halves underneath, for a caller that holds the
//! [expression tree](Expr) itself — as XSLT will, to match patterns against it.
//!
//! The whole core function library (§4) is available — the twenty-seven node-set, string,
//! boolean and number functions. Extension functions, which is what a function name with a
//! prefix is, are not: registering those comes with XSLT.

//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XPath 1.0] — W3C Recommendation 16 November 1999. The grammar and its [lexical rules
//!   (§3.7)], the [expressions (§3)], and the [core function library (§4)]. Section numbers
//!   appear in the code beside the rules they implement.
//! - [Namespaces in XML 1.0 (Third Edition)] — W3C Recommendation 8 December 2009, for what a
//!   prefix in a name test or a variable name means.
//! - `javax.xml.xpath` — the API's shape, so that what a Java caller knows carries over; see the
//!   table above. The [package documentation] is the reference.
//!
//! Where the specification allows a choice — how many digits a number is written with, the order
//! of the attribute axis — what this crate picks is recorded in the behaviour report; see the
//! README.
//!
//! [XPath 1.0]: https://www.w3.org/TR/1999/REC-xpath-19991116/
//! [lexical rules (§3.7)]: https://www.w3.org/TR/1999/REC-xpath-19991116/#exprlex
//! [expressions (§3)]: https://www.w3.org/TR/1999/REC-xpath-19991116/#section-Expressions
//! [core function library (§4)]: https://www.w3.org/TR/1999/REC-xpath-19991116/#corelib
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [package documentation]: https://docs.oracle.com/en/java/javase/21/docs/api/java.xml/javax/xml/xpath/package-summary.html

pub mod ast;
mod axis;
mod compiled;
mod context;
mod eval;
mod functions;
mod lexer;
mod parser;
mod value;

pub use ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
pub use compiled::{XPath, XPathExpression};
pub use context::{Context, Namespaces, Variables};
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

/// Evaluates an expression tree with `node` as the context node, and nothing bound.
///
/// This is the low-level entry, for a caller holding an [`Expr`] of its own; most callers want
/// [`XPathExpression`]. Use [`evaluate_with`] when the expression names a variable or uses a
/// namespace prefix.
///
/// # Errors
///
/// Returns [`ErrorKind::XPath`](xylograph_core::ErrorKind::XPath) if the expression asks for
/// something the context cannot give — an unbound variable or prefix, a function that is not
/// available — or applies an operator to a value of the wrong type.
pub fn evaluate<M: Model>(expr: &Expr, model: &M, node: M::Node) -> Result<Value<M::Node>> {
  evaluate_with(expr, model, node, &Namespaces::new(), &Variables::new())
}

/// Evaluates an expression tree with `node` as the context node and the given bindings in scope.
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
/// use xylograph_xpath::{Namespaces, Value, Variables, evaluate_with, parse};
///
/// let doc = build::parse("<a><b>x</b></a>".as_bytes())?;
/// let model = DomModel::new(&doc);
/// let variables = Variables::new().with("want", Value::String("x".to_owned()));
///
/// let expr = parse("//b[. = $want]")?;
/// let value = evaluate_with(&expr, &model, model.root_node(), &Namespaces::new(), &variables)?;
/// assert!(value.boolean(), "the b element was found");
/// # Ok::<(), xylograph_core::Error>(())
/// ```
pub fn evaluate_with<M: Model>(
  expr: &Expr,
  model: &M,
  node: M::Node,
  namespaces: &Namespaces,
  variables: &Variables<M::Node>,
) -> Result<Value<M::Node>> {
  let context = Context::new(model, node, namespaces, variables);
  eval::eval(expr, &context)
}

/// Evaluates an expression tree in a context the caller put together.
///
/// The other entry points evaluate a node on its own, at position 1 of a set of 1. XSLT needs
/// more than that: inside `xsl:for-each`, `position()` and `last()` report where the node sits
/// in the list being processed, so the caller has to say. Build the context with
/// [`Context::new`] and move it with [`Context::at`].
///
/// # Errors
///
/// As [`evaluate`].
pub fn evaluate_in<M: Model>(expr: &Expr, context: &Context<'_, M>) -> Result<Value<M::Node>> {
  eval::eval(expr, context)
}

/// Selects the nodes one [`Step`] reaches from `node`: the axis walked, the node test applied,
/// then the predicates, in document order.
///
/// This is what an XSLT pattern is settled with. Whether a node matches `a[1]` is not a question
/// about the node alone — it is whether `child::a[1]`, asked of the node's parent, selects it —
/// so a pattern needs to run a single step and look for the node in the answer.
///
/// # Errors
///
/// As [`evaluate`], for the predicates.
pub fn evaluate_step<M: Model>(
  step: &Step,
  model: &M,
  node: M::Node,
  namespaces: &Namespaces,
  variables: &Variables<M::Node>,
) -> Result<Vec<M::Node>> {
  let context = Context::new(model, node, namespaces, variables);
  eval::eval_step(step, &[node], &context)
}
