//! Converts an XPath expression into a sequence of tokens.
//!
//! XPath 1.0 tokenization is context-dependent (§3.7). The same text can represent different tokens depending on the
//! preceding token or the following characters. For example, `*` acts as a wildcard where an operand can begin, but as
//! a multiplication operator after an operand. `div` is a name in `child::div` but an operator in `1 div 2`.
//! Furthermore, `text()` is a node-type test whereas `text` is a name, and while `child::a` begins with an axis,
//! `child` is a name test. This lexical analyzer resolves such distinctions, enabling the parser to receive an
//! unambiguous sequence of tokens.

#[cfg(test)]
mod test;

use crate::chars::{is_ncname_char, is_ncname_start_char, is_whitespace};
use crate::xpath::Failure;
use crate::xpath::ast::Axis;

/// A token in an XPath expression, with context-dependent meanings resolved according to XPath 1.0 §3.7.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token {
  /// `(`, opening a grouped expression or function argument list.
  LeftParen,
  /// `)`, closing a grouped expression or function argument list.
  RightParen,
  /// `[`, opening a predicate.
  LeftBracket,
  /// `]`, closing a predicate.
  RightBracket,
  /// `.`, the abbreviated self step, or the start of a decimal number when followed by a digit.
  Dot,
  /// `..`, the abbreviated parent step.
  DotDot,
  /// `@`, introducing an attribute-axis step.
  At,
  /// `,`, separating function arguments.
  Comma,
  /// `::`, separating an axis name from its node test.
  ColonColon,
  /// `/`, separating path steps or selecting the root when used alone.
  Slash,
  /// `//`, the abbreviated descendant-or-self path separator.
  DoubleSlash,
  /// `|`, the node-set union operator.
  Pipe,
  /// `+`, numeric addition.
  Plus,
  /// `-`, subtraction or unary negation; the parser determines which form applies.
  Minus,
  /// `=`, equality comparison.
  Equal,
  /// `!=`, inequality comparison.
  NotEqual,
  /// `<`, less-than comparison.
  Less,
  /// `<=`, less-than-or-equal comparison.
  LessEqual,
  /// `>`, greater-than comparison.
  Greater,
  /// `>=`, greater-than-or-equal comparison.
  GreaterEqual,
  /// `*` in an operand position, where it is the wildcard name test.
  Star,
  /// `*` after an operand, where it is the multiplication operator.
  Multiply,
  /// `and`, the logical conjunction operator.
  And,
  /// `or`, the logical disjunction operator.
  Or,
  /// `div`, the numeric division operator.
  Div,
  /// `mod`, the numeric remainder operator.
  Mod,
  /// An unprefixed axis name followed by `::`.
  Axis(Axis),
  /// An XPath node-type name (`node`, `text`, `comment`, or `processing-instruction`) followed by `(`.
  NodeType(NodeTypeName),
  /// A function name followed by `(`. A prefix is retained when present.
  Function { prefix: Option<String>, local: String },
  /// A qualified or unqualified name used as a name test.
  Name { prefix: Option<String>, local: String },
  /// `prefix:*`, a wildcard name test restricted to the namespace bound to `prefix`.
  NamespaceWildcard(String),
  /// A variable reference beginning with `$`.
  Variable { prefix: Option<String>, local: String },
  /// A string delimited by single or double quotes.
  Literal(String),
  /// A decimal numeric literal, stored as an `f64`.
  Number(f64),
}

/// The four node types that XPath 1.0 permits in a node-type test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeTypeName {
  /// `node()`, matching any node kind.
  Node,
  /// `text()`, matching text nodes.
  Text,
  /// `comment()`, matching comment nodes.
  Comment,
  /// `processing-instruction()`, optionally followed by a target string.
  ProcessingInstruction,
}

impl Token {
  /// Returns whether this token counts as an operator when classifying the next name or `*`.
  fn is_operator(&self) -> bool {
    matches!(
      self,
      Token::Slash
        | Token::DoubleSlash
        | Token::Pipe
        | Token::Plus
        | Token::Minus
        | Token::Equal
        | Token::NotEqual
        | Token::Less
        | Token::LessEqual
        | Token::Greater
        | Token::GreaterEqual
        | Token::Multiply
        | Token::And
        | Token::Or
        | Token::Div
        | Token::Mod
    )
  }

