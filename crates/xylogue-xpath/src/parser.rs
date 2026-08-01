//! Parsing tokens into an expression tree.
//!
//! A recursive descent over the XPath 1.0 grammar, one level of precedence per function, from
//! `or` down to a single step. The abbreviations are expanded as they are recognized, so what
//! comes out is the plain form described in [`ast`](crate::ast).

use xylogue_core::error::{Error, ErrorKind, Result};

use crate::ast::{Axis, BinaryOp, Expr, NameTest, NodeTest, Path, PathStart, Step};
use crate::lexer::{NodeTypeName, Spanned, Token};

/// The binary operators, by precedence level, loosest first.
const LEVELS: &[&[(Token, BinaryOp)]] = &[
  &[(Token::Or, BinaryOp::Or)],
  &[(Token::And, BinaryOp::And)],
  &[(Token::Equal, BinaryOp::Equal), (Token::NotEqual, BinaryOp::NotEqual)],
  // `<=` and `>=` are tried before `<` and `>`, which are their prefixes as tokens are compared.
  &[
    (Token::LessEqual, BinaryOp::LessEqual),
    (Token::GreaterEqual, BinaryOp::GreaterEqual),
    (Token::Less, BinaryOp::Less),
    (Token::Greater, BinaryOp::Greater),
  ],
  &[(Token::Plus, BinaryOp::Add), (Token::Minus, BinaryOp::Subtract)],
  &[(Token::Multiply, BinaryOp::Multiply), (Token::Div, BinaryOp::Divide), (Token::Mod, BinaryOp::Modulo)],
];

/// Parses a token stream into an expression, which must account for all of it.
pub(crate) fn parse(tokens: &[Spanned], length: usize) -> Result<Expr> {
  let mut parser = Parser { tokens, at: 0, length };
  let expr = parser.expr()?;
  match parser.peek() {
    None => Ok(expr),
    Some(token) => {
      let message = format!("unexpected {} after a complete expression", describe(token));
      Err(parser.error(message))
    }
  }
}

struct Parser<'a> {
  tokens: &'a [Spanned],
  at: usize,
  /// The length of the expression, so an error at its end can point past the last token.
  length: usize,
}

