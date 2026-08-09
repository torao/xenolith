//! `xylogue` — the command line of the library of the same name.
//!
//! Four things one wants to do with XML from a shell: run a stylesheet over a document, ask an
//! XPath question, check a document against its DTD, and write one out readably.
//!
//! Every subcommand reads from a named file or, with no file, from standard input, and writes to
//! standard output unless told otherwise — so they compose with everything else in a pipeline.
//!
//! ```text
//! xylogue transform --param year=2026 report.xsl data.xml
//! xylogue xpath '//name' data.xml
//! xylogue validate data.xml
//! xylogue format --indent 4 data.xml
//! ```
//!
//! # What it exits with
//!
//! `0` when it did what was asked. `1` when the document says no — invalid, or an expression
//! that found nothing where the caller asked for a node. `2` when the request itself could not
//! be carried out: a file that is not there, XML that is not well-formed, a stylesheet that is
//! not one. The two are kept apart so a script can tell "the answer is no" from "I could not
//! ask".

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use xylogue::dom::build;
use xylogue::serialize::Serializer;
use xylogue::transform::{Source, Transformer};
use xylogue::xdm::{DomModel, DomNode, Model, NodeKind};
use xylogue::xpath::{Value, XPath};
use xylogue::xslt::Loader;

/// What the process exits with when the document answers no.
const ANSWERED_NO: u8 = 1;
/// What the process exits with when the request could not be carried out at all.
const COULD_NOT: u8 = 2;

#[derive(Debug, Parser)]
#[command(name = "xylogue", version, about = "Command-line XML tools", long_about = None)]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
  /// Run an XSLT 1.0 stylesheet over a document.
  Transform {
    /// The stylesheet.
    stylesheet: PathBuf,
    /// The document; standard input if left out.
    input: Option<PathBuf>,
    /// A value for a top-level xsl:param, as name=value. May be given more than once.
    #[arg(short, long = "param", value_name = "NAME=VALUE")]
    parameters: Vec<String>,
    /// Where to write the result; standard output if left out.
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Evaluate an XPath 1.0 expression over a document.
  Xpath {
    /// The expression.
    expression: String,
    /// The document; standard input if left out.
    input: Option<PathBuf>,
    /// A namespace binding the expression may use, as prefix=uri. May be given more than once.
    #[arg(short, long = "namespace", value_name = "PREFIX=URI")]
    namespaces: Vec<String>,
    /// Exit with 1 when the expression selects nothing or is false.
    #[arg(long)]
    fail_on_empty: bool,
  },
  /// Check a document against the DTD it declares.
  Validate {
    /// The documents; standard input if none are named.
    inputs: Vec<PathBuf>,
  },
  /// Write a document out, indented.
  Format {
    /// The document; standard input if left out.
    input: Option<PathBuf>,
    /// How many spaces one level of indentation is.
    #[arg(short, long, default_value_t = 2)]
    indent: usize,
    /// Where to write; standard output if left out.
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  match run(cli.command) {
    Ok(code) => code,
    Err(message) => {
      // To standard error, so that a pipeline reading standard output is not fed a diagnostic.
      eprintln!("xylogue: {message}");
      ExitCode::from(COULD_NOT)
    }
  }
}

/// Carries out one subcommand, giving what to exit with.
fn run(command: Command) -> Result<ExitCode, String> {
  match command {
    Command::Transform { stylesheet, input, parameters, output } => {
      transform(&stylesheet, input.as_deref(), &parameters, output.as_deref())
    }
    Command::Xpath { expression, input, namespaces, fail_on_empty } => {
      xpath(&expression, input.as_deref(), &namespaces, fail_on_empty)
    }
    Command::Validate { inputs } => validate(&inputs),
    Command::Format { input, indent, output } => format(input.as_deref(), indent, output.as_deref()),
  }
}

// --- transform -----------------------------------------------------------------------------------

/// Fetches what a stylesheet names from the filesystem, relative to where it was found.
struct Files;

impl Loader for Files {
  fn load(&mut self, uri: &str) -> xylogue::Result<Vec<u8>> {
    let path = path_of(uri);
    fs::read(&path).map_err(|error| xylogue::Error::xslt(format!("{}: {error}", display(Path::new(&path)))))
  }
}

/// The filesystem path a `file:` URI names.
///
/// The leading slash after the authority is part of the path on a system with one root, and is
/// not on a system whose paths begin with a drive letter — `file:///tmp/a.xsl` is `/tmp/a.xsl`,
/// `file:///C:/tmp/a.xsl` is `C:/tmp/a.xsl`. Getting this wrong turns an absolute path into a
/// relative one, which then resolves against the working directory and finds nothing, so both
/// forms are handled here rather than by whichever one the author's machine happens to use.
///
/// Anything that is not a `file:` URI is handed back as it stands, and fails when it is opened —
/// this tool fetches from the filesystem and nowhere else.
fn path_of(uri: &str) -> String {
  let rest = uri.strip_prefix("file://").unwrap_or(uri);
  let rest = match rest.strip_prefix('/') {
    // `/C:/…`: the slash belongs to the URI, not to the path.
    Some(after) if is_drive_letter(after) => after,
    _ => rest,
  };
  percent_decode(rest)
}

/// Whether a path begins with a drive letter and a colon, as `C:/tmp` does.
fn is_drive_letter(path: &str) -> bool {
  let mut characters = path.chars();
  matches!((characters.next(), characters.next()), (Some(letter), Some(':')) if letter.is_ascii_alphabetic())
}

/// Undoes the percent-escaping a URI puts on the characters a path may legitimately contain.
fn percent_decode(text: &str) -> String {
  let bytes = text.as_bytes();
  let mut decoded = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    // A `%` not followed by two hex digits is not an escape, and is kept as it was written.
    match (bytes[i], bytes.get(i + 1).zip(bytes.get(i + 2))) {
      (b'%', Some((high, low))) => match (hex(*high), hex(*low)) {
        (Some(high), Some(low)) => {
          decoded.push(high * 16 + low);
          i += 3;
        }
        _ => {
          decoded.push(bytes[i]);
          i += 1;
        }
      },
      _ => {
        decoded.push(bytes[i]);
        i += 1;
      }
    }
  }
  // An escape naming a byte that is not UTF-8 leaves the name unusable; the original is a better
  // thing to report than a replacement character.
  String::from_utf8(decoded).unwrap_or_else(|_| text.to_owned())
}

