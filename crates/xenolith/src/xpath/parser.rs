//! Parses XPath tokens into an expression tree.
//!
//! The parser is a recursive-descent implementation of the XPath 1.0 grammar. Its expression functions follow the
//! grammar's precedence levels, from `or` down to individual location steps. It expands abbreviated syntax while
//! parsing, producing the normalized AST described in [`ast`](crate::xpath::ast).

use crate::error::{Error, Location, Result as XResult};
use crate::xpath::ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
use crate::xpath::context::namespace_of;
use crate::xpath::lexer::{NodeTypeName, Spanned, Token};
use crate::xpath::{Failure, functions, located};

/// Binary operators grouped by precedence, from lowest to highest.
const LEVELS: &[&[(Token, BinaryOp)]] = &[
  &[(Token::Or, BinaryOp::Or)],
  &[(Token::And, BinaryOp::And)],
  &[(Token::Equal, BinaryOp::Equal), (Token::NotEqual, BinaryOp::NotEqual)],
  &[
    (Token::LessEqual, BinaryOp::LessEqual),
    (Token::GreaterEqual, BinaryOp::GreaterEqual),
    (Token::Less, BinaryOp::Less),
    (Token::Greater, BinaryOp::Greater),
  ],
  &[(Token::Plus, BinaryOp::Add), (Token::Minus, BinaryOp::Subtract)],
  &[(Token::Multiply, BinaryOp::Multiply), (Token::Div, BinaryOp::Divide), (Token::Mod, BinaryOp::Modulo)],
];

/// Parses all tokens in `expression` as one XPath expression. `location` marks the expression's start, and
/// `max_depth` limits its nesting depth.
///
/// If recursive parsing reaches the limit, the error points to the current token. If the completed AST exceeds the
/// limit, the error points to the start of the expression.
pub(crate) fn parse(
  tokens: &[Spanned],
  expression: &str,
  location: &Location,
  max_depth: Option<usize>,
) -> XResult<Expr> {
  let mut parser = Parser { tokens, at: 0, length: expression.len(), depth: 0, max_depth, too_deep: false };
  let parsed = parser.expr().and_then(|expr| match parser.peek() {
    None => Ok(expr),
    Some(token) => {
      let message = format!("unexpected {} after a complete expression", describe(token));
      Err(parser.error(message))
    }
  });
  let expr = match parsed {
    Ok(expr) => expr,
    Err((message, byte)) if parser.too_deep => {
      return Err(Error::limit(message).at(located(location, expression, byte)));
    }
    Err((message, byte)) => return Err(Error::xpath(message).at(located(location, expression, byte))),
  };
  if let Some(max) = max_depth.filter(|&max| exceeds(&expr, max)) {
    return Err(Error::limit(too_deep(max)).at(location.clone()));
  }
  Ok(expr)
}

/// The message of an expression that nests deeper than `max`.
fn too_deep(max: usize) -> String {
  format!("the XPath expression nests deeper than {max} levels; raise xpath::Limits::max_depth")
}

/// Whether `expr` is deeper than `max`, counting `expr` itself as level 1. An explicit stack avoids recursively
/// walking the deeply nested tree being checked.
fn exceeds(expr: &Expr, max: usize) -> bool {
  let mut pending = vec![(expr, 1)];
  while let Some((expr, depth)) = pending.pop() {
    if depth > max {
      return true;
    }
    let below = depth + 1;
    match expr {
      Expr::Binary { left, right, .. } => pending.extend([(&**left, below), (&**right, below)]),
      Expr::Negate(operand) => pending.push((operand, below)),
      Expr::Path(path) => {
        if let PathStart::Expr(start) = &path.start {
          pending.push((start, below));
        }
        pending.extend(path.steps.iter().flat_map(|step| &step.predicates).map(|predicate| (predicate, below)));
      }
      Expr::Filter { expr, predicates, .. } => {
        pending.push((expr, below));
        pending.extend(predicates.iter().map(|predicate| (predicate, below)));
      }
      Expr::Function { arguments, .. } => pending.extend(arguments.iter().map(|argument| (argument, below))),
      Expr::Literal(_) | Expr::Number(_) | Expr::Variable { .. } => {}
    }
  }
  false
}

