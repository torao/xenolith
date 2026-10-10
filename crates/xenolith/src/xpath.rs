//! XPath 1.0.

#[cfg(test)]
mod test;

mod ast;
mod axis;
mod context;
mod eval;
mod functions;
mod lexer;
mod model;
mod object;
mod parser;

use std::fmt;
use std::str::FromStr;

use crate::dom::{Document, NodeId};
use crate::error::{Error, Location, Result};
use crate::xpath::ast::Expr;
use crate::xpath::context::Context;
use crate::xpath::model::{DomModel, DomNode, Model};
use crate::xpath::object::{Object, number_to_string};

/// The reason why the expression could not be read, and the byte offset within the expression indicated by that reason.
type Failure = (String, usize);

/// A parsed XPath expression.
#[derive(Clone, Debug)]
pub struct XPath {
  expr: Expr,
  /// The expression as it was written, from which the position of an evaluation error in it is counted.
  source: String,
  /// The location where the expression begins and the location where the evaluation error occurs.
  location: Location,
}

impl XPath {
  /// Parses an `expression` starting at `location`, within `limits`.
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if `expression` is not a valid XPath expression, or [`Error::Limit`] if it nests deeper than
  /// [`Limits::max_depth`].
  pub fn parse(expression: &str, location: Location, limits: Limits) -> Result<Self> {
    let tokens = match lexer::tokenize(expression) {
      Ok(tokens) => tokens,
      Err((message, byte)) => return Err(Error::xpath(message).at(located(&location, expression, byte))),
    };
    let expr = parser::parse(&tokens, expression, &location, limits.max_depth)?;
    Ok(Self { expr, source: expression.to_owned(), location })
  }

  /// Evaluates the expression with `node` of `document` as the context node.
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if `node` is not a node of `document`, or if the evaluation fails.
  pub fn evaluate(&self, document: &Document, node: NodeId) -> Result<XPathResult> {
    if !document.owns(node) {
      let message = "the context node is not a node of the document";
      return Err(Error::xpath(message).at(self.location.clone()));
    }
    let model = DomModel::new(document);
    let context = Context::new(&model, model.node(node), &self.source, &self.location);
    match eval::eval(&self.expr, &context) {
      Ok(object) => Ok(result(object, document)),
      // Most errors are located where they were raised in the expression; any other is located at its start.
      Err(error) if error.location().is_unknown() => Err(error.at(self.location.clone())),
      Err(error) => Err(error),
    }
  }
}

impl fmt::Display for XPath {
  /// Writes the parsed expression back as XPath: with the abbreviations wherever they apply with `{}`, and in full
  /// (every axis written, every binary expression in parentheses) with `{:#}`.
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if f.alternate() { write!(f, "{:#}", self.expr) } else { write!(f, "{}", self.expr) }
  }
}

impl FromStr for XPath {
  type Err = Error;

  /// Parses the `expression` in the same way as [`parse`](XPath::parse), starting from [`Location::new`].
  fn from_str(expression: &str) -> Result<Self> {
    Self::parse(expression, Location::new(), Limits::default())
  }
}

/// The default of [`Limits::max_depth`].
pub(crate) const DEFAULT_MAX_DEPTH: usize = 64;

/// Limits applied when [`XPath::parse`] parses an expression.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
  /// The maximum nesting depth accepted by [`XPath::parse`]. The default is 64.
  ///
  /// `Some(n)` permits nesting up to depth `n`; `None` removes the limit. Deeply nested expressions use more stack
  /// space during parsing, evaluation, and formatting, so keep a finite limit for untrusted input. For trusted input,
  /// raise the limit only as needed and account for the additional stack use.
  ///
  /// The most stack-intensive case, nested predicates, uses approximately 8.8 KiB per level in debug builds and
  /// 2.7 KiB per level in release builds. The default of 64 is sized for a 1 MiB Windows main-thread stack in a
  /// debug build, leaving about 460 KiB for the caller. Operator chains use about 2 KiB per level, though actual
  /// stack use depends on the expression shape and build. Parentheses, predicates, function arguments, and unary
  /// minus each add a level. Binary operator chains are also counted by the depth of their syntax tree; for example,
  /// `a or b or c` is left-associative, `((a) or (b)) or (c)`, and has depth 3.
  pub max_depth: Option<usize>,
}

impl Default for Limits {
  fn default() -> Self {
    Self { max_depth: Some(DEFAULT_MAX_DEPTH) }
  }
}

