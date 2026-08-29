//! Runtime configuration for the XML parser.

/// Optional behavior performed by the parser for features beyond XML 1.0.
///
/// In addition to XML 1.0 itself, the XML parser's runtime configurations allows you to specify a base URI (XML Base)
/// and type annotations for the `xml:id` attribute to the parser. These features are able to be chosen to include
/// during the build using Cargo. The `config` also allows you to specify whether to enable those features, which are
/// already enabled in the build, for a specific parser.　Since enabling these features during the build is intentional
/// action, the default value for features enabled during the build is "on." Although the build remains enable, if you
/// want to parse documents without using that feature, turn this flag off. If the corresponding feature has not been
/// built, setting this flag will have no effect.
///
/// # Examples
///
/// ```
/// use xylogue_parser::ParserConfig;
///
/// // Default: every compiled feature is on.
/// let config = ParserConfig::default();
/// // Or start from nothing and opt in.
/// let _ = ParserConfig::none();
/// # let _ = config;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParserConfig {
  /// Compute a base URI for each element using `xml:base` and the system identifiers for entities (XML Base). This
  /// allows you to retrieve it using [`Parser::base_uri`](crate::Parser::base_uri).
  pub xml_base: bool,
  /// Treat `xml:id` as an ID-typed attribute and normalize its value as a tokenized ID. This allows you to read using
  /// [`Parser::xml_id`](crate::Parser::xml_id).
  pub xml_id: bool,
}

impl Default for ParserConfig {
  /// Builds a configuration that sets all features enabled during the compile to "on."
  fn default() -> Self {
    Self::none().with_xml_base(cfg!(feature = "xml-base")).with_xml_id(cfg!(feature = "xml-id"))
  }
}

impl ParserConfig {
  /// A configuration with every optional feature off.
  #[must_use]
  pub const fn none() -> Self {
    Self { xml_base: false, xml_id: false }
  }

  /// Turns base URI computation (XML Base) on or off.
  #[must_use]
  pub const fn with_xml_base(mut self, on: bool) -> Self {
    self.xml_base = on;
    self
  }

  /// Turns `xml:id` typing on or off.
  #[must_use]
  pub const fn with_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }
}

/// The maximum number of bytes enforced for individual tokens during parsing. This limits the size of a single markup
/// token, so that a malformed or hostile stream cannot make one token buffer without bound. Here, *bytes* refers to the
/// UTF-8 byte length reported by `str::len()`. Each field is `Some(n)` for a limit of `n` bytes, or `None` for no
/// limit.
///
/// Each markup token is buffered whole until its terminating characters, so a bound caps that buffer. The defaults (see
/// [`Bounds::default`]) are generous enough for real documents yet tight enough to stop a single token from growing
/// indefinitely; [`Bounds::unlimited`] removes every limit, for input that is known to be trustworthy. When a token
/// exceeds its limit, the parser rejects the input as ill-formed.
///
/// Contiguous character data (Text) is not bounded here: it is streamed fragment by fragment (see
/// [`text_fragment_len`](Self::text_fragment_len)) and never buffered whole.
///
/// # Examples
///
/// ```
/// use xylogue_parser::Bounds;
///
/// // The defaults bound each token kind.
/// assert_eq!(Bounds::default().max_comment, Some(1024 * 1024));
/// // Start from no limits for trusted input, then opt into the ones you want.
/// let bounds = Bounds::unlimited().with_max_tag(64 * 1024);
/// assert_eq!((bounds.max_tag, bounds.max_comment), (Some(64 * 1024), None));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
  /// Maximum bytes in a entity reference `&name;` or character reference `&#...;`.
  pub max_reference: Option<usize>,
  /// Maximum bytes in a comment, `<!-- ... -->`.
  pub max_comment: Option<usize>,
  /// Maximum bytes in a CDATA section, `<![CDATA[ ... ]]>`.
  pub max_cdata: Option<usize>,
  /// Maximum bytes in a processing instruction, `<?target ... ?>`.
  pub max_pi: Option<usize>,
  /// Maximum bytes in a start or end tag include all attributes, `<name ...>` or `</name>`.
  pub max_tag: Option<usize>,
  /// Maximum bytes in a document type declaration, `<!DOCTYPE ... >`.
  pub max_doctype: Option<usize>,

  /// The maximum UTF-8 byte length before text is fragmented, and it has a default `8 × 1024`. When the text reaches
  /// this length before detecting a `<` or `&`, it is output as a fragmented text without waiting for those characters.
  ///
  /// As with the SAX `characters` handler, a single text run may be split into any number of fragments and read out.
  /// To prevent very long (or infinite) runs from accumulating indefinitely in the read buffer, this scan fragments
  /// and outputs any run that reaches this size. Therefore, these is no guarantee that a text node will be the maximum
  /// size. If the caller requires the largest single run, it should concatenate adjacent fragments.
  ///
  pub text_fragment_len: usize,
}

impl Default for Bounds {
  /// Protective defaults: generous for real documents, tight enough to stop any single token from buffering without
  /// bound. Text is fragmented at `8 * 1024` bytes.
  fn default() -> Self {
    Self {
      max_reference: Some(1024),
      max_comment: Some(1024 * 1024),
      max_cdata: Some(16 * 1024 * 1024),
      max_pi: Some(64 * 1024),
      max_tag: Some(1024 * 1024),
      max_doctype: Some(256 * 1024),
      text_fragment_len: 8 * 1024,
    }
  }
}

impl Bounds {
  /// Bounds with every per-token limit removed, for input that is known to be trustworthy. Text is still fragmented at
  /// the default [`text_fragment_len`](Self::text_fragment_len), since streaming text is not a reject limit.
  #[must_use]
  pub const fn unlimited() -> Self {
    Self {
      max_reference: None,
      max_comment: None,
      max_cdata: None,
      max_pi: None,
      max_tag: None,
      max_doctype: None,
      text_fragment_len: 8 * 1024,
    }
  }

  /// Returns a copy with [`text_fragment_len`](Self::text_fragment_len) set.
  #[must_use]
  pub const fn with_text_fragment_len(mut self, bytes: usize) -> Self {
    self.text_fragment_len = bytes;
    self
  }

  /// Returns a copy with [`max_reference`](Self::max_reference) set.
  #[must_use]
  pub const fn with_max_reference(mut self, bytes: usize) -> Self {
    self.max_reference = Some(bytes);
    self
  }

  /// Returns a copy with [`max_comment`](Self::max_comment) set.
  #[must_use]
  pub const fn with_max_comment(mut self, bytes: usize) -> Self {
    self.max_comment = Some(bytes);
    self
  }

  /// Returns a copy with [`max_cdata`](Self::max_cdata) set.
  #[must_use]
  pub const fn with_max_cdata(mut self, bytes: usize) -> Self {
    self.max_cdata = Some(bytes);
    self
  }

  /// Returns a copy with [`max_pi`](Self::max_pi) set.
  #[must_use]
  pub const fn with_max_pi(mut self, bytes: usize) -> Self {
    self.max_pi = Some(bytes);
    self
  }

  /// Returns a copy with [`max_tag`](Self::max_tag) set.
  #[must_use]
  pub const fn with_max_tag(mut self, bytes: usize) -> Self {
    self.max_tag = Some(bytes);
    self
  }

  /// Returns a copy with [`max_doctype`](Self::max_doctype) set.
  #[must_use]
  pub const fn with_max_doctype(mut self, bytes: usize) -> Self {
    self.max_doctype = Some(bytes);
    self
  }
}
