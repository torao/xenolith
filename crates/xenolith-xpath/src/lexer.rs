//! Turning an XPath expression into tokens.
//!
//! XPath's lexical structure is context-dependent (XPath 1.0 §3.7): the same characters are one
//! token or another depending on what came before, and on what follows. `*` is a wildcard at the
//! start of a step but multiplication after an operand; `div` is a name in `child::div` but an
//! operator in `1 div 2`; `text` is a node type in `text()` but a name in `text`; `child` is an
//! axis in `child::a` but a name in `child`. The lexer settles all of this, so the parser sees
//! tokens that already mean one thing.

use xenolith_core::chars::{is_ncname_char, is_ncname_start_char, is_whitespace};
use xenolith_core::error::{Error, Result};

use crate::ast::Axis;

/// One token of an XPath expression, with the ambiguities of §3.7 already resolved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token {
  /// `(`
  LeftParen,
  /// `)`
  RightParen,
  /// `[`
  LeftBracket,
  /// `]`
  RightBracket,
  /// `.`
  Dot,
  /// `..`
  DotDot,
  /// `@`
  At,
  /// `,`
  Comma,
  /// `::`
  ColonColon,
  /// `/`
  Slash,
  /// `//`
  DoubleSlash,
  /// `|`
  Pipe,
  /// `+`
  Plus,
  /// `-`
  Minus,
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
  /// `*` where a name test belongs: the wildcard.
  Star,
  /// `*` where an operator belongs: multiplication.
  Multiply,
  /// `and`
  And,
  /// `or`
  Or,
  /// `div`
  Div,
  /// `mod`
  Mod,
  /// An axis name, followed by `::`.
  Axis(Axis),
  /// A node type — `node`, `text`, `comment`, `processing-instruction` — followed by `(`.
  NodeType(NodeTypeName),
  /// A function name, followed by `(`.
  Function { prefix: Option<String>, local: String },
  /// A name used as a name test.
  Name { prefix: Option<String>, local: String },
  /// `prefix:*`
  NamespaceWildcard(String),
  /// `$name`
  Variable { prefix: Option<String>, local: String },
  /// A quoted string.
  Literal(String),
  /// A number.
  Number(f64),
}

/// The four node types a node test may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeTypeName {
  Node,
  Text,
  Comment,
  ProcessingInstruction,
}

impl Token {
  /// Whether this token is an operator, for the rule that decides what follows it.
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

