//! Writing in an encoding other than UTF-8, and declaring it under whatever name the reader knows.

use xenolith::Error;
use xenolith::event::EventSource;
use xenolith::io::write::{WriterSource, XmlWriter};

/// Writes `build` through a writer configured by `setup`, returning the raw bytes.
fn bytes(
  setup: impl FnOnce(XmlWriter<Vec<u8>>) -> XmlWriter<Vec<u8>>,
  build: impl FnOnce(&mut WriterSource<'_>) -> Result<(), Error>,
) -> Vec<u8> {
  let mut w = setup(XmlWriter::new(Vec::new()));
  {
    let mut doc = WriterSource::new().with_handler(&mut w);
    build(&mut doc).expect("written");
    doc.end_document().expect("written");
  }
  w.into_inner()
}

/// The error of writing `build` through a writer that encodes as US-ASCII.
fn ascii_error(build: impl FnOnce(&mut WriterSource<'_>) -> Result<(), Error>) -> Error {
  let mut w = XmlWriter::new(Vec::new()).with_encoding("US-ASCII").expect("a built-in encoding");
  let mut doc = WriterSource::new().with_handler(&mut w);
  build(&mut doc).expect_err("the encoding cannot hold it")
}

#[test]
fn the_declaration_gives_the_encoding_the_bytes_are_in() {
  let out = bytes(
    |w| w.with_encoding("ISO-8859-1").expect("a built-in encoding").with_declaration(None),
    |doc| {
      doc.write_start_element("a")?;
      doc.write_characters("caf\u{e9}")?;
      doc.write_end_element()
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
    |w| {
      w.with_encoding("ISO-8859-1")
        .expect("a built-in encoding")
        .with_declared_encoding("latin1")
        .with_declaration(None)
    },
    |doc| doc.start_document(),
  );
  assert_eq!(String::from_utf8(out).unwrap(), "<?xml version=\"1.0\" encoding=\"latin1\"?>");
}

#[test]
fn text_and_attribute_values_fall_back_to_a_character_reference() {
  // A reference is markup here, and it reads back as the character it stands for.
  let out = bytes(
    |w| w.with_encoding("US-ASCII").expect("a built-in encoding"),
    |doc| {
      doc.write_start_element("a")?;
      doc.write_attribute("x", "caf\u{e9}")?;
      doc.write_characters("\u{65e5}\u{672c}")?;
      doc.write_end_element()
    },
  );
  assert_eq!(String::from_utf8(out).unwrap(), "<a x=\"caf&#233;\">&#26085;&#26412;</a>");
}

#[test]
fn a_name_the_encoding_cannot_hold_is_refused() {
  // No reference is markup in a name, so the write is refused rather than written as something else. The start tag is
  // held until its attributes are in, so the refusal arrives with the event that ends it.
  let error = ascii_error(|doc| {
    doc.write_start_element("\u{8981}\u{7d20}")?;
    doc.write_characters("x")
  });
  assert!(matches!(error, Error::Encoding { .. }), "{error}");
  assert!(error.to_string().contains("U+8981"), "the offending character is reported: {error}");
}

#[test]
fn an_attribute_the_encoding_cannot_hold_is_refused_with_its_element() {
  // An attribute name goes out with the start tag, so it is refused where that tag is written.
  let error = ascii_error(|doc| {
    doc.write_start_element("a")?;
    doc.write_attribute("\u{8981}", "1")?;
    doc.write_end_element()
  });
  assert!(matches!(error, Error::Encoding { .. }), "{error}");
}

#[test]
fn a_comment_a_pi_and_a_cdata_section_are_refused_too() {
  // None of the three admits a character reference either.
  for build in [0, 1, 2] {
    let error = ascii_error(|doc| {
      doc.write_start_element("a")?;
      match build {
        0 => doc.write_comment("\u{e9}"),
        1 => doc.write_processing_instruction("pi", "\u{e9}"),
        _ => doc.write_cdata("\u{e9}"),
      }
    });
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
    |w| {
      w.with_encoding("windows-31j")
        .expect("through encoding_rs")
        .with_declared_encoding("Shift_JIS")
        .with_declaration(None)
    },
    |doc| {
      doc.write_start_element("a")?;
      doc.write_characters("\u{65e5}\u{672c}")?;
      doc.write_end_element()
    },
  );

  let head = b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><a>";
  assert!(out.starts_with(head), "the declared name is the one asked for");
  // The text is Shift_JIS bytes, not UTF-8 and not a reference.
  assert_eq!(&out[head.len()..head.len() + 4], &[0x93, 0xFA, 0x96, 0x7B]);
}