struct Parser<'a> {
  tokens: &'a [Spanned],
  at: usize,
  /// Input length in bytes, used as the error offset when no token remains.
  length: usize,
  /// How many expressions the parser is inside: the whole one, and each parenthesized one, predicate, argument and
  /// operand of a unary minus around the cursor.
  depth: usize,
  /// The deepest `depth` may go, or `None` for no limit.
  max_depth: Option<usize>,
  /// Whether parsing stopped because `depth` went past `max_depth`, which is a limit error rather than a syntax error.
  too_deep: bool,
}

impl Parser<'_> {
  fn peek(&self) -> Option<&Token> {
    self.tokens.get(self.at).map(|spanned| &spanned.token)
  }

  fn bump(&mut self) {
    self.at += 1;
  }

  /// Consumes the next token and returns `true` if it equals `expected`; otherwise leaves the cursor unchanged.
  fn eat(&mut self, expected: &Token) -> bool {
    if self.peek() == Some(expected) {
      self.at += 1;
      return true;
    }
    false
  }

  /// Requires the next token to equal `expected` and consumes it. Returns an error if it does not.
  fn expect(&mut self, expected: &Token) -> Result<(), Failure> {
    if self.eat(expected) {
      return Ok(());
    }
    let message = format!("expected {} but found {}", describe(expected), self.found());
    Err(self.error(message))
  }

  /// An error saying what was `expected` where the cursor is, and what was found there instead.
  fn expected(&self, expected: &str) -> Failure {
    self.error(format!("expected {expected}, found {}", self.found()))
  }

  /// Describes the current token, or end of input, for an error message.
  fn found(&self) -> String {
    self.peek().map_or_else(|| "the end of the expression".to_owned(), describe)
  }

  /// Returns the next token's byte offset, or the input length when no token remains.
  fn position(&self) -> usize {
    self.tokens.get(self.at).map_or(self.length, |spanned| spanned.at)
  }

  fn error(&self, message: impl Into<String>) -> Failure {
    (message.into(), self.position())
  }

  /// Fails if `prefix` is not bound to a namespace. Which prefixes are bound is settled before the expression is
  /// evaluated, so an unbound one is refused here, where it is written, whatever document the expression is used on.
  fn bound(&self, prefix: &str) -> Result<(), Failure> {
    if namespace_of(prefix).is_some() {
      return Ok(());
    }
    Err(self.error(format!("the prefix \"{prefix}\" is not bound")))
  }

  /// Goes one expression deeper, or fails if that is deeper than the limit.
  fn descend(&mut self) -> Result<(), Failure> {
    self.depth += 1;
    match self.max_depth {
      Some(max) if self.depth > max => {
        self.too_deep = true;
        Err(self.error(too_deep(max)))
      }
      _ => Ok(()),
    }
  }

  // --- Precedence levels --------------------------------------------------------------------

  fn expr(&mut self) -> Result<Expr, Failure> {
    self.descend()?;
    let expr = self.binary(0);
    self.depth -= 1;
    expr
  }

  /// Parses binary operators of precedence level `lowest` and tighter, with the operands below them.
  ///
  /// Parses all binary precedence levels in one function rather than using a separate recursive function for each
  /// level. With one function per level, each additional parenthesis would add a call for every precedence level,
  /// increasing stack use. `lowest` is the minimum precedence handled by this call. The right operand is parsed from
  /// the next tighter level, so tighter operators are grouped within that operand. Operators at the same level are
  /// processed in a loop, producing a left-associative syntax tree.
  fn binary(&mut self, lowest: usize) -> Result<Expr, Failure> {
    let mut left = self.unary()?;
    while let Some((level, op)) = self.peek().and_then(binary_operator).filter(|&(level, _)| level >= lowest) {
      let at = self.position();
      self.bump();
      let right = self.binary(level + 1)?;
      left = Expr::Binary { op, left: Box::new(left), right: Box::new(right), at };
    }
    Ok(left)
  }

  fn unary(&mut self) -> Result<Expr, Failure> {
    if self.eat(&Token::Minus) {
      self.descend()?;
      let operand = self.unary();
      self.depth -= 1;
      return Ok(Expr::Negate(Box::new(operand?)));
    }
    self.union()
  }

  fn union(&mut self) -> Result<Expr, Failure> {
    let mut left = self.path()?;
    loop {
      let at = self.position();
      if !self.eat(&Token::Pipe) {
        return Ok(left);
      }
      let right = self.path()?;
      left = Expr::Binary { op: BinaryOp::Union, left: Box::new(left), right: Box::new(right), at };
    }
  }

  // --- Paths --------------------------------------------------------------------------------

  /// Parses either a location path or a filter expression followed by path steps.
  fn path(&mut self) -> Result<Expr, Failure> {
    if !self.starts_primary() {
      return Ok(Expr::Path(self.location_path()?));
    }
    let at = self.position();
    let filter = self.filter()?;
    self.filter_path(filter, at)
  }

  /// Parses the steps that may follow a filter expression, which begins at `at`, and makes the path they walk from it.
  fn filter_path(&mut self, filter: Expr, at: usize) -> Result<Expr, Failure> {
    // A filter expression may be followed by `/` or `//`; the remaining steps are evaluated from its node-set.
    let steps = if self.eat(&Token::Slash) {
      self.relative_steps()?
    } else if self.eat(&Token::DoubleSlash) {
      let mut steps = vec![descendant_or_self()];
      steps.extend(self.relative_steps()?);
      steps
    } else {
      return Ok(filter);
    };
    // `(P)/Q` and `P/Q` have the same path semantics when `P` is a location path. Splice those steps into one path so
    // both spellings produce the same AST and print back consistently. Do not splice a filter such as `(P)[1]/Q`:
    // its predicate applies to the node-set from `P` before `Q` is evaluated.
    if let Expr::Path(inner) = filter {
      let mut spliced = inner.steps;
      spliced.extend(steps);
      return Ok(Expr::Path(Path { start: inner.start, steps: spliced, at }));
    }
    Ok(Expr::Path(Path { start: PathStart::Expr(Box::new(filter)), steps, at }))
  }

  /// Returns whether the next token can begin a filter expression rather than a location path.
  fn starts_primary(&self) -> bool {
    matches!(
      self.peek(),
      Some(Token::Variable { .. } | Token::LeftParen | Token::Literal(_) | Token::Number(_) | Token::Function { .. })
    )
  }

  fn location_path(&mut self) -> Result<Path, Failure> {
    let at = self.position();
    if self.eat(&Token::DoubleSlash) {
      let mut steps = vec![descendant_or_self()];
      steps.extend(self.relative_steps()?);
      return Ok(Path { start: PathStart::Root, steps, at });
    }
    if self.eat(&Token::Slash) {
      // `/` alone selects the root. If a step can start here, parse the remainder as an absolute path.
      let steps = if self.starts_step() { self.relative_steps()? } else { Vec::new() };
      return Ok(Path { start: PathStart::Root, steps, at });
    }
    Ok(Path { start: PathStart::Context, steps: self.relative_steps()?, at })
  }

  /// Parses one or more steps separated by `/` or `//`.
  fn relative_steps(&mut self) -> Result<Vec<Step>, Failure> {
    let mut steps = vec![self.step()?];
    loop {
      if self.eat(&Token::Slash) {
        steps.push(self.step()?);
      } else if self.eat(&Token::DoubleSlash) {
        steps.push(descendant_or_self());
        steps.push(self.step()?);
      } else {
        return Ok(steps);
      }
    }
  }

  /// Returns whether the next token can begin a location step.
  fn starts_step(&self) -> bool {
    matches!(
      self.peek(),
      Some(
        Token::Dot
          | Token::DotDot
          | Token::At
          | Token::Axis(_)
          | Token::Star
          | Token::Name { .. }
          | Token::NamespaceWildcard(_)
          | Token::NodeType(_)
      )
    )
  }

  fn step(&mut self) -> Result<Step, Failure> {
    // The `.` and `..` abbreviations already specify their axis and node test and cannot take predicates.
    if self.eat(&Token::Dot) {
      return Ok(Step { axis: Axis::SelfAxis, node_test: NodeTest::Node, predicates: Vec::new() });
    }
    if self.eat(&Token::DotDot) {
      return Ok(Step { axis: Axis::Parent, node_test: NodeTest::Node, predicates: Vec::new() });
    }
    let axis = match self.peek() {
      Some(Token::At) => {
        self.bump();
        Axis::Attribute
      }
      Some(Token::Axis(axis)) => {
        let axis = *axis;
        self.bump();
        self.expect(&Token::ColonColon)?;
        axis
      }
      _ => Axis::Child,
    };
    let node_test = self.node_test()?;
    let predicates = self.predicates()?;
    Ok(Step { axis, node_test, predicates })
  }

  fn node_test(&mut self) -> Result<NodeTest, Failure> {
    let test = match self.peek() {
      Some(Token::Star) => NodeTest::Name(NameTest::Any),
      Some(Token::NamespaceWildcard(prefix)) => {
        self.bound(prefix)?;
        NodeTest::Name(NameTest::AnyLocal(prefix.clone()))
      }
      Some(Token::Name { prefix, local }) => {
        if let Some(prefix) = prefix {
          self.bound(prefix)?;
        }
        NodeTest::Name(NameTest::Exact { prefix: prefix.clone(), local: local.clone() })
      }
      Some(Token::NodeType(node_type)) => {
        let node_type = *node_type;
        return self.node_type_test(node_type);
      }
      _ => return Err(self.expected("a name or a node test")),
    };
    self.bump();
    Ok(test)
  }

  /// Parses a node type test, `node()` and the like, from its name on.
  fn node_type_test(&mut self, node_type: NodeTypeName) -> Result<NodeTest, Failure> {
    self.bump();
    self.expect(&Token::LeftParen)?;
    let test = match node_type {
      NodeTypeName::Node => NodeTest::Node,
      NodeTypeName::Text => NodeTest::Text,
      NodeTypeName::Comment => NodeTest::Comment,
      // A literal argument restricts the test to one processing-instruction target.
      NodeTypeName::ProcessingInstruction => match self.peek().cloned() {
        Some(Token::Literal(target)) => {
          self.bump();
          NodeTest::ProcessingInstruction(Some(target))
        }
        _ => NodeTest::ProcessingInstruction(None),
      },
    };
    self.expect(&Token::RightParen)?;
    Ok(test)
  }

  /// Parses the consecutive predicates following a step or filter expression.
  fn predicates(&mut self) -> Result<Vec<Expr>, Failure> {
    let mut predicates = Vec::new();
    while self.eat(&Token::LeftBracket) {
      predicates.push(self.expr()?);
      self.expect(&Token::RightBracket)?;
    }
    Ok(predicates)
  }

  // --- Primary expressions ------------------------------------------------------------------

  /// Parses a primary expression and its following predicates as one filter expression.
  ///
  /// Filter predicates apply from left to right, each to the node-set left by the preceding predicate. Thus
  /// `((E)[1])[2]` and `(E)[1][2]` have the same meaning. Fold their predicates into one `Filter` node so equivalent
  /// spellings produce the same AST and the printer can round-trip either form.
  fn filter(&mut self) -> Result<Expr, Failure> {
    let at = self.position();
    let primary = self.primary()?;
    let predicates = self.predicates()?;
    if predicates.is_empty() {
      return Ok(primary);
    }
    if let Expr::Filter { expr, predicates: inner, at } = primary {
      return Ok(Expr::Filter { expr, predicates: [inner, predicates].concat(), at });
    }
    Ok(Expr::Filter { expr: Box::new(primary), predicates, at })
  }

  /// Parses a primary expression. The ones that contain other expressions are parsed by functions of their own, so
  /// that their locals do not take up the frame of every nested call.
  fn primary(&mut self) -> Result<Expr, Failure> {
    let expr = match self.peek() {
      Some(Token::Variable { prefix, local }) => {
        if let Some(prefix) = prefix {
          self.bound(prefix)?;
        }
        Expr::Variable { prefix: prefix.clone(), local: local.clone(), at: self.position() }
      }
      Some(Token::Literal(value)) => Expr::Literal(value.clone()),
      Some(Token::Number(value)) => Expr::Number(*value),
      Some(Token::LeftParen) => return self.parenthesized(),
      Some(Token::Function { .. }) => return self.call(),
      _ => return Err(self.expected("an expression")),
    };
    self.bump();
    Ok(expr)
  }

  /// Parses `( Expr )`.
  fn parenthesized(&mut self) -> Result<Expr, Failure> {
    self.bump();
    let expr = self.expr()?;
    self.expect(&Token::RightParen)?;
    Ok(expr)
  }

  /// Parses a function call from its name on, and checks that it calls a function that exists with a number of
  /// arguments it takes. A failed check is located at the function's name.
  fn call(&mut self) -> Result<Expr, Failure> {
    let Some(Token::Function { prefix, local }) = self.peek().cloned() else {
      return Err(self.expected("an expression"));
    };
    let at = self.position();
    self.bump();
    self.expect(&Token::LeftParen)?;
    let arguments = self.arguments()?;
    if let Err(message) = functions::check(prefix.as_deref(), &local, arguments.len()) {
      return Err((message, at));
    }
    Ok(Expr::Function { prefix, local, arguments, at })
  }

  /// Parses a function call's arguments. The opening parenthesis has already been consumed.
  fn arguments(&mut self) -> Result<Vec<Expr>, Failure> {
    let mut arguments = Vec::new();
    if self.eat(&Token::RightParen) {
      return Ok(arguments);
    }
    loop {
      arguments.push(self.expr()?);
      if self.eat(&Token::Comma) {
        continue;
      }
      self.expect(&Token::RightParen)?;
      return Ok(arguments);
    }
  }
}

