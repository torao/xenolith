//! The two types `javax.xml.xpath` is built around: an environment, and expressions compiled
//! in it.

use std::fmt;

use xylograph_core::error::Result;
use xylograph_xdm::Model;

use crate::ast::Expr;
use crate::context::{Namespaces, Variables};
use crate::value::Value;
use crate::{evaluate_with, parse};

/// The environment expressions are compiled in: Java's `javax.xml.xpath.XPath`.
///
/// It carries the namespace bindings the expressions may use. A prefix in an expression is the
/// caller's to choose and has nothing to do with the prefixes a document happens to use — only
/// the namespace it stands for matters — so it is settled here rather than read from the tree.
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::DomModel;
/// use xylograph_xpath::XPath;
///
/// // The document binds `d`; the expression may call it whatever it likes.
/// let doc = build::parse("<r xmlns:d='urn:d'><d:a/><d:a/></r>".as_bytes())?;
/// let model = DomModel::new(&doc);
///
/// let xpath = XPath::new().with_namespace("x", "urn:d");
/// let expression = xpath.compile("count(//x:a)")?;
/// assert_eq!(expression.evaluate(&model, model.root_node())?.number(&model), 2.0);
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct XPath {
  namespaces: Namespaces,
}

impl XPath {
  /// An environment with no namespace bindings.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Binds a prefix, for the expressions compiled after it.
  #[must_use]
  pub fn with_namespace(mut self, prefix: &str, namespace: &str) -> Self {
    self.namespaces = self.namespaces.with(prefix, namespace);
    self
  }

  /// The namespace bindings.
  #[must_use]
  pub const fn namespaces(&self) -> &Namespaces {
    &self.namespaces
  }

  /// Compiles an expression in this environment.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::XPath`](xylograph_core::ErrorKind::XPath) if the expression cannot be
  /// read, with the position in it that could not be made sense of.
  pub fn compile(&self, expression: &str) -> Result<XPathExpression> {
    Ok(XPathExpression { source: expression.to_owned(), tree: parse(expression)?, namespaces: self.namespaces.clone() })
  }
}

/// A compiled expression, ready to run against a document: Java's
/// `javax.xml.xpath.XPathExpression`.
///
/// Parsing is the expensive half and does not depend on the document, so an expression used more
/// than once — a stylesheet's patterns, a query in a loop — should be compiled once and kept.
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::DomModel;
/// use xylograph_xpath::XPathExpression;
///
/// let query = XPathExpression::compile("count(//item)")?;
///
/// // The same compiled expression, run against two documents.
/// for (xml, expected) in [("<r><item/></r>", 1.0), ("<r><item/><item/></r>", 2.0)] {
///   let doc = build::parse(xml.as_bytes())?;
///   let model = DomModel::new(&doc);
///   let value = query.evaluate(&model, model.root_node())?;
///   assert_eq!(value.number(&model), expected);
/// }
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct XPathExpression {
  source: String,
  tree: Expr,
  namespaces: Namespaces,
}

impl XPathExpression {
  /// Compiles an expression that uses no namespace prefixes.
  ///
  /// For one that does, bind the prefixes on an [`XPath`] and compile it there.
  ///
  /// # Errors
  ///
  /// As [`XPath::compile`].
  pub fn compile(expression: &str) -> Result<Self> {
    XPath::new().compile(expression)
  }

  /// The expression as it was written.
  #[must_use]
  pub fn source(&self) -> &str {
    &self.source
  }

  /// The expression tree, for a caller that wants to look at what was parsed.
  #[must_use]
  pub const fn tree(&self) -> &Expr {
    &self.tree
  }

  /// Evaluates the expression with `node` as the context node and no variables bound.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::XPath`](xylograph_core::ErrorKind::XPath) if the expression asks for
  /// something the context cannot give — an unbound variable or prefix, a function that is not
  /// available — or applies an operator to a value of the wrong type.
  pub fn evaluate<M: Model>(&self, model: &M, node: M::Node) -> Result<Value<M::Node>> {
    self.evaluate_with(model, node, &Variables::new())
  }

  /// Evaluates the expression with `node` as the context node and `variables` in scope.
  ///
  /// # Errors
  ///
  /// As [`evaluate`](Self::evaluate).
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_dom::build;
  /// use xylograph_xdm::DomModel;
  /// use xylograph_xpath::{Value, Variables, XPathExpression};
  ///
  /// let doc = build::parse("<r><n>1</n><n>2</n></r>".as_bytes())?;
  /// let model = DomModel::new(&doc);
  ///
  /// let query = XPathExpression::compile("//n[. = $want]")?;
  /// let variables = Variables::new().with("want", Value::String("2".to_owned()));
  /// let value = query.evaluate_with(&model, model.root_node(), &variables)?;
  /// assert_eq!(value.string(&model), "2");
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  pub fn evaluate_with<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    variables: &Variables<M::Node>,
  ) -> Result<Value<M::Node>> {
    evaluate_with(&self.tree, model, node, &self.namespaces, variables)
  }
}

impl fmt::Display for XPathExpression {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.source)
  }
}
