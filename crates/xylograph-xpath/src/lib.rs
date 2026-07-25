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
//! # Reading the tree
//!
//! ```
//! use xylograph_xpath::{Expr, parse};
//!
//! let Expr::Binary { op, .. } = parse("1 + 2 * 3")? else { panic!("a binary expression") };
//! // Multiplication binds tighter, so the root of the tree is the addition.
//! assert_eq!(op.symbol(), "+");
//! # Ok::<(), xylograph_core::Error>(())
//! ```

pub mod ast;
mod lexer;
mod parser;

pub use ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};

use xylograph_core::error::Result;

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