/// One hexadecimal digit as its value.
fn hex(byte: u8) -> Option<u8> {
  match byte {
    b'0'..=b'9' => Some(byte - b'0'),
    b'a'..=b'f' => Some(byte - b'a' + 10),
    b'A'..=b'F' => Some(byte - b'A' + 10),
    _ => None,
  }
}

fn transform(
  stylesheet: &Path,
  input: Option<&Path>,
  parameters: &[String],
  output: Option<&Path>,
) -> Result<ExitCode, String> {
  let source = read(Some(stylesheet))?;
  let mut transformer =
    Transformer::compile_with(Source::bytes(&source).with_system_id(&system_id(stylesheet)), &mut Files)
      .map_err(|error| format!("{}: {}", display(stylesheet), error.message()))?
      .with_resolver(|| Box::new(Files));

  for parameter in parameters {
    let Some((name, value)) = parameter.split_once('=') else {
      return Err(format!("a parameter is written name=value, not {parameter:?}"));
    };
    transformer = transformer.with_parameter(name, value);
  }

  let document = read(input)?;
  let mut source = Source::bytes(&document);
  if let Some(input) = input {
    source = source.with_system_id(&system_id(input));
  }
  let result = transformer.transform(source).map_err(|error| error.message().to_owned())?;

  // A message is for whoever is watching, so it goes to standard error and leaves the result
  // alone on standard output.
  for message in result.messages() {
    eprintln!("xylogue: {message}");
  }
  write(output, result.bytes())?;
  Ok(ExitCode::SUCCESS)
}

