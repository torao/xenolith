//! Parses and evaluates XPath 1.0 expressions over a [`Document`].

#[cfg(test)]
mod test;

mod ast;
mod axis;
mod eval;
mod functions;
mod lexer;
mod model;
mod object;
mod parser;
mod state;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::chars::is_ncname;
use crate::dom::{Document, NodeId};
use crate::error::{Error, Location, Result};
use crate::name::{XML_NS_URI, XMLNS_NS_URI};
use crate::xpath::ast::Expr;
use crate::xpath::model::{DomModel, Model, make_node_set};
use crate::xpath::object::{Object, number_to_string};
use crate::xpath::state::EvalState;

/// An error message and its byte offset in the source expression.
type Failure = (String, usize);

/// A parsed XPath expression.
#[derive(Clone, Debug)]
pub struct XPath {
  expr: Expr,
  /// The original expression, used to locate evaluation errors by byte offset.
  source: String,
  /// The source location where the expression begins, used as the fallback location for evaluation errors.
  location: Location,
  /// The context used to parse the expression. Evaluation uses its namespace bindings and functions, and it retains
  /// only the variable definitions referenced by the expression.
  context: XPathContext,
}

impl XPath {
  /// Parses an `expression` starting at `location`, in the default [`XPathContext`].
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if `expression` is not a valid XPath expression, or [`Error::Limit`] if it nests deeper than
  /// [`Limits::max_depth`].
  pub fn parse(expression: &str, location: Location) -> Result<Self> {
    XPathContext::new().parse(expression, location)
  }

  /// Evaluates the expression with `node` of `document` as the context node.
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if `node` is not a node in the XPath tree of `document` (a document type, a document fragment,
  /// a namespace declaration attribute, or a node outside the document's tree), or if XPath evaluation fails. An error
  /// returned by an application-defined function is propagated with its original kind.
  pub fn evaluate(&self, document: &Document, node: NodeId) -> Result<XPathResult> {
    self.evaluate_with(document, node, &XPathVariables::new())
  }

  /// Evaluates the expression with `node` as its context node, as [`evaluate`](Self::evaluate) does. For each variable,
  /// evaluation uses the value in `variables` if present; otherwise, it uses the definition in the [`XPathContext`]
  /// that parsed this expression (see [`XPathContext::with_variable`]).
  ///
  /// An unbound variable causes [`Error::XPath`] only if evaluation reaches its reference; the error points to that
  /// reference. Parsing does not check whether a variable is bound. Evaluation converts or computes each variable's
  /// value at most once per call.
  ///
  /// # Errors
  ///
  /// The same errors as [`evaluate`](Self::evaluate). In addition, evaluation returns [`Error::XPath`] if a referenced
  /// node-set value in `variables` contains a node outside the XPath tree of `document`.
  pub fn evaluate_with(&self, document: &Document, node: NodeId, variables: &XPathVariables) -> Result<XPathResult> {
    let model = DomModel::new(document);
    let Some(node) = model.import(&Node::Dom(node)) else {
      let message = "the context node is not a node in the XPath tree of the document";
      return Err(Error::xpath(message).at(self.location.clone()));
    };
    let values = RefCell::new(HashMap::new());
    let state = EvalState::new(&model, node, self, variables, &values);
    match eval::eval(&self.expr, &state) {
      Ok(object) => Ok(result(object, &model)),
      // Most errors are located where they were raised in the expression; any other is located at its start.
      Err(error) if error.location().is_unknown() => Err(error.at(self.location.clone())),
      Err(error) => Err(error),
    }
  }
}

impl fmt::Display for XPath {
  /// Writes the expression as XPath. The default format `{}` uses abbreviations where possible; the alternate format
  /// `{:#}` writes every axis explicitly and parenthesizes binary expressions.
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if f.alternate() { write!(f, "{:#}", self.expr) } else { write!(f, "{}", self.expr) }
  }
}

