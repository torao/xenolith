//! Character decoding.
//!
//! In addition to the UTF-8 and UTF-16 encodings required for XML processors, `xenolith` implements US-ASCII and
//! ISO-8859-1 encodings. The `encodings` feature provides all other encodings and uses the [`encoding_rs`] library.
//! The [`Decoder`] trait provides this interface, removing parser dependencies and allowing the feature to be excluded
//! from minimal builds.
//!
//! [`encoding_rs`]: https://docs.rs/encoding_rs

mod builtin;
mod detect;
mod encoder;
#[cfg(feature = "encodings")]
mod encoding_rs_backend;

#[cfg(test)]
mod test;

pub use builtin::{AsciiDecoder, Latin1Decoder, Utf8Decoder, Utf16Decoder};
pub use detect::{Detected, Detection, DetectionSource, detect};
pub use encoder::{Encoder, SingleByteEncoder, Utf8Encoder};

use crate::error::{Error, Result};

/// Incremental byte-to-`char` decoder.
///
/// Data (slices) from the entity is passed to the decoder sequentially, and the decoded text is appended to a
/// [`String`]. Bytes forming an incomplete sequence at the end of a slice are left unprocessed, allowing the caller to
/// carry them over to the next read. The `last` parameter notifies the decoder that there is no further input, causing
/// any incomplete sequences to be treated as errors.
///
/// # Examples
///
/// When inputting an entity in two chunks (where the split occurs in the middle of a character):
///
/// ```
/// use xenolith::io::encoding;
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
/// # Ok::<(), xenolith::Error>(())
/// ```
pub trait Decoder: std::fmt::Debug + Send {
  /// The canonical name of the encoding to be decoded.
  fn encoding(&self) -> &str;

  /// Decodes the beginning of `src` and appends the result to `dst`.
  ///
  /// Returns the number of bytes consumed. If the slice ends in the middle of a sequence, this value may be less than
  /// `src.len()`.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] if a malformed sequence is encountered; the XML specification allows for no other
  /// behavior. Decoding failure is treated as a fatal error, and no replacement characters are ever inserted. If input
  /// containing invalid characters must be processed, those characters must be replaced prior to input.
  ///
  /// Before returning the error, the decoder appends all characters successfully decoded from the bytes preceding the
  /// failure point to `dst`. Consequently, `dst` contains only the text up to the point where the error occurred. The
  /// error information includes this location; specifically, the [`byte_offset`](Error::Encoding) indicates the index
  /// of the first byte in `src` that could not be decoded. Although processing cannot resume due to the fatal nature
  /// of the failure, the partial text and offset are provided for callers that need to report exactly where the
  /// processing failed.
  fn decode(&mut self, src: &[u8], dst: &mut String, last: bool) -> Result<usize>;
}

/// Returns the decoder corresponding to the specified encoding name. Returns `None` if the name is unknown.
///
/// Matching of the `label` is performed in a case-insensitive manner and ignores leading and trailing whitespace, in
/// accordance with XML specifications; for example, `"utf-8"`, `"UTF-8"`, and `" UTF-8 "` are all treated as the same.
/// If the `encodings` feature is disabled, only built-in encodings are eligible for resolution.
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

/// Returns the decoder corresponding to `label`. Returns an error if no decoder is available.
///
/// This corresponds to [`lookup`] and is the method that should be used by default. While both resolve the same names,
/// they differ in how they handle mismatches.
///
/// [`lookup`] returns `None` when no decoder is available, treating this as a standard processing branch (such as for
/// name validation or attempting a fallback). In contrast, `decoder_for` returns a diagnostic message intended for the
/// author of the encoding declaration.
///
/// As with [`lookup`], matching the `label` is case-insensitive and ignores leading and trailing whitespace.
///
/// # Examples
///
/// ```
/// use xenolith::Error;
/// use xenolith::io::encoding;
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
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// # Errors
///
/// In builds where `encodings` is enabled, [`Error::Encoding`] is returned for undefined encodings. In builds where
/// `encodings` is disabled, [`Error::UnsupportedFeature`] is returned for anything other than built-in encodings.
pub fn decoder_for(label: &str) -> Result<Box<dyn Decoder>> {
  match lookup(label) {
    Some(decoder) => Ok(decoder),
    #[cfg(feature = "encodings")]
    None => Err(Error::encoding(
      format!("no encoding is registered under the name {label:?}; check the spelling of the encoding declaration"),
      None,
    )),
    #[cfg(not(feature = "encodings"))]
    None => Err(Error::unsupported_feature(
      format!("decoding {label:?}"),
      "encodings",
      "this build handles only UTF-8, UTF-16, US-ASCII and ISO-8859-1",
    )),
  }
}

