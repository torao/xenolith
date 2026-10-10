//! Defines the abstract syntax tree (AST) produced by parsing an XPath 1.0 expression.
//!
//! The AST expands XPath abbreviations into explicit location steps. `//` becomes `descendant-or-self::node()`,
//! `.` becomes `self::node()`, and `..` becomes `parent::node()`. Likewise, `@x` becomes `attribute::x`, and a step
//! without an explicit axis, such as `x`, becomes `child::x`. The evaluator can therefore handle every step uniformly
//! as an axis, a node test, and an ordered list of predicates.
//!
//! [`Display`](std::fmt::Display) writes the AST as a parseable XPath expression. The default format `{}` uses
//! abbreviations where possible. The alternate format `{:#}` spells out each axis and uses parentheses to show the
//! grouping of binary expressions. Finite numbers are written as numeric literals. XPath has no numeric literal for
//! infinity or NaN, so positive infinity is written as `18` followed by 307 zeros, NaN as `(0 div 0)`, and negative
//! infinity as `(-1 div 0)`. Parsing does not produce NaN or negative infinity; those values can occur only when an
//! AST is constructed directly.
//!
//! XPath 1.0 string literals have no escape syntax. A string containing both `'` and `"` is therefore printed as
//! multiple quoted literals joined by `concat`, with each literal chosen to exclude its delimiter (for example,
//! `concat("it's ", '"x"')`). The parser cannot produce such a string value directly; it can occur only in an
//! AST constructed by other means. Re-parsing the output preserves the string value but produces a function-call
//! subtree. A processing-instruction node test accepts only a literal target, so a target containing both quote
//! characters cannot be represented as a valid XPath expression.

use std::fmt;

/// One of the 13 axes that an XPath step can traverse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // the `self` axis cannot be a variant named `Self`, which is a reserved word
pub(crate) enum Axis {
  /// `ancestor`: The parent, that parent's parent, ..., up to the root.
  Ancestor,
  /// `ancestor-or-self`: The node itself and its ancestors.
  AncestorOrSelf,
  /// `attribute`: The element's attributes.
  Attribute,
  /// `child`: The immediate children.
  Child,
  /// `descendant`: All descendants of the context node.
  Descendant,
  /// `descendant-or-self`: The node itself and its descendants.
  DescendantOrSelf,
  /// `following`: Everything that follows the node in document order (excluding descendants).
  Following,
  /// `following-sibling`: Sibling nodes that follow the node.
  FollowingSibling,
  /// `namespace`: The element's namespace nodes.
  Namespace,
  /// `parent`: The parent (if it exists).
  Parent,
  /// `preceding`: Everything that precedes the node in document order (excluding ancestors).
  Preceding,
  /// `preceding-sibling`: Sibling nodes that precede the node.
  PrecedingSibling,
  /// `self`: The node itself.
  SelfAxis,
}

impl Axis {
  /// Returns the axis with `name`, or `None` if `name` is not one of the 13 XPath axis names.
  pub(crate) fn from_name(name: &str) -> Option<Self> {
    Some(match name {
      "ancestor" => Axis::Ancestor,
      "ancestor-or-self" => Axis::AncestorOrSelf,
      "attribute" => Axis::Attribute,
      "child" => Axis::Child,
      "descendant" => Axis::Descendant,
      "descendant-or-self" => Axis::DescendantOrSelf,
      "following" => Axis::Following,
      "following-sibling" => Axis::FollowingSibling,
      "namespace" => Axis::Namespace,
      "parent" => Axis::Parent,
      "preceding" => Axis::Preceding,
      "preceding-sibling" => Axis::PrecedingSibling,
      "self" => Axis::SelfAxis,
      _ => return None,
    })
  }

  /// Returns this axis's XPath name.
  pub(crate) const fn name(self) -> &'static str {
    match self {
      Axis::Ancestor => "ancestor",
      Axis::AncestorOrSelf => "ancestor-or-self",
      Axis::Attribute => "attribute",
      Axis::Child => "child",
      Axis::Descendant => "descendant",
      Axis::DescendantOrSelf => "descendant-or-self",
      Axis::Following => "following",
      Axis::FollowingSibling => "following-sibling",
      Axis::Namespace => "namespace",
      Axis::Parent => "parent",
      Axis::Preceding => "preceding",
      Axis::PrecedingSibling => "preceding-sibling",
      Axis::SelfAxis => "self",
    }
  }
}

