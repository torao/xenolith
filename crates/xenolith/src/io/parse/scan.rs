//! Identify token boundaries.
//!
//! Scanning is the operation of determining whether the text contains a complete token and, if so, its length. This
//! operation does not consume characters or interpret the detected content. If scanning is interrupted because the
//! text runs out, a "need more input" condition is reported, and once new text becomes available the parser scans
//! again from the start of the same token; the scanner keeps no state between calls.
//!
//! The scanner applies [`TokenLimits`]; this means that processing fails if a token remains incomplete after exceeding
//! the limits, preventing unbounded token buffering.

#[cfg(test)]
mod test;

use crate::chars;
use crate::error::{Error, Result};

use crate::io::parse::TokenKind;
use crate::io::parse::config::TokenLimits;

/// For each [`TokenKind`], the scanner needs to track the name used in error messages and the applicable limits
/// defined in [`TokenLimits`]. These details are internal to the scanner and are used exclusively by it.
///
/// While the scanner detects the types of tokens reported by the parser, there are two distinctions. First, it does
/// not report [`XmlDeclaration`](TokenKind::XmlDeclaration); this is because, lexically, it is a type of
/// "processing instruction" that is distinguished on the parser side. Second, it reports "references"—which are not
/// strictly tokens—as [`Scan::Reference`].
impl TokenKind {
  /// The name of this token type. It is used to determine how error messages refer to tokens of this type.
  fn describe(self) -> &'static str {
    match self {
      TokenKind::Comment => "a comment",
      TokenKind::CData => "a CDATA section",
      TokenKind::ProcessingInstruction | TokenKind::XmlDeclaration => "a processing instruction",
      TokenKind::StartElement => "a start tag",
      TokenKind::EndElement => "an end tag",
      TokenKind::Doctype => "a document type declaration",
      TokenKind::Text => "text",
    }
  }

  /// The byte-based limit for this token type and the name of the [`TokenLimits`] field from which the limit is
  /// derived. "Text" is reported in fragments, so there is no limit.
  fn limit(self, bounds: &TokenLimits) -> (Option<usize>, &'static str) {
    match self {
      TokenKind::Comment => (bounds.max_comment, "max_comment"),
      TokenKind::CData => (bounds.max_cdata, "max_cdata"),
      TokenKind::ProcessingInstruction | TokenKind::XmlDeclaration => (bounds.max_pi, "max_pi"),
      TokenKind::StartElement | TokenKind::EndElement => (bounds.max_tag, "max_tag"),
      TokenKind::Doctype => (bounds.max_doctype, "max_doctype"),
      TokenKind::Text => (None, ""),
    }
  }
}

/// The constructs within content that begin with `<!`: comments, CDATA sections, and document type declarations.
const MARKUP_PREFIXES: [&str; 3] = ["<!--", "<![CDATA[", "<!DOCTYPE"];

/// The result of [`scan`]: complete token, complete reference, or need for further input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scan {
  /// A complete token: its type and its length in bytes, measured from the beginning of the remaining data.
  Found(TokenKind, usize),

  /// A complete reference (`&name;` or `&#...;`): its length in bytes, including delimiters. The parser does not
  /// report this as a token; instead, it expands it into text or opens the entity referenced by the name.
  Reference(usize),

  /// The remaining data ends in the middle of a token. The caller provides further input and re-scans from the same
  /// starting position.
  Pending,
}

impl Scan {
  /// The scan in the [`Pending`](Self::Pending) state is marked as a "well-formedness error" (because the token cannot
  /// terminate within that limit) if `rest` exceeds the limit defined by `bounds` for the given `kind`.
  fn bounded(self, kind: TokenKind, rest: &str, bounds: &TokenLimits) -> Result<Self> {
    let (limit, field) = kind.limit(bounds);
    self.within(limit, field, kind.describe(), rest)
  }

  /// The same applies to references, where a comparison is made against [`TokenLimits::max_reference`].
  fn bounded_reference(self, rest: &str, bounds: &TokenLimits) -> Result<Self> {
    self.within(bounds.max_reference, "max_reference", "a reference", rest)
  }

  /// The pending scan is marked as a well-formedness error if `rest` exceeds `limit` bytes; in this case, the error
  /// message identifies the token as `what` and the field reporting the error as `field`.
  fn within(self, limit: Option<usize>, field: &str, what: &str, rest: &str) -> Result<Self> {
    match (self, limit) {
      (Scan::Pending, Some(max)) if rest.len() > max => Err(Error::well_formedness(format!(
        "{what} longer than {max} bytes was found; increase ParserConfig.limits.tokens.{field} if the input is valid"
      ))),
      _ => Ok(self),
    }
  }
}

