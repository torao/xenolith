//! Writing in an encoding other than UTF-8, and declaring it under whatever name the reader knows.

use xenolith_core::Error;
use xenolith_serialize::XmlWriter;

/// Writes `build` through a writer configured by `setup`, returning the raw bytes.
fn bytes(
  setup: impl FnOnce(XmlWriter<Vec<u8>>) -> XmlWriter<Vec<u8>>,
  build: impl FnOnce(&mut XmlWriter<Vec<u8>>) -> Result<(), Error>,
) -> Vec<u8> {
  let mut w = setup(XmlWriter::new(Vec::new()));
  build(&mut w).expect("written");
  w.into_inner()
}

#[test]
fn the_declaration_gives_the_encoding_the_bytes_are_in() {
  let out = bytes(
    |w| w.with_encoding("ISO-8859-1").expect("a built-in encoding"),
    |w| {
      w.write_declaration(None)?;
      w.write_start_element("a")?;
      w.write_characters("caf\u{e9}")?;
      w.write_end_element()
    },
  );

  // `é` is one byte in ISO-8859-1, so the text is not UTF-8 and the declaration says as much.
  assert!(out.starts_with(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>"));
  assert!(out.ends_with(&[b'<', b'a', b'>', 0x63, 0x61, 0x66, 0xE9, b'<', b'/', b'a', b'>']));
}

#[test]
fn the_declared_name_can_differ_from_the_encoding_written() {
  // The bytes are what the encoder produces; the name is what the reader is expected to know it by.
  let out = bytes(
    |w| w.with_encoding("ISO-8859-1").expect("a built-in encoding").with_declared_encoding("latin1"),
    |w| w.write_declaration(None),
  );
  assert_eq!(String::from_utf8(out).unwrap(), "<?xml version=\"1.0\" encoding=\"latin1\"?>");
}

#[test]
fn text_and_attribute_values_fall_back_to_a_character_reference() {
  // A reference is markup here, and it reads back as the character it stands for.
  let out = bytes(
    |w| w.with_encoding("US-ASCII").expect("a built-in encoding"),
    |w| {
      w.write_start_element("a")?;
      w.write_attribute("x", "caf\u{e9}")?;
      w.write_characters("\u{65e5}\u{672c}")?;
      w.write_end_element()
    },
  );
  assert_eq!(String::from_utf8(out).unwrap(), "<a x=\"caf&#233;\">&#26085;&#26412;</a>");
}

#[test]
fn a_name_the_encoding_cannot_hold_is_refused() {
  // No reference is markup in a name, so the write is refused rather than written as something else.
  let mut w = XmlWriter::new(Vec::new()).with_encoding("US-ASCII").expect("a built-in encoding");
  let error = w.write_start_element("\u{8981}\u{7d20}").unwrap_err();
  assert!(matches!(error, Error::Encoding { .. }), "{error}");
  assert!(error.to_string().contains("U+8981"), "the offending character is reported: {error}");
}

#[test]
fn a_comment_a_pi_and_a_cdata_section_are_refused_too() {
  // None of the three admits a character reference either.
  for build in [0, 1, 2] {
    let mut w = XmlWriter::new(Vec::new()).with_encoding("US-ASCII").expect("a built-in encoding");
    w.write_start_element("a").expect("ASCII");
    let error = match build {
      0 => w.write_comment("\u{e9}").unwrap_err(),
      1 => w.write_processing_instruction("pi", "\u{e9}").unwrap_err(),
      _ => w.write_cdata("\u{e9}").unwrap_err(),
    };
    assert!(matches!(error, Error::Encoding { .. }), "{error}");
  }
}

#[test]
fn utf16_can_be_read_but_not_written() {
  // The Encoding Standard defines no UTF-16 encoder, and the error says what to do instead.
  let error = XmlWriter::new(Vec::new()).with_encoding("UTF-16").map(|_| ()).unwrap_err();
  assert!(matches!(error, Error::Encoding { .. }), "{error}");
  assert!(error.to_string().contains("UTF-8"), "{error}");
}

#[cfg(feature = "encodings")]
#[test]
fn a_legacy_japanese_encoding_writes_under_the_name_a_reader_knows() {
  // The case this was built for: the bytes are windows-31j, and the declaration says Shift_JIS.
  let out = bytes(
    |w| w.with_encoding("windows-31j").expect("through encoding_rs").with_declared_encoding("Shift_JIS"),
    |w| {
      w.write_declaration(None)?;
      w.write_start_element("a")?;
      w.write_characters("\u{65e5}\u{672c}")?;
      w.write_end_element()
    },
  );

  let head = b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><a>";
  assert!(out.starts_with(head), "the declared name is the one asked for");
  // The text is Shift_JIS bytes, not UTF-8 and not a reference.
  assert_eq!(&out[head.len()..head.len() + 4], &[0x93, 0xFA, 0x96, 0x7B]);
}
