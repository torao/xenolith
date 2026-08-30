//! An implementation of [`Decoder`] based on `encoding_rs`.
//!
//! `encoding_rs` is a Mozilla implementation of the WHATWG Encoding Standard used by Firefox and Gecko. The features
//! in this module are accessible only through [`super::lookup`] and only when the `encoding_rs` feature is enabled.
//! `encoding_rs` typically substisutes U+FFFD for malformed input; however, since this is prohibited in XML, it is
//! treated as a fatal error.
//!

use encoding_rs::{DecoderResult, Encoding};

use super::{Decoder, malformed};
use crate::error::Result;

pub(super) fn lookup(label: &str) -> Option<Box<dyn Decoder>> {
  let encoding = Encoding::for_label(label.as_bytes())?;
  // Replacement is a decoding mechanism designed to prevent attacks using unsupported encodings. Some legacy stateful
  // encodings (those where the state changes due to escape sequence) can be exploited in browser for XSS and content
  // sniffing attacks; WHATWG neutralizes these by mapping all such labels to this Replacement rather than to an actual
  // decoder. Since it simply decodes the input to a single replacement character silently, we do not support those
  // encodings and should reject them.
  if encoding == encoding_rs::REPLACEMENT {
    return None;
  }
  Some(Box::new(EncodingRsDecoder { name: encoding.name(), inner: encoding.new_decoder_without_bom_handling() }))
}

/// The encodings provided by this backend in addition to the built-in functionality.
///
/// `encoding_rs` does not have its own enumeration, we are defining the set here. This set consists of the WHATWG set
/// minus three encodings: specifically, the encoding rejected by the substitution encoding [`lookup`], as well as
/// UTF-8 and UTF-16, which are handled by the built-in decoders.
///
/// See also: https://encoding.spec.whatwg.org/#names-and-labels
///
pub(super) fn extra_encodings() -> impl Iterator<Item = &'static str> {
  const EXTRA: &[&Encoding] = &[
    encoding_rs::BIG5,
    encoding_rs::EUC_JP,
    encoding_rs::EUC_KR,
    encoding_rs::GB18030,
    encoding_rs::GBK,
    encoding_rs::IBM866,
    encoding_rs::ISO_2022_JP,
    encoding_rs::ISO_8859_2,
    encoding_rs::ISO_8859_3,
    encoding_rs::ISO_8859_4,
    encoding_rs::ISO_8859_5,
    encoding_rs::ISO_8859_6,
    encoding_rs::ISO_8859_7,
    encoding_rs::ISO_8859_8,
    encoding_rs::ISO_8859_8_I,
    encoding_rs::ISO_8859_10,
    encoding_rs::ISO_8859_13,
    encoding_rs::ISO_8859_14,
    encoding_rs::ISO_8859_15,
    encoding_rs::ISO_8859_16,
    encoding_rs::KOI8_R,
    encoding_rs::KOI8_U,
    encoding_rs::MACINTOSH,
    encoding_rs::SHIFT_JIS,
    encoding_rs::WINDOWS_874,
    encoding_rs::WINDOWS_1250,
    encoding_rs::WINDOWS_1251,
    encoding_rs::WINDOWS_1252,
    encoding_rs::WINDOWS_1253,
    encoding_rs::WINDOWS_1254,
    encoding_rs::WINDOWS_1255,
    encoding_rs::WINDOWS_1256,
    encoding_rs::WINDOWS_1257,
    encoding_rs::WINDOWS_1258,
    encoding_rs::X_MAC_CYRILLIC,
    encoding_rs::X_USER_DEFINED,
  ];
  EXTRA.iter().map(|encoding| encoding.name())
}

struct EncodingRsDecoder {
  name: &'static str,
  inner: encoding_rs::Decoder,
}

impl std::fmt::Debug for EncodingRsDecoder {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("EncodingRsDecoder").field("encoding", &self.name).finish()
  }
}

impl Decoder for EncodingRsDecoder {
  fn encoding(&self) -> &str {
    self.name
  }

  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize> {
    let mut total = 0;
    let mut floor = 4;
    loop {
      let remaining = &src[total..];
      // Estimated upper bound on the UTF-8 output for the whole input; enough for a single call in the common case.
      // Doubled floor will be added on OutputFull so the retry always enlarges the buffer.
      let headroom =
        self.inner.max_utf8_buffer_length_without_replacement(remaining.len()).unwrap_or(remaining.len()).max(floor);
      dst.reserve(headroom);
      let (result, read) = self.inner.decode_to_string_without_replacement(&src[total..], dst, last);
      total += read;
      match result {
        // All input has been decoded.
        DecoderResult::InputEmpty => return Ok(total),
        // The reservation did not fit one step. Grow it so the next attempt strictly enlarges the buffer and go round
        // again.
        DecoderResult::OutputFull => floor = floor.saturating_mul(2),
        // The start of the malformed bytes is located exactly one position behind `total` in the `bad` field.
        DecoderResult::Malformed(bad, _) => return Err(malformed(self.name, total - usize::from(bad))),
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn decode(label: &str, src: &[u8]) -> Result<String> {
    let mut d = lookup(label).expect("known label");
    let mut out = String::new();
    d.decode(src, &mut out, true)?;
    Ok(out)
  }

  #[test]
  fn decodes_japanese_legacy_encodings() {
    assert_eq!(decode("shift_jis", &[0x82, 0xA0]).unwrap(), "あ");
    assert_eq!(decode("euc-jp", &[0xA4, 0xA2]).unwrap(), "あ");
    assert_eq!(decode("iso-2022-jp", b"\x1b$B$\"\x1b(B").unwrap(), "あ");
  }

  #[test]
  fn reports_the_canonical_encoding_name() {
    let d = lookup("sjis").expect("alias");
    assert_eq!(d.encoding(), "Shift_JIS");
  }

  #[test]
  fn malformed_input_is_fatal() {
    assert!(decode("shift_jis", &[0x82]).is_err(), "truncated at eof");
    assert!(decode("euc-jp", &[0xA4, 0x20]).is_err(), "bad trail byte");
  }

  #[test]
  fn output_grows_across_iterations() {
    // Long enough to exercise the OutputFull path and reallocation.
    let src: Vec<u8> = std::iter::repeat_n([0x82, 0xA0], 5000).flatten().collect();
    assert_eq!(decode("shift_jis", &src).unwrap().chars().count(), 5000);
  }

  #[test]
  fn the_replacement_encoding_is_refused() {
    assert!(lookup("iso-2022-cn").is_none());
  }

  #[test]
  fn a_malformed_byte_is_located_within_the_slice() {
    // 'A' decodes, then a lone Shift_JIS lead byte is truncated at end of input: the fault is
    // the second byte, reported relative to the slice, and the good prefix is kept.
    let mut d = lookup("shift_jis").expect("known label");
    let mut out = String::new();
    let err = d.decode(&[0x41, 0x82], &mut out, true).unwrap_err();
    assert_eq!(out, "A");
    assert!(matches!(err, crate::error::Error::Encoding { byte_offset: Some(1), .. }));
  }

  #[test]
  fn every_extra_encoding_resolves_to_its_own_name() {
    for name in extra_encodings() {
      let decoder = super::super::lookup(name).unwrap_or_else(|| panic!("{name} does not resolve"));
      assert_eq!(decoder.encoding(), name, "{name} is not its own canonical name");
    }
  }
}