  /// Whether a name or `*` after this token begins an operand rather than continuing one.
  ///
  /// XPath 1.0 §3.7: after `@`, `::`, `(`, `[`, `,` or an operator — or at the very start — a
  /// name is a name and `*` is the wildcard; anywhere else a name must be an operator name and
  /// `*` must be multiplication.
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

/// A token and the byte offset it starts at, so an error can point at it.
#[derive(Clone, Debug)]
pub(crate) struct Spanned {
  pub(crate) token: Token,
  pub(crate) at: usize,
}

/// Tokenizes a whole expression.
pub(crate) fn tokenize(input: &str) -> Result<Vec<Spanned>> {
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

  fn error(&self, at: usize, message: impl Into<String>) -> Error {
    Error::xpath(format!("{} at position {at} of the XPath expression", message.into()))
  }

  /// Reads the next token, given the one before it.
  fn next_token(&mut self, previous: Option<&Token>) -> Result<Token> {
    let at = self.at;
    // The two-character tokens have to be tried before their one-character prefixes.
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
    let c = self.peek().expect("the caller checked there is input");
    if let Some(token) = self.punctuation(c, previous) {
      self.bump();
      return Ok(token);
    }
    match c {
      '"' | '\'' => self.literal(),
      '$' => self.variable(),
      // A `.` begins a number only when a digit follows; otherwise it is the self step.
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
      _ => Err(self.error(at, format!("unexpected character {c:?}"))),
    }
  }

  /// The single-character tokens, including the `*` whose meaning depends on what came before.
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

  fn literal(&mut self) -> Result<Token> {
    let at = self.at;
    let quote = self.bump().expect("the caller saw the quote");
    let Some(end) = self.rest().find(quote) else {
      return Err(self.error(at, format!("the string starting with {quote} is never closed")));
    };
    let value = self.rest()[..end].to_owned();
    self.at += end + quote.len_utf8();
    Ok(Token::Literal(value))
  }

  fn number(&mut self) -> Result<Token> {
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
    let value: f64 = text.parse().map_err(|_| self.error(at, format!("{text:?} is not a number")))?;
    self.at += end;
    Ok(Token::Number(value))
  }

  fn variable(&mut self) -> Result<Token> {
    let at = self.at;
    self.bump();
    let (prefix, local) = self.qname()?;
    match local {
      Some(local) => Ok(Token::Variable { prefix, local }),
      None => Err(self.error(at, "a variable reference needs a name after \"$\"")),
    }
  }

  /// Reads a name and decides, from what precedes and follows it, which token it is.
  fn name(&mut self, previous: Option<&Token>) -> Result<Token> {
    let at = self.at;
    let (prefix, local) = self.qname()?;
    let Some(local) = local else {
      // `prefix:*` — a name test over one namespace.
      let prefix = prefix.expect("a wildcard is only reached after a prefix");
      return Ok(Token::NamespaceWildcard(prefix));
    };

    // An unprefixed name in operator position is an operator name, and nothing else.
    if prefix.is_none() && !Token::precedes_operand(previous) {
      match local.as_str() {
        "and" => return Ok(Token::And),
        "or" => return Ok(Token::Or),
        "div" => return Ok(Token::Div),
        "mod" => return Ok(Token::Mod),
        // Not an operator name: leave it as a name so the parser can say what it expected.
        _ => {}
      }
    }

    // `name::` is an axis; `name(` is a node type or a function.
    let after = self.rest().trim_start_matches(is_whitespace);
    if after.starts_with("::") {
      let Some(axis) = Axis::from_name(&local).filter(|_| prefix.is_none()) else {
        return Err(self.error(at, format!("{local:?} is not one of the thirteen XPath axes")));
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
      // A node type is never prefixed; a prefixed name that looks like one is a function.
      if let Some(node_type) = node_type.filter(|_| prefix.is_none()) {
        return Ok(Token::NodeType(node_type));
      }
      return Ok(Token::Function { prefix, local });
    }
    Ok(Token::Name { prefix, local })
  }

  /// Reads `NCName`, `NCName ':' NCName` or `NCName ':' '*'`, returning the parts. A `None`
  /// local part means the `*` of a `prefix:*` name test.
  fn qname(&mut self) -> Result<(Option<String>, Option<String>)> {
    let first = self.ncname()?;
    // A `::` is the axis separator, not a name separator, so only a lone `:` continues a name.
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

  fn ncname(&mut self) -> Result<String> {
    let at = self.at;
    if !self.peek().is_some_and(is_ncname_start_char) {
      let found = self.peek().map_or_else(|| "the end of the expression".to_owned(), |c| format!("{c:?}"));
      return Err(self.error(at, format!("expected a name, found {found}")));
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

#[cfg(test)]
mod tests {
  use super::*;

  fn tokens(input: &str) -> Vec<Token> {
    tokenize(input).expect("tokenizes").into_iter().map(|spanned| spanned.token).collect()
  }

  #[test]
  fn star_is_a_wildcard_at_the_start_of_a_step_and_multiplication_after_an_operand() {
    assert_eq!(tokens("*"), [Token::Star]);
    assert_eq!(tokens("@*"), [Token::At, Token::Star]);
    assert_eq!(tokens("a/*"), [Token::Name { prefix: None, local: "a".into() }, Token::Slash, Token::Star]);
    assert_eq!(
      tokens("2 * 3"),
      [Token::Number(2.0), Token::Multiply, Token::Number(3.0)],
      "after a number, * is multiplication"
    );
  }

  #[test]
  fn operator_names_are_names_where_a_name_belongs() {
    assert_eq!(tokens("div"), [Token::Name { prefix: None, local: "div".into() }]);
    assert_eq!(tokens("1 div 2"), [Token::Number(1.0), Token::Div, Token::Number(2.0)]);
    assert_eq!(
      tokens("child::div"),
      [Token::Axis(Axis::Child), Token::ColonColon, Token::Name { prefix: None, local: "div".into() }]
    );
  }

  #[test]
  fn a_name_before_a_parenthesis_is_a_node_type_or_a_function() {
    assert_eq!(tokens("text()"), [Token::NodeType(NodeTypeName::Text), Token::LeftParen, Token::RightParen]);
    assert_eq!(
      tokens("count(a)"),
      [
        Token::Function { prefix: None, local: "count".into() },
        Token::LeftParen,
        Token::Name { prefix: None, local: "a".into() },
        Token::RightParen
      ]
    );
    assert_eq!(tokens("text"), [Token::Name { prefix: None, local: "text".into() }], "without ( it is a name");
  }

  #[test]
  fn a_name_before_a_double_colon_is_an_axis() {
    assert_eq!(tokens("ancestor-or-self::"), [Token::Axis(Axis::AncestorOrSelf), Token::ColonColon]);
    assert!(tokenize("nosuch::a").is_err(), "an unknown axis is refused where an axis must be");
  }

  #[test]
  fn reads_qualified_names_and_wildcards() {
    assert_eq!(tokens("p:a"), [Token::Name { prefix: Some("p".into()), local: "a".into() }]);
    assert_eq!(tokens("p:*"), [Token::NamespaceWildcard("p".into())]);
    assert_eq!(tokens("$p:v"), [Token::Variable { prefix: Some("p".into()), local: "v".into() }]);
  }

  #[test]
  fn reads_numbers_literals_and_the_dot_tokens() {
    assert_eq!(tokens("1 1.5 .5"), [Token::Number(1.0), Token::Number(1.5), Token::Number(0.5)]);
    assert_eq!(tokens("'a' \"b\""), [Token::Literal("a".into()), Token::Literal("b".into())]);
    assert_eq!(tokens(". .. ./."), [Token::Dot, Token::DotDot, Token::Dot, Token::Slash, Token::Dot]);
  }

  #[test]
  fn reports_what_it_could_not_read() {
    assert!(tokenize("'unclosed").unwrap_err().message().contains("never closed"));
    assert!(tokenize("a # b").unwrap_err().message().contains("unexpected character"));
    assert!(tokenize("$").unwrap_err().message().contains("expected a name"));
  }
}
