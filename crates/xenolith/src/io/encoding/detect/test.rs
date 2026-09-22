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
