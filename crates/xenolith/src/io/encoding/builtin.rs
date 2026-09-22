//! Decoders for the encoding that all XML processors must support.

#[cfg(test)]
mod test;

use super::{Decoder, UTF_8, UTF_16, malformed};
use crate::error::{Error, Result};

/// Decoder for UTF-8.
#[derive(Clone, Copy, Debug, Default)]
pub struct Utf8Decoder;

impl Utf8Decoder {
  /// Creates a decoder.
  #[must_use]
  pub const fn new() -> Self {
    Self
  }
}

impl Decoder for Utf8Decoder {
  fn encoding(&self) -> &str {
    UTF_8
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize> {
    match std::str::from_utf8(src) {
      Ok(s) => {
        dst.push_str(s);
        Ok(src.len())
      }
      Err(e) => {
        // Append the good prefix whatever happens next, so `dst` holds the text up to the fault.
        let valid_up_to = e.valid_up_to();
        if valid_up_to > 0 {
          dst.push_str(std::str::from_utf8(&src[..valid_up_to]).unwrap_or_default());
        }
        // `error_len() == None` means the input merely ends mid-sequence, which is only fatal at `last`.
        if e.error_len().is_some() || last { Err(malformed(self.encoding(), valid_up_to)) } else { Ok(valid_up_to) }
      }
    }
  }
}

/// A decoder for UTF-16.
///
/// The byte order is either explicitly specified as [`little_endian`](Self::little_endian) or
/// [`big_endian`](Self::big_endian), or automatically determined from the first two bytes of the byte sequence using
/// [`from_mark`](Self::from_mark). This format is required for entities labeled "UTF-16" in Appendix F of XML 1.0.
#[derive(Clone, Copy, Debug)]
pub struct Utf16Decoder {
  order: ByteOrder,
}

/// The byte order of a [`Utf16Decoder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ByteOrder {
  /// Not yet decided: taken from a leading mark, or little-endian if there is none.
  FromMark,
  Little,
  Big,
}

impl Utf16Decoder {
  /// Creates a little-endian decoder.
  #[must_use]
  pub const fn little_endian() -> Self {
    Self { order: ByteOrder::Little }
  }

  /// Creates a big-endian decoder.
  #[must_use]
  pub const fn big_endian() -> Self {
    Self { order: ByteOrder::Big }
  }

  /// Create a decoder that determines the byte order from the leading Byte Order Mark (BOM).
  ///
  /// `FE FF` indicates big-endian, while `FF FE` indicates little-endian. This mark is read (consumed) but not
  /// included in the output. Sequences without the mark are read as little-endian, per the WHATWG Encoding Standard.
  #[must_use]
  pub const fn from_mark() -> Self {
    Self { order: ByteOrder::FromMark }
  }

  fn unit(&self, bytes: &[u8]) -> u16 {
    let pair = [bytes[0], bytes[1]];
    if self.order == ByteOrder::Big { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) }
  }
}

impl Decoder for Utf16Decoder {
  fn encoding(&self) -> &str {
    match self.order {
      ByteOrder::FromMark => UTF_16,
      ByteOrder::Little => "UTF-16LE",
      ByteOrder::Big => "UTF-16BE",
    }
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize> {
    // A decoder in an indeterminate state determines the order from a leading mark, and once the order is established,
    // it performs a recursive operation to decode the subsequent data.
    if self.order == ByteOrder::FromMark {
      if src.len() < 2 {
        if last && !src.is_empty() {
          // A single byte at the end of the input is neither a mark nor a code unit.
          return Err(malformed(self.encoding(), 0));
        }
        return Ok(0); // waits for the next input without consuming anything
      }
      let mark = match [src[0], src[1]] {
        [0xFF, 0xFE] => {
          self.order = ByteOrder::Little;
          2
        }
        [0xFE, 0xFF] => {
          self.order = ByteOrder::Big;
          2
        }
        _ => {
          self.order = ByteOrder::Little;
          0
        }
      };
      return match self.decode(&src[mark..], dst, last) {
        Ok(consumed) => Ok(mark + consumed),
        // The inner call saw the bytes after the mark, so its offset is short of the caller's by that much.
        Err(Error::Encoding { location, message, byte_offset }) => {
          Err(Error::encoding(message, byte_offset.map(|offset| offset + mark)).at(location))
        }
        Err(other) => Err(other),
      };
    }

    let mut i = 0;
    while i + 2 <= src.len() {
      let unit = self.unit(&src[i..]);
      match unit {
        0xDC00..=0xDFFF => {
          // Unpaired low surrogate: always malformed.
          return Err(malformed(self.encoding(), i));
        }
        0xD800..=0xDBFF => {
          if i + 4 > src.len() {
            // the pair may continue in the next slice
            break;
          }
          let low = self.unit(&src[i + 2..]);
          if !(0xDC00..=0xDFFF).contains(&low) {
            return Err(malformed(self.encoding(), i));
          }
          let code = 0x1_0000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
          dst.push(char::from_u32(code).ok_or_else(|| malformed(self.encoding(), i))?);
          i += 4;
        }
        _ => {
          dst.push(char::from_u32(u32::from(unit)).ok_or_else(|| malformed(self.encoding(), i))?);
          i += 2;
        }
      }
    }
    if last && i != src.len() {
      return Err(malformed(self.encoding(), i));
    }
    Ok(i)
  }
}

/// Decoder for US-ASCII, which rejects any byte with the high bit set.
#[derive(Clone, Copy, Debug, Default)]
pub struct AsciiDecoder;

impl AsciiDecoder {
  /// Creates a decoder.
  #[must_use]
  pub const fn new() -> Self {
    Self
  }
}

impl Decoder for AsciiDecoder {
  fn encoding(&self) -> &str {
    "US-ASCII"
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, _last: bool) -> Result<usize> {
    for (i, &byte) in src.iter().enumerate() {
      if byte >= 0x80 {
        return Err(malformed(self.encoding(), i));
      }
      dst.push(char::from(byte));
    }
    Ok(src.len())
  }
}

/// Decoder for ISO-8859-1, where every byte is a valid code point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Latin1Decoder;

impl Latin1Decoder {
  /// Creates a decoder.
  #[must_use]
  pub const fn new() -> Self {
    Self
  }
}

impl Decoder for Latin1Decoder {
  fn encoding(&self) -> &str {
    "ISO-8859-1"
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, _last: bool) -> Result<usize> {
    dst.extend(src.iter().map(|&b| char::from(b)));
    Ok(src.len())
  }
}
