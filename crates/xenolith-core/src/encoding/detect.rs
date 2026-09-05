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

/// A fixed byte pattern placed at the beginning of an entity that determines or implies its encoding.
struct Signature {
  /// The leading bytes to match.
  bytes: &'static [u8],
  /// The encoding those bytes indicate.
  encoding: &'static str,
  /// The number of leading bytes that form a byte-order mark and must be skipped before decoding.
  bom_length: usize,
  /// Whether the pattern is a byte-order mark or the raw bytes of `<?xml`.
  source: DetectionSource,
}

/// The byte-order marks (U+FEFF) and the raw `<?xml` byte patterns, in match order.
///
/// The UTF-32 marks must be matched before the UTF-16LE mark, which may serve as its prefix. The `<?xml` pattern
/// conforms to XML 1.0 §2.8. The declaration begins with `<` (U+003C), followed by `?` (U+003F). The width and
/// endianness can be determined from this byte sequence.
///
/// The last pattern is EBCDIC, which the declaration alone cannot narrow it down to a specific code page.
///
const SIGNATURES: &[Signature] = &[
  Signature { bytes: &[0x00, 0x00, 0xFE, 0xFF], encoding: "UTF-32BE", bom_length: 4, source: DetectionSource::Bom },
  Signature { bytes: &[0xFF, 0xFE, 0x00, 0x00], encoding: "UTF-32LE", bom_length: 4, source: DetectionSource::Bom },
  Signature { bytes: &[0xEF, 0xBB, 0xBF], encoding: "UTF-8", bom_length: 3, source: DetectionSource::Bom },
  Signature { bytes: &[0xFE, 0xFF], encoding: "UTF-16BE", bom_length: 2, source: DetectionSource::Bom },
  Signature { bytes: &[0xFF, 0xFE], encoding: "UTF-16LE", bom_length: 2, source: DetectionSource::Bom },
  Signature {
    bytes: &[0x00, 0x00, 0x00, 0x3C],
    encoding: "UTF-32BE",
    bom_length: 0,
    source: DetectionSource::InitialBytes,
  },
  Signature {
    bytes: &[0x3C, 0x00, 0x00, 0x00],
    encoding: "UTF-32LE",
    bom_length: 0,
    source: DetectionSource::InitialBytes,
  },
  Signature {
    bytes: &[0x00, 0x3C, 0x00, 0x3F],
    encoding: "UTF-16BE",
    bom_length: 0,
    source: DetectionSource::InitialBytes,
  },
  Signature {
    bytes: &[0x3C, 0x00, 0x3F, 0x00],
    encoding: "UTF-16LE",
    bom_length: 0,
    source: DetectionSource::InitialBytes,
  },
  Signature {
    bytes: &[0x4C, 0x6F, 0xA7, 0x94],
    encoding: "EBCDIC",
    bom_length: 0,
    source: DetectionSource::InitialBytes,
  },
];

/// The literal bytes that begin an XML or text declaration.
const XML_DECL_START: &[u8] = b"<?xml";

/// The result of [`detect`] on a beginning, possibly incomplete, part of an entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detected {
  /// The encoding is determined. This includes a declaration that does not specify an encoding name, and an entity for
  /// which no declaration exists at all; in either case, the encoding is the default UTF-8.
  Determined(Detection),
  /// The bytes seen so far are either a prefix common to more than one encoding, or a declaration whose `encoding`
  /// pseudo-attribute has not yet been fully received. More bytes are needed to decide.
  Incomplete,
}

impl Detected {
  /// Determine the encoding based on the detection result. The caller can use this to determine the default encoding
  /// UTF-8 from an unconfirmed detection result when there are no more bytes to supply.
  #[must_use]
  pub fn or_default(self) -> Detection {
    match self {
      Detected::Determined(detection) => detection,
      Detected::Incomplete => Detection::new("UTF-8", 0, DetectionSource::Default),
    }
  }
}