impl FromStr for XPath {
  type Err = Error;

  /// Parses the `expression` in the same way as [`parse`](XPath::parse), starting from [`Location::new`].
  fn from_str(expression: &str) -> Result<Self> {
    Self::parse(expression, Location::new())
  }
}

/// Configuration for parsing and evaluating XPath expressions.
///
/// The context stores a nesting limit, namespace bindings for expression prefixes, and application-defined functions
/// and variables.
/// An [`XPath`] parsed with this context retains its namespace bindings and function definitions, along with the
/// definitions of the variables that expression references. Evaluation uses those settings and definitions. Context
/// clones share their namespace, function, and variable tables until a clone modifies one of them.
///
/// ```
/// use xenolith::error::Location;
/// use xenolith::xpath::{XPathContext, XPathResult};
///
/// let context = XPathContext::new()
///   .with_namespace("ex", "urn:example")?
///   .with_function("urn:example", "upper", 1, Some(1), |arguments, call| {
///     Ok(XPathResult::String(arguments[0].string(call.document())?.to_uppercase()))
///   })?;
/// let xpath = context.parse("ex:upper('abc')", Location::new())?;
/// # let doc = xenolith::dom::Document::new();
/// assert_eq!(xpath.evaluate(&doc, doc.document_node())?, XPathResult::String("ABC".to_owned()));
/// # Ok::<(), xenolith::error::Error>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct XPathContext {
  limits: Limits,
  /// The namespace URI bound to each prefix, except `xml`.
  namespaces: Arc<HashMap<String, String>>,
  /// The application-defined functions, by namespace URI and then local name.
  functions: Arc<HashMap<String, HashMap<String, Function>>>,
  /// The application-defined variables, by namespace URI (empty for no namespace) and then local name.
  variables: Arc<HashMap<String, HashMap<String, Arc<Definition>>>>,
}

impl XPathContext {
  /// Creates a context with the default [`Limits`], only the predefined `xml` namespace binding, and no
  /// application-defined functions or variables.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Replaces the limits applied when parsing expressions and defining variables.
  #[must_use]
  pub fn with_limits(mut self, limits: Limits) -> Self {
    self.limits = limits;
    self
  }

  /// Binds the expression prefix `prefix` to namespace URI `uri`, replacing any existing binding for that prefix.
  ///
  /// XPath resolves prefixes from this context, independently of the prefixes used in the document. The `xml` prefix
  /// is predefined and bound to the XML namespace (Namespaces in XML §3).
  ///
  /// # Errors
  ///
  /// [`Error::Name`] if `prefix` is not an NCName. [`Error::Namespace`] if `prefix` is `xmlns`, if `xml` is bound to
  /// any URI other than the XML namespace, if any other prefix is bound to the XML namespace, if `uri` is the XMLNS
  /// namespace, or if `uri` is empty.
  pub fn with_namespace(mut self, prefix: &str, uri: &str) -> Result<Self> {
    if !is_ncname(prefix) {
      return Err(Error::name(format!("the prefix \"{prefix}\" is not an NCName")));
    }
    // Only `xml` is bound to the XML namespace, and nothing to the XMLNS namespace (Namespaces in XML §3).
    let reserved = prefix == "xmlns" || uri == XMLNS_NS_URI || (prefix == "xml") != (uri == XML_NS_URI);
    if reserved || uri.is_empty() {
      return Err(Error::namespace(format!("the prefix \"{prefix}\" cannot be bound to \"{uri}\"")));
    }
    Arc::make_mut(&mut self.namespaces).insert(prefix.to_owned(), uri.to_owned());
    Ok(self)
  }