/// The precedence level in [`LEVELS`] and the operator of a binary operator token, or `None` for any other token.
fn binary_operator(token: &Token) -> Option<(usize, BinaryOp)> {
  LEVELS.iter().enumerate().find_map(|(level, operators)| {
    operators.iter().find(|(candidate, _)| candidate == token).map(|&(_, op)| (level, op))
  })
}

/// Returns the `descendant-or-self::node()` step represented by `//`.
fn descendant_or_self() -> Step {
  Step { axis: Axis::DescendantOrSelf, node_test: NodeTest::Node, predicates: Vec::new() }
}

/// Returns a readable description of a token for use in an error message.
fn describe(token: &Token) -> String {
  let text = match token {
    Token::LeftParen => "(",
    Token::RightParen => ")",
    Token::LeftBracket => "[",
    Token::RightBracket => "]",
    Token::Dot => ".",
    Token::DotDot => "..",
    Token::At => "@",
    Token::Comma => ",",
    Token::ColonColon => "::",
    Token::Slash => "/",
    Token::DoubleSlash => "//",
    Token::Pipe => "|",
    Token::Plus => "+",
    Token::Minus => "-",
    Token::Equal => "=",
    Token::NotEqual => "!=",
    Token::Less => "<",
    Token::LessEqual => "<=",
    Token::Greater => ">",
    Token::GreaterEqual => ">=",
    Token::Star | Token::Multiply => "*",
    Token::And => "and",
    Token::Or => "or",
    Token::Div => "div",
    Token::Mod => "mod",
    Token::Axis(axis) => return format!("the axis \"{axis}\""),
    Token::NodeType(_) => return "a node test".to_owned(),
    Token::Function { local, .. } => return format!("the function \"{local}\""),
    Token::Name { local, .. } => return format!("the name \"{local}\""),
    Token::NamespaceWildcard(prefix) => return format!("\"{prefix}:*\""),
    Token::Variable { local, .. } => return format!("the variable \"${local}\""),
    Token::Literal(_) => return "a string".to_owned(),
    Token::Number(_) => return "a number".to_owned(),
  };
  format!("\"{text}\"")
}