/// Identifies the token or reference at the beginning of `rest`, which represents the unprocessed text of the current
/// entity.
///
/// `complete` indicates whether `rest` extends to the end of the entity. If `rest` contains only the beginning of a
/// token, the result is `Scan::Pending`; however, if `complete` is true, this results in a well-formedness error
/// because no further input can complete the token.
///
/// `bounds` constrains the various types of markup tokens, and contiguous sections of text are reported as fragments
/// of `text_fragment_len` bytes.
///
/// # Errors
///
/// Text that cannot constitute the start or end of a token, or tokens exceeding the specified bounds, result in a
/// well-formedness error. An empty `rest` (though the caller would not pass such a value) results in an internal
/// error.
pub(crate) fn scan(rest: &str, complete: bool, bounds: &TokenLimits, text_fragment_len: usize) -> Result<Scan> {
  if rest.is_empty() {
    return Err(Error::internal("scan was called with an empty remainder"));
  }
  // A reference, `&...;`.
  if rest.starts_with('&') {
    return scan_reference(rest, complete)?.bounded_reference(rest, bounds);
  }
  // Text. It has no limit: it is reported in fragments, never buffered whole.
  if !rest.starts_with('<') {
    return scan_text(rest, complete, text_fragment_len);
  }
  match rest.as_bytes().get(1) {
    None => incomplete(TokenKind::StartElement, rest, complete),
    Some(b'?') => {
      let kind = TokenKind::ProcessingInstruction;
      delimited(rest, kind, 2, "?>", complete)?.bounded(kind, rest, bounds)
    }
    Some(b'/') => {
      let kind = TokenKind::EndElement;
      delimited(rest, kind, 2, ">", complete)?.bounded(kind, rest, bounds)
    }
    Some(b'!') => scan_markup_declaration(rest, complete, bounds),
    _ => scan_start_tag(rest, complete)?.bounded(TokenKind::StartElement, rest, bounds),
  }
}

/// Scan the content for any constructs starting with `<!` (comments, CDATA sections, or document type declarations).
fn scan_markup_declaration(rest: &str, complete: bool, bounds: &TokenLimits) -> Result<Scan> {
  // A comment, `<!-- ... -->`.
  if rest.starts_with(MARKUP_PREFIXES[0]) {
    return delimited(rest, TokenKind::Comment, 4, "-->", complete)?.bounded(TokenKind::Comment, rest, bounds);
  }
  // A CDATA section, `<![CDATA[ ... ]]>`.
  if rest.starts_with(MARKUP_PREFIXES[1]) {
    return delimited(rest, TokenKind::CData, 9, "]]>", complete)?.bounded(TokenKind::CData, rest, bounds);
  }
  // A document type declaration, `<!DOCTYPE ... >`.
  if rest.starts_with(MARKUP_PREFIXES[2]) {
    let doctype = match scan_doctype(rest) {
      Some(len) => Scan::Found(TokenKind::Doctype, len),
      None if complete => return Err(Error::well_formedness("the document type declaration is not closed")),
      None => Scan::Pending,
    };
    return doctype.bounded(TokenKind::Doctype, rest, bounds);
  }
  // Too short yet to tell which of them it is, or none of them.
  if !complete && MARKUP_PREFIXES.iter().any(|p| p.starts_with(rest)) {
    Ok(Scan::Pending)
  } else {
    Err(Error::well_formedness(format!("{} is not markup", clip(rest, 10))))
  }
}

/// Scans for a token of type `kind` that ends with the first `terminator`, starting from the `from`-th byte, the
/// search proceeds past the token's starting delimiter.
fn delimited(rest: &str, kind: TokenKind, from: usize, terminator: &str, complete: bool) -> Result<Scan> {
  if rest.len() <= from {
    return incomplete(kind, rest, complete);
  }
  match rest[from..].find(terminator) {
    Some(i) => Ok(Scan::Found(kind, from + i + terminator.len())),
    None if complete => Err(Error::well_formedness(format!(
      "{} is not terminated by {terminator:?}: {}",
      kind.describe(),
      clip(rest, 20)
    ))),
    None => Ok(Scan::Pending),
  }
}

/// Scans for start tags such as `<name ...>` or `<name .../>`. A `>` character inside a quoted attribute value is not
/// treated as the end of that attribute value, whereas a `<` character outside of quotes cannot appear within a tag
/// and thus results in an error.
fn scan_start_tag(rest: &str, complete: bool) -> Result<Scan> {
  let mut quote = None;
  for (i, c) in rest.char_indices().skip(1) {
    if let Some(q) = quote {
      if c == q {
        quote = None;
      }
    } else {
      match c {
        '"' | '\'' => quote = Some(c),
        '>' => return Ok(Scan::Found(TokenKind::StartElement, i + 1)),
        '<' => return Err(Error::well_formedness("'<' may not appear inside a tag")),
        _ => {}
      }
    }
  }
  incomplete(TokenKind::StartElement, rest, complete)
}

