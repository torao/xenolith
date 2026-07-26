//! Attribute value templates.
//!
//! An attribute of a literal result element, and a few of XSLT's own, may hold expressions in
//! braces: `href="{$base}/{@id}.html"`. XSLT 1.0 §7.6.2 calls this an attribute value template.
//! A brace that is meant literally is doubled — `{{` and `}}` — which is the only escape there
//! is, so the grammar is small enough to read by hand.

use xylograph_core::error::{Error, ErrorKind, Result};

/// One part of an attribute value template.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Piece {
  /// Text that is used as it stands, with any doubled braces already halved.
  Literal(String),
  /// An expression to evaluate, and whose string-value takes its place.
  Expression(String),
}

/// Splits an attribute value into its literal and expression parts.
///
/// A value with no braces gives one [`Literal`](Piece::Literal), which is the common case and
/// costs one allocation.
pub(crate) fn parse(value: &str) -> Result<Vec<Piece>> {
  let mut pieces = Vec::new();
  let mut literal = String::new();
  let mut rest = value;

  while let Some(at) = rest.find(['{', '}']) {
    let (before, from) = rest.split_at(at);
    literal.push_str(before);
    let brace = from.as_bytes()[0];
    let after = &from[1..];

    // A doubled brace stands for one of itself, whichever brace it is.
    if after.as_bytes().first() == Some(&brace) {
      literal.push(brace as char);
      rest = &after[1..];
      continue;
    }
    if brace == b'}' {
      return Err(avt_error(value, "a \"}\" outside an expression has to be written \"}}\""));
    }

    // An expression runs to the next unquoted `}`; a brace inside a string literal is text.
    let Some(end) = expression_end(after) else {
      return Err(avt_error(value, "an expression opened with \"{\" is never closed"));
    };
    if !literal.is_empty() {
      pieces.push(Piece::Literal(std::mem::take(&mut literal)));
    }
    let expression = after[..end].trim();
    if expression.is_empty() {
      return Err(avt_error(value, "an expression in braces is empty"));
    }
    pieces.push(Piece::Expression(expression.to_owned()));
    rest = &after[end + 1..];
  }

  literal.push_str(rest);
  if !literal.is_empty() || pieces.is_empty() {
    pieces.push(Piece::Literal(literal));
  }
  Ok(pieces)
}

/// Where the expression starting a slice ends: the offset of its closing `}`.
///
/// A `}` inside a string literal belongs to the string, not to the template, so the quotes have
/// to be tracked while looking for it.
fn expression_end(text: &str) -> Option<usize> {
  let mut quote: Option<u8> = None;
  for (offset, byte) in text.bytes().enumerate() {
    match quote {
      Some(open) if byte == open => quote = None,
      Some(_) => {}
      None => match byte {
        b'\'' | b'"' => quote = Some(byte),
        b'}' => return Some(offset),
        _ => {}
      },
    }
  }
  None
}

fn avt_error(value: &str, why: &str) -> Error {
  Error::new(ErrorKind::Xslt, format!("{value:?} is not a valid attribute value template: {why}"))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn literal(text: &str) -> Piece {
    Piece::Literal(text.to_owned())
  }

  fn expression(text: &str) -> Piece {
    Piece::Expression(text.to_owned())
  }

  #[test]
  fn a_value_without_braces_is_one_literal() {
    assert_eq!(parse("plain").unwrap(), [literal("plain")]);
    assert_eq!(parse("").unwrap(), [literal("")]);
  }

  #[test]
  fn braces_mark_expressions() {
    assert_eq!(parse("{$x}").unwrap(), [expression("$x")]);
    assert_eq!(parse("a{$x}b").unwrap(), [literal("a"), expression("$x"), literal("b")]);
    assert_eq!(parse("{@a}/{@b}").unwrap(), [expression("@a"), literal("/"), expression("@b")]);
    assert_eq!(parse("{ @a }").unwrap(), [expression("@a")], "the expression is trimmed");
  }

  #[test]
  fn a_doubled_brace_is_one_literal_brace() {
    assert_eq!(parse("{{").unwrap(), [literal("{")]);
    assert_eq!(parse("}}").unwrap(), [literal("}")]);
    assert_eq!(parse("{{$x}}").unwrap(), [literal("{$x}")]);
    assert_eq!(parse("a{{b}}c").unwrap(), [literal("a{b}c")]);
  }

  #[test]
  fn a_brace_inside_a_string_belongs_to_the_string() {
    assert_eq!(parse("{concat('}', @a)}").unwrap(), [expression("concat('}', @a)")]);
    assert_eq!(parse("{\"}\"}").unwrap(), [expression("\"}\"")]);
  }

  #[test]
  fn what_cannot_be_read_says_why() {
    assert!(parse("{$x").unwrap_err().message().contains("never closed"));
    assert!(parse("a}b").unwrap_err().message().contains("has to be written"));
    assert!(parse("{}").unwrap_err().message().contains("is empty"));
  }
}