  /// Returns whether the next name or `*` begins an operand rather than continuing the preceding expression.
  ///
  /// Under XPath 1.0 §3.7, a name in an operand position is a name test, and `*` is a wildcard. Operand positions
  /// occur at the start of the expression and after `@`, `::`, `(`, `[`, `,`, or an operator. In other positions,
  /// recognized operator names such as `div` and `and` are operators, and `*` means multiplication.
  fn precedes_operand(previous: Option<&Token>) -> bool {
    match previous {
      None => true,
      Some(token) => {
        matches!(token, Token::At | Token::ColonColon | Token::LeftParen | Token::LeftBracket | Token::Comma)
          || token.is_operator()
      }
    }
  }
}

/// A token and its starting byte offset in the input, used to locate parse errors.
#[derive(Clone, Debug)]
pub(crate) struct Spanned {
  pub token: Token,
  pub at: usize,
}

/// Tokenizes the entire expression. On failure, returns an error message and the byte offset where tokenization failed.
pub(crate) fn tokenize(input: &str) -> Result<Vec<Spanned>, Failure> {
  let mut lexer = Lexer { input, at: 0 };
  let mut tokens: Vec<Spanned> = Vec::new();
  loop {
    lexer.skip_whitespace();
    if lexer.rest().is_empty() {
      return Ok(tokens);
    }
    let at = lexer.at;
    let token = lexer.next_token(tokens.last().map(|spanned| &spanned.token))?;
    tokens.push(Spanned { token, at });
  }
}

struct Lexer<'a> {
  input: &'a str,
  at: usize,
}

