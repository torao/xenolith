//! Encoding detection (XML 1.0, Appendix F).
//!
//! As the declaration specifying the encoding of an XML entity is itself encoded, it must be parsed before it can be read. In the case of UTF-8, the order is fixed: byte-order mark (BOM), then the byte pattern for `<xml`, followed by the `encoding` pseudo-attribute, and finally `UTF-8`.

/// Enum type that indicates how the encoding was determined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionSource {
  /// Indicates that it was determined by a byte-order mark (BOM).
  Bom,
  /// Indicates that it was determined by the byte pattern of the `<?xml` declaration at the start of a byte sequence.
  InitialBytes,
  /// Indicates that it was determined by the `encoding` pseudo-attribute in the XML or text declaration.
  Declaration,
  /// Indicates that, in the absence of any specification of the encoding, it is assumed to be UTF-8.
  Default,
}

/// The result of sniffing the start of an entity to determine its encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
  /// Encoding name that can be used with [`super::decoder_for`]. However, some encodings may not be supported by
  /// [`super::decoder_for`].
  pub encoding: String,
  /// The number of bytes in the byte-order mark that the decoder should skip.
  pub bom_length: usize,
  /// The origin of this encoding name.
  pub source: DetectionSource,
}

impl Detection {
  fn new(encoding: &str, bom_length: usize, source: DetectionSource) -> Self {
    Self { encoding: encoding.to_owned(), bom_length, source }
  }
}