  /// Defines the function `{namespace}local`, replacing any existing definition with that name. An XPath expression
  /// calls it through a prefix bound to `namespace`, for example `ex:upper(//title)`.
  ///
  /// The function accepts at least `min` arguments and at most `max` arguments; `None` means there is no upper bound.
  /// Parsing checks the function name and argument count, so [`parse`](Self::parse) rejects unavailable functions and
  /// calls with an invalid number of arguments.
  ///
  /// The callback receives evaluated arguments and a [`FunctionCall`] describing the call's evaluation context. The
  /// evaluator sorts a returned node-set into document order and removes duplicates. Evaluation returns
  /// [`Error::XPath`] if the callback returns a node that is not in the XPath tree of the document being evaluated. If
  /// the callback returns an error without a source location, the evaluator locates it at the function name.
  ///
  /// # Errors
  ///
  /// [`Error::Name`] if `local` is not an NCName. [`Error::Namespace`] if `namespace` is empty; unprefixed names are
  /// reserved for core functions. [`Error::Internal`] if `max` is less than `min`.
  pub fn with_function<F>(
    mut self,
    namespace: &str,
    local: &str,
    min: usize,
    max: Option<usize>,
    function: F,
  ) -> Result<Self>
  where
    F: Fn(Vec<XPathResult>, &FunctionCall<'_>) -> Result<XPathResult> + Send + Sync + 'static,
  {
    if !is_ncname(local) {
      return Err(Error::name(format!("the function name \"{local}\" is not an NCName")));
    }
    if namespace.is_empty() {
      return Err(Error::namespace(format!("the function \"{local}\" needs a namespace")));
    }
    if max.is_some_and(|max| max < min) {
      return Err(Error::misuse(format!("the function \"{local}\" takes at most {max:?} arguments, fewer than {min}")));
    }
    let function = Function { min, max, body: Arc::new(function) };
    let functions = Arc::make_mut(&mut self.functions);
    functions.entry(namespace.to_owned()).or_default().insert(local.to_owned(), function);
    Ok(self)
  }

  /// Defines the variable `{namespace}local` as the value of `definition`. Use `None` for `namespace` to define a
  /// variable without a namespace, such as `$x`. A new definition replaces any existing definition with the same
  /// name.
  ///
  /// Evaluation computes `definition` the first time the variable is referenced in an evaluation, with the document
  /// node as the context node and position and size both set to 1. Evaluation reuses that value for the rest of the
  /// call and discards it afterward. A value with the same name passed to [`XPath::evaluate_with`] takes precedence.
  ///
  /// The context that parsed `definition` resolves its variable references. A definition can therefore refer to
  /// definitions that were already present in that context and to values passed to [`XPath::evaluate_with`]; it cannot
  /// refer to later definitions, so definitions cannot form a cycle. A chain of definitions adds to the evaluation
  /// depth and counts toward [`Limits::max_depth`]. Parsing an expression that exceeds this limit returns
  /// [`Error::Limit`].
  ///
  /// # Errors
  ///
  /// [`Error::Name`] if `local` is not an NCName; [`Error::Namespace`] if `namespace` is `Some("")`; or
  /// [`Error::Limit`] if defining a new name would exceed [`Limits::max_variables`].
  pub fn with_variable(mut self, namespace: Option<&str>, local: &str, definition: XPath) -> Result<Self> {
    check_variable_name(namespace, local)?;
    let namespace = namespace.unwrap_or_default();
    let replaces = self.variable(namespace, local).is_some();
    if let Some(max) = self.limits.max_variables.filter(|&max| !replaces && self.variable_count() >= max) {
      let message = format!("more than {max} variables are defined; raise xpath::Limits::max_variables");
      return Err(Error::limit(message));
    }
    let depth = parser::depth(&definition.expr, &definition.context);
    let definition = Arc::new(Definition { xpath: definition, depth });
    let variables = Arc::make_mut(&mut self.variables);
    variables.entry(namespace.to_owned()).or_default().insert(local.to_owned(), definition);
    Ok(self)
  }

  /// Parses an `expression` starting at `location`, in this context.
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if `expression` is not a valid XPath expression, uses an unbound prefix, or calls a function
  /// that is not available with that number of arguments; [`Error::Limit`] if it nests deeper than
  /// [`Limits::max_depth`], counting the definitions of the variables it refers to.
  pub fn parse(&self, expression: &str, location: Location) -> Result<XPath> {
    let tokens = match lexer::tokenize(expression) {
      Ok(tokens) => tokens,
      Err((message, byte)) => return Err(Error::xpath(message).at(located(&location, expression, byte))),
    };
    let expr = parser::parse(&tokens, expression, &location, self)?;
    // Only the definitions the expression refers to: a variable is resolved in the context it is parsed in, so no other
    // is used. Keeping the whole table would share it with every definition parsed here, and each `with_variable`
    // after them would copy it, keeping a copy per definition: quadratic memory.
    let variables = Arc::new(parser::referenced(&expr, self));
    let context = XPathContext {
      limits: self.limits,
      namespaces: Arc::clone(&self.namespaces),
      functions: Arc::clone(&self.functions),
      variables,
    };
    Ok(XPath { expr, source: expression.to_owned(), location, context })
  }

  /// Returns the namespace URI bound to a prefix in an XPath expression, or `None` if the prefix is unbound.
  ///
  /// Prefixes in an XPath expression are resolved against the expression's namespace bindings, not the prefixes used in
  /// the document. `xml` is predefined as bound to the XML namespace (Namespaces in XML §3).
  pub(crate) fn namespace_of(&self, prefix: &str) -> Option<&str> {
    if prefix == "xml" { Some(XML_NS_URI) } else { self.namespaces.get(prefix).map(String::as_str) }
  }

  /// The application-defined function named `local` in `namespace`, if there is one.
  fn function(&self, namespace: &str, local: &str) -> Option<&Function> {
    self.functions.get(namespace)?.get(local)
  }

  /// The definition of the application-defined variable named `local` in `namespace` (empty for no namespace), if
  /// there is one.
  fn variable(&self, namespace: &str, local: &str) -> Option<&Arc<Definition>> {
    self.variables.get(namespace)?.get(local)
  }

  /// The number of application-defined variables.
  fn variable_count(&self) -> usize {
    self.variables.values().map(HashMap::len).sum()
  }
}

/// The definition of an application-defined variable.
#[derive(Debug)]
struct Definition {
  xpath: XPath,
  /// How deep evaluating the definition nests, counting the definitions it refers to.
  depth: usize,
}

/// The variable values passed to [`XPath::evaluate_with`], keyed by expanded name (namespace URI and local name).
///
/// An evaluation reads these values without modifying them. Call [`set`](Self::set) or [`remove`](Self::remove)
/// between evaluations to change bindings. Evaluation checks a node-set value's nodes against the document's XPath
/// tree when it first references that variable.
#[derive(Clone, Debug, Default)]
pub struct XPathVariables {
  /// The value of each variable, by namespace URI (empty for no namespace) and then local name.
  values: HashMap<String, HashMap<String, XPathResult>>,
}

impl XPathVariables {
  /// Creates an empty set of variable values.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Sets the value of `{namespace}local` to `value`, replacing any existing value with that name. Use `None` for
  /// `namespace` to set a variable without a namespace.
  ///
  /// # Errors
  ///
  /// [`Error::Name`] if `local` is not an NCName, or [`Error::Namespace`] if `namespace` is `Some("")`.
  pub fn set(&mut self, namespace: Option<&str>, local: &str, value: XPathResult) -> Result<()> {
    check_variable_name(namespace, local)?;
    self.values.entry(namespace.unwrap_or_default().to_owned()).or_default().insert(local.to_owned(), value);
    Ok(())
  }

