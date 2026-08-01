//! Character decoding.
//!
//! xylogue implements only the encodings an XML processor is required to support — UTF-8,
//! UTF-16, US-ASCII and ISO-8859-1 — and delegates the rest to [`encoding_rs`] behind the
//! `encodings` feature. The [`Decoder`] trait is the seam: it keeps that dependency out of
//! the parser and lets a minimal build drop it entirely.

mod builtin;
mod detect;
#[cfg(feature = "encodings")]
mod encoding_rs_backend;

pub use builtin::{AsciiDecoder, Latin1Decoder, Utf8Decoder, Utf16Decoder};
pub use detect::{Detection, DetectionSource, detect, parse_declared_encoding};

use crate::error::{Error, ErrorKind, Result};

/// Incremental byte-to-`char` decoder.
///
/// A decoder is fed successive slices of an entity and appends the decoded text to a
/// [`String`]. Bytes that form an incomplete sequence at the end of a slice are left
/// unconsumed so the caller can carry them into the next read; `last` tells the decoder that
/// no more input follows, turning a truncated sequence into an error.
///
/// # Examples
///
/// Feeding an entity in two pieces, where the split falls inside a character:
///
/// ```
/// use xylogue_core::encoding;
///
/// let bytes = "日本".as_bytes(); // 6 bytes, 2 characters
/// let mut decoder = encoding::decoder_for("UTF-8")?;
/// let mut text = String::new();
///
/// // The first chunk ends mid-character, so its last byte is not consumed.
/// let consumed = decoder.decode(&bytes[..4], &mut text, false)?;
/// assert_eq!((text.as_str(), consumed), ("日", 3));
///
/// // Hand the unconsumed byte back along with the rest.
/// decoder.decode(&bytes[consumed..], &mut text, true)?;
/// assert_eq!(text, "日本");
/// # Ok::<(), xylogue_core::Error>(())
/// ```
pub trait Decoder: std::fmt::Debug + Send {
  /// The canonical name of the encoding being decoded.
  fn encoding(&self) -> &str;

  /// Decodes a prefix of `src`, appending the result to `dst`.
  ///
  /// Returns the number of bytes consumed, which may be less than `src.len()` when the
  /// slice ends mid-sequence.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::Encoding`] on a malformed sequence. XML gives processors no
  /// latitude here: a decoding failure is a fatal error, never a replacement character.
  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize>;
}

/// Returns a decoder for the given encoding name, or `None` if the name is unknown.
///
/// Names are matched case-insensitively, as XML requires. When the `encodings` feature is
/// disabled only the built-in encodings resolve; use [`decoder_for`] to get an error that
/// says so.
#[must_use]
pub fn lookup(label: &str) -> Option<Box<dyn Decoder>> {
  let normalized = label.trim().to_ascii_lowercase();
  let builtin: Option<Box<dyn Decoder>> = match normalized.as_str() {
    "utf-8" | "utf8" => Some(Box::new(Utf8Decoder::new())),
    "utf-16" | "utf16" | "utf-16le" | "utf16le" => Some(Box::new(Utf16Decoder::little_endian())),
    "utf-16be" | "utf16be" => Some(Box::new(Utf16Decoder::big_endian())),
    "us-ascii" | "ascii" | "ansi_x3.4-1968" => Some(Box::new(AsciiDecoder::new())),
    "iso-8859-1" | "iso8859-1" | "latin1" | "iso_8859-1" => Some(Box::new(Latin1Decoder::new())),
    _ => None,
  };
  #[cfg(feature = "encodings")]
  {
    builtin.or_else(|| encoding_rs_backend::lookup(&normalized))
  }
  #[cfg(not(feature = "encodings"))]
  {
    builtin
  }
}

/// Returns a decoder for `label`, or an error explaining why it is unavailable.
///
/// # Examples
///
/// ```
/// use xylogue_core::{ErrorKind, encoding};
///
/// let mut decoder = encoding::decoder_for("iso-8859-1")?;
/// let mut text = String::new();
/// decoder.decode(&[0x63, 0x61, 0x66, 0xE9], &mut text, true)?;
/// assert_eq!(text, "café");
///
/// // A decoding failure is fatal: XML has no replacement character.
/// let mut ascii = encoding::decoder_for("US-ASCII")?;
/// let err = ascii.decode(&[0xE9], &mut String::new(), true).unwrap_err();
/// assert_eq!(err.kind(), ErrorKind::Encoding);
/// # Ok::<(), xylogue_core::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`ErrorKind::Encoding`] for an unknown encoding, or
/// [`ErrorKind::UnsupportedFeature`] when the name is one that only the `encodings` feature
/// can provide.
pub fn decoder_for(label: &str) -> Result<Box<dyn Decoder>> {
  match lookup(label) {
    Some(decoder) => Ok(decoder),
    #[cfg(feature = "encodings")]
    None => Err(Error::new(
      ErrorKind::Encoding,
      format!("no encoding is registered under the name {label:?}; check the spelling of the encoding declaration"),
    )),
    #[cfg(not(feature = "encodings"))]
    None => Err(Error::unsupported_feature(
      format!("decoding {label:?}"),
      "encodings",
      "this build handles only UTF-8, UTF-16, US-ASCII and ISO-8859-1",
    )),
  }
}

/// Builds the fatal error for a malformed byte sequence.
fn malformed(encoding: &str, offset: usize) -> Error {
  Error::new(ErrorKind::Encoding, format!("malformed {encoding} sequence at byte offset {offset}"))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn decode_all(label: &str, bytes: &[u8]) -> Result<String> {
    let mut decoder = decoder_for(label)?;
    let mut out = String::new();
    let consumed = decoder.decode(bytes, &mut out, true)?;
    assert_eq!(consumed, bytes.len());
    Ok(out)
  }

  #[test]
  fn labels_are_case_insensitive() {
    assert_eq!(decode_all("UTF-8", b"a").unwrap(), "a");
    assert_eq!(decode_all(" utf8 ", b"a").unwrap(), "a");
    assert_eq!(decode_all("US-ASCII", b"a").unwrap(), "a");
  }

  #[test]
  fn unknown_encoding_is_an_error() {
    let err = decoder_for("no-such-encoding").unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::Encoding | ErrorKind::UnsupportedFeature));
  }

  #[cfg(feature = "encodings")]
  #[test]
  fn delegated_encodings_resolve() {
    assert_eq!(decode_all("Shift_JIS", &[0x93, 0xFA, 0x96, 0x7B]).unwrap(), "日本");
    assert_eq!(decode_all("EUC-JP", &[0xC6, 0xFC, 0xCB, 0xDC]).unwrap(), "日本");
    assert_eq!(decode_all("windows-1252", &[0x80]).unwrap(), "\u{20AC}");
  }

  #[cfg(feature = "encodings")]
  #[test]
  fn delegated_encodings_reject_malformed_input() {
    // A lead byte with no trailing byte is fatal, not U+FFFD.
    let err = decode_all("Shift_JIS", &[0x93, 0x20, 0x93]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Encoding);
  }
}
