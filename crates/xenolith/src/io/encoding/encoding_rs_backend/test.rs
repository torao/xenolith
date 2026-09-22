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
