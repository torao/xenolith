//! `http://exslt.org/strings` — string handling beyond `substring-before`.
//!
//! XPath 1.0 can find a substring and translate characters, and that is about all. This module
//! adds splitting, padding, aligning and URI escaping.
//!
//! # Two of them answer with nodes
//!
//! `str:tokenize` and `str:split` give a node-set of `token` elements rather than a string,
//! because a node-set is the only XPath 1.0 value that can hold several strings at once. That
//! means building a tree, and a tree has to go somewhere the model can read — the same handover
//! `exsl:node-set()` needs. [`register_with`](crate::register_with) is how that somewhere is
//! supplied; without one, these two report what they need instead of answering with a guess.
//!
//! # Examples
//!
//! ```
//! use std::rc::Rc;
//! use xylograph_dom::build;
//! use xylograph_xdm::{DomModel, Documents};
//! use xylograph_xpath::Functions;
//! use xylograph_xslt::{DocumentSource, Stylesheet, Transform, TreeSpace};
//!
//! let stylesheet = Stylesheet::compile(
//!   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
//!                       xmlns:str="http://exslt.org/str">
//!         <xsl:template match="/">
//!           <xsl:value-of select="count(str:tokenize('a b c'))"/>
//!         </xsl:template>
//!       </xsl:stylesheet>"#,
//!   "file:///s.xsl",
//! )?;
//!
//! let source = build::parse("<a/>".as_bytes())?;
//! let documents = Documents::new();
//! let model = DomModel::with_documents(&source, &documents);
//! let space: Rc<dyn DocumentSource<_>> = Rc::new(TreeSpace::new(&documents));
//! let functions = xylograph_exslt::register_with(Functions::new(), &space);
//!
//! let result = Transform::new()
//!   .run_with_documents(&stylesheet, &model, model.root_node(), functions, space)?;
//! assert_eq!(result.text().trim(), "3");
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! # Specifications
//!
//! - [`exslt:strings`](http://exslt.org/str/index.html)

use std::rc::Rc;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_dom::Document;
use xylograph_xdm::Model;
use xylograph_xpath::{Context, Functions, Value};
use xylograph_xslt::DocumentSource;

use crate::support::arity;

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/str";

/// What `str:tokenize` and `str:split` name each piece they find.
const TOKEN: &str = "token";

/// Adds this module's functions, with `trees` for the two that answer with nodes.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>, trees: &Rc<dyn DocumentSource<M::Node>>) -> Functions<M> {
  let for_tokenize = Rc::clone(trees);
  let for_split = Rc::clone(trees);

  functions
    .with(NAMESPACE, "concat", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:concat", &arguments, 1, Some(1))?;
      // The string-values of a node-set, run together in document order.
      let joined = match &arguments[0] {
        Value::NodeSet(nodes) => nodes.iter().map(|node| context.model.string_value(*node)).collect(),
        other => other.string(context.model),
      };
      Ok(Value::String(joined))
    })
    .with(NAMESPACE, "padding", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:padding", &arguments, 1, Some(2))?;
      let length = arguments[0].number(context.model);
      let filler = match arguments.get(1) {
        Some(value) => value.string(context.model),
        None => " ".to_owned(),
      };
      Ok(Value::String(padding(length, &filler)))
    })
    .with(NAMESPACE, "align", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:align", &arguments, 2, Some(3))?;
      let text = arguments[0].string(context.model);
      let width = arguments[1].string(context.model);
      let alignment = match arguments.get(2) {
        Some(value) => value.string(context.model),
        None => "left".to_owned(),
      };
      Ok(Value::String(align(&text, &width, &alignment)))
    })
    .with(NAMESPACE, "encode-uri", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:encode-uri", &arguments, 2, Some(3))?;
      let text = arguments[0].string(context.model);
      let reserved_too = arguments[1].boolean();
      Ok(Value::String(encode_uri(&text, reserved_too)))
    })
    .with(NAMESPACE, "decode-uri", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:decode-uri", &arguments, 1, Some(2))?;
      let text = arguments[0].string(context.model);
      Ok(Value::String(decode_uri(&text)))
    })
    .with(NAMESPACE, "tokenize", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:tokenize", &arguments, 1, Some(2))?;
      let text = arguments[0].string(context.model);
      // Every character of the second argument is a delimiter of its own, and the default is
      // the whitespace XML calls whitespace.
      let delimiters = match arguments.get(1) {
        Some(value) => value.string(context.model),
        None => " \t\n\r".to_owned(),
      };
      let pieces: Vec<String> = if delimiters.is_empty() {
        // EXSLT: with no delimiters at all, every character is a token of its own.
        text.chars().map(|character| character.to_string()).collect()
      } else {
        text.split(|character| delimiters.contains(character)).map(ToOwned::to_owned).collect()
      };
      as_tokens(&pieces, &for_tokenize, "str:tokenize", context)
    })
    .with(NAMESPACE, "split", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("str:split", &arguments, 1, Some(2))?;
      let text = arguments[0].string(context.model);
      // Unlike tokenize, the second argument is one whole separator rather than a set of them.
      let separator = match arguments.get(1) {
        Some(value) => value.string(context.model),
        None => " ".to_owned(),
      };
      let pieces: Vec<String> = if separator.is_empty() {
        text.chars().map(|character| character.to_string()).collect()
      } else {
        text.split(separator.as_str()).map(ToOwned::to_owned).collect()
      };
      as_tokens(&pieces, &for_split, "str:split", context)
    })
}

