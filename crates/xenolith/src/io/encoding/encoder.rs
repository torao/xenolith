//! Converts text into the byte sequence required by a specific encoding.
//!
//! This is the output-side counterpart to the [`Decoder`](super::Decoder). While a reader decodes given input, a
//! writer encodes data into the required format. These two processes are not symmetrical. Decoding fails when it
//! encounters a byte sequence it cannot interpret as a character. Encoding, on the other hand, fails when it
//! encounters a character that cannot be represented in the target encoding; however, in XML, behavior differs
//! depending on where the character appears.
//!
//! Within character data or attribute values, characters that cannot be represented in the encoding are written out as
//! "character references" and restored to their original characters when read back. In other locations (such as
//! element names, attribute names, comments, processing instructions, or CDATA sections), references are not
//! recognized as XML markup, so you cannot write out the character itself. [`encode`](Encoder::encode) is used in
//! these contexts and reports any characters that could not be written. Conversely,
//! [`encode_with_references`](Encoder::encode_with_references) is used in the two aforementioned cases (character data
//! and attribute values) where the use of character references is permitted.

#[cfg(test)]
mod test;

use crate::error::{Error, Result};

/// Converts text into a byte sequence using a specific encoding.
///
/// Obtain an encoder from [`encoder_for`](super::encoder_for). Since the encoder does not maintain state between
/// calls, the same encoder can be used to process the entire document.
pub trait Encoder: std::fmt::Debug + Send {
  /// The canonical name of the encoding to be written.
  fn encoding(&self) -> &str;

  /// Encodes `text` and appends the resulting byte sequence to `out`.
  ///
  /// Use this method when character references are not treated as markup (such as names, comments, processing
  /// instructions, or CDATA sections).
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] identifying the first character that cannot be encoded. In this case, nothing is
  /// appended to `out`, so the output remains unchanged.
  ///
  fn encode(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()>;

  /// Encodes `text`, writing out character references for any characters that cannot be represented in the specified
  /// encoding.
  ///
  /// Use this only in contexts where character references are interpreted as the original characters (such as
  /// character data or attribute values).
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] if even the ASCII characters required to write the character references cannot be
  /// represented in the encoding (note that no encodings permitted by XML impose such a constraint).
  fn encode_with_references(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()>;
}

/// An error that occurs when the character count exceeds the limit for `encoding` by one character.
pub(super) fn unrepresentable(encoding: &str, c: char) -> Error {
  // The fault is a character the caller asked to write, not a byte of input, so there is no byte offset to give.
  Error::encoding(format!("{encoding} cannot hold {c:?} (U+{:04X})", u32::from(c)), None)
}

/// `c` is written out as a decimal character reference. It is an ASCII character and is included in all XML-permitted
/// encodings.
fn push_reference(out: &mut Vec<u8>, c: char) {
  out.extend_from_slice(format!("&#{};", u32::from(c)).as_bytes());
}

/// A UTF-8 encoder capable of handling all available characters.
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

/// A single-byte encoder that treats the first `limit` code points as byte values.
///
/// Since US-ASCII and ISO-8859-1 differ only in their end positions, a single encoder can support both.
///
#[derive(Clone, Copy, Debug)]
pub struct SingleByteEncoder {
  name: &'static str,
  limit: u32,
}

impl SingleByteEncoder {
  /// Creates an encoder for US-ASCII that supports the range U+0000 to U+007F.
  #[must_use]
  pub const fn ascii() -> Self {
    Self { name: "US-ASCII", limit: 0x80 }
  }

  /// Create an ISO-8859-1 encoder that handles U+0000 through U+00FF.
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
