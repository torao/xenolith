//! A character stream representing a single entity.
//!
//! [`CharStream`] uses a [`Decoder`] to convert the input byte sequence into a sequence of characters as defined in
//! §2.11 of the XML 1.0 specification. During this process, it normalizes line endings and validates each character
//! against the `Char` production rule.
//!
//! The stream does not consume input until the caller instructs it to. If the caller encounters an incomplete token at
//! the end of the available data, it leaves the token unconsumed; after supplying additional bytes, the caller then
//! rescans the token from the beginning. This design enables the parser to resume processing from any arbitrary byte
//! boundary in the input.

#[cfg(test)]
mod test;

use std::sync::Arc;

use crate::chars;
use crate::error::{Error, Location, Result};
use crate::io::encoding::{self, Decoder};

/// The maximum number of bytes stored in the buffer to determine the encoding before falling back to the default
/// encoding.
///
/// Since both the Byte Order Mark (BOM) and the XML declaration are placed at the beginning of the entity, this limit
/// is reached only when the input does not contain a complete declaration.
const SNIFF_LIMIT: usize = 256;

/// How many bytes of consumed text accumulate before the buffer is compressed.
const COMPACT_THRESHOLD: usize = 8 * 1024;

/// This represents the maximum number of unconsumed bytes a stream can retain before accepting new input. This limit
/// prevents the unconsumed-byte buffer from growing without bound due to a faulty or malicious [`Decoder`].
///
/// A well-behaved decoder consumes all complete data units and retains only the trailing incomplete unit, resulting in
/// minimal carried-over data (for example, a maximum of 3 bytes for the built-in UTF-8 and UTF-16 decoders). The
/// 16-byte limit is well above the requirements for these decoders and for typical custom [`Decoder`] implementations.
/// If a decoder retains more data than this, it implies the buffer is expanding without any actual consumption.
/// Because a malicious stream could exploit this behavior to launch a memory exhaustion attack, the system rejects
/// such input.
const MAX_PENDING: usize = 16;

#[derive(Debug)]
enum State {
  /// Bytes are being buffered until the encoding can be decided.
  Sniffing(Vec<u8>),
  /// The encoding is determined, and this decoder reads bytes.
  Decoding(Box<dyn Decoder>),
}

