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
