//! The character stream of a single entity.
//!
//! A [`CharStream`] sits directly on top of a [`Decoder`] and turns fed bytes into the
//! character sequence XML 1.0 §2.11 defines: decoded, with line ends normalized, and with
//! every character checked against `Char`.
//!
//! Nothing is consumed until the caller says so. A tokenizer that finds an incomplete token
//! at the end of the buffer simply does not call [`advance`](CharStream::advance), waits for
//! more input, and rescans the token from its start — which is what makes the parser
//! resumable at any byte boundary without a suspendable state machine.

use std::sync::Arc;

use xylogue_core::chars;
use xylogue_core::encoding::{self, Decoder};
use xylogue_core::error::{Error, Location, Result};

/// How many bytes to accumulate before deciding the encoding of an entity.
///
/// Enough to cover any XML or text declaration; a shorter entity is sniffed at end of input.
const SNIFF_BYTES: usize = 256;

/// Bytes of consumed text tolerated before the buffer is compacted.
const COMPACT_THRESHOLD: usize = 8 * 1024;

#[derive(Debug)]
enum State {
  /// Bytes are being accumulated to determine the encoding.
  Sniffing(Vec<u8>),
  /// The encoding is known.
  Decoding(Box<dyn Decoder>),
}

/// The decoded, normalized character stream of one entity.
///
/// # Examples
///
/// ```
/// use xylogue_parser::CharStream;
///
/// let mut stream = CharStream::new().with_system_id("file:///doc.xml");
/// stream.feed(b"<doc>\r\n  text\r\n</doc>", true)?;
///
/// // Line ends are normalized: CR LF became a single LF.
/// assert_eq!(stream.remainder(), "<doc>\n  text\n</doc>");
///
/// // Consuming text moves the reported position.
/// stream.advance("<doc>\n".len());
/// let at = stream.location();
/// assert_eq!((at.line, at.column), (2, 1));
/// assert_eq!(at.system_id.as_deref(), Some("file:///doc.xml"));
/// # Ok::<(), xylogue_core::Error>(())
/// ```
#[derive(Debug)]
pub struct CharStream {
  state: State,
  encoding: Option<String>,
  /// Decoded text: consumed characters before `start`, unconsumed after.
  buf: String,
  start: usize,
  /// Bytes the decoder could not consume yet, held for the next feed.
  pending: Vec<u8>,
  /// The previously appended character was a carriage return, so a following line feed is
  /// part of the same line end even if it arrives in a later chunk.
  after_cr: bool,
  finished: bool,
  chars_appended: u64,
  system_id: Option<Arc<str>>,
  public_id: Option<Arc<str>>,
  line: u32,
  column: u32,
  offset: u64,
}

impl Default for CharStream {
  fn default() -> Self {
    Self::new()
  }
}

impl CharStream {
  /// Creates a stream that determines its encoding from the bytes it is fed.
  ///
  /// Detection follows XML 1.0 Appendix F; see [`xylogue_core::encoding::detect`]. The
  /// first bytes of the entity are held back until there are enough of them to cover an XML
  /// declaration, or until the entity ends, so [`remainder`](Self::remainder) stays empty
  /// over the first few feeds. Use [`with_encoding`](Self::with_encoding) when the encoding
  /// is already known.
  #[must_use]
  pub fn new() -> Self {
    Self {
      state: State::Sniffing(Vec::new()),
      encoding: None,
      buf: String::new(),
      start: 0,
      pending: Vec::new(),
      after_cr: false,
      finished: false,
      chars_appended: 0,
      system_id: None,
      public_id: None,
      line: 1,
      column: 1,
      offset: 0,
    }
  }

  /// Creates a stream with the encoding already decided.
  ///
  /// Used when the encoding is dictated from outside the entity — a protocol header, an
  /// external entity's text declaration, or an explicit override.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] if the name is unknown, or
  /// [`Error::UnsupportedFeature`] if it needs a feature that was not compiled in.
  pub fn with_encoding(encoding: &str) -> Result<Self> {
    let decoder = encoding::decoder_for(encoding)?;
    Ok(Self { encoding: Some(decoder.encoding().to_owned()), state: State::Decoding(decoder), ..Self::new() })
  }

  /// Creates a stream over text that has already been decoded.
  ///
  /// This is how the replacement text of an internal entity enters the parser: it never
  /// existed as bytes. The text is still normalized and checked.
  ///
  /// # Errors
  ///
  /// Returns [`Error::WellFormedness`] if the text contains a character `Char` forbids.
  pub fn from_text(text: &str) -> Result<Self> {
    let mut stream = Self { state: State::Decoding(Box::new(NoDecoder)), ..Self::new() };
    stream.append(text)?;
    stream.finished = true;
    Ok(stream)
  }

  /// Sets the system identifier reported in locations and used as the base URI.
  #[must_use]
  pub fn with_system_id(mut self, system_id: impl Into<Arc<str>>) -> Self {
    self.system_id = Some(system_id.into());
    self
  }

  /// Sets the public identifier reported in locations.
  #[must_use]
  pub fn with_public_id(mut self, public_id: impl Into<Arc<str>>) -> Self {
    self.public_id = Some(public_id.into());
    self
  }

