use super::*;

fn fed(bytes: &[u8]) -> CharStream {
  let mut stream = CharStream::new();
  stream.feed(bytes, true).expect("decodes");
  stream
}

#[test]
fn normalizes_all_three_line_ends() {
  assert_eq!(fed(b"a\r\nb\rc\nd").remainder(), "a\nb\nc\nd");
  // Two carriage returns are two line ends, not one.
  assert_eq!(fed(b"a\r\rb").remainder(), "a\n\nb");
}

#[test]
fn normalizes_a_cr_lf_split_across_feeds() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  stream.feed(b"a\r", false).unwrap();
  stream.feed(b"\nb", true).unwrap();
  assert_eq!(stream.remainder(), "a\nb");
}

#[test]
fn tracks_line_column_and_offset() {
  let mut stream = fed("a\nbc\nxyz".as_bytes());
  let at = stream.location();
  assert_eq!((at.line, at.column, at.offset), (1, 1, 0));

  stream.advance_chars(2); // "a\n"
  let at = stream.location();
  assert_eq!((at.line, at.column, at.offset), (2, 1, 2));

  stream.advance_chars(3); // "bc\n"
  let at = stream.location();
  assert_eq!((at.line, at.column, at.offset), (3, 1, 5));

  stream.advance_chars(2); // "xy"
  let at = stream.location();
  assert_eq!((at.line, at.column, at.offset), (3, 3, 7));
  assert_eq!(stream.remainder(), "z");
}

#[test]
fn nothing_is_consumed_until_advance() {
  let mut stream = fed(b"<doc/>");
  assert_eq!(stream.remainder(), "<doc/>");
  assert_eq!(stream.remainder(), "<doc/>", "peeking does not consume");
  stream.advance(1);
  assert_eq!(stream.remainder(), "doc/>");
}

#[test]
fn detects_the_encoding_from_the_bytes() {
  let mut stream = CharStream::new();
  stream.feed(b"\xEF\xBB\xBF<doc/>", true).unwrap();
  assert_eq!(stream.encoding(), Some("UTF-8"));
  assert_eq!(stream.remainder(), "<doc/>", "the byte-order mark is not part of the text");

  // Without a mark, UTF-16 is recognized only from the bytes of `<?xml`.
  let utf16: Vec<u8> = "<?xml version='1.0'?><d/>".encode_utf16().flat_map(u16::to_be_bytes).collect();
  let mut stream = CharStream::new();
  stream.feed(&utf16, true).unwrap();
  assert_eq!(stream.encoding(), Some("UTF-16BE"));
  assert!(stream.remainder().ends_with("<d/>"));
}

#[test]
fn utf16_without_a_mark_or_declaration_is_not_utf16() {
  // XML requires such an entity to carry a byte-order mark; read as UTF-8 it is full of
  // NUL, so it fails as a character error rather than being silently guessed.
  let utf16: Vec<u8> = "<d/>".encode_utf16().flat_map(u16::to_be_bytes).collect();
  let mut stream = CharStream::new();
  assert!(matches!(stream.feed(&utf16, true).unwrap_err(), Error::WellFormedness { .. }));
}

#[test]
fn declared_encoding_is_honoured() {
  let mut stream = CharStream::new();
  stream.feed("<?xml version='1.0' encoding='ISO-8859-1'?><a>é</a>".as_bytes(), true).unwrap();
  assert_eq!(stream.encoding(), Some("ISO-8859-1"));
  // The source was UTF-8 bytes read as Latin-1, so the accent decodes as two characters.
  assert!(stream.remainder().ends_with("</a>"));
}

#[test]
fn sniffing_waits_for_enough_bytes() {
  let mut stream = CharStream::new();
  stream.feed(b"<?xml ", false).unwrap();
  assert_eq!(stream.encoding(), None, "too early to decide");
  assert_eq!(stream.remainder(), "");
  stream.feed(b"version='1.0' encoding='UTF-8'?><a/>", true).unwrap();
  assert_eq!(stream.encoding(), Some("UTF-8"));
  assert!(stream.remainder().ends_with("<a/>"));
}

#[test]
fn rejects_characters_that_char_forbids() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  let err = stream.feed(b"<a>\x0c</a>", true).unwrap_err();
  assert!(matches!(err, Error::WellFormedness { .. }));
  assert_eq!(err.location().column, 4, "reports where the character is");
}

#[test]
fn reports_where_a_bad_character_is_after_earlier_lines() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  let err = stream.feed(b"<a>\n\n\x00</a>", true).unwrap_err();
  assert_eq!((err.location().line, err.location().column), (3, 1));
}

#[test]
fn an_entity_ending_mid_character_is_an_encoding_error() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  let err = stream.feed(&"あ".as_bytes()[..2], true).unwrap_err();
  assert!(matches!(err, Error::Encoding { .. }));
}

