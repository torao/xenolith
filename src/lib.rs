use std::fmt::Display;

use thiserror::Error;

pub mod io;
pub mod xml;

#[cfg(test)]
pub mod test;

/// A structure that indicates a position within a paticular file or stream by line and column numbers.
///
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
  /// The line number indicates the number of line feeds (CR, LF, or CRLF sequences) that have appeared from the
  /// beginning of the stream to that position. 0 means the first line.
  ///
  pub line_number: u64,

  /// The column number indicates the number of characters from the beginning of the line to that position. 0 means the
  /// beginning of the line.
  ///
  pub column_number: u64,
}

impl Location {
  /// A name to refer to the location within a node parsed.
  /// `Node.get_user_data(Location::USERDATA_NAME)`
  pub const USERDATA_NAME: &'static str = "xenolith.xml.parser.location";

  /// Constructs location for the specified line/column.
  pub fn new(line_number: u64, column_number: u64) -> Location {
    return Location { line_number, column_number };
  }
}

impl Display for Location {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "({},{})", self.line_number + 1, self.column_number + 1)
  }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
  #[error(transparent)]
  W3C(#[from] crate::xml::w3c::dom::DOMException),
  #[error(transparent)]
  IO(#[from] std::io::Error),
}
