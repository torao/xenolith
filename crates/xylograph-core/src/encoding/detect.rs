//! Encoding detection (XML 1.0, Appendix F).
//!
//! An XML entity has to be sniffed before it can be read, because the declaration that names
//! its encoding is itself encoded. The order is fixed: byte-order mark, then the byte pattern
//! of `<?xml`, then the `encoding` pseudo-attribute, then UTF-8.

/// How an encoding was determined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionSource {
  /// A byte-order mark.
  Bom,
  /// The byte pattern of the `<?xml` that starts the declaration.
  InitialBytes,
  /// The `encoding` pseudo-attribute of the XML or text declaration.
  Declaration,
  /// Nothing indicated an encoding, so UTF-8 was assumed.
  Default,
}

/// The outcome of sniffing the start of an entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
  /// Encoding name, suitable for [`super::decoder_for`].
  pub encoding: String,
  /// Length in bytes of the byte-order mark, which the decoder must not see.
  pub bom_length: usize,
  /// Where the name came from.
  pub source: DetectionSource,
}

impl Detection {
  fn new(encoding: &str, bom_length: usize, source: DetectionSource) -> Self {
    Self { encoding: encoding.to_owned(), bom_length, source }
  }
}

/// Determines the encoding of an entity from its leading bytes.
///
/// `bytes` should hold at least the first few hundred bytes of the entity. A declared
/// encoding is only looked for when the initial bytes are ASCII-compatible; for UTF-16 the
/// byte pattern already settles it, and any declaration must agree.
///
/// # Examples
///
/// ```
/// use xylograph_core::encoding::{DetectionSource, detect};
///
/// // A byte-order mark settles it, and must be skipped before decoding.
/// let d = detect(b"\xEF\xBB\xBF<doc/>");
/// assert_eq!((d.encoding.as_str(), d.bom_length), ("UTF-8", 3));
/// assert_eq!(d.source, DetectionSource::Bom);
///
/// // Otherwise the declaration is consulted.
/// let d = detect(b"<?xml version='1.0' encoding='EUC-JP'?><doc/>");
/// assert_eq!((d.encoding.as_str(), d.bom_length), ("EUC-JP", 0));
///
/// // With neither, UTF-8 is assumed.
/// assert_eq!(detect(b"<doc/>").source, DetectionSource::Default);
/// ```
#[must_use]
pub fn detect(bytes: &[u8]) -> Detection {
  if let Some(detection) = detect_bom(bytes) {
    return detection;
  }
  match bytes {
    [0x00, 0x00, 0x00, 0x3C, ..] => Detection::new("UTF-32BE", 0, DetectionSource::InitialBytes),
    [0x3C, 0x00, 0x00, 0x00, ..] => Detection::new("UTF-32LE", 0, DetectionSource::InitialBytes),
    [0x00, 0x3C, 0x00, 0x3F, ..] => Detection::new("UTF-16BE", 0, DetectionSource::InitialBytes),
    [0x3C, 0x00, 0x3F, 0x00, ..] => Detection::new("UTF-16LE", 0, DetectionSource::InitialBytes),
    // EBCDIC, in one of several code pages; the declaration cannot disambiguate it here.
    [0x4C, 0x6F, 0xA7, 0x94, ..] => Detection::new("EBCDIC", 0, DetectionSource::InitialBytes),
    _ => match parse_declared_encoding(bytes) {
      Some(encoding) => Detection { encoding, bom_length: 0, source: DetectionSource::Declaration },
      None => Detection::new("UTF-8", 0, DetectionSource::Default),
    },
  }
}

fn detect_bom(bytes: &[u8]) -> Option<Detection> {
  // UTF-32 marks must be tested before UTF-16LE, whose mark is their prefix.
  match bytes {
    [0x00, 0x00, 0xFE, 0xFF, ..] => Some(Detection::new("UTF-32BE", 4, DetectionSource::Bom)),
    [0xFF, 0xFE, 0x00, 0x00, ..] => Some(Detection::new("UTF-32LE", 4, DetectionSource::Bom)),
    [0xEF, 0xBB, 0xBF, ..] => Some(Detection::new("UTF-8", 3, DetectionSource::Bom)),
    [0xFE, 0xFF, ..] => Some(Detection::new("UTF-16BE", 2, DetectionSource::Bom)),
    [0xFF, 0xFE, ..] => Some(Detection::new("UTF-16LE", 2, DetectionSource::Bom)),
    _ => None,
  }
}