impl Parser<'_> {
  fn peek(&self) -> Option<&Token> {
    self.tokens.get(self.at).map(|spanned| &spanned.token)
  }

  fn bump(&mut self) {
    self.at += 1;
  }

  /// Consumes the next token if it is `expected`.
  fn eat(&mut self, expected: &Token) -> bool {
    if self.peek() == Some(expected) {
      self.at += 1;
      return true;
    }
    false
  }

  /// Consumes the next token, which must be `expected`.
  fn expect(&mut self, expected: &Token) -> Result<()> {
    if self.eat(expected) {
      return Ok(());
    }
    let message = format!("expected {} but found {}", describe(expected), self.found());
    Err(self.error(message))
  }

  /// Names what is at the cursor, for an error message.
  fn found(&self) -> String {
    self.peek().map_or_else(|| "the end of the expression".to_owned(), describe)
  }

  /// The byte offset the next token starts at, or the end of the expression.
  fn position(&self) -> usize {
    self.tokens.get(self.at).map_or(self.length, |spanned| spanned.at)
  }

  fn error(&self, message: impl Into<String>) -> Error {
    Error::new(ErrorKind::XPath, format!("{} at position {} of the XPath expression", message.into(), self.position()))
  }

  // --- Precedence levels --------------------------------------------------------------------

  fn expr(&mut self) -> Result<Expr> {
    self.binary_level(0)
  }

  /// Parses one left-associative binary level, then everything binding more tightly.
  fn binary_level(&mut self, level: usize) -> Result<Expr> {
    let Some(operators) = LEVELS.get(level) else {
      // Below the binary levels sits unary minus, and below that the union.
      return self.unary();
    };
    let mut left = self.binary_level(level + 1)?;
    'operand: loop {
      for (token, op) in *operators {
        if self.eat(token) {
          let right = self.binary_level(level + 1)?;
          left = Expr::Binary { op: *op, left: Box::new(left), right: Box::new(right) };
          continue 'operand;
        }
      }
      return Ok(left);
    }
  }

  fn unary(&mut self) -> Result<Expr> {
    if self.eat(&Token::Minus) {
      return Ok(Expr::Negate(Box::new(self.unary()?)));
    }
    self.union()
  }

  fn union(&mut self) -> Result<Expr> {
    let mut left = self.path()?;
    while self.eat(&Token::Pipe) {
      let right = self.path()?;
      left = Expr::Binary { op: BinaryOp::Union, left: Box::new(left), right: Box::new(right) };
    }
    Ok(left)
  }

  // --- Paths --------------------------------------------------------------------------------

  /// A path: a location path, or an expression whose node-set a path continues from.
  fn path(&mut self) -> Result<Expr> {
    if !self.starts_primary() {
      return Ok(Expr::Path(self.location_path()?));
    }
    let filter = self.filter()?;
    // A filter expression may be followed by a path, walked from each node it yields.
    let steps = if self.eat(&Token::Slash) {
      self.relative_steps()?
    } else if self.eat(&Token::DoubleSlash) {
      let mut steps = vec![descendant_or_self()];
      steps.extend(self.relative_steps()?);
      steps
    } else {
      return Ok(filter);
    };
    // `(P)/Q` walks Q from each node P yields, which is what `P/Q` does — so when the
    // parenthesised part is itself a location path, its steps are spliced in rather than left
    // wrapped. Otherwise one expression would have two shapes in the tree, and printing the
    // wrapped one gives the text that parses to the flat one. A filter (`(P)[1]/Q`) is not
    // spliced: there the predicate applies to the whole node-set, which is a different question.
    if let Expr::Path(inner) = filter {
      let mut spliced = inner.steps;
      spliced.extend(steps);
      return Ok(Expr::Path(Path { start: inner.start, steps: spliced }));
    }
    Ok(Expr::Path(Path { start: PathStart::Expr(Box::new(filter)), steps }))
  }

  /// Whether the cursor is on a primary expression rather than a location path.
  fn starts_primary(&self) -> bool {
    matches!(
      self.peek(),
      Some(Token::Variable { .. } | Token::LeftParen | Token::Literal(_) | Token::Number(_) | Token::Function { .. })
    )
  }

  fn location_path(&mut self) -> Result<Path> {
    if self.eat(&Token::DoubleSlash) {
      let mut steps = vec![descendant_or_self()];
      steps.extend(self.relative_steps()?);
      return Ok(Path { start: PathStart::Root, steps });
    }
    if self.eat(&Token::Slash) {
      // A lone `/` is the root itself; anything that can begin a step continues the path.
      let steps = if self.starts_step() { self.relative_steps()? } else { Vec::new() };
      return Ok(Path { start: PathStart::Root, steps });
    }
    Ok(Path { start: PathStart::Context, steps: self.relative_steps()? })
  }

  /// One or more steps, separated by `/` or `//`.
  fn relative_steps(&mut self) -> Result<Vec<Step>> {
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

  /// Whether the cursor is on a token that can begin a step.
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

  fn step(&mut self) -> Result<Step> {
    // The abbreviated steps carry their own node test and take no predicates.
    if self.eat(&Token::Dot) {
      return Ok(Step { axis: Axis::SelfAxis, node_test: NodeTest::Node, predicates: Vec::new() });
    }
    if self.eat(&Token::DotDot) {
      return Ok(Step { axis: Axis::Parent, node_test: NodeTest::Node, predicates: Vec::new() });
    }
    let named_axis = match self.peek() {
      Some(Token::Axis(axis)) => Some(*axis),
      _ => None,
    };
    let axis = if self.eat(&Token::At) {
      Axis::Attribute
    } else if let Some(axis) = named_axis {
      self.bump();
      self.expect(&Token::ColonColon)?;
      axis
    } else {
      Axis::Child
    };
    let node_test = self.node_test()?;
    let predicates = self.predicates()?;
    Ok(Step { axis, node_test, predicates })
  }

  fn node_test(&mut self) -> Result<NodeTest> {
    let Some(token) = self.peek().cloned() else {
      return Err(self.error("expected a name or a node test, found the end of the expression"));
    };
    match token {
      Token::Star => {
        self.bump();
        Ok(NodeTest::Name(NameTest::Any))
      }
      Token::NamespaceWildcard(prefix) => {
        self.bump();
        Ok(NodeTest::Name(NameTest::Namespace(prefix)))
      }
      Token::Name { prefix, local } => {
        self.bump();
        Ok(NodeTest::Name(NameTest::Name { prefix, local }))
      }
      Token::NodeType(node_type) => {
        self.bump();
        self.expect(&Token::LeftParen)?;
        let test = match node_type {
          NodeTypeName::Node => NodeTest::Node,
          NodeTypeName::Text => NodeTest::Text,
          NodeTypeName::Comment => NodeTest::Comment,
          // `processing-instruction('target')` narrows to one target.
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
      other => Err(self.error(format!("expected a name or a node test, found {}", describe(&other)))),
    }
  }

  /// The predicates that follow a step or a filter expression.
  fn predicates(&mut self) -> Result<Vec<Expr>> {
    let mut predicates = Vec::new();
    while self.eat(&Token::LeftBracket) {
      predicates.push(self.expr()?);
      self.expect(&Token::RightBracket)?;
    }
    Ok(predicates)
  }

  // --- Primary expressions ------------------------------------------------------------------

  /// A primary expression with any predicates that filter it.
  ///
  /// Predicates on a filter apply left to right, each to what the one before it left, so
  /// `((E)[1])[2]` and `(E)[1][2]` are the same expression — and are folded into one filter here
  /// so that they are also the same tree. Without that, one expression would have two spellings
  /// in the syntax tree, and printing either would give text that parses to the other one.
  fn filter(&mut self) -> Result<Expr> {
    let primary = self.primary()?;
    let predicates = self.predicates()?;
    if predicates.is_empty() {
      return Ok(primary);
    }
    if let Expr::Filter { expr, predicates: inner } = primary {
      return Ok(Expr::Filter { expr, predicates: [inner, predicates].concat() });
    }
    Ok(Expr::Filter { expr: Box::new(primary), predicates })
  }

  fn primary(&mut self) -> Result<Expr> {
    let Some(token) = self.peek().cloned() else {
      return Err(self.error("expected an expression, found the end of the expression"));
    };
    match token {
      Token::Variable { prefix, local } => {
        self.bump();
        Ok(Expr::Variable { prefix, local })
      }
      Token::Literal(value) => {
        self.bump();
        Ok(Expr::Literal(value))
      }
      Token::Number(value) => {
        self.bump();
        Ok(Expr::Number(value))
      }
      Token::LeftParen => {
        self.bump();
        let expr = self.expr()?;
        self.expect(&Token::RightParen)?;
        Ok(expr)
      }
      Token::Function { prefix, local } => {
        self.bump();
        self.expect(&Token::LeftParen)?;
        let arguments = self.arguments()?;
        Ok(Expr::Function { prefix, local, arguments })
      }
      other => Err(self.error(format!("expected an expression, found {}", describe(&other)))),
    }
  }

  /// The arguments of a function call, the opening parenthesis already consumed.
  fn arguments(&mut self) -> Result<Vec<Expr>> {
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

/// The step `//` stands for.
fn descendant_or_self() -> Step {
  Step { axis: Axis::DescendantOrSelf, node_test: NodeTest::Node, predicates: Vec::new() }
}

/// Names a token in an error message, the way it is written.
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