/// Determines the encoding based on the leading bytes read from the entity.
///
/// This method returns [`Detected::Determined`] when the encoding can be determined from the given `bytes`. If
/// additional bytes are required to determine the encoding, it returns [`Detected::Incomplete`]. In this case, the
/// caller should read additional bytes, concatenate them, and call this again.
///
/// This allows the caller to terminate the determination process as soon as the encoding is identified from the bytes
/// that have been read in fragments. This feature is important in scenarios where entities are sent interactively and
/// the receiver does not send any further data until a response to the declaration is confirmed.
///
/// The encoding that was detected may not be supported by [`super::decoder_for`].
///
/// # Examples
///
/// ```
/// use xenolith_core::encoding::{Detected, DetectionSource, detect};
///
/// // A byte-order mark is recognized at once; its bytes must be skipped before decoding.
/// let d = detect(b"\xEF\xBB\xBF<doc/>").or_default();
/// assert_eq!((d.encoding.as_str(), d.bom_length), ("UTF-8", 3));
///
/// // A declaration split before its `encoding` value is not yet decided.
/// assert_eq!(detect(b"<?xml version='1.0' enc"), Detected::Incomplete);
///
/// // Once the value is complete, the encoding is known even before `?>` arrives.
/// let Detected::Determined(d) = detect(b"<?xml version='1.0' encoding='EUC-JP'") else {
///   panic!("the encoding is fully present");
/// };
/// assert_eq!(d.encoding, "EUC-JP");
///
/// // With no declaration, and no more bytes to come, UTF-8 is assumed.
/// assert_eq!(detect(b"<doc/>").or_default().source, DetectionSource::Default);
/// ```
#[must_use]
pub fn detect(bytes: &[u8]) -> Detected {
  if bytes.is_empty() {
    return Detected::Incomplete;
  }
  match match_signature_incremental(bytes) {
    SignatureMatch::Determined(detection) => Detected::Determined(detection),
    SignatureMatch::Incomplete => Detected::Incomplete,
    SignatureMatch::NotASignature => detect_declaration_incremental(bytes),
  }
}

/// How `bytes` stand against the byte-order-mark and `<?xml` [`SIGNATURES`].
enum SignatureMatch {
  /// A signature matched and no longer signature can still extend it.
  Determined(Detection),
  /// A signature matched but a longer one could still match, or `bytes` is a prefix of one.
  Incomplete,
  /// No signature matches `bytes` or begins with it; the ASCII declaration path applies.
  NotASignature,
}

fn match_signature_incremental(bytes: &[u8]) -> SignatureMatch {
  // One pass collects both facts needed: the first signature `bytes` fully contains, and whether `bytes` is still the
  // strict prefix of some (necessarily longer) signature, so a longer one could yet match. The UTF-16LE mark FF FE is
  // the prefix of the UTF-32LE mark FF FE 00 00, so FF FE alone is ambiguous.
  let mut matched: Option<&Signature> = None;
  let mut could_extend = false;
  for signature in SIGNATURES {
    if matched.is_none() && bytes.starts_with(signature.bytes) {
      matched = Some(signature);
    }
    if signature.bytes.len() > bytes.len() && signature.bytes.starts_with(bytes) {
      could_extend = true;
    }
  }
  match matched {
    Some(signature) if !could_extend => {
      SignatureMatch::Determined(Detection::new(signature.encoding, signature.bom_length, signature.source))
    }
    Some(_) => SignatureMatch::Incomplete,
    None if could_extend => SignatureMatch::Incomplete,
    None => SignatureMatch::NotASignature,
  }
}

