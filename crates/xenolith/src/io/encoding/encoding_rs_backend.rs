//! An implementation of [`Decoder`] based on `encoding_rs`.
//!
//! `encoding_rs` is Mozilla's implementation of the WHATWG Encoding Standard, used in Firefox and Gecko. The
//! functionality of this module is available only via [`super::lookup`] and only when the `encoding_rs` feature is
//! enabled. While `encoding_rs` typically substitutes U+FFFD for malformed input, XML forbids this, so such cases are
//! treated as fatal errors.

#[cfg(test)]
mod test;

use encoding_rs::{DecoderResult, Encoding};

use super::{Decoder, malformed};
use crate::error::Result;

pub(super) fn lookup(label: &str) -> Option<Box<dyn Decoder>> {
  let encoding = Encoding::for_label(label.as_bytes())?;
  // Replacement is a decoding mechanism designed to prevent attacks that exploit unsupported character encodings.
  // Certain legacy stateful encodings — encodings where state changes based on escape sequences — can be exploited for
  // XSS or content-sniffing attacks in browsers. The WHATWG mitigates this risk by mapping the labels for such
  // encodings to "Replacement" rather than to an actual decoder. Since this mechanism simply and silently decodes
  // input into a single replacement character, these encodings are effectively treated as unsupported and are meant to
  // be rejected.
  if encoding == encoding_rs::REPLACEMENT {
    return None;
  }
  Some(Box::new(EncodingRsDecoder { name: encoding.name(), inner: encoding.new_decoder_without_bom_handling() }))
}

/// Encodings provided by this backend in addition to the built-in ones.
///
/// Since `encoding_rs` does not define its own enumeration type, we define the set here ourselves. This set comprises
/// the WHATWG set—minus the three encodings excluded via "replacement encoding [`lookup`]"—plus UTF-8 and UTF-16,
/// which are handled by built-in decoders.
///
/// See also: https://encoding.spec.whatwg.org/#names-and-labels
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

/// Returns the encoder corresponding to `normalized`. Returns `None` if this backend does not support that encoding.
///
/// UTF-16 is intentionally excluded. Since the WHATWG Encoding Standard does not define a UTF-16 encoder,
/// `encoding_rs` does not provide one either.
pub(crate) fn encoder_lookup(normalized: &str) -> Option<Box<dyn super::Encoder>> {
  let encoding = Encoding::for_label(normalized.as_bytes())?;
  if encoding.output_encoding() != encoding {
    // A replaced encoding, which the standard maps to something else on the way out. Refusing it keeps the bytes and
    // the declared name in step.
    return None;
  }
  Some(Box::new(BackendEncoder { encoding }))
}

/// Encodes through `encoding_rs`.
#[derive(Debug)]
struct BackendEncoder {
  encoding: &'static Encoding,
}

impl super::Encoder for BackendEncoder {
  fn encoding(&self) -> &str {
    self.encoding.name()
  }

  fn encode(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()> {
    let (bytes, _, unmappable) = self.encoding.encode(text);
    if unmappable {
      // `encode` substitutes a reference for what it cannot hold, so the substitution says one was needed. Which
      // character it was takes a second pass, and only a refused write pays for it.
      let c = text.chars().find(|c| self.encoding.encode(&c.to_string()).2).unwrap_or('\u{fffd}');
      return Err(super::encoder::unrepresentable(self.encoding.name(), c));
    }
    out.extend_from_slice(&bytes);
    Ok(())
  }

  fn encode_with_references(&mut self, text: &str, out: &mut Vec<u8>) -> Result<()> {
    // `encoding_rs` writes a decimal character reference for anything it cannot hold, which is what XML asks for.
    let (bytes, _, _) = self.encoding.encode(text);
    out.extend_from_slice(&bytes);
    Ok(())
  }
}
