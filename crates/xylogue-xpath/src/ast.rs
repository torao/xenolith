//! The expression tree an XPath 1.0 expression parses to.
//!
//! The tree keeps the structure of the grammar but drops its abbreviations: `//` becomes a
//! `descendant-or-self::node()` step, `.` and `..` become `self::node()` and `parent::node()`,
//! `@x` becomes `attribute::x`, and a step with no axis becomes `child::`. What is left is the
//! form the evaluator walks — every step an axis, a node test and its predicates.
//!
//! The [`Display`](std::fmt::Display) of a tree writes that unabbreviated form back, which is
//! valid XPath and shows exactly what was parsed; binary expressions are parenthesized so
//! precedence is visible.

use std::fmt;

/// One of the thirteen axes an XPath step may walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
  /// `ancestor`: the parent, its parent, and so on to the root.
  Ancestor,
  /// `ancestor-or-self`: the node itself, then its ancestors.
  AncestorOrSelf,
  /// `attribute`: the attributes of an element.
  Attribute,
  /// `child`: the immediate children.
  Child,
  /// `descendant`: the children, their children, and so on.
  Descendant,
  /// `descendant-or-self`: the node itself, then its descendants.
  DescendantOrSelf,
  /// `following`: everything after the node in document order, bar its descendants.
  Following,
  /// `following-sibling`: the siblings after the node.
  FollowingSibling,
  /// `namespace`: the namespace nodes of an element.
  Namespace,
  /// `parent`: the parent, if any.
  Parent,
  /// `preceding`: everything before the node in document order, bar its ancestors.
  Preceding,
  /// `preceding-sibling`: the siblings before the node.
  PrecedingSibling,
  /// `self`: the node itself.
  SelfAxis,
}

impl Axis {
  /// The axis an `AxisName` names, or `None` if the name is not one of the thirteen.
  #[must_use]
  pub fn from_name(name: &str) -> Option<Self> {
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

  /// The name of the axis, as it is written.
  #[must_use]
  pub const fn name(self) -> &'static str {
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

/// A test on the name of a node, for a step that tests names rather than kinds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameTest {
  /// `*`: any name.
  Any,
  /// `prefix:*`: any name in the namespace the prefix binds.
  Namespace(String),
  /// A qualified name, with a prefix if it was written with one.
  Name {
    /// The prefix, if the name was written with one.
    prefix: Option<String>,
    /// The local part.
    local: String,
  },
}

impl fmt::Display for NameTest {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      NameTest::Any => f.write_str("*"),
      NameTest::Namespace(prefix) => write!(f, "{prefix}:*"),
      NameTest::Name { prefix: Some(prefix), local } => write!(f, "{prefix}:{local}"),
      NameTest::Name { prefix: None, local } => f.write_str(local),
    }
  }
}

/// What a step selects from the nodes an axis reaches: a name, or a kind of node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeTest {
  /// A name test, which also restricts to the axis's principal node type.
  Name(NameTest),
  /// `node()`: any node at all.
  Node,
  /// `text()`: text nodes.
  Text,
  /// `comment()`: comments.
  Comment,
  /// `processing-instruction()`, or `processing-instruction('target')` for one target.
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

/// One step of a path: an axis, a test on what it reaches, and the predicates that filter it.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
  /// The axis the step walks.
  pub axis: Axis,
  /// The test applied to the nodes the axis reaches.
  pub node_test: NodeTest,
  /// The predicates, applied in order, each to what the previous one left.
  pub predicates: Vec<Expr>,
}

impl fmt::Display for Step {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}::{}", self.axis, self.node_test)?;
    for predicate in &self.predicates {
      write!(f, "[{predicate}]")?;
    }
    Ok(())
  }
}

/// Where a path begins.
#[derive(Clone, Debug, PartialEq)]
pub enum PathStart {
  /// An absolute path: at the root of the tree holding the context node.
  Root,
  /// A relative path: at the context node.
  Context,
  /// At each node of the node-set another expression yields (`$x/a`, `f()/a`).
  Expr(Box<Expr>),
}

/// A path: a starting point and the steps walked from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
  /// Where the path begins.
  pub start: PathStart,
  /// The steps, walked in order.
  pub steps: Vec<Step>,
}