  /// Removes the value of the variable `{namespace}local`, returning it if there was one.
  pub fn remove(&mut self, namespace: Option<&str>, local: &str) -> Option<XPathResult> {
    self.values.get_mut(namespace.unwrap_or_default())?.remove(local)
  }

  /// The value of the variable named `local` in `namespace` (empty for no namespace), if there is one.
  fn get(&self, namespace: &str, local: &str) -> Option<&XPathResult> {
    self.values.get(namespace)?.get(local)
  }
}

/// Checks the name of a variable. An empty namespace URI is refused, since the tables use it for no namespace.
fn check_variable_name(namespace: Option<&str>, local: &str) -> Result<()> {
  if !is_ncname(local) {
    return Err(Error::name(format!("the variable name \"{local}\" is not an NCName")));
  }
  if namespace == Some("") {
    return Err(Error::namespace(format!("the variable \"{local}\" has an empty namespace URI; use None for none")));
  }
  Ok(())
}

/// The callback type for an application-defined function.
type Body = dyn Fn(Vec<XPathResult>, &FunctionCall<'_>) -> Result<XPathResult> + Send + Sync;

/// An application-defined function and the range of argument counts it accepts.
#[derive(Clone)]
struct Function {
  min: usize,
  /// The maximum number of arguments, or `None` if the function has no maximum.
  max: Option<usize>,
  body: Arc<Body>,
}

impl fmt::Debug for Function {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Function").field("min", &self.min).field("max", &self.max).finish_non_exhaustive()
  }
}

/// The evaluation context passed to an application-defined function.
///
/// It provides the document being evaluated and the context node's position and size in the current node-set.
#[derive(Debug)]
pub struct FunctionCall<'a> {
  document: &'a Document,
  node: Node,
  position: usize,
  size: usize,
}