// --- xpath ---------------------------------------------------------------------------------------

fn xpath(expression: &str, input: Option<&Path>, bindings: &[String], fail_on_empty: bool) -> Result<ExitCode, String> {
  let mut query = XPath::new();
  for binding in bindings {
    let Some((prefix, uri)) = binding.split_once('=') else {
      return Err(format!("a namespace is written prefix=uri, not {binding:?}"));
    };
    query = query.with_namespace(prefix, uri);
  }

  let source = read(input)?;
  let document = build::parse(source.as_slice()).map_err(|error| where_it_was(input, &error))?;
  let model = DomModel::new(&document);
  let compiled = query.compile(expression).map_err(|error| error.message().to_owned())?;
  let value = compiled.evaluate(&model, model.root_node()).map_err(|error| error.message().to_owned())?;

  // A node-set prints one node per line, since that is what a shell can loop over; the other
  // three types print as XPath's string() would render them. An element prints as its own
  // markup — asking for `//item` and being handed the text of each is rarely what was meant.
  let empty = match &value {
    Value::NodeSet(nodes) => {
      let mut out = io::stdout().lock();
      for node in nodes.iter().copied() {
        let written = match node {
          DomNode::Tree { node: id, .. } if model.kind(node) == NodeKind::Element => {
            Serializer::new().to_string(&document, id)
          }
          _ => model.string_value(node),
        };
        writeln!(out, "{written}").map_err(|error| error.to_string())?;
      }
      nodes.is_empty()
    }
    other => {
      println!("{}", other.string(&model));
      matches!(other, Value::Boolean(false))
    }
  };

  if fail_on_empty && empty {
    return Ok(ExitCode::from(ANSWERED_NO));
  }
  Ok(ExitCode::SUCCESS)
}

// --- validate ------------------------------------------------------------------------------------

fn validate(inputs: &[PathBuf]) -> Result<ExitCode, String> {
  let named: Vec<Option<&Path>> =
    if inputs.is_empty() { vec![None] } else { inputs.iter().map(|path| Some(path.as_path())).collect() };

  let mut all_valid = true;
  for input in named {
    let source = read(input)?;
    let report = xylogue_validate::validate(source.as_slice()).map_err(|error| where_it_was(input, &error))?;
    let name = input.map_or_else(|| "<stdin>".to_owned(), display);

    if !report.had_dtd() {
      // Not invalid — there was nothing to be valid against, which is a different answer.
      println!("{name}: no DOCTYPE, so there is nothing to validate against");
      all_valid = false;
      continue;
    }
    if report.errors().is_empty() {
      println!("{name}: valid");
      continue;
    }
    all_valid = false;
    println!("{name}: {} validity error(s)", report.errors().len());
    for error in report.errors() {
      let at = error.location();
      println!("  {}:{}: {}", at.line, at.column, error.message());
    }
  }
  Ok(if all_valid { ExitCode::SUCCESS } else { ExitCode::from(ANSWERED_NO) })
}

// --- format --------------------------------------------------------------------------------------

fn format(input: Option<&Path>, indent: usize, output: Option<&Path>) -> Result<ExitCode, String> {
  let source = read(input)?;
  let document = build::parse(source.as_slice()).map_err(|error| where_it_was(input, &error))?;
  let Some(root) = document.document_element() else {
    return Err("the document has no element to write".to_owned());
  };
  let written = Serializer::new().with_indent(&" ".repeat(indent)).to_string(&document, root);
  write(output, written.as_bytes())?;
  Ok(ExitCode::SUCCESS)
}

// --- reading and writing -------------------------------------------------------------------------

/// Reads a named file, or standard input when nothing is named.
fn read(path: Option<&Path>) -> Result<Vec<u8>, String> {
  match path {
    Some(path) => fs::read(path).map_err(|error| format!("{}: {error}", display(path))),
    None => {
      let mut bytes = Vec::new();
      io::stdin().read_to_end(&mut bytes).map_err(|error| format!("standard input: {error}"))?;
      Ok(bytes)
    }
  }
}