impl Limits {
  /// All limits removed. This should only be used for trusted expressions.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_depth: None }
  }
}

/// The result returned by an XPath expression.
#[derive(Clone, Debug, PartialEq)]
pub enum XPathResult {
  /// A node-set, in document order.
  NodeSet(Vec<Node>),
  /// A boolean.
  Boolean(bool),
  /// A number.
  Number(f64),
  /// A string.
  String(String),
}

impl XPathResult {
  /// The first node of the node-set, or `None` if the node-set is empty or the result is not a node-set.
  #[must_use]
  pub fn node(&self) -> Option<&Node> {
    match self {
      XPathResult::NodeSet(nodes) => nodes.first(),
      _ => None,
    }
  }

  /// The result as a boolean value, similar to the conversion performed by the XPath `boolean()` function.
  #[must_use]
  pub fn boolean(&self) -> bool {
    match self {
      XPathResult::NodeSet(nodes) => !nodes.is_empty(),
      XPathResult::Boolean(value) => *value,
      XPathResult::Number(value) => *value != 0.0 && !value.is_nan(),
      XPathResult::String(value) => !value.is_empty(),
    }
  }

  /// Returns the result as a string, using the conversion performed by the XPath `string()` function. For a node-set,
  /// this is the string value of its first node in document order, or an empty string if the set is empty.
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if the first node of a node-set is not a node of `document`.
  pub fn string(&self, document: &Document) -> Result<String> {
    match self {
      XPathResult::NodeSet(nodes) => match nodes.first() {
        Some(node) => string_value(node, document),
        None => Ok(String::new()),
      },
      XPathResult::Boolean(value) => Ok(value.to_string()),
      XPathResult::Number(value) => Ok(number_to_string(*value)),
      XPathResult::String(value) => Ok(value.clone()),
    }
  }
}

/// A node in a node-set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
  /// A DOM node. In the case of a text node, it is the first DOM node in a sequence of contiguous text and CDATA
  /// sections.
  Dom(NodeId),
  /// A namespace node.
  Namespace {
    /// The element to which the namespace node belongs.
    element: NodeId,
    /// The namespace prefix, or `None` for the default namespace.
    prefix: Option<String>,
  },
}

/// Converts the evaluator's internal value — an `Object<DomNode>` (represented as a data model node) — into a public
/// `XPathResult` (represented by a `NodeId` and a string). The `document` is accepted because namespace node prefixes
/// are stored internally as `NameId`s (indices in the document's name pool); this allows them to be converted back
/// into strings using that document's pool.
fn result(object: Object<DomNode>, document: &Document) -> XPathResult {
  match object {
    Object::NodeSet(nodes) => XPathResult::NodeSet(
      nodes
        .into_iter()
        .map(|node| match node {
          DomNode::Tree(id) | DomNode::Text(id) => Node::Dom(id),
          DomNode::Namespace { element, prefix } => {
            Node::Namespace { element, prefix: prefix.map(|prefix| document.pool().resolve(prefix).to_owned()) }
          }
        })
        .collect(),
    ),
    Object::Boolean(value) => XPathResult::Boolean(value),
    Object::Number(value) => XPathResult::Number(value),
    Object::String(value) => XPathResult::String(value),
  }
}

/// Returns the XPath string-value of `node` in `document`.
fn string_value(node: &Node, document: &Document) -> Result<String> {
  let (owner, node) = match node {
    Node::Dom(id) => (*id, None),
    Node::Namespace { element, prefix } => (*element, Some(prefix.as_deref())),
  };
  if !document.owns(owner) {
    return Err(Error::xpath("the node is not a node of the document"));
  }
  let model = DomModel::new(document);
  let node = match node {
    None => model.node(owner),
    // A prefix that is not interned within the document is not bound anywhere in the document; therefore, the
    // namespace node does not bind anything.
    Some(Some(prefix)) => match document.pool().get(prefix) {
      Some(prefix) => DomNode::Namespace { element: owner, prefix: Some(prefix) },
      None => return Ok(String::new()),
    },
    Some(None) => DomNode::Namespace { element: owner, prefix: None },
  };
  Ok(model.string_value(node))
}

/// Returns the source location at byte offset `byte` in `text`, measured from `start`.
fn located(start: &Location, text: &str, byte: usize) -> Location {
  let mut at = start.clone();
  at.advance_over(&text[..byte]);
  at
}