impl fmt::Display for Axis {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.name())
  }
}

/// A test on the name of a node. This applies to steps that test the name, rather than the node type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NameTest {
  /// `*`: Any name.
  Any,
  /// `prefix:*`: Any name within the namespace to which the prefix is bound.
  AnyLocal(String),
  /// A qualified name (including the prefix, if written with one).
  Exact {
    /// The prefix (if written with one).
    prefix: Option<String>,
    /// The local part of the qualified name.
    local: String,
  },
}

impl fmt::Display for NameTest {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NameTest::Any => f.write_str("*"),
      NameTest::AnyLocal(prefix) => write!(f, "{prefix}:*"),
      NameTest::Exact { prefix: Some(prefix), local } => write!(f, "{prefix}:{local}"),
      NameTest::Exact { prefix: None, local } => f.write_str(local),
    }
  }
}

/// The name or node type selected by the step from the nodes reached by the axis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NodeTest {
  /// A name test. This also restricts the selection to the axis's principal node type: attributes for the `attribute`
  /// axis, namespace nodes for the `namespace` axis, and elements for other axes.
  Name(NameTest),
  /// `node()`: Any node.
  Node,
  /// `text()`: Text nodes.
  Text,
  /// `comment()`: Comments.
  Comment,
  /// `processing-instruction()`, or `processing-instruction('target')` to specify a particular target.
  ProcessingInstruction(Option<String>),
}

impl fmt::Display for NodeTest {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NodeTest::Name(name) => write!(f, "{name}"),
      NodeTest::Node => f.write_str("node()"),
      NodeTest::Text => f.write_str("text()"),
      NodeTest::Comment => f.write_str("comment()"),
      NodeTest::ProcessingInstruction(None) => f.write_str("processing-instruction()"),
      NodeTest::ProcessingInstruction(Some(target)) => {
        write!(f, "processing-instruction({})", Literal(target))
      }
    }
  }
}

/// A path step that selects nodes along an axis, tests each node, and applies predicates in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Step {
  /// The axis followed by the step.
  pub axis: Axis,
  /// The test applied to the nodes reached by the axis.
  pub node_test: NodeTest,
  /// The predicates applied sequentially; each predicate is applied to the result of the filtering performed by the
  /// preceding predicate.
  pub predicates: Vec<Expr>,
}

/// The starting position of the path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PathStart {
  /// An absolute path: The root of the tree containing the context node.
  Root,
  /// A relative path: The context node.
  Context,
  /// Continue from each node in the node-set returned by the expression. Examples: `$x/a`, `f()/a`.
  Expr(Box<Expr>),
}

/// A location path consists of a starting position and a sequence of steps followed in order from that position.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Path {
  /// The path's starting position.
  pub start: PathStart,
  /// The steps applied sequentially, starting from the initial position.
  pub steps: Vec<Step>,
  /// The byte offset in the expression where the path begins, which an evaluation error of the path is located at.
  pub at: usize,
}

/// A binary operator that combines two expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryOp {
  /// `or`: Logical disjunction.
  Or,
  /// `and`: Logical conjunction.
  And,
  /// `=`: Tests whether the operands are equal.
  Equal,
  /// `!=`: Tests whether the operands are unequal.
  NotEqual,
  /// `<`: Tests whether the left operand is less than the right operand.
  Less,
  /// `<=`: Tests whether the left operand is less than or equal to the right operand.
  LessEqual,
  /// `>`: Tests whether the left operand is greater than the right operand.
  Greater,
  /// `>=`: Tests whether the left operand is greater than or equal to the right operand.
  GreaterEqual,
  /// `+`: Adds the operands.
  Add,
  /// `-`: Subtracts the right operand from the left operand.
  Subtract,
  /// `*`: Multiplies the operands.
  Multiply,
  /// `div`: Divides the left operand by the right operand.
  Divide,
  /// `mod`: Returns the remainder when the left operand is divided by the right operand.
  Modulo,
  /// `|`: Returns the union of two node-sets.
  Union,
}