impl fmt::Display for Path {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match &self.start {
      // A lone `/` selects the root; with steps, the leading slash separates it from the first.
      PathStart::Root if self.steps.is_empty() => return f.write_str("/"),
      PathStart::Root => {}
      PathStart::Context => {}
      // `(/)` prints as `/`, and the separator before the first step would then make `//` —
      // which reads back as the abbreviation for `/descendant-or-self::node()/`, a different
      // tree. See `embedded`, which is the same guard every other embedding site uses.
      PathStart::Expr(expr) => f.write_str(&embedded(expr))?,
    }
    for (index, step) in self.steps.iter().enumerate() {
      let separator = index > 0 || !matches!(self.start, PathStart::Context);
      if separator {
        f.write_str("/")?;
      }
      write!(f, "{step}")?;
    }
    Ok(())
  }
}

/// The operators that join two expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
  /// `or`
  Or,
  /// `and`
  And,
  /// `=`
  Equal,
  /// `!=`
  NotEqual,
  /// `<`
  Less,
  /// `<=`
  LessEqual,
  /// `>`
  Greater,
  /// `>=`
  GreaterEqual,
  /// `+`
  Add,
  /// `-`
  Subtract,
  /// `*`
  Multiply,
  /// `div`
  Divide,
  /// `mod`
  Modulo,
  /// `|`, the union of two node-sets.
  Union,
}

impl BinaryOp {
  /// The operator as it is written.
  #[must_use]
  pub const fn symbol(self) -> &'static str {
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
pub enum Expr {
  /// Two expressions joined by an operator.
  Binary {
    /// The operator.
    op: BinaryOp,
    /// The left operand.
    left: Box<Expr>,
    /// The right operand.
    right: Box<Expr>,
  },
  /// Unary minus.
  Negate(Box<Expr>),
  /// A path.
  Path(Path),
  /// An expression with predicates applied to the node-set it yields.
  Filter {
    /// The expression being filtered.
    expr: Box<Expr>,
    /// The predicates, applied in order.
    predicates: Vec<Expr>,
  },
  /// A string literal.
  Literal(String),
  /// A number.
  Number(f64),
  /// A variable reference, `$name`.
  Variable {
    /// The prefix, if the name was written with one.
    prefix: Option<String>,
    /// The local part.
    local: String,
  },
  /// A function call.
  Function {
    /// The prefix, if the name was written with one.
    prefix: Option<String>,
    /// The local part.
    local: String,
    /// The arguments, in order.
    arguments: Vec<Expr>,
  },
}

/// A subexpression's text, parenthesised where it could run into what is printed beside it.
///
/// XPath's lexer decides what `*`, `mod`, `div`, `and` and `or` mean from the token before them:
/// after an operator they are name tests rather than operators, and `/` counts as an operator
/// (XPath 1.0 §3.7). So an operand whose text ends with `/` — which only the root path does —
/// changes the meaning of whatever is printed next to it: `(/) * b` printed as `/ * child::b`
/// reads back as the path `/child::*` followed by a stray name.
///
/// Every place that embeds one expression in another goes through here rather than each judging
/// for itself. The judgement is on the printed text rather than on which shapes can end in a
/// slash, because deciding that by eye is what got this wrong twice: once for a path's starting
/// point, and again for an operand and a filter.
fn embedded(expr: &Expr) -> String {
  let printed = expr.to_string();
  if printed.ends_with('/') { format!("({printed})") } else { printed }
}

impl fmt::Display for Expr {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      // Parenthesized so the precedence the parser settled on is plain to see.
      Expr::Binary { op, left, right } => write!(f, "({} {op} {})", embedded(left), embedded(right)),
      Expr::Negate(expr) => write!(f, "-{}", embedded(expr)),
      Expr::Path(path) => write!(f, "{path}"),
      Expr::Filter { expr, predicates } => {
        f.write_str(&embedded(expr))?;
        for predicate in predicates {
          write!(f, "[{predicate}]")?;
        }
        Ok(())
      }
      Expr::Literal(value) => write!(f, "{}", Literal(value)),
      Expr::Number(value) => write!(f, "{value}"),
      Expr::Variable { prefix: Some(prefix), local } => write!(f, "${prefix}:{local}"),
      Expr::Variable { prefix: None, local } => write!(f, "${local}"),
      Expr::Function { prefix, local, arguments } => {
        if let Some(prefix) = prefix {
          write!(f, "{prefix}:")?;
        }
        write!(f, "{local}(")?;
        for (index, argument) in arguments.iter().enumerate() {
          if index > 0 {
            f.write_str(", ")?;
          }
          write!(f, "{argument}")?;
        }
        f.write_str(")")
      }
    }
  }
}

/// Writes a string literal in quotes, choosing the quote the value does not contain.
struct Literal<'a>(&'a str);

impl fmt::Display for Literal<'_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if self.0.contains('\'') { write!(f, "\"{}\"", self.0) } else { write!(f, "'{}'", self.0) }
  }
}
