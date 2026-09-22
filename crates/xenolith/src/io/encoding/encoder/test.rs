use super::*;

fn encoded(encoder: &mut dyn Encoder, text: &str) -> Vec<u8> {
  let mut out = Vec::new();
  encoder.encode(text, &mut out).unwrap();
  out
}

fn with_references(encoder: &mut dyn Encoder, text: &str) -> String {
  let mut out = Vec::new();
  encoder.encode_with_references(text, &mut out).unwrap();
  String::from_utf8(out).unwrap()
}

#[test]
fn utf8_holds_everything() {
  assert_eq!(encoded(&mut Utf8Encoder::new(), "日本 & café"), "日本 & café".as_bytes());
  assert_eq!(with_references(&mut Utf8Encoder::new(), "日本"), "日本");
}

#[test]
fn a_single_byte_encoding_stops_where_it_stops() {
  assert_eq!(encoded(&mut SingleByteEncoder::ascii(), "abc"), b"abc");
  assert_eq!(encoded(&mut SingleByteEncoder::latin1(), "café"), &[0x63, 0x61, 0x66, 0xE9]);
  // `é` is beyond US-ASCII but within ISO-8859-1.
  assert!(SingleByteEncoder::ascii().encode("café", &mut Vec::new()).is_err());
}

#[test]
fn what_cannot_be_held_becomes_a_reference() {
  assert_eq!(with_references(&mut SingleByteEncoder::ascii(), "caf\u{e9}"), "caf&#233;");
  assert_eq!(with_references(&mut SingleByteEncoder::latin1(), "\u{20ac}"), "&#8364;");
}

#[test]
fn a_refusal_leaves_the_output_as_it_was() {
  // The whole run is checked before any of it is written, so a caller can report the failure and keep going with
  // what it had.
  let mut out = b"<a>".to_vec();
  let error = SingleByteEncoder::ascii().encode("ok\u{e9}", &mut out).unwrap_err();
  assert!(error.to_string().contains("U+00E9"), "{error}");
  assert_eq!(out, b"<a>", "nothing of the refused run was appended");
}