/// Scans a `<!DOCTYPE ... >` (including the internal subset) and returns its length. Returns `None` if the string
/// ends prematurely.
///
/// The declaration terminates at the first `>` that appears outside of quotes and outside the internal subset's
/// `[ ... ]`. Parsing of individual declarations within the subset is delegated to the DTD parser. Internal comments
/// and processing instructions are skipped entirely; thus, even if they contain quotes or brackets, such as
/// `<!--doesn't-->`—these are not interpreted as markup.
fn scan_doctype(rest: &str) -> Option<usize> {
  let mut i = 0;
  let mut quote: Option<char> = None;
  let mut depth = 0usize;
  while i < rest.len() {
    let tail = &rest[i..];
    if quote.is_none() {
      // Skip a comment or processing instruction whole, so that its content is not read as markup.
      if let Some(after) = tail.strip_prefix("<!--") {
        i += 4 + after.find("-->").map_or(after.len(), |j| j + 3);
        continue;
      }
      if let Some(after) = tail.strip_prefix("<?") {
        i += 2 + after.find("?>").map_or(after.len(), |j| j + 2);
        continue;
      }
    }
    let c = tail.chars().next()?;
    match (quote, c) {
      (Some(q), c) if c == q => quote = None,
      (Some(_), _) => {}
      (None, '"' | '\'') => quote = Some(c),
      (None, '[') => depth += 1,
      (None, ']') => depth = depth.saturating_sub(1),
      (None, '>') if depth == 0 => return Some(i + 1),
      (None, _) => {}
    }
    i += c.len_utf8();
  }
  None
}

/// Scans the beginning of `rest` for a contiguous sequence of character data that does not start with `<` or `&`.
///
/// This sequence (or "run") of data ends immediately before the next `<` or `&`, or, if `complete` is true, at the
/// end of the entity. If neither delimiter is found, a run of `fragment_len` bytes or longer is reported as a fragment
/// without being buffered, while a shorter run is treated as [`Scan::Pending`] and awaits further input.
fn scan_text(rest: &str, complete: bool, fragment_len: usize) -> Result<Scan> {
  match rest.find(['<', '&']) {
    // The caller never passes a `rest` that begins with `<` or `&`. Reporting a zero-length token instead would stall
    // the parser, so it is an internal error.
    Some(0) => Err(Error::internal("scan_text ran on a text run that begins with '<' or '&'")),
    Some(i) => Ok(Scan::Found(TokenKind::Text, i)),
    // No end in sight: the whole of `rest` at the end of the entity, a fragment once it is long enough, or a wait.
    None if complete => Ok(Scan::Found(TokenKind::Text, rest.len())),
    None if rest.len() >= fragment_len => Ok(text_fragment(rest)),
    None => Ok(Scan::Pending),
  }
}

/// The fragments reported by `rest` are contiguous sections of text that do not contain `<` or `&`.
///
/// Character data cannot contain `]]>` (see XML 1.0 §2.4). The parser detects this sequence. To prevent `]]>` from
/// being split across two fragments and thus overlooked by the parser, any trailing `]` or `]]` are excluded from
/// the fragment and re-scanned along with the subsequent data. If nothing remains after this exclusion (such as when
/// `rest` consists solely of `]` or `]]`), the result is `Scan::Pending`. This is because generating a zero-length
/// fragment would cause the parser to halt.
fn text_fragment(rest: &str) -> Scan {
  let held = rest.bytes().rev().take(2).take_while(|&b| b == b']').count();
  match rest.len() - held {
    0 => Scan::Pending,
    take => Scan::Found(TokenKind::Text, take),
  }
}

/// Scans for the end position of a reference in the format `&...;`.
///
/// This process merely locates the `;` and does not validate the intervening content (name or numeric checks are
/// performed by the parser). An error occurs if `<`, `&`, or a whitespace character appears before the `;`, as these
/// cannot appear within a reference; consequently, the `&` is treated as a literal ampersand rather than the start
/// of a reference.
fn scan_reference(rest: &str, complete: bool) -> Result<Scan> {
  let unterminated = || Error::well_formedness("a reference must end with \";\"");
  for (i, c) in rest.char_indices().skip(1) {
    if c == ';' {
      return Ok(Scan::Reference(i + 1));
    }
    if c == '<' || c == '&' || chars::is_whitespace(c) {
      return Err(unterminated());
    }
  }
  // `rest` ran out before a `;`: wait for more, or fail if the entity ends here.
  if complete { Err(unterminated()) } else { Ok(Scan::Pending) }
}

/// The result for a token of `kind` where `rest` terminates internally is either [`Scan::Pending`] or an error if the
/// entity ends at that position.
fn incomplete(kind: TokenKind, rest: &str, complete: bool) -> Result<Scan> {
  if complete {
    let message = format!("the entity has ended within {}: {}", kind.describe(), clip(rest, 20));
    return Err(Error::well_formedness(message));
  }
  Ok(Scan::Pending)
}

/// The string `s` cited in the error message exceeds `max_chars` characters, it is truncated to the first `max_chars`
/// characters and an ellipsis (`…`) is appended to the end (e.g., `"abc…"`).
fn clip(s: &str, max_chars: usize) -> String {
  match s.char_indices().nth(max_chars) {
    Some((end, _)) => format!("{:?}", format!("{}…", &s[..end])),
    None => format!("{s:?}"),
  }
}
