//! The character stream of a single entity.
//!
//! A [`CharStream`] uses a [`Decoder`] to convert the supplied bytes into a character sequence as defined in XML 1.0
//! §2.11.
//!
//! The stream does not consume anything until instructed to do so by the caller. If the tokenizer detects an incomplete
//! token ato the end of the buffer, it waits for further input and re-scans from the beginning fo that token. This
//! allows the parser to resume processing from any byte boundary.
//!

use std::sync::Arc;

use crate::chars;
use crate::encoding::{self, Decoder};
use crate::error::{Error, Location, Result};

/// The most bytes to buffer while determining the encoding before assuming the default.
///
/// The encoding is normally settled by the byte-order mark or the XML declaration, both near the very
/// start of an entity, so this is only a safety bound for input whose declaration never completes.
const SNIFF_LIMIT: usize = 256;

/// The number of bytes of text that can be consumed before the buffer is compacted.
const COMPACT_THRESHOLD: usize = 8 * 1024;

/// The maximum number of unconsumed bytes a stream can hold between feeds before input is rejected. This prevents the
/// buffer to unconsumed bytes from expanding indefinitely due to decoder bug or malicious implementations.
///
/// Property functioning decoders consume all complete units and retain only incomplete trailing units, so the carry
/// over amount is minimal. For example, the built-in UTF-8 and UTF-16 decoders leave a maximum of 3 bytes. The value
/// of 16 is considered to sufficiently exceed that threshold and the number of incomplete units that a reasonable
/// custom [`Decoder`] might retain. Decoders that exceed this threshold are simply inflating the buffer without
/// consuming anything, and since a malicious stream could exploit this to exhaust memory, their input is rejected.
///
const MAX_PENDING: usize = 16;

#[derive(Debug)]
enum State {
  /// Bytes are being accumulated to determine the encoding.
  Sniffing(Vec<u8>),
  /// The encoding is known.
  Decoding(Box<dyn Decoder>),
}

/// The decoded, normalized character stream of one entity.
///
/// [`CharStream`] is a character input stream that can read a continuous byte sequence from a single entity as a
/// character sequence as defined in XML 1.0 §2.11.
///
/// This structure is based on a "sans-I/O" model; it has no underlying input of its own, and the caller passes (pushes)
/// fragmented byte sequences read by any method via a [`feed`](CharStream::feed). The [`CharStream`] decodes as many
/// characters as possible from the passed byte sequence and internally retains incomplete byte sequences that were cut
/// off mid-chunk until the rest arrives in the next [`feed`](CharStream::feed) call. Therefore, the input may be split
/// at any byte boundary, and processing can be resumed from any boundary. This design is identical to that of
/// incremental, push-based parsers and decoders.
///
/// Note that the direction of input is opposite to that of common "pull"-style stream, such as those in Java, which
/// read bytes from an underlying source input.
///
/// # Examples
///
/// ```
/// use xenolith_core::stream::CharStream;
///
/// let mut stream = CharStream::new().with_system_id("file:///doc.xml");
/// stream.feed(b"<doc>\r\n  text\r\n</doc>", true)?;
///
/// // Line ends are normalized: CR LF became a single LF.
/// assert_eq!(stream.remainder(), "<doc>\n  text\n</doc>");
///
/// // When the text is consumed, the reported location will move.
/// stream.advance("<doc>\n".len());
/// let at = stream.location();
/// assert_eq!((at.line, at.column), (2, 1));
/// assert_eq!(at.system_id.as_deref(), Some("file:///doc.xml"));
/// # Ok::<(), xenolith_core::Error>(())
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
  bytes_decoded: u64,
  /// The current reading position, carrying the entity's identifiers.
  at: Location,
}

impl Default for CharStream {
  fn default() -> Self {
    Self::new()
  }
}

