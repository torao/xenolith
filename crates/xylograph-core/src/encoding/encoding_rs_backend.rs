//! [`Decoder`] implementation backed by `encoding_rs`.
//!
//! Only reachable through [`super::lookup`], and only when the `encodings` feature is on.
//! `encoding_rs` normally substitutes U+FFFD for malformed input; XML forbids that, so the
//! `*_without_replacement` entry points are used and a malformed sequence is turned into a
//! fatal error.

use encoding_rs::{DecoderResult, Encoding};

use super::{Decoder, malformed};
use crate::error::Result;

pub(super) fn lookup(label: &str) -> Option<Box<dyn Decoder>> {
  let encoding = Encoding::for_label(label.as_bytes())?;
  // Replacement is a decode-only guard against attacks on encodings we do not want to
  // support; refuse it rather than silently decoding to nothing.
  if encoding == encoding_rs::REPLACEMENT {
    return None;
  }
  Some(Box::new(EncodingRsDecoder {
    name: encoding.name(),
    inner: encoding.new_decoder_without_bom_handling(),
    consumed: 0,
  }))
}

struct EncodingRsDecoder {
  name: &'static str,
  inner: encoding_rs::Decoder,
  consumed: usize,
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
    loop {
      let remaining = &src[total..];
      let headroom =
        self.inner.max_utf8_buffer_length_without_replacement(remaining.len()).unwrap_or(remaining.len()).max(4);
      dst.reserve(headroom);

      let (result, read) = self.inner.decode_to_string_without_replacement(remaining, dst, last);
      total += read;
      match result {
        DecoderResult::InputEmpty => break,
        // The buffer we reserved was not enough; go round again with more.
        DecoderResult::OutputFull => continue,
        DecoderResult::Malformed(bad, _) => {
          let offset = self.consumed + total - usize::from(bad);
          return Err(malformed(self.name, offset));
        }
      }
    }
    self.consumed += total;
    Ok(total)
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
}