#[test]
fn feeding_after_the_end_is_a_bug() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  stream.feed(b"a", true).unwrap();
  assert!(matches!(stream.feed(b"b", true).unwrap_err(), Error::Internal { .. }));
}

#[test]
fn use_encoding_can_be_chosen_until_decoding_begins() {
  // 0xE9 is 'é' in ISO-8859-1 but not valid UTF-8, so the choice of encoding is visible.
  let mut stream = CharStream::new();
  stream.use_encoding("ISO-8859-1").unwrap();
  stream.feed(&[0xE9], true).unwrap();
  assert_eq!(stream.remainder(), "é");

  // Bytes fed while sniffing is still undecided (here, a partial declaration) are only buffered,
  // not decoded, so the encoding can still be chosen; the buffered bytes decode with it.
  let mut buffered = CharStream::new();
  buffered.feed(b"<?xml ", false).unwrap();
  buffered.use_encoding("ISO-8859-1").unwrap();
  buffered.feed(&[0xE9], true).unwrap();
  assert_eq!(buffered.remainder(), "<?xml é");

  // Once decoding has begun, the encoding can no longer be chosen. Feeding the last bytes ends
  // sniffing and decodes them.
  let mut decoding = CharStream::new();
  decoding.feed(b"x", true).unwrap();
  assert!(matches!(decoding.use_encoding("UTF-8").unwrap_err(), Error::Internal { .. }));
}

#[test]
fn use_encoding_leaves_a_byte_order_mark_to_the_decoder() {
  // Sniffing strips a leading BOM: detection reports how many bytes to skip before decoding.
  let mut sniffed = CharStream::new();
  sniffed.feed(b"\xEF\xBB\xBF<doc/>", true).unwrap();
  assert_eq!(sniffed.remainder(), "<doc/>");

  // use_encoding skips detection, so nothing strips the mark for it. Under UTF-8, whose decoder has
  // no notion of a mark, a leading BOM is decoded as content: U+FEFF.
  let mut utf8 = CharStream::new();
  utf8.use_encoding("UTF-8").unwrap();
  utf8.feed(b"\xEF\xBB\xBF<doc/>", true).unwrap();
  assert_eq!(utf8.remainder(), "\u{FEFF}<doc/>");
}

#[test]
fn use_encoding_utf16_reads_the_mark_only_when_the_order_is_open() {
  let content = b"\xFF\xFE<\x00d\x00o\x00c\x00/\x00>\x00"; // UTF-16LE BOM, then `<doc/>`

  // UTF-16 leaves the byte order to the mark, so its decoder reads and consumes the leading mark.
  let mut utf16 = CharStream::new();
  utf16.use_encoding("UTF-16").unwrap();
  utf16.feed(content, true).unwrap();
  assert_eq!(utf16.remainder(), "<doc/>");

  // UTF-16LE fixes the order, so there is no mark to read and the leading FF FE is content: U+FEFF.
  let mut utf16le = CharStream::new();
  utf16le.use_encoding("UTF-16LE").unwrap();
  utf16le.feed(content, true).unwrap();
  assert_eq!(utf16le.remainder(), "\u{FEFF}<doc/>");
}

#[test]
fn a_decoder_that_never_makes_progress_is_refused_not_buffered() {
  // A hostile or broken decoder that consumes nothing without erroring would otherwise let a stream
  // grow `pending` without bound, one fed byte at a time. The carry-over cap turns that into an error.
  #[derive(Debug)]
  struct StallingDecoder;
  impl Decoder for StallingDecoder {
    fn encoding(&self) -> &str {
      "x-stalling"
    }
    fn decode(&mut self, _src: &[u8], _dst: &mut String, _last: bool) -> Result<usize> {
      Ok(0) // never consumes anything, never reports an error
    }
  }

  let mut stream = CharStream { state: State::Decoding(Box::new(StallingDecoder)), ..CharStream::new() };
  // Each byte is buffered because the decoder consumes none; `pending` grows by one every feed.
  for _ in 0..MAX_PENDING {
    stream.feed(b"a", false).unwrap();
  }
  // The byte that would push `pending` past the cap is refused instead of buffered.
  assert!(matches!(stream.feed(b"a", false).unwrap_err(), Error::Encoding { .. }));
}

#[test]
fn sniffing_decodes_as_soon_as_the_declaration_is_read() {
  // A short declaration and element, fed in small pieces with more to come, must decode without
  // waiting for a byte count or the end of input, so an interactive peer is not left waiting.
  let mut stream = CharStream::new();
  stream.feed(b"<?xml version='1.0' encoding='UTF-8'?>", false).unwrap();
  assert_eq!(stream.encoding(), Some("UTF-8"));
  assert_eq!(stream.remainder(), "<?xml version='1.0' encoding='UTF-8'?>");

  stream.feed(b"<greeting/>", false).unwrap();
  assert_eq!(stream.remainder(), "<?xml version='1.0' encoding='UTF-8'?><greeting/>");
}