impl<'a> FunctionCall<'a> {
  /// The document being evaluated.
  #[must_use]
  pub const fn document(&self) -> &'a Document {
    self.document
  }

  /// The context node.
  #[must_use]
  pub const fn node(&self) -> &Node {
    &self.node
  }

  /// The context node's one-based position in the current node-set, as `position()` returns it.
  #[must_use]
  pub const fn position(&self) -> usize {
    self.position
  }

  /// The size of the current node-set, as `last()` returns it.
  #[must_use]
  pub const fn size(&self) -> usize {
    self.size
  }
}

/// The default of [`Limits::max_depth`].
pub(crate) const DEFAULT_MAX_DEPTH: usize = 64;

/// The default of [`Limits::max_variables`].
pub(crate) const DEFAULT_MAX_VARIABLES: usize = 2048;

/// Limits applied when an [`XPathContext`] parses an expression or defines a variable.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
  /// The maximum nesting depth accepted by [`XPathContext::parse`]. The default is 64.
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
  ///
  /// A reference to a variable defined with [`XPathContext::with_variable`] reaches as deep as the reference plus its
  /// definition, including the definitions that one refers to, because evaluating the reference evaluates them there.
  /// Each definition in such a chain uses about 3.1 KiB in debug builds and 1.9 KiB in release builds, less than a
  /// nested predicate.
  ///
  /// With a limit of `d`, stack space grows linearly with `d`, by the amounts per level above. In the worst case, heap
  /// space during evaluation grows as `d × N` for a document of `N` nodes, because each level can hold a node-set
  /// while the levels inside it are evaluated. Checking the depth takes time linear in the length of the expression,
  /// but evaluation time can grow as `N` to the power of `d`: a nested predicate such as `//*[//*[…]]` walks the
  /// document again for every node of the level around it. Raising the limit raises both bounds; lowering it lowers
  /// them.
  pub max_depth: Option<usize>,
  /// The maximum number of variables an [`XPathContext`] may define with [`XPathContext::with_variable`]. The default
  /// is 2048.
  ///
  /// `Some(n)` permits up to `n` definitions; `None` removes the limit. Redefining a name does not count again. Each
  /// definition holds its parsed expression, so memory grows with the number of definitions: about 0.7 MiB for 2048
  /// of the simplest kind. Keep a finite limit when the definitions come from untrusted input, such as the top-level
  /// variables of a stylesheet.
  ///
  /// With a limit of `n`, defining variables takes time and memory linear in `n`, plus the size of their expressions;
  /// looking one up takes constant time whatever `n` is. One evaluation evaluates each definition it reaches at most
  /// once and keeps the value until the evaluation ends. Since a definition can refer to several earlier ones, a
  /// single reference can reach up to `n` definitions, so one evaluation may take `n` times the time of a definition
  /// and memory that grows as `n × N` for a document of `N` nodes.
  pub max_variables: Option<usize>,
}