  /// Fixes the encoding of a stream still waiting to sniff one, skipping detection.
  ///
  /// This is how an encoding named outside the entity — a transport header, a caller's override —
  /// takes effect on a stream that was built to sniff: the sniffing state is replaced with a
  /// decoder for `encoding`, and the stream's identifiers and position are kept. Detection is
  /// skipped entirely, so a leading byte-order mark is *not* stripped; feed undecorated bytes, or
  /// let the stream sniff instead when the input may carry one.
  ///
  /// # Errors
  ///
  /// Returns whatever [`encoding::decoder_for`] does for an unknown or unavailable encoding, and
  /// an [`Error::Internal`] if the stream has already been fed — the encoding is a decision that
  /// has to be made before the first byte.
  pub fn use_encoding(&mut self, encoding: &str) -> Result<()> {
    if !matches!(&self.state, State::Sniffing(sniffed) if sniffed.is_empty()) {
      let message = "the encoding must be chosen before any bytes are fed to the stream";
      return Err(Error::Internal { message: message.into() });
    }
    let decoder = encoding::decoder_for(encoding)?;
    self.encoding = Some(decoder.encoding().to_owned());
    self.state = State::Decoding(decoder);
    Ok(())
  }

  /// Supplies more bytes, decoding as much as possible.
  ///
  /// `last` marks the end of the entity. A sequence left incomplete at the end of `bytes` is
  /// carried over to the next call, and becomes an error only if `last` is set.
  ///
  /// # Examples
  ///
  /// Input may be split anywhere, including inside a character:
  ///
  /// ```
  /// use xylogue_parser::CharStream;
  ///
  /// let bytes = "<a>日</a>".as_bytes();
  /// let mut stream = CharStream::new();
  /// stream.feed(&bytes[..4], false)?; // ends mid-character
  /// stream.feed(&bytes[4..], true)?;
  /// assert_eq!(stream.remainder(), "<a>日</a>");
  /// # Ok::<(), xylogue_core::Error>(())
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] for undecodable bytes, [`Error::WellFormedness`] for
  /// a character `Char` forbids, and [`Error::Internal`] if called after `last`.
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    if self.finished {
      let message = "this entity was already fed its last bytes; feed(.., true) may only be called once";
      return Err(Error::Internal { message: message.into() }.at(self.location()));
    }
    if let State::Sniffing(sniffed) = &mut self.state {
      sniffed.extend_from_slice(bytes);
      if sniffed.len() < SNIFF_BYTES && !last {
        return Ok(()); // not enough to decide yet
      }
      let sniffed = std::mem::take(sniffed);
      let detection = encoding::detect(&sniffed);
      let decoder = encoding::decoder_for(&detection.encoding).map_err(|e| e.at(self.location()))?;
      self.encoding = Some(decoder.encoding().to_owned());
      self.state = State::Decoding(decoder);
      return self.decode(&sniffed[detection.bom_length..], last);
    }
    self.decode(bytes, last)
  }

  fn decode(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    let State::Decoding(decoder) = &mut self.state else {
      return Err(Error::internal("decoding began before the encoding was determined"));
    };

    let mut text = String::new();
    if self.pending.is_empty() {
      let n = decoder.decode(bytes, &mut text, last).map_err(|e| e.at(self.location()))?;
      self.pending.extend_from_slice(&bytes[n..]);
    } else {
      self.pending.extend_from_slice(bytes);
      let held = std::mem::take(&mut self.pending);
      let n = decoder.decode(&held, &mut text, last).map_err(|e| e.at(self.location()))?;
      self.pending.extend_from_slice(&held[n..]);
    }

    if last && !self.pending.is_empty() {
      let message = "the entity ends with an incomplete character";
      return Err(Error::encoding(message).at(self.location()));
    }
    self.finished = last;
    self.append(&text)
  }

  /// Appends decoded text, normalizing line ends (XML 1.0 §2.11) and checking `Char`.
  fn append(&mut self, text: &str) -> Result<()> {
    self.buf.reserve(text.len());
    for c in text.chars() {
      if self.after_cr && c == '\n' {
        // The LF of a CR LF pair: the line end was already emitted for the CR.
        self.after_cr = false;
        continue;
      }
      self.after_cr = c == '\r';
      let c = if c == '\r' { '\n' } else { c };
      if !chars::is_char(c) {
        // Usually a NUL or a C0 control from a mislabelled encoding, so name that first.
        let message = format!(
          "U+{:04X} may not appear in XML, in any form; if the entity is not really {}, correct its encoding declaration",
          c as u32,
          self.encoding.as_deref().unwrap_or("this encoding")
        );
        return Err(Error::well_formedness(message).at(self.location_of(self.buf.len())));
      }
      self.buf.push(c);
      self.chars_appended += 1;
    }
    Ok(())
  }

  /// The decoded characters that have not been consumed.
  ///
  /// A tokenizer scans this and, only once it recognizes a complete token, calls
  /// [`advance`](Self::advance).
  #[must_use]
  pub fn remainder(&self) -> &str {
    &self.buf[self.start..]
  }

  /// Consumes the first `len` bytes of [`remainder`](Self::remainder), advancing the position.
  ///
  /// # Panics
  ///
  /// If `len` is not a character boundary of the remainder, or is past its end.
  pub fn advance(&mut self, len: usize) {
    let consumed = &self.buf[self.start..self.start + len];
    for c in consumed.chars() {
      if c == '\n' {
        self.line += 1;
        self.column = 1;
      } else {
        self.column += 1;
      }
      self.offset += 1;
    }
    self.start += len;
    self.compact();
  }

  /// Consumes `count` characters of [`remainder`](Self::remainder).
  ///
  /// Consumes everything if the remainder is shorter, so the caller must have checked that
  /// the characters are actually there.
  pub fn advance_chars(&mut self, count: usize) {
    let len = self.remainder().char_indices().nth(count).map_or(self.remainder().len(), |(i, _)| i);
    self.advance(len);
  }

  /// Drops consumed text once enough has accumulated to be worth the copy.
  fn compact(&mut self) {
    if self.start >= COMPACT_THRESHOLD && self.start * 2 >= self.buf.len() {
      self.buf.drain(..self.start);
      self.start = 0;
    }
  }

  /// The current position: the start of the unconsumed text.
  #[must_use]
  pub fn location(&self) -> Location {
    Location {
      system_id: self.system_id.clone(),
      public_id: self.public_id.clone(),
      line: self.line,
      column: self.column,
      offset: self.offset,
    }
  }

  /// The position of the character at byte index `index` of `buf`.
  fn location_of(&self, index: usize) -> Location {
    let mut at = self.location();
    for c in self.buf[self.start..index.max(self.start)].chars() {
      if c == '\n' {
        at.line += 1;
        at.column = 1;
      } else {
        at.column += 1;
      }
      at.offset += 1;
    }
    at
  }

  /// The system identifier of this entity, if known.
  #[must_use]
  pub fn system_id(&self) -> Option<&Arc<str>> {
    self.system_id.as_ref()
  }

  /// The public identifier of this entity, if known.
  #[must_use]
  pub fn public_id(&self) -> Option<&Arc<str>> {
    self.public_id.as_ref()
  }

  /// The encoding in use, or `None` while it is still being determined.
  #[must_use]
  pub fn encoding(&self) -> Option<&str> {
    self.encoding.as_deref()
  }

  /// True once the last bytes have been fed, whether or not they are all consumed.
  #[must_use]
  pub fn is_complete(&self) -> bool {
    self.finished
  }

  /// True when the entity is complete and every character has been consumed.
  #[must_use]
  pub fn is_exhausted(&self) -> bool {
    self.finished && self.start == self.buf.len()
  }

  /// Total number of characters decoded so far, consumed or not.
  ///
  /// The entity expansion budget is measured in these.
  #[must_use]
  pub fn chars_decoded(&self) -> u64 {
    self.chars_appended
  }
}