/// A decoded and normalized character stream for an entity.
///
/// [`CharStream`] reads a sequence of bytes belonging to a single entity as a sequence of characters, as defined in
/// Section 2.11 of the XML 1.0 specification.
///
/// This stream is I/O-agnostic; it has no underlying input source of its own. The caller reads a byte sequence using
/// any desired method and supplies the data in chunks via the [`feed`](CharStream::feed) method. The stream decodes as
/// many characters as possible from the supplied data. Any byte sequence split in the middle of a character is
/// buffered until the remaining data arrives in a subsequent [`feed`](CharStream::feed) call. Consequently, the input
/// can be split at arbitrary byte boundaries, allowing stream processing to be paused and resumed.
///
/// This direction of input flow is the opposite of pull-style streams, such as those in Java, which autonomously read
/// bytes from an underlying source.
///
/// # Examples
///
/// ```
/// use xenolith::io::stream::CharStream;
///
/// let mut stream = CharStream::new().with_system_id("file:///doc.xml");
/// stream.feed(b"<doc>\r\n  text\r\n</doc>", true)?;
///
/// // Line ends are normalized: CR LF became a single LF.
/// assert_eq!(stream.remainder(), "<doc>\n  text\n</doc>");
///
/// // Consuming text moves the reported location.
/// stream.advance("<doc>\n".len());
/// let at = stream.location();
/// assert_eq!((at.line, at.column), (2, 1));
/// assert_eq!(at.system_id.as_deref(), Some("file:///doc.xml"));
/// # Ok::<(), xenolith::Error>(())
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
  /// Creates a character stream that decodes the input XML byte stream.
  ///
  /// The stream created here automatically detects the encoding in accordance with Appendix F of XML 1.0. For details
  /// on the detection mechanism, please refer to [`crate::io::encoding::detect`].
  ///
  /// Until the encoding is determined, [`remainder`](Self::remainder) remains empty. The byte stream is buffered until
  /// the `encoding` pseudo-attribute of the XML declaration is read, the entity ends, or the buffer reaches 256 bytes
  /// (at which point it is treated as UTF-8). If the encoding is already known, use
  /// [`with_encoding`](Self::with_encoding) or [`use_encoding`](Self::use_encoding) instead.
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
      at: Location::new(),
    }
  }

  /// Creates a stream with a pre-determined encoding.
  ///
  /// Use this when the encoding is specified externally to the entity—such as via protocol headers or system
  /// requirements. Because it doesn't perform automatic detection, it doesn't remove any leading BOM. The byte
  /// sequence is passed directly to the decoder, so the handling of a U+FEFF character at the beginning of the text is
  /// left to the decoder or the caller.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Encoding`] if the encoding name is unknown, or [`Error::UnsupportedFeature`] if a feature not
  /// included at compile time is required.
  pub fn with_encoding(encoding: &str) -> Result<Self> {
    let decoder = encoding::decoder_for(encoding)?;
    Ok(Self { encoding: Some(decoder.encoding().to_owned()), state: State::Decoding(decoder), ..Self::new() })
  }

  /// Creates a stream from already decoded text.
  ///
  /// The `text` constitutes the entire entity. While line-ending normalization and character validity checks are
  /// performed (just as when supplying a byte sequence), no decoding takes place. The stream is already at the end of
  /// the input upon creation and does not accept additional input via [`feed`](Self::feed). Use this method when
  /// reading internal entities where the replacement text is already a `&str`.
  ///
  /// # Errors
  ///
  /// If `text` contains characters prohibited by the `Char` production rule, [`Error::WellFormedness`] is returned.
  ///
  pub fn from_text(text: &str) -> Result<Self> {
    let mut stream = Self { state: State::Decoding(Box::new(NoDecoder)), ..Self::new() };
    stream.append(text)?;
    stream.finished = true;
    Ok(stream)
  }

  /// Sets the system identifier that is reported in the location and used as the base URI.
  #[must_use]
  pub fn with_system_id(mut self, system_id: impl Into<Arc<str>>) -> Self {
    self.at.system_id = Some(system_id.into());
    self
  }

  /// Sets the public identifier that is reported in the location.
  #[must_use]
  pub fn with_public_id(mut self, public_id: impl Into<Arc<str>>) -> Self {
    self.at.public_id = Some(public_id.into());
    self
  }

  /// Specifies the encoding for this stream and skips automatic detection.
  ///
  /// Use this method when the encoding becomes known after the stream is created but before text decoding begins (for
  /// example, from an `encoding` attribute in a text declaration that the caller reads). Any bytes already fed into
  /// the stream are retained and decoded using the new decoder.
  ///
  /// # Errors
  ///
  /// For unknown or unavailable encodings, it returns the value provided by [`encoding::decoder_for`]; however, if
  /// decoding has already started, it returns [`Error::Internal`]. This is because you can't change the encoding
  /// mid-entity.
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

  /// Transitions from the sniffing state to the decoding state using the `decoder` and decodes the byte sequence
  /// buffered so far.
  ///
  /// `bytes` represents the data buffered during sniffing; if a Byte Order Mark (BOM) was detected, it has been
  /// removed (however, the mark remains intact if the caller specified an encoding). The meaning of `last` is the
  /// same as for [`feed`](Self::feed).
  fn start_decoding(&mut self, decoder: Box<dyn Decoder>, bytes: &[u8], last: bool) -> Result<()> {
    self.encoding = Some(decoder.encoding().to_owned());
    self.state = State::Decoding(decoder);
    self.decode(bytes, last)
  }

  /// Supplies the next chunk of the entity's byte sequence and decodes as many characters as possible.
  ///
  /// Returns the number of characters appended to [`remainder`](Self::remainder) by this call. A return value of 0
  /// indicates that `remainder` did not grow; this signifies that the stream is still determining the encoding or
  /// that the supplied byte sequence merely extended an incomplete character. The caller controlling the parser can
  /// treat a non-zero return value as a signal that there is new text to process.
  ///
  /// `last` indicates whether `bytes` is the final chunk of the entity. If the byte sequence is incomplete after
  /// processing the chunk, the remaining part is carried over to the next call; however, if `last` is true, it is
  /// reported as an error.
  ///
  /// # Examples
  ///
  /// The input may be split at arbitrary boundaries, including positions that fall within a single character:
  ///
  /// ```
  /// use xenolith::io::stream::CharStream;
  ///
  /// let bytes = "<a>日</a>".as_bytes();
  /// let mut stream = CharStream::new();
  /// let n = stream.feed(&bytes[..4], false)?; // ends mid-character
  /// assert_eq!(n, 3); // "<a>" was decoded; the first byte of 日 is held back
  /// let n = stream.feed(&bytes[4..], true)?;
  /// assert_eq!(n, 5); // 日 completes, then "</a>": five characters
  /// assert_eq!(stream.remainder(), "<a>日</a>");
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  ///
  /// # Errors
  ///
  /// [`Error::Encoding`] is returned for byte sequences that cannot be decoded, entities that terminate in the middle
  /// of a character, and decoders that have stalled. [`Error::WellFormedness`] is returned for characters prohibited
  /// by the `Char` production rule, and [`Error::Internal`] is returned if the method is called after inputting data
  /// with the `last` flag set.
  ///
  /// When these errors occur, the stream retains its intermediate processing state, but that state is undefined; do
  /// not attempt to input or read data again. The error terminates the entity.
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<usize> {
    let before = self.chars_appended;
    self.feed_bytes(bytes, last)?;
    Ok((self.chars_appended - before) as usize)
  }

  /// Decodes `bytes` into the buffer. This corresponds to the body of [`feed`](Self::feed), excluding the character
  /// count.
  fn feed_bytes(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    if self.finished {
      let message = "this entity was already fed its last bytes; feed(.., true) may only be called once";
      return Err(Error::Internal { message: message.into() }.at(self.location()));
    }
    if let State::Sniffing(sniffed) = &mut self.state {
      sniffed.extend_from_slice(bytes);
      // Decide the encoding as soon as the bytes so far allow it. Waiting for a fixed number of bytes would stall an
      // entity that arrives interactively and sends nothing more until it is answered. Once the input ends or runs
      // past where a declaration could still be, the default of UTF-8 is assumed.
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
      let held = std::mem::take(&mut self.pending); // decode the carried-over bytes and the new ones as one slice
      let n = decoder.decode(&held, &mut text, last).map_err(|e| e.at(self.location()))?;
      self.pending.extend_from_slice(&held[n..]);
      n
    };
    self.bytes_decoded += consumed as u64;

    if last && !self.pending.is_empty() {
      let message = "the entity ends with an incomplete character";
      return Err(Error::encoding(message, None).at(self.location()));
    }
    if self.pending.len() > MAX_PENDING {
      let message = format!(
        "the decoder is not making progress: {} bytes remain unconsumed, more than any incomplete character needs",
        self.pending.len()
      );
      return Err(Error::encoding(message, None).at(self.location()));
    }
    self.finished = last;
    self.append(&text)
  }

  /// Appends decoded text, normalizing line ends (XML 1.0 §2.11) and checking every character against `Char`.
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

  /// The decoded text held by the stream that has not yet been consumed.
  ///
  /// Since this text remains here until the caller consumes it, the caller can wait for a complete unit of text, such
  /// as a token, to arrive before processing it. It is the caller's responsibility to consume the text via
  /// [`advance`](Self::advance). If you continue adding byte data without calling `advance`, the buffer will keep
  /// growing until memory is exhausted.
  #[must_use]
  pub fn remainder(&self) -> &str {
    &self.buf[self.start..]
  }

  /// Consumes the first `len` bytes of [`remainder`](Self::remainder) and advances the read position accordingly.
  ///
  /// The `len` specified here represents the **number of bytes** of UTF-8 text, not the number of characters, and must
  /// align with a character boundary (positions within a multi-byte character are invalid). Callers intending to
  /// process the remainder as a `&str` should already be aware of such byte offsets. If you need to consume based on
  /// the **number of characters** or wish to guarantee that a panic (forced termination) does not occur, use
  /// [`advance_chars`](Self::advance_chars) instead.
  ///
  /// # Panics
  ///
  /// If `len` extends beyond the end of the remaining data, or if it falls within a character rather than at a
  /// character boundary.
  pub fn advance(&mut self, len: usize) {
    let consumed = &self.buf[self.start..self.start + len];
    for c in consumed.chars() {
      self.at.advance(c);
    }
    self.start += len;
    self.compact();
  }

  /// Consumes the first `count` characters of [`remainder`](Self::remainder) and advances the read position past them.
  ///
  /// If `count` exceeds the number of remaining characters, all remaining characters are consumed. Unlike
  /// [`advance`](Self::advance), this method does not panic.
  pub fn advance_chars(&mut self, count: usize) {
    let len = self.remainder().char_indices().nth(count).map_or(self.remainder().len(), |(i, _)| i);
    self.advance(len);
  }

  /// Removes the consumed text from the buffer and move the remaining part to the beginning.
  fn compact(&mut self) {
    if self.start >= COMPACT_THRESHOLD && self.start * 2 >= self.buf.len() {
      self.buf.drain(..self.start);
      self.start = 0;
    }
  }

  /// Current read position: the location within the entity where [`remainder`](Self::remainder) begins.
  #[must_use]
  pub fn location(&self) -> Location {
    self.at.clone()
  }

  /// The location of the character at byte index `index` in `buf`. If an index beyond the current read position is
  /// specified, the read position itself is returned, as the preceding text has already been lost.
  fn location_of(&self, index: usize) -> Location {
    let mut at = self.at.clone();
    for c in self.buf[self.start..index.max(self.start)].chars() {
      at.advance(c);
    }
    at
  }

  /// The system identifier of this entity (if known).
  #[must_use]
  pub fn system_id(&self) -> Option<&Arc<str>> {
    self.at.system_id.as_ref()
  }

  /// The public identifier of this entity (if known).
  #[must_use]
  pub fn public_id(&self) -> Option<&Arc<str>> {
    self.at.public_id.as_ref()
  }

  /// The encoding in use, or `None` if undetermined.
  #[must_use]
  pub fn encoding(&self) -> Option<&str> {
    self.encoding.as_deref()
  }

  /// Whether the stream is ready to accept input—that is, whether a call to [`feed`](Self::feed) setting `last` has
  /// not yet been made.
  #[must_use]
  pub fn can_be_fed(&self) -> bool {
    !self.finished
  }

  /// True when the entity has been exhausted (the last byte has been supplied and [`remainder`](Self::remainder) has
  /// become empty).
  #[must_use]
  pub fn is_fully_read(&self) -> bool {
    self.finished && self.start == self.buf.len()
  }

  /// The number of characters decoded so far, regardless of whether they were consumed.
  ///
  /// Limits on entity expansion are expressed in this unit. This ensures that the cost of entity expansion attacks
  /// (such as billion laughs attacks or exponential expansion) is counted based on the number of decoded characters.
  #[must_use]
  pub fn chars_decoded(&self) -> u64 {
    self.chars_appended
  }

  /// The number of input bytes consumed by the decoder so far.
  ///
  /// Each byte of an entity is counted once when it is decoded into text. Bytes held back as part of an incomplete
  /// trailing sequence are counted once the remaining data arrives and the character is decoded. A BOM removed during
  /// detection is not passed to the decoder and thus is not counted; likewise, streams constructed via
  /// [`from_text`](Self::from_text) do not decode any bytes.
  #[must_use]
  pub fn bytes_decoded(&self) -> u64 {
    self.bytes_decoded
  }
}

/// A placeholder decoder for a stream constructed from unencoded text.
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