impl BinaryOp {
  /// Returns the operator symbol or keyword used in an XPath expression.
  pub(crate) const fn symbol(self) -> &'static str {
    match self {
      BinaryOp::Or => "or",
      BinaryOp::And => "and",
      BinaryOp::Equal => "=",
      BinaryOp::NotEqual => "!=",
      BinaryOp::Less => "<",
      BinaryOp::LessEqual => "<=",
      BinaryOp::Greater => ">",
      BinaryOp::GreaterEqual => ">=",
      BinaryOp::Add => "+",
      BinaryOp::Subtract => "-",
      BinaryOp::Multiply => "*",
      BinaryOp::Divide => "div",
      BinaryOp::Modulo => "mod",
      BinaryOp::Union => "|",
    }
  }
}

impl fmt::Display for BinaryOp {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.symbol())
  }
}

/// An XPath 1.0 expression.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expr {
  /// Two expressions joined by a binary operator.
  Binary {
    /// The operator joining the operands.
    op: BinaryOp,
    /// The left operand.
    left: Box<Expr>,
    /// The right operand.
    right: Box<Expr>,
    /// The byte offset of the operator in the expression, which an evaluation error of the operator is located at.
    at: usize,
  },
  /// Unary minus applied to an expression.
  Negate(Box<Expr>),
  /// A location path.
  Path(Path),
  /// An expression followed by predicates that filter the node-set it returns.
  Filter {
    /// The expression whose node-set is filtered.
    expr: Box<Expr>,
    /// Predicates applied in order; each filters the preceding predicate's result.
    predicates: Vec<Expr>,
    /// The byte offset in the expression where the filtered expression begins, which an evaluation error of the
    /// filter is located at.
    at: usize,
  },
  /// A string literal.
  Literal(String),
  /// A number stored as a 64-bit floating-point value.
  Number(f64),
  /// A variable reference such as `$name`.
  Variable {
    /// The prefix from the input, or `None` if the name was unprefixed.
    prefix: Option<String>,
    /// The local part of the variable name.
    local: String,
    /// The byte offset of the `$` in the expression, which an evaluation error of the reference is located at.
    at: usize,
  },
  /// A function call.
  Function {
    /// The prefix from the input, or `None` if the name was unprefixed.
    prefix: Option<String>,
    /// The local part of the function name.
    local: String,
    /// Arguments in call order.
    arguments: Vec<Expr>,
    /// The byte offset of the function's name in the expression, which an evaluation error of the call is located at.
    at: usize,
  },
}

/// Returns an expression in expanded form, but parentheses are added if it is embedded within another expression.
///
/// Parentheses are required when concatenation with surrounding expressions would alter the results of tokenization or
/// parsing. This determination is based on the leading and trailing characters of the output string, rather than on
/// AST variants.
///
/// **The output ends with `/`:** This occurs in the case of the root path `/`. Under XPath 1.0 tokenization rules,
/// `*`, `mod`, `div`, `and`, or `or` immediately following a `/` are interpreted as *name tests* rather than
/// operators. Therefore, if `(/) * b` were output as `/ * child::b`, the `*` would be treated as a wildcard, changing
/// the meaning of the expression.
///
/// **The output begins with `-`:** In XPath 1.0, a union expression can serve as the operand for a unary minus
/// operator; thus, `-a | b` signifies `-(a | b)`. Parentheses must be preserved for union expressions where the
/// left-hand operand is negated. Otherwise, outputting `Union(Negate(a), b)` as `-a | b` would alter the grouping
/// semantics.
fn embedded(expr: &Expr) -> String {
  let printed = print(expr, true);
  if printed.ends_with('/') || printed.starts_with('-') { format!("({printed})") } else { printed }
}

impl fmt::Display for Expr {
  /// Writes the expression as an XPath. The default format `{}` uses the *abbreviated form*, while the alternative
  /// format `{:#}` uses the *expanded form*.
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&print(self, f.alternate()))
  }
}

/// Binding strength of unary minus in [`precedence`].
const UNARY: u8 = 7;
/// Binding strength of union in [`precedence`].
const UNION: u8 = 8;
/// Binding strength of a path expression in [`precedence`].
const PATH: u8 = 9;
/// Binding strength of primary and filter expressions in [`precedence`].
const PRIMARY: u8 = 10;

