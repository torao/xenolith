//! Turning text into the bytes an encoding asks for.
//!
//! This is the counterpart of [`Decoder`](super::Decoder), for the way out. A reader decodes whatever it is given; a
//! writer encodes into whatever it was asked for, and the two questions are not symmetric. Decoding fails on bytes
//! that are not a character. Encoding fails on characters the target cannot hold, and XML answers that in two ways
//! depending on where the character stands.
//!
//! In character data and an attribute value, a character the encoding cannot hold is written as a character reference,
//! which reads back as the character it stands for. Everywhere else- an element or attribute name, a comment, a
//! processing instruction, or a CDATA section- a reference is not markup XML recognizes, so the character cannot be
//! written at all. [`encode`](Encoder::encode) is for those places and reports the character it could not write;
//! [`encode_with_references`](Encoder::encode_with_references) is for the two that admit a reference.
//!

use crate::error::{Error, Result};

/// Turns text into the bytes of one encoding.
///
/// Get one from [`encoder_for`](super::encoder_for). An encoder holds no state between calls, so the same one serves
/// a whole document.
///
pub trait Encoder: std::fmt::Debug + Send {
  /// The canonical name of the encoding being written.
  ///
  fn encoding(&self) -> &str;

  /// Encodes `text`, appending the bytes to `out`.
  ///
  /// Use this where a character reference is not markup: a name, a comment, a processing instruction, or a CDATA
  /// section.
  ///
  /// # Errors
  ///
  /// [`Error::Encoding`] naming the first character the encoding cannot hold. Nothing is appended to `out` in that
  /// case, so a refused write leaves the output as it was.
  ///
  fn encode(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()>;

  /// Encodes `text`, writing a character reference for anything the encoding cannot hold.
  ///
  /// Use this only in character data and attribute values, where a reference is markup that reads back as the
  /// character it stands for.
  ///
  /// # Errors
  ///
  /// [`Error::Encoding`] if the encoding cannot hold even the ASCII a reference is written with, which no encoding
  /// XML admits is short of.
  ///
  fn encode_with_references(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()>;
}

/// The error one character too large for `encoding` raises.
///
pub(super) fn unrepresentable(encoding: &str, c: char) -> Error {
  Error::encoding(format!("{encoding} cannot hold {c:?} (U+{:04X})", u32::from(c)))
}

/// Writes `c` as a decimal character reference, which is ASCII and so within every encoding XML admits.
///
fn push_reference(out: &mut Vec<u8>, c: char) {
  out.extend_from_slice(format!("&#{};", u32::from(c)).as_bytes());
}

/// Encoder for UTF-8, which holds every character.
///
#[derive(Clone, Copy, Debug, Default)]
pub struct Utf8Encoder;

impl Utf8Encoder {
  /// Creates an encoder.
  #[must_use]
  pub const fn new() -> Self {
    Self
  }
}

impl Encoder for Utf8Encoder {
  fn encoding(&self) -> &str {
    "UTF-8"
  }

  fn encode(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()> {
    out.extend_from_slice(text.as_bytes());
    Ok(())
  }

  fn encode_with_references(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()> {
    // Nothing is out of reach, so no reference is ever needed.
    self.encode(text, out)
  }
}

/// Encoder for a single-byte encoding whose bytes are the first `limit` code points.
///
/// US-ASCII and ISO-8859-1 differ only in where they stop, so one encoder serves both.
///
#[derive(Clone, Copy, Debug)]
pub struct SingleByteEncoder {
  name: &'static str,
  limit: u32,
}

impl SingleByteEncoder {
  /// Creates an encoder for US-ASCII, which holds U+0000 to U+007F.
  #[must_use]
  pub const fn ascii() -> Self {
    Self { name: "US-ASCII", limit: 0x80 }
  }

  /// Creates an encoder for ISO-8859-1, which holds U+0000 to U+00FF.
  #[must_use]
  pub const fn latin1() -> Self {
    Self { name: "ISO-8859-1", limit: 0x100 }
  }
}

impl Encoder for SingleByteEncoder {
  fn encoding(&self) -> &str {
    self.name
  }

  fn encode(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()> {
    // Check the whole run first, so a refusal leaves `out` untouched.
    if let Some(c) = text.chars().find(|c| u32::from(*c) >= self.limit) {
      return Err(unrepresentable(self.name, c));
    }
    out.extend(text.chars().map(|c| u32::from(c) as u8));
    Ok(())
  }

  fn encode_with_references(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()> {
    for c in text.chars() {
      if u32::from(c) < self.limit {
        out.push(u32::from(c) as u8);
      } else {
        push_reference(out, c);
      }
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn encoded(encoder: &mut dyn Encoder, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    encoder.encode(text, &mut out).unwrap();
    out
  }

  fn with_references(encoder: &mut dyn Encoder, text: &str) -> String {
    let mut out = Vec::new();
    encoder.encode_with_references(text, &mut out).unwrap();
    String::from_utf8(out).unwrap()
  }

  #[test]
  fn utf8_holds_everything() {
    assert_eq!(encoded(&mut Utf8Encoder::new(), "日本 & café"), "日本 & café".as_bytes());
    assert_eq!(with_references(&mut Utf8Encoder::new(), "日本"), "日本");
  }

  #[test]
  fn a_single_byte_encoding_stops_where_it_stops() {
    assert_eq!(encoded(&mut SingleByteEncoder::ascii(), "abc"), b"abc");
    assert_eq!(encoded(&mut SingleByteEncoder::latin1(), "café"), &[0x63, 0x61, 0x66, 0xE9]);
    // `é` is beyond US-ASCII but within ISO-8859-1.
    assert!(SingleByteEncoder::ascii().encode("café", &mut Vec::new()).is_err());
  }

  #[test]
  fn what_cannot_be_held_becomes_a_reference() {
    assert_eq!(with_references(&mut SingleByteEncoder::ascii(), "caf\u{e9}"), "caf&#233;");
    assert_eq!(with_references(&mut SingleByteEncoder::latin1(), "\u{20ac}"), "&#8364;");
  }

  #[test]
  fn a_refusal_leaves_the_output_as_it_was() {
    // The whole run is checked before any of it is written, so a caller can report the failure and keep going with
    // what it had.
    let mut out = b"<a>".to_vec();
    let error = SingleByteEncoder::ascii().encode("ok\u{e9}", &mut out).unwrap_err();
    assert!(error.to_string().contains("U+00E9"), "{error}");
    assert_eq!(out, b"<a>", "nothing of the refused run was appended");
  }
}
