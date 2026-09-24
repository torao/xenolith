//! The definition of the character written at points where a line break occurs in the output.

/// The characters written at points where a line break occurs in the output, such as immediately after the XML
/// declaration.
///
/// Since XML processors normalize line endings to the line feed character (LF) (XML 1.0 §2.11), the consumer of the
/// output treats [`Cr`](Self::Cr) and [`CrLf`](Self::CrLf) the same as [`Lf`](Self::Lf). [`Space`](Self::Space) leaves
/// the output as a single line without inserting a line break.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineBreak {
  /// A carriage return (U+000D).
  Cr,
  /// A carriage return followed by a line feed (U+000D U+000A).
  CrLf,
  /// A line feed (U+000A). This is the default.
  #[default]
  Lf,
  /// A single space (U+0020).
  Space,
}

impl LineBreak {
  /// Returns the characters representing this line break.
  #[must_use]
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Cr => "\r",
      Self::CrLf => "\r\n",
      Self::Lf => "\n",
      Self::Space => " ",
    }
  }
}