/// Returns the binding strength of the expression. A higher value indicates a stronger binding, and the order follows
/// the operator precedence rules of XPath 1.0.
///
/// A negative finite value stored directly as a `Number` is prefixed with a `-` upon output, so it shares the same
/// binding strength as a unary minus from an output perspective.
fn precedence(expr: &Expr) -> u8 {
  match expr {
    Expr::Binary { op, .. } => match op {
      BinaryOp::Or => 1,
      BinaryOp::And => 2,
      BinaryOp::Equal | BinaryOp::NotEqual => 3,
      BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => 4,
      BinaryOp::Add | BinaryOp::Subtract => 5,
      BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 6,
      BinaryOp::Union => UNION,
    },
    Expr::Negate(_) => UNARY,
    Expr::Number(value) if value.is_sign_negative() && value.is_finite() => UNARY,
    Expr::Path(_) => PATH,
    Expr::Filter { .. } | Expr::Literal(_) | Expr::Number(_) | Expr::Variable { .. } | Expr::Function { .. } => PRIMARY,
  }
}

/// Returns the XPath text for an expression.
///
/// If `full` is `true`, axis names are explicitly stated for each step, and all binary expressions are enclosed in
/// parentheses. Otherwise, the output uses abbreviations such as `a`, `@x`, `.`, `..`, and `//`, and parentheses are
/// omitted only when the AST is preserved upon re-parsing.
fn print(expr: &Expr, full: bool) -> String {
  match expr {
    // Explicitly enclose the priorities determined by the parser in parentheses.
    Expr::Binary { op, left, right, .. } if full => format!("({} {op} {})", embedded(left), embedded(right)),
    Expr::Binary { op, left, right, .. } => {
      // The operators are left-associative, so parentheses are required only on the right side for operands that bind
      // with the same precedence; in other words, `1 - (2 - 3)` is different from `1 - 2 - 3`.
      let level = precedence(expr);
      format!("{} {op} {}", operand(left, level), operand(right, level + 1))
    }
    Expr::Negate(inner) if full => format!("-{}", embedded(inner)),
    Expr::Negate(inner) => format!("-{}", operand(inner, UNARY)),
    Expr::Path(path) => print_path(path, full),
    Expr::Filter { expr, predicates, .. } => {
      // A predicate following a path binds to the last step of that path, yet the two constructs have different
      // meanings. For example, `(//a)[1]` selects the first of all `a` elements, whereas `//a[1]` selects the first
      // `a` element under each parent element. Consequently, in a path containing steps, parentheses are preserved
      // here — even in cases where the `embedded` processing would not otherwise add them. Since this is a matter of
      // grammar rather than the interpretation of adjacent characters, it is handled at this stage rather than during
      // `embedded` processing. Other elements that may be accompanied by predicates (such as variables, function
      // calls, or literals) are parsed as-is, without parentheses.
      let path_absorbs_it = matches!(&**expr, Expr::Path(path) if !path.steps.is_empty());
      let mut printed = if !full {
        operand(expr, PRIMARY)
      } else if path_absorbs_it {
        format!("({})", print(expr, true))
      } else {
        embedded(expr)
      };
      push_predicates(&mut printed, predicates, full);
      printed
    }
    Expr::Literal(value) => string(value),
    Expr::Number(value) => number(*value),
    Expr::Variable { prefix: Some(prefix), local, .. } => format!("${prefix}:{local}"),
    Expr::Variable { prefix: None, local, .. } => format!("${local}"),
    Expr::Function { prefix, local, arguments, .. } => {
      let arguments: Vec<String> = arguments.iter().map(|argument| print(argument, full)).collect();
      match prefix {
        Some(prefix) => format!("{prefix}:{local}({})", arguments.join(", ")),
        None => format!("{local}({})", arguments.join(", ")),
      }
    }
  }
}

/// Prints the expression using an abbreviated form. `level` represents the minimum binding strength required by the
/// surrounding context.
///
/// Expressions with a binding strength weaker than `level` are enclosed in parentheses. Parentheses are also used if
/// the output ends with `/`; this prevents a subsequent `*`, `div`, or similar token from being interpreted as a name
/// test (see [`embedded`]).
fn operand(expr: &Expr, level: u8) -> String {
  let printed = print(expr, false);
  if precedence(expr) < level || printed.ends_with('/') { format!("({printed})") } else { printed }
}