/// Builds a `token` element for each piece and puts the tree where the model can read it.
fn as_tokens<M: Model>(
  pieces: &[String],
  trees: &Rc<dyn DocumentSource<M::Node>>,
  name: &str,
  context: &Context<'_, M>,
) -> Result<Value<M::Node>> {
  let mut document = Document::new();
  let root = document.create_document_fragment();
  for piece in pieces {
    let element =
      document.create_element(TOKEN).map_err(|error| Error::internal(format!("building a {TOKEN}: {error}")))?;
    let text = document.create_text_node(piece);
    document.append_child(element, text).map_err(|error| Error::internal(format!("{error}")))?;
    document.append_child(root, element).map_err(|error| Error::internal(format!("{error}")))?;
  }

  let adopted = trees.adopt(document, root)?.ok_or_else(|| {
    let message = format!(
      "{name}() answers with nodes, and needs somewhere to put them; register the EXSLT \
       functions with register_with and a TreeSpace sharing the model's Documents handle"
    );
    Error::new(ErrorKind::Xslt, message)
  })?;

  // The answer is the tokens themselves, which are the children of the tree that was taken in —
  // not the root, which is only what holds them.
  Ok(Value::NodeSet(context.model.children(adopted)))
}

/// `str:padding`: `filler` repeated, or cut, to `length` characters.
fn padding(length: f64, filler: &str) -> String {
  if !length.is_finite() || length < 1.0 || filler.is_empty() {
    return String::new();
  }
  #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
  let wanted = length.trunc() as usize;
  filler.chars().cycle().take(wanted).collect()
}

/// `str:align`: `text` placed within a field as wide as `width`.
///
/// EXSLT gives the width as a *string* rather than a number, so that the field can be spelled
/// out — `str:align('x', '.....')` is five characters wide. Text longer than the field is cut,
/// and which end is cut depends on the alignment, since the point is to keep what is aligned.
fn align(text: &str, width: &str, alignment: &str) -> String {
  let field: Vec<char> = width.chars().collect();
  let characters: Vec<char> = text.chars().collect();
  if characters.len() >= field.len() {
    return match alignment {
      "right" => characters[characters.len() - field.len()..].iter().collect(),
      "center" => {
        let start = (characters.len() - field.len()) / 2;
        characters[start..start + field.len()].iter().collect()
      }
      _ => characters[..field.len()].iter().collect(),
    };
  }
  let spare = field.len() - characters.len();
  let (before, after) = match alignment {
    "right" => (spare, 0),
    "center" => (spare / 2, spare - spare / 2),
    _ => (0, spare),
  };
  let mut aligned = String::new();
  aligned.extend(&field[..before]);
  aligned.push_str(text);
  aligned.extend(&field[field.len() - after..]);
  aligned
}