/// Reads an ASCII-compatible declaration until the encoding name can be identified, or until `?>` is reached without
/// the encoding name being identified, or until it becomes clear that more bytes are required.
fn detect_declaration_incremental(bytes: &[u8]) -> Detected {
  let rest = skip_whitespace(bytes);
  if rest.len() < XML_DECL_START.len() {
    // The leading `<?xml` may still be arriving; anything else means there is no declaration.
    return if XML_DECL_START.starts_with(rest) { Detected::Incomplete } else { default_encoding() };
  }
  let Some(rest) = rest.strip_prefix(XML_DECL_START) else {
    return default_encoding();
  };
  match rest.first() {
    None => return Detected::Incomplete, // exactly `<?xml`; the byte after it decides
    Some(byte) if byte.is_ascii_whitespace() => {}
    // `<?xml` not followed by whitespace, such as `<?xml-stylesheet`, is a processing instruction.
    Some(_) => return default_encoding(),
  }
  match find(rest, b"?>") {
    // The declaration is complete: its `encoding`, or the default when it gives none.
    Some(end) => match scan_for_encoding(&rest[..end]) {
      Some(encoding) => Detected::Determined(Detection::new(&encoding, 0, DetectionSource::Declaration)),
      None => default_encoding(),
    },
    // No `?>` yet: decided only if the `encoding` value has already arrived in full.
    None => match scan_for_encoding(rest) {
      Some(encoding) => Detected::Determined(Detection::new(&encoding, 0, DetectionSource::Declaration)),
      None => Detected::Incomplete,
    },
  }
}

fn default_encoding() -> Detected {
  Detected::Determined(Detection::new("UTF-8", 0, DetectionSource::Default))
}

