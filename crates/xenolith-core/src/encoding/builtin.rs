//! Decoders for the encodings every XML processor must support.

use super::{Decoder, malformed};
use crate::{
  encoding::{UTF_8, UTF_16},
  error::{Error, Result},
};

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

/// Decoder for UTF-16.
///
/// The byte order is either explicitly fixed as [`little_endian`](Self::little_endian) or
/// [`big_endian`](Self::big_endian), or is automatically determined by the first two bytes of the byte sequence via
/// [`from_mark`](Self::from_mark). This is the format required by Appendix F of XML 1.0 for entity labelled
/// "UTF-16".
///
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

  /// Creates a decoder that reads its byte order from a leading byte-order mark (BOM).
  ///
  /// `FE FF` indicates big-endian and `FF FE` little endian. This mark is consumed and is not output. Sequences without
  /// a marker are read as little-endian, in accordance with the WHATWG Encoding Standard.
  ///
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
        Err(Error::Encoding { location, message, byte_offset }) => {
          Err(Error::Encoding { location, message, byte_offset: byte_offset.map(|offset| offset + mark) })
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

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::Error;

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
    assert!(matches!(err, Error::Encoding { .. }));
  }

  #[test]
  fn utf8_invalid_sequence_is_fatal_even_mid_stream() {
    let mut d = Utf8Decoder::new();
    let mut out = String::from("<");
    let err = d.decode(&[0x41, 0xC0, 0x41], &mut out, false).unwrap_err();
    // The good prefix is kept, appended after whatever `dst` already held, and the byte at
    // fault is named relative to the slice just handed over.
    assert_eq!(out, "<A");
    assert!(matches!(err, Error::Encoding { byte_offset: Some(1), .. }));
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
    let mut out = String::new();
    let err = a.decode(&[0x61, 0x62, 0xE9], &mut out, true).unwrap_err();
    assert_eq!(out, "ab");
    assert!(matches!(err, Error::Encoding { byte_offset: Some(2), .. }));
    let mut l = Latin1Decoder::new();
    assert_eq!(decode(&mut l, &[0xE9], true).unwrap().0, "é");
  }

  #[test]
  fn utf16_keeps_what_it_decoded_before_a_bad_unit() {
    // "A" then a lone low surrogate: the "A" survives, and the fault is at byte 2.
    let mut d = Utf16Decoder::little_endian();
    let mut out = String::new();
    let err = d.decode(&[0x41, 0x00, 0x00, 0xDC], &mut out, true).unwrap_err();
    assert_eq!(out, "A");
    assert!(matches!(err, Error::Encoding { byte_offset: Some(2), .. }));
  }

  #[test]
  fn utf16_bom_reads_the_byte_order_from_the_mark_and_drops_it() {
    let mut le = Utf16Decoder::from_mark();
    assert_eq!(le.encoding(), "UTF-16"); // undecided until it sees the mark
    let (text, n) = decode(&mut le, &[0xFF, 0xFE, 0x41, 0x00], true).unwrap();
    assert_eq!((text.as_str(), n), ("A", 4)); // the mark is consumed, not emitted
    assert_eq!(le.encoding(), "UTF-16LE"); // and the order is settled

    let mut be = Utf16Decoder::from_mark();
    let (text, _) = decode(&mut be, &[0xFE, 0xFF, 0x00, 0x41], true).unwrap();
    assert_eq!((text, be.encoding()), ("A".to_owned(), "UTF-16BE"));
  }

  #[test]
  fn utf16_bom_defaults_to_little_endian_without_a_mark() {
    let mut d = Utf16Decoder::from_mark();
    let (text, n) = decode(&mut d, &[0x41, 0x00, 0x42, 0x00], true).unwrap();
    assert_eq!((text.as_str(), n), ("AB", 4));
  }

  #[test]
  fn utf16_bom_waits_for_the_second_byte_of_the_mark() {
    let mut d = Utf16Decoder::from_mark();
    let mut out = String::new();
    assert_eq!(d.decode(&[0xFF], &mut out, false).unwrap(), 0); // one byte cannot settle the order
    // The caller carries the byte over and feeds it again with the rest.
    assert_eq!(d.decode(&[0xFF, 0xFE, 0x41, 0x00], &mut out, true).unwrap(), 4);
    assert_eq!(out, "A");
  }

  #[test]
  fn utf16_bom_reports_a_bad_unit_relative_to_the_whole_slice() {
    // A little-endian mark, then a lone low surrogate: the fault is byte 2, past the mark.
    let mut d = Utf16Decoder::from_mark();
    let mut out = String::new();
    let err = d.decode(&[0xFF, 0xFE, 0x00, 0xDC], &mut out, true).unwrap_err();
    assert!(matches!(err, Error::Encoding { byte_offset: Some(2), .. }));
  }

  #[test]
  fn utf16_bom_rejects_a_lone_byte_at_end_of_input() {
    let mut d = Utf16Decoder::from_mark();
    assert!(decode(&mut d, &[0x41], true).is_err());
  }
}
