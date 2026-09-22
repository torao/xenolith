//! The parser settings encapsulated in [`ParserConfig`] include specifications such as which recommendations other
//! than XML 1.0 to apply, the extent of processing the parser should perform on the document, and how character data
//! should be reported.
//!
//! Each group of settings is utilized by a specific component of the parser: [`Extensions`] and [`DocumentLimits`] are
//! used by the parser core, [`TokenLimits`] by the scanner that detects token boundaries, and [`EntityLimits`] by the
//! stack that manages entities during parsing. Applications configure all these settings by passing a single
//! [`ParserConfig`] instance to the [`Parser`](crate::io::Parser) or the source that drives it.

/// The settings of a parser.
///
/// Default values are provided for each group, so applications start with [`ParserConfig::default`] and modify only
/// the necessary fields.
///
/// # Examples
///
/// ```
/// use xenolith::io::{Limits, ParserConfig};
///
/// // Trusted input: every limit removed.
/// let mut config = ParserConfig::default();
/// config.limits = Limits::unlimited();
///
/// // One limit tightened, and `xml:id` not applied.
/// let mut config = ParserConfig::default();
/// config.limits.entities.max_expansions = Some(10_000);
/// config.extensions.xml_id = false;
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParserConfig {
  /// Recommendations other than XML 1.0 applied by the parser.
  pub extensions: Extensions,

  /// To what size should the parser be allowed to process documents.
  pub limits: Limits,

  /// The length in bytes of a contiguous sequence of characters being parsed when it is reported as a "fragment",
  /// without waiting for the `<` or `&` characters that mark its end. The default value is `8 * 1024`.
  ///
  /// Even for extremely long sequences of data, the entire sequence is not held in memory at once. Similar to the SAX
  /// `characters` method, a single contiguous sequence of characters may be reported in multiple fragments, with the
  /// split points depending on the timing of input arrival. Callers requiring the complete sequence must concatenate
  /// the reported adjacent fragments. This is merely an internal buffering threshold; reaching this length does not
  /// mean the data is rejected.
  pub text_fragment_len: usize,
}

impl Default for ParserConfig {
  /// Enable extensions, apply default limits, and fragment text at 8 KiB.
  fn default() -> Self {
    Self { extensions: Extensions::default(), limits: Limits::default(), text_fragment_len: 8 * 1024 }
  }
}

/// Which recommendations beyond XML 1.0 the parser applies: XML Base and `xml:id`.
///
/// Both are on by default, and either can be turned off for a parser that does not need it. They are runtime settings
/// only; no Cargo feature enables or removes them.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extensions {
  /// Compute the base URI of each element (XML Base): the system identifier of the entity it is in, overridden by the
  /// `xml:base` attributes on it and its ancestors. [`Parser::base_uri`](crate::io::Parser::base_uri) reports it, and
  /// reports `None` when this is off.
  pub xml_base: bool,
  /// Normalize the value of an `xml:id` attribute as a tokenized `ID` value even when no DTD declares it (xml:id §4),
  /// and report it through [`Parser::xml_id`](crate::io::Parser::xml_id), which reports `None` when this is off.
  /// Whether the value is a valid `NCName` and unique is checked by validation, not here.
  pub xml_id: bool,
}

impl Default for Extensions {
  /// Both XML Base and `xml:id` on.
  fn default() -> Self {
    Self { xml_base: true, xml_id: true }
  }
}

impl Extensions {
  /// Both XML Base and `xml:id` off: XML 1.0 alone.
  #[must_use]
  pub const fn none() -> Self {
    Self { xml_base: false, xml_id: false }
  }
}