#[test]
fn sniffing_waits_while_the_declaration_is_incomplete() {
  // The encoding pseudo-attribute has not arrived, so nothing is decoded yet.
  let mut stream = CharStream::new();
  stream.feed(b"<?xml version='1.0' enc", false).unwrap();
  assert_eq!(stream.encoding(), None);
  assert_eq!(stream.remainder(), "");

  // Once the rest of the declaration arrives, the buffered bytes decode with the named encoding.
  stream.feed(b"oding='ISO-8859-1'?>", false).unwrap();
  stream.feed(&[0xE9], true).unwrap();
  assert_eq!(stream.encoding(), Some("ISO-8859-1"));
  assert_eq!(stream.remainder(), "<?xml version='1.0' encoding='ISO-8859-1'?>é");
}

#[test]
fn feeding_and_reading_finish_separately() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  stream.feed(b"ab", false).unwrap();
  assert!(stream.can_be_fed() && !stream.is_fully_read());
  stream.feed(b"", true).unwrap();
  assert!(!stream.can_be_fed(), "all bytes fed");
  assert!(!stream.is_fully_read(), "but not all characters consumed");
  stream.advance_chars(2);
  assert!(stream.is_fully_read());
}

#[test]
fn text_streams_need_no_decoder() {
  let stream = CharStream::from_text("a\r\nb").unwrap();
  assert_eq!(stream.remainder(), "a\nb");
  assert!(!stream.can_be_fed(), "a text stream is already at its end");
  assert!(matches!(CharStream::from_text("\u{0}").unwrap_err(), Error::WellFormedness { .. }));
}

#[test]
fn the_buffer_is_compacted_as_text_is_consumed() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  let text = "x".repeat(COMPACT_THRESHOLD * 3);
  stream.feed(text.as_bytes(), true).unwrap();
  stream.advance(COMPACT_THRESHOLD * 2);
  assert_eq!(stream.buf.len(), COMPACT_THRESHOLD, "consumed text was dropped");
  assert_eq!(stream.remainder().len(), COMPACT_THRESHOLD);
  assert_eq!(stream.location().offset, (COMPACT_THRESHOLD * 2) as u64, "position survives compaction");
}

#[test]
fn feed_returns_the_number_of_newly_decoded_characters() {
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  assert_eq!(stream.feed(b"abc", false).unwrap(), 3);

  // A split multi-byte character yields no new character until the held byte is completed.
  let euro = "€".as_bytes(); // E2 82 AC
  assert_eq!(stream.feed(&euro[..2], false).unwrap(), 0, "incomplete character, nothing appended");
  assert_eq!(stream.feed(&euro[2..], true).unwrap(), 1, "the character completes");
  assert_eq!(stream.remainder(), "abc€");

  // A stream still deciding the encoding decodes nothing yet.
  let mut sniffing = CharStream::new();
  assert_eq!(sniffing.feed(b"<?xml ", false).unwrap(), 0, "still sniffing");
}

#[test]
fn decoded_bytes_are_counted_as_the_decoder_consumes_them() {
  // A 3-byte character split across two feeds: the byte held back is counted only once it decodes.
  let bytes = "日".as_bytes(); // E6 97 A5
  let mut stream = CharStream::with_encoding("UTF-8").unwrap();
  stream.feed(&bytes[..2], false).unwrap();
  assert_eq!(stream.bytes_decoded(), 0, "the incomplete character is held, not yet decoded");
  stream.feed(&bytes[2..], true).unwrap();
  assert_eq!(stream.bytes_decoded(), 3);

  // A stripped byte-order mark is never passed to the decoder, so it is not counted.
  let mut sniffed = CharStream::new();
  sniffed.feed(b"\xEF\xBB\xBF<x/>", true).unwrap();
  assert_eq!(sniffed.remainder(), "<x/>");
  assert_eq!(sniffed.bytes_decoded(), 4, "the 3-byte BOM is excluded; only <x/> is decoded");

  // A stream from already-decoded text decodes no bytes.
  assert_eq!(CharStream::from_text("hello").unwrap().bytes_decoded(), 0);
}

#[test]
fn decoded_characters_are_counted_toward_the_expansion_limit() {
  let mut stream = fed("日本語".as_bytes());
  assert_eq!(stream.chars_decoded(), 3);
  stream.advance_chars(3);
  assert_eq!(stream.chars_decoded(), 3, "consuming does not change the count");
}