/// Writes to a named file, or to standard output when nothing is named.
fn write(path: Option<&Path>, bytes: &[u8]) -> Result<(), String> {
  match path {
    Some(path) => fs::write(path, bytes).map_err(|error| format!("{}: {error}", display(path))),
    None => io::stdout().write_all(bytes).map_err(|error| format!("standard output: {error}")),
  }
}

/// A path as text, whatever it holds.
fn display(path: &Path) -> String {
  path.display().to_string()
}

/// The system identifier of a file, which relative references are resolved against.
fn system_id(path: &Path) -> String {
  let absolute = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
  // A Windows path uses backslashes and may begin with a verbatim prefix; neither belongs in a
  // URI.
  let written = absolute.display().to_string().replace('\\', "/");
  let written = written.strip_prefix("//?/").unwrap_or(&written);
  format!("file:///{}", percent_encode(written.trim_start_matches('/')))
}

/// Escapes what a path may hold and a URI may not, so that the result survives being resolved
/// against and read back by [`path_of`].
fn percent_encode(path: &str) -> String {
  let mut encoded = String::with_capacity(path.len());
  for byte in path.bytes() {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
        encoded.push(byte as char);
      }
      _ => encoded.push_str(&format!("%{byte:02X}")),
    }
  }
  encoded
}

/// An error, said with the name of what it was reading.
fn where_it_was(input: Option<&Path>, error: &xylogue::Error) -> String {
  let name = input.map_or_else(|| "<stdin>".to_owned(), display);
  format!("{name}: {}", error.message())
}

#[cfg(test)]
mod tests {
  use super::{path_of, percent_encode};

  #[test]
  fn a_rooted_path_keeps_its_leading_slash() {
    // The bug this exists for: dropping it makes the path relative, and it then resolves
    // against the working directory instead of naming the file the stylesheet meant.
    assert_eq!(path_of("file:///tmp/styles/base.xsl"), "/tmp/styles/base.xsl");
  }

  #[test]
  fn a_drive_letter_loses_the_slash_the_uri_added() {
    assert_eq!(path_of("file:///C:/styles/base.xsl"), "C:/styles/base.xsl");
    assert_eq!(path_of("file:///c:/styles/base.xsl"), "c:/styles/base.xsl");
  }

  #[test]
  fn what_is_not_a_file_uri_is_left_alone() {
    // It fails when opened, saying what it was — better than being silently mangled first.
    assert_eq!(path_of("https://example.com/base.xsl"), "https://example.com/base.xsl");
    assert_eq!(path_of("base.xsl"), "base.xsl");
  }

  #[test]
  fn an_escape_becomes_the_character_it_names() {
    assert_eq!(path_of("file:///tmp/my%20styles/base.xsl"), "/tmp/my styles/base.xsl");
    assert_eq!(path_of("file:///tmp/%E6%97%A5%E6%9C%AC/base.xsl"), "/tmp/日本/base.xsl");
  }

  #[test]
  fn a_percent_that_is_not_an_escape_stays_as_written() {
    assert_eq!(path_of("file:///tmp/100%/base.xsl"), "/tmp/100%/base.xsl");
    assert_eq!(path_of("file:///tmp/%zz/base.xsl"), "/tmp/%zz/base.xsl");
  }

  #[test]
  fn a_path_survives_being_written_as_a_uri_and_read_back() {
    for path in ["/tmp/styles/base.xsl", "/tmp/my styles/base.xsl", "/tmp/日本/base.xsl", "/tmp/100%/base.xsl"] {
      let uri = format!("file:///{}", percent_encode(path.trim_start_matches('/')));
      assert_eq!(path_of(&uri), path, "{uri}");
    }
  }

  #[test]
  fn a_separator_and_a_drive_letter_are_not_escaped() {
    // Escaping either would make the URI name one long segment rather than a path.
    assert_eq!(percent_encode("C:/styles/base.xsl"), "C:/styles/base.xsl");
  }
}