impl<'a> Lexer<'a> {
  fn rest(&self) -> &'a str {
    &self.input[self.at..]
  }

  fn peek(&self) -> Option<char> {
    self.rest().chars().next()
  }

  fn bump(&mut self) -> Option<char> {
    let c = self.peek()?;
    self.at += c.len_utf8();
    Some(c)
  }

  fn eat(&mut self, text: &str) -> bool {
    if self.rest().starts_with(text) {
      self.at += text.len();
      return true;
    }
    false
  }

  fn skip_whitespace(&mut self) {
    while self.peek().is_some_and(is_whitespace) {
      self.bump();
    }
  }

  /// Reads the next token, classifying context-sensitive names and `*` using the preceding token.
  fn next_token(&mut self, previous: Option<&Token>) -> Result<Token, Failure> {
    let at = self.at;
    // Match multi-character tokens first so they are not split into their one-character prefixes.
    for (text, token) in [
      ("//", Token::DoubleSlash),
      ("::", Token::ColonColon),
      ("!=", Token::NotEqual),
      ("<=", Token::LessEqual),
      (">=", Token::GreaterEqual),
      ("..", Token::DotDot),
    ] {
      if self.eat(text) {
        return Ok(token);
      }
    }
    let Some(c) = self.peek() else {
      return Err(("expected a token, found the end of the expression".to_owned(), at));
    };
    if let Some(token) = self.punctuation(c, previous) {
      self.bump();
      return Ok(token);
    }
    match c {
      '"' | '\'' => self.literal(),
      '$' => self.variable(),
      // A period starts a decimal literal only when the next character is a digit; otherwise it is the self step.
      '.' => {
        if self.rest()[1..].starts_with(|c: char| c.is_ascii_digit()) {
          self.number()
        } else {
          self.bump();
          Ok(Token::Dot)
        }
      }
      c if c.is_ascii_digit() => self.number(),
      c if is_ncname_start_char(c) => self.name(previous),
      _ => Err((format!("unexpected character {c:?}"), at)),
    }
  }

  /// Returns the single-character token for `c`, using the previous token to classify `*`.
  fn punctuation(&self, c: char, previous: Option<&Token>) -> Option<Token> {
    Some(match c {
      '(' => Token::LeftParen,
      ')' => Token::RightParen,
      '[' => Token::LeftBracket,
      ']' => Token::RightBracket,
      '@' => Token::At,
      ',' => Token::Comma,
      '/' => Token::Slash,
      '|' => Token::Pipe,
      '+' => Token::Plus,
      '-' => Token::Minus,
      '=' => Token::Equal,
      '<' => Token::Less,
      '>' => Token::Greater,
      '*' if Token::precedes_operand(previous) => Token::Star,
      '*' => Token::Multiply,
      _ => return None,
    })
  }

  fn literal(&mut self) -> Result<Token, Failure> {
    let at = self.at;
    let Some(quote) = self.bump() else {
      return Err(("expected a string, found the end of the expression".to_owned(), at));
    };
    let Some(end) = self.rest().find(quote) else {
      return Err((format!("the string starting with {quote} is never closed"), at));
    };
    let value = self.rest()[..end].to_owned();
    self.at += end + quote.len_utf8();
    Ok(Token::Literal(value))
  }

  fn number(&mut self) -> Result<Token, Failure> {
    let at = self.at;
    let mut end = 0;
    let bytes = self.rest().as_bytes();
    while end < bytes.len() && bytes[end].is_ascii_digit() {
      end += 1;
    }
    if end < bytes.len() && bytes[end] == b'.' {
      end += 1;
      while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
      }
    }
    let text = &self.rest()[..end];
    let value: f64 = text.parse().map_err(|_| (format!("{text:?} is not a number"), at))?;
    self.at += end;
    Ok(Token::Number(value))
  }

  fn variable(&mut self) -> Result<Token, Failure> {
    let at = self.at;
    self.bump();
    let (prefix, local) = self.qname()?;
    match local {
      Some(local) => Ok(Token::Variable { prefix, local }),
      None => Err(("a variable reference needs a name after \"$\"".to_owned(), at)),
    }
  }

  /// Reads a QName and classifies it from the preceding token and the text that follows it.
  fn name(&mut self, previous: Option<&Token>) -> Result<Token, Failure> {
    let at = self.at;
    let (prefix, local) = self.qname()?;
    let (prefix, local) = match (prefix, local) {
      (prefix, Some(local)) => (prefix, local),
      // A missing local part after a prefix denotes the namespace wildcard `prefix:*`.
      (Some(prefix), None) => return Ok(Token::NamespaceWildcard(prefix)),
      (None, None) => return Err(("expected a name".to_owned(), at)),
    };

    // In operator position, an unprefixed operator name is always an operator, never a name test.
    if prefix.is_none() && !Token::precedes_operand(previous) {
      match local.as_str() {
        "and" => return Ok(Token::And),
        "or" => return Ok(Token::Or),
        "div" => return Ok(Token::Div),
        "mod" => return Ok(Token::Mod),
        // Leave other names unchanged so the parser can report the syntax error in context.
        _ => {}
      }
    }

    // A following `::` marks an axis; a following `(` marks a node-type test or function call.
    let after = self.rest().trim_start_matches(is_whitespace);
    if after.starts_with("::") {
      let Some(axis) = Axis::from_name(&local).filter(|_| prefix.is_none()) else {
        return Err((format!("{local:?} is not one of the thirteen XPath axes"), at));
      };
      return Ok(Token::Axis(axis));
    }
    if after.starts_with('(') {
      let node_type = match local.as_str() {
        "node" => Some(NodeTypeName::Node),
        "text" => Some(NodeTypeName::Text),
        "comment" => Some(NodeTypeName::Comment),
        "processing-instruction" => Some(NodeTypeName::ProcessingInstruction),
        _ => None,
      };
      // Node-type tests cannot have prefixes; a prefixed name followed by `(` is a function call.
      if let Some(node_type) = node_type.filter(|_| prefix.is_none()) {
        return Ok(Token::NodeType(node_type));
      }
      return Ok(Token::Function { prefix, local });
    }
    Ok(Token::Name { prefix, local })
  }

  /// Reads an `NCName`, a `prefix:local` QName, or a `prefix:*` wildcard. A missing local part represents `*`.
  fn qname(&mut self) -> Result<(Option<String>, Option<String>), Failure> {
    let first = self.ncname()?;
    // Do not consume the first colon of `::`; it separates an axis from its node test.
    if !self.rest().starts_with(':') || self.rest().starts_with("::") {
      return Ok((None, Some(first)));
    }
    self.bump();
    if self.peek() == Some('*') {
      self.bump();
      return Ok((Some(first), None));
    }
    let local = self.ncname()?;
    Ok((Some(first), Some(local)))
  }

  fn ncname(&mut self) -> Result<String, Failure> {
    let at = self.at;
    if !self.peek().is_some_and(is_ncname_start_char) {
      let found = self.peek().map_or_else(|| "the end of the expression".to_owned(), |c| format!("{c:?}"));
      return Err((format!("expected a name, found {found}"), at));
    }
    let mut end = 0;
    for (index, c) in self.rest().char_indices() {
      if !is_ncname_char(c) {
        break;
      }
      end = index + c.len_utf8();
    }
    let name = self.rest()[..end].to_owned();
    self.at += end;
    Ok(name)
  }
}