/// Determines the encoding of an entity from its leading bytes.
///
/// `bytes` should hold at least the first few hundred bytes of the entity. For a declared `encoding` attribute to be
/// recognized, the first byte must be ASCII-compatible. In the case of UTF-16, as the encoding is determined by the
/// byte pattern, any encoding declaration must match the actual encoding.
///
/// This function may identify encodings that [`super::decoder_for`] does not support.
///
/// # Examples
///
/// ```
/// use xylogue_core::encoding::{DetectionSource, detect};
///
/// // If the encoding is determined by the byte-order mark, these BOMs should be skipped before decoding.
/// let d = detect(b"\xEF\xBB\xBF<doc/>");
/// assert_eq!((d.encoding.as_str(), d.bom_length), ("UTF-8", 3));
/// assert_eq!(d.source, DetectionSource::Bom);
///
/// // If it begins with an ASCII-compatible `XMLDecl`, the value of its `encoding` pseudo-attribute.
/// let d = detect(b"<?xml version='1.0' encoding='EUC-JP'?><doc/>");
/// assert_eq!((d.encoding.as_str(), d.bom_length), ("EUC-JP", 0));
///
/// // With neither, UTF-8 is assumed.
/// assert_eq!(detect(b"<doc/>").source, DetectionSource::Default);
/// ```
#[must_use]
pub fn detect(bytes: &[u8]) -> Detection {
  match bytes {
    // Determination based on a byte-order mark (U+FEFF).
    // UTF-32 marks must be tested before UTF-16LE, whose mark is their prefix.
    [0x00, 0x00, 0xFE, 0xFF, ..] => Detection::new("UTF-32BE", 4, DetectionSource::Bom),
    [0xFF, 0xFE, 0x00, 0x00, ..] => Detection::new("UTF-32LE", 4, DetectionSource::Bom),
    [0xEF, 0xBB, 0xBF, ..] => Detection::new("UTF-8", 3, DetectionSource::Bom),
    [0xFE, 0xFF, ..] => Detection::new("UTF-16BE", 2, DetectionSource::Bom),
    [0xFF, 0xFE, ..] => Detection::new("UTF-16LE", 2, DetectionSource::Bom),

    // Determination based on the first character sequence to appear within the declaration.
    // The first character in byte sequence must be `<` (U+003C), followed by `?` (U+003F) by the definition XML 1.0
    // §2.8 of `prolog ::= XMLDecl? Misc*` and `XMLDecl ::= '<?xml' VersionInfo ... '?>'`.
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

/// Extracts the `encoding` pseudo-attribute of an XML or text declaration.
///
/// As this function operates on a raw byte sequence, it only works with ASCII-compatible encodings. It returns `None`
/// if no declaration is present in the entity, if the declaration lacks the `encoding` pseudo-attiribute, or if its
/// value is not `EncName` (XML 1.0, Appendix F).
///
#[must_use]
fn parse_declared_encoding(bytes: &[u8]) -> Option<String> {
  let rest = skip_whitespace(bytes).strip_prefix(b"<?xml")?;
  // The name must be followed by whitespace: `<?xmlfoo` is not a declaration.
  if !rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
    return None;
  }
  // Read up to the end of the XML declaration.
  let end = find(rest, b"?>").unwrap_or(rest.len());
  let mut decl = &rest[..end];

  // Walk the pseudo-attributes in order and return the value of the one named `encoding`, matched
  // as a whole name: only a well-formed `name="value"` carries its name this far, so a malformed
  // one cannot be mistaken for it. A value that is not an `EncName` is no encoding at all.
  loop {
    let (name, value, remaining) = parse_pseudo_attribute(decl)?;
    if name == b"encoding" {
      let encoding = std::str::from_utf8(value).ok()?;
      if crate::chars::is_enc_name(encoding) {
        return Some(encoding.to_owned());
      }
    }
    decl = remaining;
  }
}

/// Reads one pseudo-attribute from the start of `bytes` (ignoring any leading whitespace), and return its name, the
/// value excluding the quotation marks, and the byte sequence following it.
///
/// Only a well-formed `name = "value"` (using either tye of quotation mark) can be read. If the string consists of just
/// a name, or if `=` is follwed by a value that is not closed in quotation marks, it is skipped. In this case, an
/// *empty name* is returned. Returns `None` when the end is detected.
///
fn parse_pseudo_attribute(bytes: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
  let start = skip_whitespace(bytes);
  if start.is_empty() {
    return None;
  }
  let name_end = start.iter().position(|&b| b == b'=' || b.is_ascii_whitespace()).unwrap_or(start.len());
  let name = &start[..name_end];
  let after_name = skip_whitespace(&start[name_end..]);
  // A bare name with no `=` is not a `name="value"`: hide it behind an empty name and resume after it, so a later,
  // well-formed attribute is still reached.
  let Some(after_eq) = after_name.strip_prefix(b"=") else {
    return Some((b"", b"", after_name));
  };
  let open = skip_whitespace(after_eq);
  match open.first() {
    // A quoted value: return what is inside and resume after the closing quote. Left unclosed, the
    // rest of the declaration sits inside a string that never ends, so there is nothing to resume.
    Some(&quote) if quote == b'"' || quote == b'\'' => {
      let body = &open[1..];
      let end = body.iter().position(|&b| b == quote)?;
      Some((name, &body[..end], &body[end + 1..]))
    }
    // An `=` with an unquoted value: not well-formed. Hide it behind an empty name too, and resume
    // after the `=`.
    _ => Some((b"", b"", open)),
  }
}

fn skip_whitespace(bytes: &[u8]) -> &[u8] {
  let n = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
  &bytes[n..]
}

fn find(bytes: &[u8], pattern: &[u8]) -> Option<usize> {
  bytes.windows(pattern.len()).position(|w| w == pattern)
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
  fn parse_declared_encoding_reads_the_pseudo_attribute() {
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0' encoding='Shift_JIS'?>"), Some("Shift_JIS".to_owned()));
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0' encoding=\"Shift_JIS\"?>"), Some("Shift_JIS".to_owned()));
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0' encoding = 'Shift_JIS'?>"), Some("Shift_JIS".to_owned()));
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0'?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xmlfoo encoding='Shift_JIS'?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml fooencoding='Shift_JIS'?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml disabled encoding='Shift_JIS'?>"), Some("Shift_JIS".to_owned()));
    assert_eq!(parse_declared_encoding(b"<?xml encoding?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml encoding=?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml encoding=Shift_JIS?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml encoding='Shift_JIS?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml encoding='Shift_JIS?>'"), None);
    assert_eq!(parse_declared_encoding(b"<doc encoding='x'/>"), None);
    // `encoding` must match a whole pseudo-attribute name, not the tail of another.
    assert_eq!(parse_declared_encoding(b"<?xml version='1.0' fooencoding='x'?>"), None);
    assert_eq!(parse_declared_encoding(b"<?xml notencoding='x' encoding='EUC-JP'?>"), Some("EUC-JP".to_owned()));
    // A value that happens to contain the word is not mistaken for the attribute either.
    assert_eq!(parse_declared_encoding(b"<?xml version='encoding=utf-7'?>"), None);
  }

  #[test]
  fn a_pseudo_attribute_yields_its_name_value_and_remainder() {
    let (name, value, rest) = parse_pseudo_attribute(b"encoding='UTF-8' standalone='yes'").unwrap();
    assert_eq!((name, value, rest), (&b"encoding"[..], &b"UTF-8"[..], &b" standalone='yes'"[..]));
    // Leading whitespace is ignored, and whitespace around `=`, and the other quote, are accepted.
    let (name, value, rest) = parse_pseudo_attribute(b"  version = \"1.0\"").unwrap();
    assert_eq!((name, value, rest), (&b"version"[..], &b"1.0"[..], &b""[..]));
  }

  #[test]
  fn a_malformed_pseudo_attribute_is_hidden_behind_an_empty_name() {
    // Empty or all-whitespace input ends the walk.
    assert!(parse_pseudo_attribute(b"").is_none());
    assert!(parse_pseudo_attribute(b"   ").is_none());

    // A bare name: an empty name so it cannot match `encoding`, resuming after the name so the
    // next attribute is still read.
    let (name, value, rest) = parse_pseudo_attribute(b"standalone version='1.0'").unwrap();
    assert_eq!((name, value, rest), (&b""[..], &b""[..], &b"version='1.0'"[..]));

    // An `=` with an unquoted value: likewise an empty name, resuming after the `=`.
    let (name, value, rest) = parse_pseudo_attribute(b"encoding=utf-8").unwrap();
    assert_eq!((name, value, rest), (&b""[..], &b""[..], &b"utf-8"[..]));

    // A quoted value left unclosed ends the walk: the rest is inside a string that never closes.
    assert!(parse_pseudo_attribute(b"encoding='utf-8").is_none());
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
