//! Character decoding.
//!
//! `xenolith` implements mondatory requirements for XML processor encoding UTF-8 and UTF-16, along with additional
//! encoding US-ASCII and ISO-8859-1. All other encodings are provided by the `encodings` feature through the
//! [`encoding_rs`] library. The [`Decoder`] trait serves as this interface, eliminating dependencies on the parser and
//! allowing it to be omitted entirely in minimal builds.

mod builtin;
mod detect;
#[cfg(feature = "encodings")]
mod encoding_rs_backend;

pub use builtin::{AsciiDecoder, Latin1Decoder, Utf8Decoder, Utf16Decoder};
pub use detect::{Detected, Detection, DetectionSource, detect};

use crate::error::{Error, Location, Result};

/// Incremental byte-to-`char` decoder.
///
/// A decoder is passed slices of the entity sequentially, and the decoded text is appended to the
/// [`String`]. Bytes at the end of a slice that form an incomplete sequence are left unprocessed,
/// allowing the caller to carry them over to the next read. `last` notifies the decoder that there
/// is no further input and treats the truncated sequence as an error.
///
/// # Examples
///
/// Feeding an entity in two pieces, where the split falls inside a character:
///
/// ```
/// use xenolith_core::encoding;
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
/// # Ok::<(), xenolith_core::Error>(())
/// ```
pub trait Decoder: std::fmt::Debug + Send {
  /// The canonical name of the encoding being decoded.
  fn encoding(&self) -> &str;

  /// Decodes a prefix of `src`, appending the result to `dst`.
  ///
  /// Returns the number of bytes consumed, which may be less than `src.len()` when the slice ends mid-sequence.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] on a malformed sequence. XML does not grant processors any discretion in this
  /// regard. In other words, a decoding failure is a fatal error, and substitute characters are never inserted.
  ///
  /// Before returning, the decoder appends every character it could decode from the bytes that precede the failure, so
  /// `dst` holds the text up to the point of the error and no further. The error reports that point: its
  /// [`byte_offset`](Error::Encoding) is the index, within `src`, of the first byte that could not be decoded. Since the
  /// failure is fatal there is nothing to resume, but the partial text and the offset are there for a caller that wants
  /// to report where the entity went wrong.
  ///
  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize>;
}