/// Extracts the `encoding` pseudo-attribute of an XML or text declaration.
///
/// Operates on raw bytes, so it only works for ASCII-compatible encodings — which is exactly
/// when it is needed. Returns `None` if there is no declaration, no `encoding` pseudo-
/// attribute, or the value is not an `EncName`.
///
/// # Examples
///
/// ```
/// use xylograph_core::encoding::parse_declared_encoding;
///
/// assert_eq!(
///   parse_declared_encoding(b"<?xml version='1.0' encoding='Shift_JIS'?>"),
///   Some("Shift_JIS".to_owned())
/// );
/// assert_eq!(parse_declared_encoding(b"<?xml version='1.0'?>"), None);
/// assert_eq!(parse_declared_encoding(b"<doc encoding='x'/>"), None);
/// ```
#[must_use]
pub fn parse_declared_encoding(bytes: &[u8]) -> Option<String> {
  let rest = bytes.strip_prefix(b"<?xml")?;
  // The name must be followed by whitespace: `<?xmlfoo` is not a declaration.
  if !rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
    return None;
  }
  // Scan no further than the end of the declaration.
  let end = find(rest, b"?>").unwrap_or(rest.len());
  let decl = &rest[..end];

  let start = find(decl, b"encoding")?;
  let after = &decl[start + b"encoding".len()..];
  let after = skip_whitespace(after);
  let after = after.strip_prefix(b"=")?;
  let after = skip_whitespace(after);
  let quote = *after.first()?;
  if quote != b'"' && quote != b'\'' {
    return None;
  }
  let value_end = after[1..].iter().position(|&b| b == quote)?;
  let value = &after[1..=value_end];

  let name = std::str::from_utf8(value).ok()?;
  crate::chars::is_enc_name(name).then(|| name.to_owned())
}

fn skip_whitespace(bytes: &[u8]) -> &[u8] {
  let n = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
  &bytes[n..]
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
  haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn boms_win_over_everything() {
    let d = detect(b"\xEF\xBB\xBF<?xml version='1.0' encoding='Shift_JIS'?>");
    assert_eq!(d.encoding, "UTF-8");
    assert_eq!(d.bom_length, 3);
    assert_eq!(d.source, DetectionSource::Bom);

    assert_eq!(detect(b"\xFE\xFF\x00<").encoding, "UTF-16BE");
    assert_eq!(detect(b"\xFF\xFE<\x00").encoding, "UTF-16LE");
    assert_eq!(detect(b"\xFF\xFE\x00\x00").encoding, "UTF-32LE");
    assert_eq!(detect(b"\x00\x00\xFE\xFF").encoding, "UTF-32BE");
  }

  #[test]
  fn utf16_is_recognised_without_a_bom() {
    let d = detect(b"\x00<\x00?\x00x\x00m\x00l");
    assert_eq!(d.encoding, "UTF-16BE");
    assert_eq!(d.bom_length, 0);
    assert_eq!(d.source, DetectionSource::InitialBytes);
    assert_eq!(detect(b"<\x00?\x00x\x00m\x00").encoding, "UTF-16LE");
  }

  #[test]
  fn declaration_names_the_encoding() {
    let d = detect(b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><doc/>");
    assert_eq!(d.encoding, "Shift_JIS");
    assert_eq!(d.source, DetectionSource::Declaration);
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0' encoding = 'euc-jp' ?>"), Some("euc-jp".to_owned()));
  }

  #[test]
  fn defaults_to_utf8() {
    assert_eq!(detect(b"<doc/>").encoding, "UTF-8");
    assert_eq!(detect(b"<doc/>").source, DetectionSource::Default);
    assert_eq!(detect(b"<?xml version='1.0'?>").encoding, "UTF-8");
    assert_eq!(detect(b"").encoding, "UTF-8");
  }

  #[test]
  fn declaration_parsing_is_strict() {
    assert_eq!(parse_declared_encoding(b"<?xmlversion='1.0' encoding='x'?>"), None);
    assert_eq!(parse_declared_encoding(b"<doc encoding='x'/>"), None);
    // The pseudo-attribute must be inside the declaration.
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0'?><a encoding='x'/>"), None);
    // Unquoted and unterminated values are rejected.
    assert_eq!(parse_declared_encoding(b"<?xml encoding=utf-8?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml encoding='utf-8?>"), None);
    // An EncName may not start with a digit.
    assert_eq!(parse_declared_encoding(b"<?xml encoding='8859-1'?>"), None);
  }
}