/// An encoding label representing "UTF-8". This is one of the encodings that all XML processors are required to
/// support.
pub const UTF_8: &str = "UTF-8";

/// An encoding label representing `"UTF-16"`. This is one of the encodings that all XML processors are required to
///  support.
pub const UTF_16: &str = "UTF-16";

/// The canonical name of the built-in encoding.
const BUILTIN_ENCODINGS: [&str; 6] = [UTF_8, UTF_16, "UTF-16LE", "UTF-16BE", "US-ASCII", "ISO-8859-1"];

/// Iterates the canonical names of all encodings resolvable by [`decoder_for`].
///
/// If the `encodings` feature is enabled, this iterator includes encoding names provided by [`encoding_rs`] in
/// addition to the built-in encodings. Case-insensitive aliases (such as `latin1` for `ISO-8859-1` or `sjis` for
/// `Shift_JIS`) are also resolvable via [`decoder_for`], even if they are not listed here.
///
/// This set may expand as new encoding capabilities are added to [`encoding_rs`].
///
/// [`encoding_rs`]: https://docs.rs/encoding_rs
///
/// # Examples
///
/// ```
/// use xenolith::io::encoding;
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
  Error::encoding(format!("malformed {encoding} sequence"), Some(offset))
}

/// Returns an encoder corresponding to the given `label`. An error is thrown if the current build does not support
/// writing in that encoding.
///
/// `label` matching is performed in the same way as [`lookup`]: it is case-insensitive, and leading or trailing
/// whitespace is ignored.
///
/// UTF-8, US-ASCII, and ISO-8859-1 are supported out of the box; other encodings require the `encodings` feature.
/// **Outputting to UTF-16 is not supported.** This differs from the behavior for reading. The WHATWG Encoding Standard
/// does not define a UTF-16 encoder; furthermore, while spec-compliant UTF-16 data must begin with a Byte Order Mark
/// (BOM), adding a BOM is the responsibility of the writer process rather than the encoder itself.
///
/// # Examples
///
/// ```
/// use xenolith::io::encoding::encoder_for;
///
/// let mut ascii = encoder_for("US-ASCII")?;
/// let mut out = Vec::new();
///
/// // Where a character reference is markup, one stands in for what the encoding cannot hold.
/// ascii.encode_with_references("caf\u{e9}", &mut out)?;
/// assert_eq!(out, b"caf&#233;");
///
/// // Where it is not, the same character is refused rather than written as something else.
/// assert!(ascii.encode("caf\u{e9}", &mut Vec::new()).is_err());
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::Encoding`] is raised for labels that do not have a corresponding encoding, and
/// [`Error::UnsupportedFeature`] is raised for labels that require the `encodings` feature for writing.
pub fn encoder_for(label: &str) -> Result<Box<dyn Encoder>> {
  let normalized = label.trim().to_ascii_lowercase();
  let builtin: Option<Box<dyn Encoder>> = match normalized.as_str() {
    "utf-8" | "utf8" => Some(Box::new(Utf8Encoder::new())),
    "us-ascii" | "ascii" | "ansi_x3.4-1968" => Some(Box::new(SingleByteEncoder::ascii())),
    "iso-8859-1" | "iso8859-1" | "latin1" | "iso_8859-1" => Some(Box::new(SingleByteEncoder::latin1())),
    _ => None,
  };
  if let Some(encoder) = builtin {
    return Ok(encoder);
  }
  if normalized.starts_with("utf-16") || normalized.starts_with("utf16") {
    return Err(Error::encoding(
      format!("{label:?} can be read but not written; no UTF-16 encoder is defined, so write UTF-8 instead"),
      None,
    ));
  }
  #[cfg(feature = "encodings")]
  {
    encoding_rs_backend::encoder_lookup(&normalized)
      .ok_or_else(|| Error::encoding(format!("no encoding answers to {label:?}"), None))
  }
  #[cfg(not(feature = "encodings"))]
  {
    Err(Error::unsupported_feature(format!("writing {label:?}"), "encodings", "write UTF-8 instead"))
  }
}