/// Returns the value of the `encoding` pseudo-attribute located within the `decl`, which the section of declaration
/// between `<?xml` and `?>`, if its value represents a valid `EncName`.
fn scan_for_encoding(mut decl: &[u8]) -> Option<String> {
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
/// a name, or if `=` is followed by a value that is not closed in quotation marks, it is skipped. In this case, an
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

  /// Asserts the encoding is determined from the whole of `bytes` and returns it.
  fn determined(bytes: &[u8]) -> Detection {
    match detect(bytes) {
      Detected::Determined(detection) => detection,
      Detected::Incomplete => panic!("expected a determined encoding for {bytes:?}"),
    }
  }

  #[test]
  fn boms_win_over_everything() {
    let d = determined(b"\xEF\xBB\xBF<?xml version='1.0' encoding='Shift_JIS'?>");
    assert_eq!((d.encoding.as_str(), d.bom_length, d.source), ("UTF-8", 3, DetectionSource::Bom));

    assert_eq!(determined(b"\xFE\xFF\x00<").encoding, "UTF-16BE");
    assert_eq!(determined(b"\xFF\xFE<\x00").encoding, "UTF-16LE");
    assert_eq!(determined(b"\xFF\xFE\x00\x00").encoding, "UTF-32LE");
    assert_eq!(determined(b"\x00\x00\xFE\xFF").encoding, "UTF-32BE");
  }

  #[test]
  fn utf16_is_recognised_without_a_bom() {
    let d = determined(b"\x00<\x00?\x00x\x00m\x00l");
    assert_eq!((d.encoding.as_str(), d.bom_length, d.source), ("UTF-16BE", 0, DetectionSource::InitialBytes));
    assert_eq!(determined(b"<\x00?\x00x\x00m\x00").encoding, "UTF-16LE");
  }

  #[test]
  fn the_encoding_pseudo_attribute_is_read_from_the_declaration() {
    // The value is read whichever quote is used, and whitespace may surround `=`.
    assert_eq!(determined(b"<?xml version='1.0' encoding='Shift_JIS'?>").encoding, "Shift_JIS");
    assert_eq!(determined(b"<?xml version='1.0' encoding=\"Shift_JIS\"?>").encoding, "Shift_JIS");
    assert_eq!(determined(b"<?xml version='1.0' encoding = 'Shift_JIS'?>").encoding, "Shift_JIS");

    // A malformed pseudo-attribute is skipped, so a later well-formed `encoding` is still found.
    assert_eq!(determined(b"<?xml disabled encoding='Shift_JIS'?>").encoding, "Shift_JIS");
    assert_eq!(determined(b"<?xml notencoding='x' encoding='EUC-JP'?>").encoding, "EUC-JP");

    // `encoding` must match a whole pseudo-attribute, not the tail of another or a value spelling it.
    assert_eq!(determined(b"<?xml version='1.0' fooencoding='x'?>").source, DetectionSource::Default);
    assert_eq!(determined(b"<?xml version='encoding=utf-7'?>").source, DetectionSource::Default);

    // A complete declaration that gives no usable encoding is the default.
    for decl in [
      &b"<?xml version='1.0'?>"[..],
      b"<?xml?>", // `<?xml` with no following whitespace is not a declaration
      b"<?xmlfoo encoding='Shift_JIS'?>",
      b"<?xml fooencoding='Shift_JIS'?>",
      b"<?xml encoding?>",            // a bare name with no value
      b"<?xml encoding=?>",           // `=` with no value
      b"<?xml encoding=Shift_JIS?>",  // an unquoted value
      b"<?xml encoding='Shift_JIS?>", // a value whose quote never closes
      b"<?xml encoding='Shift_JIS?>'",
      b"<doc encoding='x'/>", // not a declaration at all
    ] {
      assert_eq!(determined(decl).source, DetectionSource::Default, "{decl:?}");
    }

    // A closed quote with no `?>` yet is already decided; an open one is still arriving.
    assert_eq!(determined(b"<?xml encoding='Shift_JIS'").encoding, "Shift_JIS");
    assert_eq!(detect(b"<?xml encoding='Shift_JIS"), Detected::Incomplete);
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
    let d = determined(b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><doc/>");
    assert_eq!((d.encoding.as_str(), d.source), ("Shift_JIS", DetectionSource::Declaration));
    assert_eq!(determined(b"<?xml version='1.0' encoding = 'euc-jp' ?>").encoding, "euc-jp");
  }

  #[test]
  fn defaults_to_utf8() {
    assert_eq!(determined(b"<doc/>").source, DetectionSource::Default);
    assert_eq!(determined(b"<?xml version='1.0'?>").encoding, "UTF-8");
    // No bytes yet is undecided, but an ended empty entity settles on the default.
    assert_eq!(detect(b""), Detected::Incomplete);
    assert_eq!(detect(b"").or_default().encoding, "UTF-8");
  }

  #[test]
  fn detection_waits_until_the_encoding_can_be_known() {
    // A byte-order mark split across feeds: FF FE could still grow into the UTF-32LE mark FF FE 00 00.
    assert_eq!(detect(b"\xFF\xFE"), Detected::Incomplete);
    assert!(matches!(detect(b"\xFF\xFE\x00\x41"), Detected::Determined(d) if d.encoding == "UTF-16LE"));
    assert!(matches!(detect(b"\xFF\xFE\x00\x00"), Detected::Determined(d) if d.encoding == "UTF-32LE"));

    // A declaration is undecided until either its `encoding` value or its `?>` has arrived.
    assert_eq!(detect(b"<?xml version='1.0' encoding='EUC"), Detected::Incomplete);
    let full = detect(b"<?xml version='1.0' encoding='EUC-JP'");
    assert!(
      matches!(full, Detected::Determined(d) if d.source == DetectionSource::Declaration && d.encoding == "EUC-JP")
    );

    // A prefix of `<?xml`, and no bytes at all, are not yet decided.
    assert_eq!(detect(b"<?xm"), Detected::Incomplete);
    assert_eq!(detect(b"<?"), Detected::Incomplete);
    assert_eq!(detect(b""), Detected::Incomplete);

    // `<?xml` not followed by whitespace is a processing instruction, so the default applies.
    assert!(matches!(detect(b"<?xml-stylesheet "), Detected::Determined(d) if d.source == DetectionSource::Default));
  }

  #[test]
  fn a_complete_but_malformed_declaration_falls_back_to_the_default() {
    for decl in [
      &b"<?xmlversion='1.0' encoding='x'?>"[..], // `<?xml` not followed by whitespace: a PI, not a declaration
      b"<doc encoding='x'/>",                    // not a declaration at all
      b"<?xml version='1.0'?><a encoding='x'/>", // the attribute is past the declaration's `?>`
      b"<?xml encoding=utf-8?>",                 // an unquoted value
      b"<?xml encoding='utf-8?>",                // a value whose quote never closes
      b"<?xml encoding='8859-1'?>",              // an EncName may not start with a digit
    ] {
      assert_eq!(determined(decl).source, DetectionSource::Default, "{decl:?}");
    }
  }
}