/// `str:encode-uri`: the characters a URI may not hold, written as `%` escapes.
///
/// `reserved_too` says whether the characters reserved *for* URI syntax — `;/?:@&=+$,[]` — are
/// escaped as well. Escaping is of the UTF-8 bytes, as RFC 3986 requires.
fn encode_uri(text: &str, reserved_too: bool) -> String {
  const UNRESERVED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()";
  const RESERVED: &str = ";/?:@&=+$,[]";
  let mut written = String::new();
  for character in text.chars() {
    let keep = UNRESERVED.contains(character) || (!reserved_too && RESERVED.contains(character));
    if keep {
      written.push(character);
      continue;
    }
    let mut bytes = [0u8; 4];
    for byte in character.encode_utf8(&mut bytes).as_bytes() {
      written.push_str(&format!("%{byte:02X}"));
    }
  }
  written
}

/// `str:decode-uri`: `%` escapes read back, as UTF-8.
///
/// An escape that is not two hexadecimal digits, or bytes that are not UTF-8, is left as it
/// stands. EXSLT says the result is the empty string when the input is not a valid URI, but text
/// that survives a round trip is more use than nothing, and losing it silently would be worse.
fn decode_uri(text: &str) -> String {
  let mut bytes: Vec<u8> = Vec::new();
  let mut characters = text.chars().peekable();
  while let Some(character) = characters.next() {
    if character != '%' {
      let mut buffer = [0u8; 4];
      bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
      continue;
    }
    let digits: String = characters.clone().take(2).collect();
    match u8::from_str_radix(&digits, 16) {
      Ok(byte) if digits.len() == 2 => {
        bytes.push(byte);
        characters.next();
        characters.next();
      }
      _ => bytes.push(b'%'),
    }
  }
  String::from_utf8(bytes).unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn padding_repeats_or_cuts_to_the_length_asked_for() {
    assert_eq!(padding(5.0, " "), "     ");
    assert_eq!(padding(5.0, "ab"), "ababa");
    assert_eq!(padding(0.0, "-"), "");
    assert_eq!(padding(-1.0, "-"), "");
    assert_eq!(padding(3.0, ""), "", "nothing to repeat");
  }

  #[test]
  fn align_places_text_within_a_field_as_wide_as_the_second_argument() {
    assert_eq!(align("x", "-----", "left"), "x----");
    assert_eq!(align("x", "-----", "right"), "----x");
    assert_eq!(align("x", "-----", "center"), "--x--");
    assert_eq!(align("x", "-----", "anything else"), "x----", "left is the default");
  }

  #[test]
  fn text_too_long_for_the_field_is_cut_at_the_end_that_is_not_aligned() {
    assert_eq!(align("abcdef", "---", "left"), "abc");
    assert_eq!(align("abcdef", "---", "right"), "def");
    assert_eq!(align("abcdef", "---", "center"), "bcd");
  }

  #[test]
  fn encoding_escapes_what_a_uri_may_not_hold() {
    assert_eq!(encode_uri("a b", false), "a%20b");
    assert_eq!(encode_uri("a/b", false), "a/b", "reserved characters are kept by default");
    assert_eq!(encode_uri("a/b", true), "a%2Fb");
    assert_eq!(encode_uri("a-_.!~*'()b", false), "a-_.!~*'()b", "the unreserved set is untouched");
  }

  #[test]
  fn encoding_escapes_the_utf8_bytes_of_a_character() {
    // Three bytes, so three escapes — not one escape of a code point.
    assert_eq!(encode_uri("\u{65e5}", false), "%E6%97%A5");
  }

  #[test]
  fn decoding_reads_the_escapes_back() {
    assert_eq!(decode_uri("a%20b"), "a b");
    assert_eq!(decode_uri("%E6%97%A5"), "\u{65e5}");
    assert_eq!(decode_uri("plain"), "plain");
  }

  #[test]
  fn a_round_trip_gives_back_what_went_in() {
    for text in ["a b", "\u{65e5}\u{672c}", "a/b?c=d", "100%"] {
      assert_eq!(decode_uri(&encode_uri(text, true)), text, "{text}");
    }
  }

  #[test]
  fn an_escape_that_is_not_one_is_left_as_it_stands() {
    assert_eq!(decode_uri("100% sure"), "100% sure");
    assert_eq!(decode_uri("%zz"), "%zz");
  }
}