/// Defines the extent of processing permitted for a document, aiming to prevent the exhaustion of memory or processing
/// time caused by malformed or malicious input and instead cause parsing to fail.
///
/// For each field, a limit `n` is specified as `Some(n)`, while `None` indicates no limit. The default values are
/// appropriately configured to allow sufficient headroom for legitimate documents while ensuring that typical attacks
/// fail. If an attempt is made to parse a document that exceeds these limits, parsing fails and an error is raised,
/// explicitly identifying the relevant field.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
  /// The size of a single markup token, checked during the scanning of each token.
  pub tokens: TokenLimits,
  /// Entity expansion across the entire document.
  pub entities: EntityLimits,
  /// The structure of the document.
  pub document: DocumentLimits,
}

impl Limits {
  /// All restrictions are lifted for trusted input.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self {
      tokens: TokenLimits::unlimited(),
      entities: EntityLimits::unlimited(),
      document: DocumentLimits::unlimited(),
    }
  }
}

/// The maximum size in UTF-8 bytes that each kind of markup tokens can reach when buffered by the parser.
///
/// Since the parser retains the entire markup token until the closing delimiter is reached, a single token could grow
/// indefinitely without such a limit. Tokens that exceed this limit cause parsing to fail due to a well-formedness
/// error. Note that this limit does not apply to character data; character data is reported in fragments of
/// [`ParserConfig::text_fragment_len`] bytes and is not retained in its entirety.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenLimits {
  /// Maximum entity reference `&name;` or character reference `&#...;` (including delimiters). Default is 1 KiB.
  pub max_reference: Option<usize>,
  /// Maximum comment `<!-- ... -->`. Default is 1 MiB.
  pub max_comment: Option<usize>,
  /// Maximum CDATA section `<![CDATA[ ... ]]>`. Default is 16 MiB.
  pub max_cdata: Option<usize>,
  /// Maximum processing instruction `<?target ... ?>` and maximum XML declaration. Default is 64 KiB.
  pub max_pi: Option<usize>,
  /// Maximum start tag `<name ...>` or end tag `</name>`, including all attributes. Default is 1 MiB.
  pub max_tag: Option<usize>,
  /// Maximum document type declaration `<!DOCTYPE ... >` (including the internal subset). Default is 256 KiB.
  pub max_doctype: Option<usize>,
}

impl Default for TokenLimits {
  fn default() -> Self {
    Self {
      max_reference: Some(1024),
      max_comment: Some(1024 * 1024),
      max_cdata: Some(16 * 1024 * 1024),
      max_pi: Some(64 * 1024),
      max_tag: Some(1024 * 1024),
      max_doctype: Some(256 * 1024),
    }
  }
}

impl TokenLimits {
  /// Remove all token limits.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_reference: None, max_comment: None, max_cdata: None, max_pi: None, max_tag: None, max_doctype: None }
  }
}

/// Indicate the potential extent of entity expansion across the entire document.
///
/// These measures are intended to prevent typical expansion attacks, such as the "Billion Laughs" attack.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityLimits {
  /// The maximum number of entities that can be open simultaneously (including document entities). The default is 64.
  pub max_depth: Option<usize>,
  /// The maximum number of entity expansions within a single document. The default is 100,000.
  pub max_expansions: Option<u32>,
  /// The maximum total number of characters generated by all expansions. The default is 64 Mi characters.
  pub max_expansion_chars: Option<u64>,
}

impl Default for EntityLimits {
  fn default() -> Self {
    Self { max_depth: Some(64), max_expansions: Some(100_000), max_expansion_chars: Some(64 * 1024 * 1024) }
  }
}

impl EntityLimits {
  /// Every entity limit removed.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_depth: None, max_expansions: None, max_expansion_chars: None }
  }
}

/// Limits on the extent to which the document structure can expand.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentLimits {
  /// Element nesting depth; the default limit is 1,024 levels.
  ///
  /// The parser maintains records for each open element without using recursion. However, the process that traverses
  /// the generated tree consumes stack space proportional to that depth.
  pub max_element_depth: Option<usize>,
}

impl Default for DocumentLimits {
  fn default() -> Self {
    Self { max_element_depth: Some(1024) }
  }
}

impl DocumentLimits {
  /// All limitations regarding document parsing are removed.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self { max_element_depth: None }
  }
}
