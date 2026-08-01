//! Decoders for the encodings every XML processor must support.

use super::{Decoder, malformed};
use crate::error::Result;

/// Decoder for UTF-8.
#[derive(Clone, Copy, Debug, Default)]
pub struct Utf8Decoder {
  consumed: usize,
}

impl Utf8Decoder {
  /// Creates a decoder.
  #[must_use]
  pub const fn new() -> Self {
    Self { consumed: 0 }
  }
}

impl Decoder for Utf8Decoder {
  fn encoding(&self) -> &str {
    "UTF-8"
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize> {
    let valid = match std::str::from_utf8(src) {
      Ok(s) => {
        dst.push_str(s);
        src.len()
      }
      Err(e) => {
        let valid_up_to = e.valid_up_to();
        // `error_len() == None` means the input merely ends mid-sequence.
        if e.error_len().is_some() || last {
          return Err(malformed("UTF-8", self.consumed + valid_up_to));
        }
        dst.push_str(std::str::from_utf8(&src[..valid_up_to]).unwrap_or_default());
        valid_up_to
      }
    };
    self.consumed += valid;
    Ok(valid)
  }
}

/// Decoder for UTF-16, in either byte order.
#[derive(Clone, Copy, Debug)]
pub struct Utf16Decoder {
  big_endian: bool,
  consumed: usize,
}

impl Utf16Decoder {
  /// Creates a little-endian decoder.
  #[must_use]
  pub const fn little_endian() -> Self {
    Self { big_endian: false, consumed: 0 }
  }

  /// Creates a big-endian decoder.
  #[must_use]
  pub const fn big_endian() -> Self {
    Self { big_endian: true, consumed: 0 }
  }

  fn unit(&self, bytes: &[u8]) -> u16 {
    let pair = [bytes[0], bytes[1]];
    if self.big_endian { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) }
  }
}

impl Decoder for Utf16Decoder {
  fn encoding(&self) -> &str {
    if self.big_endian { "UTF-16BE" } else { "UTF-16LE" }
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize> {
    let mut i = 0;
    while i + 2 <= src.len() {
      let unit = self.unit(&src[i..]);
      match unit {
        // Unpaired low surrogate: always malformed.
        0xDC00..=0xDFFF => return Err(malformed(self.encoding(), self.consumed + i)),
        0xD800..=0xDBFF => {
          if i + 4 > src.len() {
            break; // the pair may continue in the next slice
          }
          let low = self.unit(&src[i + 2..]);
          if !(0xDC00..=0xDFFF).contains(&low) {
            return Err(malformed(self.encoding(), self.consumed + i));
          }
          let code = 0x1_0000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
          dst.push(char::from_u32(code).ok_or_else(|| malformed(self.encoding(), self.consumed + i))?);
          i += 4;
        }
        _ => {
          dst.push(char::from_u32(u32::from(unit)).ok_or_else(|| malformed(self.encoding(), self.consumed + i))?);
          i += 2;
        }
      }
    }
    if last && i != src.len() {
      return Err(malformed(self.encoding(), self.consumed + i));
    }
    self.consumed += i;
    Ok(i)
  }
}

/// Decoder for US-ASCII, which rejects any byte with the high bit set.
#[derive(Clone, Copy, Debug, Default)]
pub struct AsciiDecoder {
  consumed: usize,
}

impl AsciiDecoder {
  /// Creates a decoder.
  #[must_use]
  pub const fn new() -> Self {
    Self { consumed: 0 }
  }
}

impl Decoder for AsciiDecoder {
  fn encoding(&self) -> &str {
    "US-ASCII"
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, _last: bool) -> Result<usize> {
    for (i, &byte) in src.iter().enumerate() {
      if byte >= 0x80 {
        return Err(malformed("US-ASCII", self.consumed + i));
      }
      dst.push(char::from(byte));
    }
    self.consumed += src.len();
    Ok(src.len())
  }
}

/// Decoder for ISO-8859-1, where every byte is a valid code point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Latin1Decoder {
  consumed: usize,
}

impl Latin1Decoder {
  /// Creates a decoder.
  #[must_use]
  pub const fn new() -> Self {
    Self { consumed: 0 }
  }
}

impl Decoder for Latin1Decoder {
  fn encoding(&self) -> &str {
    "ISO-8859-1"
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, _last: bool) -> Result<usize> {
    dst.extend(src.iter().map(|&b| char::from(b)));
    self.consumed += src.len();
    Ok(src.len())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::ErrorKind;

  fn decode(decoder: &mut dyn Decoder, src: &[u8], last: bool) -> Result<(String, usize)> {
    let mut out = String::new();
    let n = decoder.decode(src, &mut out, last)?;
    Ok((out, n))
  }

  #[test]
  fn utf8_split_sequence_is_carried_over() {
    let mut d = Utf8Decoder::new();
    let bytes = "あa".as_bytes(); // E3 81 82 61
    let (text, n) = decode(&mut d, &bytes[..2], false).unwrap();
    assert_eq!((text.as_str(), n), ("", 0));
    let (text, n) = decode(&mut d, bytes, true).unwrap();
    assert_eq!((text.as_str(), n), ("あa", 4));
  }

  #[test]
  fn utf8_truncated_at_eof_is_fatal() {
    let mut d = Utf8Decoder::new();
    let err = decode(&mut d, &"あ".as_bytes()[..2], true).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Encoding);
  }

  #[test]
  fn utf8_invalid_sequence_is_fatal_even_mid_stream() {
    let mut d = Utf8Decoder::new();
    assert!(decode(&mut d, &[0x41, 0xC0, 0x41], false).is_err());
  }

  #[test]
  fn utf16_decodes_both_byte_orders_and_surrogate_pairs() {
    // U+1F600, as a surrogate pair.
    let mut le = Utf16Decoder::little_endian();
    let (text, n) = decode(&mut le, &[0x3D, 0xD8, 0x00, 0xDE], true).unwrap();
    assert_eq!((text.as_str(), n), ("\u{1F600}", 4));

    let mut be = Utf16Decoder::big_endian();
    let (text, _) = decode(&mut be, &[0x00, 0x41, 0xD8, 0x3D, 0xDE, 0x00], true).unwrap();
    assert_eq!(text, "A\u{1F600}");
  }

  #[test]
  fn utf16_holds_back_a_split_pair() {
    let mut d = Utf16Decoder::little_endian();
    let (text, n) = decode(&mut d, &[0x3D, 0xD8], false).unwrap();
    assert_eq!((text.as_str(), n), ("", 0));
    // An odd trailing byte is also held back.
    let (_, n) = decode(&mut d, &[0x41, 0x00, 0x3D], false).unwrap();
    assert_eq!(n, 2);
  }

  #[test]
  fn utf16_rejects_unpaired_surrogates() {
    let mut d = Utf16Decoder::little_endian();
    assert!(decode(&mut d, &[0x00, 0xDC], true).is_err(), "lone low surrogate");
    let mut d = Utf16Decoder::little_endian();
    assert!(decode(&mut d, &[0x3D, 0xD8, 0x41, 0x00], true).is_err(), "high, then non-low");
    let mut d = Utf16Decoder::little_endian();
    assert!(decode(&mut d, &[0x3D, 0xD8], true).is_err(), "truncated pair at eof");
  }

  #[test]
  fn ascii_rejects_the_high_bit_and_latin1_does_not() {
    let mut a = AsciiDecoder::new();
    assert!(decode(&mut a, &[0xE9], true).is_err());
    let mut l = Latin1Decoder::new();
    assert_eq!(decode(&mut l, &[0xE9], true).unwrap().0, "é");
  }
}
