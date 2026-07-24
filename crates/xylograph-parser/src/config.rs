//! Runtime configuration for the parser.
//!
//! Beyond XML 1.0 itself, the parser can be asked to compute base URIs (XML Base) and to type
//! `xml:id` attributes. Each is behind a Cargo feature that compiles the machinery in; the flag
//! here then turns it on or off for a given parser. A flag defaults to on, since enabling the
//! feature is the deliberate act — turn it off to parse a document without that layer while
//! keeping the code available. A flag set without its feature compiled in has no effect.

/// Optional processing the parser performs beyond XML 1.0, each gated by a Cargo feature.
///
/// # Examples
///
/// ```
/// use xylograph_parser::ParserConfig;
///
/// // Default: every compiled layer is on.
/// let config = ParserConfig::default();
/// // Or start from nothing and opt in.
/// let _ = ParserConfig::none();
/// # let _ = config;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParserConfig {
  /// Compute a base URI for every element from `xml:base` and the entity's system identifier
  /// (XML Base). Read it with [`Parser::base_uri`](crate::Parser::base_uri). Requires the
  /// `xml-base` feature; inert without it.
  pub xml_base: bool,
  /// Treat `xml:id` as an ID-typed attribute, normalizing its value as a tokenized ID. Read it
  /// with [`Parser::xml_id`](crate::Parser::xml_id). Requires the `xml-id` feature; inert
  /// without it.
  pub xml_id: bool,
}

impl Default for ParserConfig {
  fn default() -> Self {
    Self { xml_base: true, xml_id: true }
  }
}

impl ParserConfig {
  /// A configuration with every optional layer off.
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