/// Returns a decoder for the given encoding name, or `None` if the name is unknown.
///
/// The `label` is matched case-insensitively and with surrounding whitespace ignored, as XML requires; `"utf-8"`,
/// `"UTF-8"` and `" UTF-8 "` all resolve alike. When the `encodings` feature is disabled only the built-in encodings
/// resolve.
///
#[must_use]
pub fn lookup(label: &str) -> Option<Box<dyn Decoder>> {
  let normalized = label.trim().to_ascii_lowercase();
  let builtin: Option<Box<dyn Decoder>> = match normalized.as_str() {
    "utf-8" | "utf8" => Some(Box::new(Utf8Decoder::new())),
    "utf-16" | "utf16" => Some(Box::new(Utf16Decoder::from_mark())),
    "utf-16le" | "utf16le" => Some(Box::new(Utf16Decoder::little_endian())),
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

/// Returns a decoder for the `label`, or an error if one is not available.
///
/// This is the counterpart to the [`lookup`], and shoulud be used by default. Both resolve the exact same name, but
/// differ in how they handle mismatches.
///
/// [`lookup`] returns `None` if no available decoder exists, treating this as a normal branch (name verification,
/// fallback attempt). In contract, `decoder_for` returns a diagnostic message directed at the author fo the encoding
/// declaration.
///
/// The `label` is matched case-insensitively and with surrounding whitespace ignored, just as [`lookup`] does.
///
/// # Examples
///
/// ```
/// use xenolith_core::{Error, encoding};
///
/// let mut decoder = encoding::decoder_for("iso-8859-1")?;
/// let mut text = String::new();
/// decoder.decode(&[0x63, 0x61, 0x66, 0xE9], &mut text, true)?;
/// assert_eq!(text, "café");
///
/// // A decoding failure is fatal: XML has no replacement character. The error reports the byte
/// // at fault — here the second one.
/// let mut ascii = encoding::decoder_for("US-ASCII")?;
/// let err = ascii.decode(&[0x61, 0xE9], &mut String::new(), true).unwrap_err();
/// assert!(matches!(err, Error::Encoding { byte_offset: Some(1), .. }));
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// # Errors
///
/// In builds where `encodings` is enabled, [`Error::Encoding`] is returned for undefined encodings. In builds where
/// `encodings` is disabled, [`Error::UnsupportedFeature`] is returned for encodings other than the built-in ones.
///
pub fn decoder_for(label: &str) -> Result<Box<dyn Decoder>> {
  match lookup(label) {
    Some(decoder) => Ok(decoder),
    #[cfg(feature = "encodings")]
    None => Err(Error::encoding(format!(
      "no encoding is registered under the name {label:?}; check the spelling of the encoding declaration"
    ))),
    #[cfg(not(feature = "encodings"))]
    None => Err(Error::unsupported_feature(
      format!("decoding {label:?}"),
      "encodings",
      "this build handles only UTF-8, UTF-16, US-ASCII and ISO-8859-1",
    )),
  }
}

/// Encoding label representing `"UTF-8"`. This is one of the encodings that all XML processors are requred to support.
pub const UTF_8: &str = "UTF-8";

/// Encoding label representing `"UTF-16"`. This is one of the encodings that all XML processors are requred to support.
pub const UTF_16: &str = "UTF-16";

/// The canonical names of built-in encoding.
const BUILTIN_ENCODINGS: [&str; 6] = [UTF_8, UTF_16, "UTF-16LE", "UTF-16BE", "US-ASCII", "ISO-8859-1"];

/// Iterates the canonical names of all encodings that [`decoder_for`] can resolve.
///
/// When the `encodings` feature is enabled, the itetor includes the encoding names providec by [`encoding_rs`] in
/// addition to the built-in encodings. Case-insensitive aliases (such as `latin1` for `ISO-8859-1` and `sjis` for
/// `Shift_JIS`) are resolved in the same way via [`decoder_for`], even if they don't appear here.
///
/// This set may be expanded as new encoding features are added to [`encoding_rs`].
///
/// # Examples
///
/// ```
/// use xenolith_core::encoding;
///
/// let names: Vec<_> = encoding::supported_encodings().collect();
/// assert!(names.contains(&"UTF-8"));
/// assert!(names.contains(&"ISO-8859-1"));
/// // Every name it reports is one `decoder_for` accepts.
/// assert!(names.iter().all(|name| encoding::decoder_for(name).is_ok()));
/// ```
pub fn supported_encodings() -> impl Iterator<Item = &'static str> {
  let builtin = BUILTIN_ENCODINGS.into_iter();
  #[cfg(feature = "encodings")]
  {
    // Drop backeend names that are already in the built-in set.
    let extra = encoding_rs_backend::extra_encodings()
      .filter(|&name| !BUILTIN_ENCODINGS.iter().any(|builtin| builtin.eq_ignore_ascii_case(name)));
    builtin.chain(extra)
  }
  #[cfg(not(feature = "encodings"))]
  {
    builtin
  }
}

/// Generates a fatal error for an malformed byte sequence.
///
/// `offset` is the index of the first undecodable byte within the slice passed to the decoder. As this is stored in the
/// `byte_offset` field of the [`Error::Encoding`].
///
fn malformed(encoding: &str, offset: usize) -> Error {
  Error::Encoding {
    location: Location::unknown(),
    message: format!("malformed {encoding} sequence"),
    byte_offset: Some(offset),
  }
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
  fn mondatory_encodings_are_supported() {
    assert_eq!(decode_all(UTF_8, b"a").unwrap(), "a");
    assert_eq!(decode_all(UTF_16, &[0xFE, 0xFF, 0x00, 0x61]).unwrap(), "a");
    let encodings = supported_encodings().collect::<Vec<_>>();
    assert!(encodings.contains(&UTF_8));
    assert!(encodings.contains(&UTF_16));
    encodings.iter().for_each(|encoding| {
      println!("{}", encoding);
    });
  }

  #[test]
  fn labels_are_case_insensitive() {
    assert_eq!(decode_all("UTF-8", b"a").unwrap(), "a");
    assert_eq!(decode_all(" utf8 ", b"a").unwrap(), "a");
    assert_eq!(decode_all("US-ASCII", b"a").unwrap(), "a");
  }

  #[test]
  fn the_supported_set_holds_the_built_ins_once_each_and_all_resolve() {
    let names: Vec<_> = supported_encodings().collect();
    for builtin in BUILTIN_ENCODINGS {
      assert_eq!(names.iter().filter(|name| **name == builtin).count(), 1, "{builtin} listed other than once");
    }
    // No name appears twice, and every one resolves back to a decoder that reports that name.
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "the supported set contains a duplicate");
    for name in names {
      assert_eq!(lookup(name).expect("resolves").encoding(), name, "{name} is not canonical");
    }
  }

  #[cfg(not(feature = "encodings"))]
  #[test]
  fn without_the_feature_only_the_built_ins_are_supported() {
    assert_eq!(supported_encodings().count(), BUILTIN_ENCODINGS.len());
  }

  #[test]
  fn unknown_encoding_is_an_error() {
    let err = decoder_for("no-such-encoding").unwrap_err();
    assert!(matches!(err, Error::Encoding { .. } | Error::UnsupportedFeature { .. }));
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
    assert!(matches!(err, Error::Encoding { .. }));
  }
}