impl Default for Limits {
  fn default() -> Self {
    Self { max_depth: Some(DEFAULT_MAX_DEPTH), max_variables: Some(DEFAULT_MAX_VARIABLES) }
  }
}

impl Limits {
  /// All limits removed. This should only be used for trusted expressions.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_depth: None, max_variables: None }
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
  /// Returns the first node in document order, or `None` if the result is not a node-set or the set is empty.
  #[must_use]
  pub fn node(&self) -> Option<&Node> {
    match self {
      XPathResult::NodeSet(nodes) => nodes.first(),
      _ => None,
    }
  }

  /// Returns the result as a boolean using the XPath `boolean()` conversion rules.
  #[must_use]
  pub fn boolean(&self) -> bool {
    match self {
      XPathResult::NodeSet(nodes) => !nodes.is_empty(),
      XPathResult::Boolean(value) => *value,
      XPathResult::Number(value) => *value != 0.0 && !value.is_nan(),
      XPathResult::String(value) => !value.is_empty(),
    }
  }

  /// Returns the result as a string using the XPath `string()` conversion rules. For a node-set,
  /// this is the string value of its first node in document order, or an empty string if the set is empty.
  ///
  /// # Errors
  ///
  /// [`Error::XPath`] if the first node of a node-set is not a node in the XPath tree of `document`.
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

/// Converts the evaluator's internal value into a public [`XPathResult`]. The model exports each node; for namespace
/// nodes, it resolves the internal prefix ID to a string using the document's name pool.
fn result<M: Model>(object: Object<M::Node>, model: &M) -> XPathResult {
  match object {
    Object::NodeSet(nodes) => XPathResult::NodeSet(nodes.into_iter().map(|node| model.export(node)).collect()),
    Object::Boolean(value) => XPathResult::Boolean(value),
    Object::Number(value) => XPathResult::Number(value),
    Object::String(value) => XPathResult::String(value),
  }
}

/// Converts a public [`XPathResult`] into the evaluator's internal value, with its nodes put into document order
/// without duplicates. Returns `None` if a node is not in the XPath tree of the model's document.
fn object<M: Model>(result: &XPathResult, model: &M) -> Option<Object<M::Node>> {
  Some(match result {
    XPathResult::NodeSet(nodes) => {
      let mut imported = nodes.iter().map(|node| model.import(node)).collect::<Option<Vec<_>>>()?;
      make_node_set(model, &mut imported);
      Object::NodeSet(imported)
    }
    XPathResult::Boolean(value) => Object::Boolean(*value),
    XPathResult::Number(value) => Object::Number(*value),
    XPathResult::String(value) => Object::String(value.clone()),
  })
}

/// Returns the XPath string-value of `node` in `document`.
fn string_value(node: &Node, document: &Document) -> Result<String> {
  let model = DomModel::new(document);
  match model.import(node) {
    Some(node) => Ok(model.string_value(node)),
    None => Err(Error::xpath("the node is not a node in the XPath tree of the document")),
  }
}

/// Returns the source location at byte offset `byte` in `text`, measured from `start`.
fn located(start: &Location, text: &str, byte: usize) -> Location {
  let mut at = start.clone();
  at.advance_over(&text[..byte]);
  at
}