impl CharStream {
  /// Creates a new character stream that decodes characters from the XML byte sequence being fed to it.
  ///
  /// The stream that constructed with this [`new`](Self::new) attempts to automatically detect the encoding in
  /// accordance with XML 1.0 Appendix F; see [`crate::encoding::detect`] for more details.
  ///
  /// Until the fed byte sequence is long enough to cover the encoding specified in the XML declaration of the entity
  /// (or until the entity ends), [`remainder`](Self::remainder) may return an empty result. If the encoding is already
  /// known, use [`with_encoding`](Self::with_encoding) or [`use_encoding`](Self::use_encoding).
  ///
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
      bytes_decoded: 0,
      at: Location { system_id: None, public_id: None, line: 1, column: 1, offset: 0 },
    }
  }

  /// Creates a stream for which the encoding has already been determined.
  ///
  /// Use this when the encoding is specified outside the entity (such as in a protocol header or by system
  /// requirements). However, if you specify an encoding, the byte-order mark will not be removed via detection. In
  /// this case, caller should detect and remove the U+FEFF at the beginning of the stream.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] if the name is unknown, or [`Error::UnsupportedFeature`] if it needs a feature that
  /// was not compiled in.
  ///
  pub fn with_encoding(encoding: &str) -> Result<Self> {
    let decoder = encoding::decoder_for(encoding)?;
    Ok(Self { encoding: Some(decoder.encoding().to_owned()), state: State::Decoding(decoder), ..Self::new() })
  }

  /// Creates a stream that uses the already-decoded text as the entity to be read.
  ///
  /// Instead of decoding character from a fed byte array, this creates an input stream that reads the specified `text`.
  /// This stream has reached the end of its input and cannot accept any further data.
  ///
  /// # Errors
  ///
  /// Returns [`Error::WellFormedness`] if the text contains a character `Char` forbids.
  ///
  pub fn from_text(text: &str) -> Result<Self> {
    let mut stream = Self { state: State::Decoding(Box::new(NoDecoder)), ..Self::new() };
    stream.append(text)?;
    stream.finished = true;
    Ok(stream)
  }

  /// Sets the system identifier reported in locations and used as the base URI.
  #[must_use]
  pub fn with_system_id(mut self, system_id: impl Into<Arc<str>>) -> Self {
    self.at.system_id = Some(system_id.into());
    self
  }

  /// Sets the public identifier reported in locations.
  #[must_use]
  pub fn with_public_id(mut self, public_id: impl Into<Arc<str>>) -> Self {
    self.at.public_id = Some(public_id.into());
    self
  }

  /// Specifies an encoding for this stream and skips detection. If [`encoding`](Self::encoding) is not `None`, that is,
  /// if the encoding has already been detected, an error is returned.
  ///
  /// # Errors
  ///
  /// Returns the processing performed by [`encoding::decoder_for`] for an unknown or unavailable encoding. It also
  /// returns [`Error::Internal`] if the encoding has already been determined. The encoding must be determined before
  /// the first byte is read.
  ///
  pub fn use_encoding(&mut self, encoding: &str) -> Result<()> {
    let State::Sniffing(sniffed) = &mut self.state else {
      let message = "the encoding must be chosen before decoding begins";
      return Err(Error::Internal { message: message.into() });
    };
    // Build the decoder before taking the buffer, so an unknown encoding leaves the stream untouched.
    let decoder = encoding::decoder_for(encoding)?;
    let sniffed = std::mem::take(sniffed);
    // No detection ran, so a leading byte-order mark is not stripped from the buffered bytes.
    self.start_decoding(decoder, &sniffed, false)
  }

  /// Switches from the encoding detection state to the decoding state using `decoder`, and decode the initial byte
  /// sequence `bytes`.
  ///
  /// If the character encoding is explicitly specified, `bytes` may contain a byte-order mark. `last` marks the `bytes`
  /// as the last input, similar to [`feed`](Self::feed).
  ///
  fn start_decoding(&mut self, decoder: Box<dyn Decoder>, bytes: &[u8], last: bool) -> Result<()> {
    self.encoding = Some(decoder.encoding().to_owned());
    self.state = State::Decoding(decoder);
    self.decode(bytes, last)
  }

  /// Supplies the remainder of the byte sequence. This call decodes as many characters as possible, and
  /// [`remainder`](Self::remainder) may provide even more characters.
  ///
  /// This returns the number of characters appended to the end of [`remainder`](Self::remainder). If the result is
  /// returned, the number of remaining characters has not increase. This means the stream is still parsing the
  /// encoding, so you can skip the call of [`remainder`](Self::remainder).
  /// Returns the number of characters this call appended to the end of [`remainder`](Self::remainder). Zero means the
  /// remainder did not grow: the stream is still sniffing the encoding, or the bytes only extended an incomplete
  /// character. A caller driving a parser can treat a non-zero result as the signal that there is new text to process.
  ///
  /// `last` indicates that the specified `bytes` is the last fragment of the entity. `bytes` is a fragment of the byte
  /// sequence that this stream should decode. Any sequence that remains incomplete at the time of decoding is either
  /// carried over to the next call or results in an error if `last` is true.
  ///
  /// # Examples
  ///
  /// Input may be split at arbitrary boundaries, including within a single character:
  ///
  /// ```
  /// use xenolith_core::stream::CharStream;
  ///
  /// let bytes = "<a>日</a>".as_bytes();
  /// let mut stream = CharStream::new();
  /// let n = stream.feed(&bytes[..4], false)?; // ends mid-character
  /// assert_eq!(n, 3); // "<a>" was decoded; the first byte of 日 is held back
  /// let n = stream.feed(&bytes[4..], true)?;
  /// assert_eq!(n, 5); // 日 completes, then "</a>": five characters
  /// assert_eq!(stream.remainder(), "<a>日</a>");
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] for undecodable bytes, [`Error::WellFormedness`] for a character `Char` forbids, and
  /// [`Error::Internal`] if called after `last`.
  ///
  /// On any of these errors occur, the stream remains in an unspecified state with partial progress. The caller must
  /// not write data to or read from that stream.
  ///
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<usize> {
    let before = self.chars_appended;
    self.feed_bytes(bytes, last)?;
    Ok((self.chars_appended - before) as usize)
  }

  /// Decodes `bytes` into the buffer; the body of [`feed`](Self::feed) without the character count.
  fn feed_bytes(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    if self.finished {
      let message = "this entity was already fed its last bytes; feed(.., true) may only be called once";
      return Err(Error::Internal { message: message.into() }.at(self.location()));
    }
    if let State::Sniffing(sniffed) = &mut self.state {
      sniffed.extend_from_slice(bytes);
      // Determine the encoding as soon as possible based on the bytes that have been input. The purpose of this
      // behavior is to prevent deadlocks that can occur when waiting for a certain amount of input in entities sent
      // interactively. If input ends or exceeds the scope of the XML declaration, the default encoding, UTF-8, is
      // applied.
      let detected = encoding::detect(sniffed);
      if matches!(detected, encoding::Detected::Incomplete) && !last && sniffed.len() < SNIFF_LIMIT {
        return Ok(());
      }
      let detection = detected.or_default();
      let sniffed = std::mem::take(sniffed);
      let decoder = encoding::decoder_for(&detection.encoding).map_err(|e| e.at(self.location()))?;
      self.start_decoding(decoder, &sniffed[detection.bom_length..], last)
    } else {
      self.decode(bytes, last)
    }
  }

  fn decode(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    let State::Decoding(decoder) = &mut self.state else {
      return Err(Error::internal("decoding began before the encoding was determined"));
    };

    let mut text = String::new();
    let consumed = if self.pending.is_empty() {
      let n = decoder.decode(bytes, &mut text, last).map_err(|e| e.at(self.location()))?;
      self.pending.extend_from_slice(&bytes[n..]);
      n
    } else {
      self.pending.extend_from_slice(bytes);
      let held = std::mem::take(&mut self.pending); // retrieve the bytes from `self.pending` and clear it
      let n = decoder.decode(&held, &mut text, last).map_err(|e| e.at(self.location()))?;
      self.pending.extend_from_slice(&held[n..]);
      n
    };
    self.bytes_decoded += consumed as u64;

    if last && !self.pending.is_empty() {
      let message = "the entity ends with an incomplete character";
      return Err(Error::encoding(message).at(self.location()));
    }
    if self.pending.len() > MAX_PENDING {
      let message = format!(
        "the decoder is not making progress: {} bytes remain unconsumed, more than any incomplete character needs",
        self.pending.len()
      );
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

  /// Refers to a decoded, unconsumed string held by this stream.
  ///
  /// The stream can continue to held a decoded string that have not yet been consumed. This allow the user to suspend
  /// processing until a certain unit of text, such as a "token", is read. It is the user's responsibility to consume
  /// the read string using [`advance`](Self::advance). Advancing the feed without consuming the characters may result
  /// in severe memory shortages.
  ///
  #[must_use]
  pub fn remainder(&self) -> &str {
    &self.buf[self.start..]
  }

  /// Consumes the first `len` bytes of [`remainder`](Self::remainder) and advances the read position of the next
  /// [`remainder`](Self::remainder) by that number of bytes.
  ///
  /// Note that `len` represents the number of *bytes* remaining in the UTF-8 string, not the number of *characters*.
  /// Therefore, `len` must correspond to a character boundary (the start of the remaining characters). You cannot
  /// specify a position that corresponds to a byte in the middle of a multi-byte character. Use
  /// [`advance_chars`](Self::advance_chars) instead to consume a specific number of *characters*, or prioritize safety
  /// over efficiency and want to avoid panics.
  ///
  /// # Panics
  ///
  /// If `len` extends beyond the end of remainder, or if it lies within a character rather than at a character
  /// boundary.
  ///
  pub fn advance(&mut self, len: usize) {
    let consumed = &self.buf[self.start..self.start + len];
    for c in consumed.chars() {
      self.at.advance(c);
    }
    self.start += len;
    self.compact();
  }

  /// Consumes the first `count` characters of [`remainder`](Self::remainder) and advances the read position of the next
  /// [`remainder`](Self::remainder) by that number of characters.
  ///
  /// If the `count` is greater than [`remainder`](Self::remainder), all remaining input is consumed.
  ///
  pub fn advance_chars(&mut self, count: usize) {
    let len = self.remainder().char_indices().nth(count).map_or(self.remainder().len(), |(i, _)| i);
    self.advance(len);
  }

  /// Drops consumed test from the buffer. For efficiency, the actual drop occurs only after a certain amount has
  /// accumulated.
  fn compact(&mut self) {
    if self.start >= COMPACT_THRESHOLD && self.start * 2 >= self.buf.len() {
      self.buf.drain(..self.start);
      self.start = 0;
    }
  }

  /// The current reading location; that is, the unconsumed text position that can be referred via the next
  /// [`remainder`](Self::remainder).
  #[must_use]
  pub fn location(&self) -> Location {
    self.at.clone()
  }

  /// The position of the character at byte index `index` of `buf`.
  fn location_of(&self, index: usize) -> Location {
    let mut at = self.at.clone();
    for c in self.buf[self.start..index.max(self.start)].chars() {
      at.advance(c);
    }
    at
  }

  /// The system identifier of this entity, if known.
  #[must_use]
  pub fn system_id(&self) -> Option<&Arc<str>> {
    self.at.system_id.as_ref()
  }

  /// The public identifier of this entity, if known.
  #[must_use]
  pub fn public_id(&self) -> Option<&Arc<str>> {
    self.at.public_id.as_ref()
  }

  /// The currently used encoding, or `None` if not detected.
  #[must_use]
  pub fn encoding(&self) -> Option<&str> {
    self.encoding.as_deref()
  }

  /// True as long as the stream can still accept input. This is the case when the [`feed`](Self::feed) with `last=true`
  /// has not yet been called.
  #[must_use]
  pub fn can_be_fed(&self) -> bool {
    !self.finished
  }

  /// True if all characters have been read. In other words, it indicates that the last byte has been read and that all
  /// decoded characters [`remaiander`](Self::remainder) are ampty.
  #[must_use]
  pub fn is_fully_read(&self) -> bool {
    self.finished && self.start == self.buf.len()
  }

  /// Total number of characters decoded so far, regardless of whether they have been consumed or not.
  ///
  /// This is used to measure the level of protection against entity expansion attacks (billion laughs / exponential
  /// expansion) in terms of "decoded characters".
  ///
  #[must_use]
  pub fn chars_decoded(&self) -> u64 {
    self.chars_appended
  }

  /// Total number of input bytes the decoder has processed so far.
  ///
  /// Each entity byte is counted once when it is decoded as text. Bytes held in reserve as incomplete trailing
  /// sequences are counted later, once the remaining data arrives and is decoded. Data from which byte-order marks have
  /// been removed is never passed to the decoder and is therefore not counted. Additionally, streams constructed from
  /// text using [`from_text`](Self::from_text) do not decode any bytes.
  ///
  #[must_use]
  pub fn bytes_decoded(&self) -> u64 {
    self.bytes_decoded
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
}