/// Prints the path. The meaning of `full` is the same as for [`print`](fn@print).
fn print_path(path: &Path, full: bool) -> String {
  let mut printed = match &path.start {
    // A standalone `/` selects the root. When followed by a step, the leading `/` serves to separate the root from the
    // first step.
    PathStart::Root if path.steps.is_empty() => return "/".to_owned(),
    PathStart::Root | PathStart::Context => String::new(),
    // When printing a path that follows the root `(/)`, a `/` is output first, and another `/` is placed before the
    // path's initial step, resulting in `//`. This is interpreted as an abbreviation for
    // `/descendant-or-self::node()/`, which differs from the original tree structure. Therefore, in `embedded`, the
    // root is enclosed in parentheses — similar to how an expression is handled when output within another expression.
    PathStart::Expr(expr) if full => embedded(expr),
    PathStart::Expr(expr) => operand(expr, PRIMARY),
  };
  let mut abbreviated = false;
  for (index, step) in path.steps.iter().enumerate() {
    let separator = index > 0 || !matches!(path.start, PathStart::Context);
    if separator {
      printed.push('/');
    }
    // When `descendant-or-self::node()` is placed between two steps, it is not explicitly written out; instead, the
    // preceding and following separators combine to form `//`. This requires a separator immediately beforehand
    // (otherwise, it would be interpreted as an absolute path) and a step immediately following it; furthermore, there
    // must not be an abbreviated separator immediately preceding it, as that would result in three consecutive slashes.
    abbreviated = !full
      && separator
      && !abbreviated
      && index + 1 < path.steps.len()
      && step.axis == Axis::DescendantOrSelf
      && step.node_test == NodeTest::Node
      && step.predicates.is_empty();
    if !abbreviated {
      printed.push_str(&print_step(step, full));
    }
  }
  printed
}

/// Prints the step. If `full` is true, the axis is explicitly specified.
fn print_step(step: &Step, full: bool) -> String {
  // `.` and `..` cannot take predicates, so a step with a predicate retains its own axis.
  let bare = step.predicates.is_empty();
  let mut printed = match (step.axis, &step.node_test) {
    (axis, test) if full => format!("{axis}::{test}"),
    (Axis::SelfAxis, NodeTest::Node) if bare => ".".to_owned(),
    (Axis::Parent, NodeTest::Node) if bare => "..".to_owned(),
    (Axis::Child, test) => test.to_string(),
    (Axis::Attribute, test) => format!("@{test}"),
    (axis, test) => format!("{axis}::{test}"),
  };
  push_predicates(&mut printed, &step.predicates, full);
  printed
}

/// Enclose each predicate in square brackets and add them to `printed` in order.
fn push_predicates(printed: &mut String, predicates: &[Expr], full: bool) {
  for predicate in predicates {
    printed.push('[');
    printed.push_str(&print(predicate, full));
    printed.push(']');
  }
}

/// Converts a numeric value into its XPath text representation.
///
/// Finite values are written as numeric literals. Values that cannot be directly represented as XPath numeric
/// literals are expressed via expressions. Positive infinity is written as `18` followed by 307 zeros; this value
/// exceeds the maximum finite value representable by a 64-bit floating-point number, resulting in positive infinity
/// in XPath numeric conversion.
fn number(value: f64) -> String {
  if value.is_nan() {
    "(0 div 0)".to_owned()
  } else if value == f64::INFINITY {
    format!("18{}", "0".repeat(307))
  } else if value == f64::NEG_INFINITY {
    "(-1 div 0)".to_owned()
  } else {
    value.to_string()
  }
}

/// Returns an XPath expression that represents `value` as a string.
///
/// XPath 1.0 string literals cannot escape their delimiter. If `value` contains both quote characters, this function
/// splits it into chunks so each chunk can be written with a delimiter that does not occur in that chunk, then joins
/// the literals with `concat`. A split is made when the current chunk already contains the other quote character.
/// Since the input contains both kinds of quote, the result has at least two arguments, as required by `concat`.
fn string(value: &str) -> String {
  if !(value.contains('\'') && value.contains('"')) {
    return Literal(value).to_string();
  }
  let mut parts: Vec<String> = Vec::new();
  let mut part = String::new();
  for c in value.chars() {
    let conflicts = match c {
      '\'' => part.contains('"'),
      '"' => part.contains('\''),
      _ => false,
    };
    if conflicts {
      parts.push(Literal(&part).to_string());
      part.clear();
    }
    part.push(c);
  }
  parts.push(Literal(&part).to_string());
  format!("concat({})", parts.join(", "))
}

/// Select a quote character that is not contained in the value, and write out the XPath string literal.
struct Literal<'a>(&'a str);

impl fmt::Display for Literal<'_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if self.0.contains('\'') { write!(f, "\"{}\"", self.0) } else { write!(f, "'{}'", self.0) }
  }
}