/// Placeholder decoder for streams built from text that was never encoded.
#[derive(Debug)]
struct NoDecoder;

impl Decoder for NoDecoder {
  fn encoding(&self) -> &str {
    "none"
  }

  fn decode(&mut self, _src: &[u8], _dst: &mut String, _last: bool) -> Result<usize> {
    Err(Error::internal("an entity built from text was fed bytes"))
  }
}

#[cfg(test)]
mod tests {
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
  fn use_encoding_fixes_a_fresh_stream_but_not_one_already_fed() {
    // 0xE9 is 'é' in ISO-8859-1 but not valid UTF-8, so the choice of encoding is visible.
    let mut stream = CharStream::new();
    stream.use_encoding("ISO-8859-1").unwrap();
    stream.feed(&[0xE9], true).unwrap();
    assert_eq!(stream.remainder(), "é");

    // Once a byte has been fed, the encoding can no longer be chosen.
    let mut fed = CharStream::new();
    fed.feed(b"x", false).unwrap();
    assert!(matches!(fed.use_encoding("UTF-8").unwrap_err(), Error::Internal { .. }));
  }

  #[test]
  fn completion_and_exhaustion_are_different() {
    let mut stream = CharStream::with_encoding("UTF-8").unwrap();
    stream.feed(b"ab", false).unwrap();
    assert!(!stream.is_complete() && !stream.is_exhausted());
    stream.feed(b"", true).unwrap();
    assert!(stream.is_complete(), "all bytes fed");
    assert!(!stream.is_exhausted(), "but not all characters consumed");
    stream.advance_chars(2);
    assert!(stream.is_exhausted());
  }

  #[test]
  fn text_streams_need_no_decoder() {
    let stream = CharStream::from_text("a\r\nb").unwrap();
    assert_eq!(stream.remainder(), "a\nb");
    assert!(stream.is_complete());
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
  fn decoded_characters_are_counted_for_the_expansion_budget() {
    let mut stream = fed("日本語".as_bytes());
    assert_eq!(stream.chars_decoded(), 3);
    stream.advance_chars(3);
    assert_eq!(stream.chars_decoded(), 3, "consuming does not change the count");
  }
}
