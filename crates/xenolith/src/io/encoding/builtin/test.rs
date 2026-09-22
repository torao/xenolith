use super::*;
use crate::error::Error;

fn decode(decoder: &mut dyn Decoder, src: &[u8], last: bool) -> Result<(String, usize)> {
  let mut out = String::new();
  let n = decoder.decode(src, &mut out, last)?;
  Ok((out, n))
}

#[test]
fn utf8_split_sequence_is_carried_over() {
  let mut d = Utf8Decoder::new();
  let bytes = "あa".as_bytes(); // E3 81 82 61
  let (text, n) = decode(&mut d, &bytes[..2], false).unwrap();
  assert_eq!((text.as_str(), n), ("", 0));
  let (text, n) = decode(&mut d, bytes, true).unwrap();
  assert_eq!((text.as_str(), n), ("あa", 4));
}

#[test]
fn utf8_truncated_at_eof_is_fatal() {
  let mut d = Utf8Decoder::new();
  let err = decode(&mut d, &"あ".as_bytes()[..2], true).unwrap_err();
  assert!(matches!(err, Error::Encoding { .. }));
}

#[test]
fn utf8_invalid_sequence_is_fatal_even_mid_stream() {
  let mut d = Utf8Decoder::new();
  let mut out = String::from("<");
  let err = d.decode(&[0x41, 0xC0, 0x41], &mut out, false).unwrap_err();
  // The good prefix is kept, appended after whatever `dst` already held, and the byte at
  // fault is reported relative to the slice just handed over.
  assert_eq!(out, "<A");
  assert!(matches!(err, Error::Encoding { byte_offset: Some(1), .. }));
}

#[test]
fn utf16_decodes_both_byte_orders_and_surrogate_pairs() {
  // U+1F600, as a surrogate pair.
  let mut le = Utf16Decoder::little_endian();
  let (text, n) = decode(&mut le, &[0x3D, 0xD8, 0x00, 0xDE], true).unwrap();
  assert_eq!((text.as_str(), n), ("\u{1F600}", 4));

  let mut be = Utf16Decoder::big_endian();
  let (text, _) = decode(&mut be, &[0x00, 0x41, 0xD8, 0x3D, 0xDE, 0x00], true).unwrap();
  assert_eq!(text, "A\u{1F600}");
}

#[test]
fn utf16_holds_back_a_split_pair() {
  let mut d = Utf16Decoder::little_endian();
  let (text, n) = decode(&mut d, &[0x3D, 0xD8], false).unwrap();
  assert_eq!((text.as_str(), n), ("", 0));
  // An odd trailing byte is also held back.
  let (_, n) = decode(&mut d, &[0x41, 0x00, 0x3D], false).unwrap();
  assert_eq!(n, 2);
}

#[test]
fn utf16_rejects_unpaired_surrogates() {
  let mut d = Utf16Decoder::little_endian();
  assert!(decode(&mut d, &[0x00, 0xDC], true).is_err(), "lone low surrogate");
  let mut d = Utf16Decoder::little_endian();
  assert!(decode(&mut d, &[0x3D, 0xD8, 0x41, 0x00], true).is_err(), "high, then non-low");
  let mut d = Utf16Decoder::little_endian();
  assert!(decode(&mut d, &[0x3D, 0xD8], true).is_err(), "truncated pair at eof");
}

#[test]
fn ascii_rejects_the_high_bit_and_latin1_does_not() {
  let mut a = AsciiDecoder::new();
  let mut out = String::new();
  let err = a.decode(&[0x61, 0x62, 0xE9], &mut out, true).unwrap_err();
  assert_eq!(out, "ab");
  assert!(matches!(err, Error::Encoding { byte_offset: Some(2), .. }));
  let mut l = Latin1Decoder::new();
  assert_eq!(decode(&mut l, &[0xE9], true).unwrap().0, "é");
}

#[test]
fn utf16_keeps_what_it_decoded_before_a_bad_unit() {
  // "A" then a lone low surrogate: the "A" survives, and the fault is at byte 2.
  let mut d = Utf16Decoder::little_endian();
  let mut out = String::new();
  let err = d.decode(&[0x41, 0x00, 0x00, 0xDC], &mut out, true).unwrap_err();
  assert_eq!(out, "A");
  assert!(matches!(err, Error::Encoding { byte_offset: Some(2), .. }));
}

#[test]
fn utf16_bom_reads_the_byte_order_from_the_mark_and_drops_it() {
  let mut le = Utf16Decoder::from_mark();
  assert_eq!(le.encoding(), "UTF-16"); // undecided until it sees the mark
  let (text, n) = decode(&mut le, &[0xFF, 0xFE, 0x41, 0x00], true).unwrap();
  assert_eq!((text.as_str(), n), ("A", 4)); // the mark is consumed, not emitted
  assert_eq!(le.encoding(), "UTF-16LE"); // and the order is settled

  let mut be = Utf16Decoder::from_mark();
  let (text, _) = decode(&mut be, &[0xFE, 0xFF, 0x00, 0x41], true).unwrap();
  assert_eq!((text, be.encoding()), ("A".to_owned(), "UTF-16BE"));
}

#[test]
fn utf16_bom_defaults_to_little_endian_without_a_mark() {
  let mut d = Utf16Decoder::from_mark();
  let (text, n) = decode(&mut d, &[0x41, 0x00, 0x42, 0x00], true).unwrap();
  assert_eq!((text.as_str(), n), ("AB", 4));
}

#[test]
fn utf16_bom_waits_for_the_second_byte_of_the_mark() {
  let mut d = Utf16Decoder::from_mark();
  let mut out = String::new();
  assert_eq!(d.decode(&[0xFF], &mut out, false).unwrap(), 0); // one byte cannot settle the order
  // The caller carries the byte over and feeds it again with the rest.
  assert_eq!(d.decode(&[0xFF, 0xFE, 0x41, 0x00], &mut out, true).unwrap(), 4);
  assert_eq!(out, "A");
}

#[test]
fn utf16_bom_reports_a_bad_unit_relative_to_the_whole_slice() {
  // A little-endian mark, then a lone low surrogate: the fault is byte 2, past the mark.
  let mut d = Utf16Decoder::from_mark();
  let mut out = String::new();
  let err = d.decode(&[0xFF, 0xFE, 0x00, 0xDC], &mut out, true).unwrap_err();
  assert!(matches!(err, Error::Encoding { byte_offset: Some(2), .. }));
}

#[test]
fn utf16_bom_rejects_a_lone_byte_at_end_of_input() {
  let mut d = Utf16Decoder::from_mark();
  assert!(decode(&mut d, &[0x41], true).is_err());
}
