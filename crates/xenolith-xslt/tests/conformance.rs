//! The OASIS/Xalan XSLT 1.0 conformance suite.
//!
//! The suite is several thousand files and is not vendored here. Point `XSLTCONF` at a copy to
//! run against it:
//!
//! ```text
//! git clone --depth 1 https://github.com/apache/xalan-test.git xslt-conformance
//! XSLTCONF=xslt-conformance cargo test -p xenolith-xslt --test conformance -- --nocapture
//! ```
//!
//! Without it the test prints what it would have needed and passes, so a checkout with no suite
//! is not a checkout that silently tests nothing.
//!
//! # Where the suite comes from
//!
//! The tests are the OASIS XSLT/XPath Conformance TC's, which is what everyone means by "the
//! XSLT 1.0 suite". OASIS's own distribution is no longer downloadable — the TC's document
//! library moved and the old links are gone — so the copy used here is Apache's, which imported
//! the suite into [`apache/xalan-test`](https://github.com/apache/xalan-test) and still
//! maintains it.
//!
//! That copy carries the cases but not the TC's `catalog.xml`: Apache's harness works from the
//! directory layout instead. Both are read here — a directory holding `catalog.xml` is read as
//! the catalogue describes, and one holding `conf` / `conf-gold` / `conferr` is read as Xalan
//! lays it out — so a catalogued copy still works if one turns up.
//!
//! # What a comparison means here
//!
//! A conformance case gives an expected result as a file, and two conforming processors may
//! write the same result tree differently — `<a/>` against `<a></a>`, one quotation mark or the
//! other. Comparing the bytes would count those as failures. So for an XML comparison both sides
//! are parsed and written again by *this* serializer, and the two are compared after that: a
//! difference then is a difference in the tree, not in how it was written. A text comparison is
//! exact, since there is nothing to normalize.
//!
//! Cases whose result is HTML are skipped and counted: HTML is not XML, so neither comparison is
//! honest, and counting them as passes would be worse than not judging them.
//!
//! # The two kinds of case
//!
//! `conf` holds cases that must run and produce a given result; `conferr` holds cases that must
//! be *refused*. They are judged and reported separately, because a processor that refuses
//! everything would otherwise score well on the second kind, and because the headline number
//! ought to mean "this is how much of XSLT works".
//!
//! # Failing the run
//!
//! Set `XSLTCONF_MAX_FAILURES` to the number of failures the run may have before it is a test
//! failure. Without it the report is printed and nothing is asserted, because a threshold nobody
//! has measured is a threshold that means nothing — see `ROADMAP.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use xenolith_dom::build;
use xenolith_parser::Reader;
use xenolith_parser::resolve::{EntityRequest, UriResolver};
use xenolith_xdm::{Documents, DomModel};
use xenolith_xpath::Functions;
use xenolith_xslt::{LoadedDocuments, Loader, OutputMethod, Stylesheet, Transform};

/// How a case's result is to be compared with what it expected.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Compare {
  /// As a tree: both sides through this serializer, then compared.
  Xml,
  /// As text, exactly.
  Text,
  /// A comparison this harness cannot make honestly, and the name the suite gave it.
  Cannot(String),
  /// Whatever the stylesheet's own `xsl:output method` turns out to ask for, which is only
  /// known once it has been compiled.
  ByOutputMethod,
}

/// What a case expects to happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expectation {
  /// It runs, and produces the result file beside it.
  Runs,
  /// It is refused — the suite's error cases.
  IsRefused,
}

/// One case of the suite.
#[derive(Debug)]
struct Case {
  name: String,
  expectation: Expectation,
  compare: Compare,
  stylesheet: PathBuf,
  data: Option<PathBuf>,
  expected: Option<PathBuf>,
}

/// Locates the suite and works out which of the two layouts it is in.
fn suite() -> Option<PathBuf> {
  let root = PathBuf::from(std::env::var_os("XSLTCONF")?);
  if root.join("catalog.xml").exists() || xalan_root(&root).is_some() {
    return Some(root);
  }
  eprintln!("XSLTCONF is set to {}, which holds neither a catalog.xml nor a conf/conf-gold pair", root.display());
  None
}

/// The directory the Xalan layout hangs from, whether `XSLTCONF` points at the checkout or the
/// `tests` directory inside it.
fn xalan_root(root: &Path) -> Option<PathBuf> {
  [root.to_path_buf(), root.join("tests")]
    .into_iter()
    .find(|candidate| candidate.join("conf").is_dir() && candidate.join("conf-gold").is_dir())
}

/// Reads whichever layout is there into the cases it describes.
fn cases(root: &Path) -> Vec<Case> {
  if root.join("catalog.xml").exists() {
    return catalogued_cases(root);
  }
  match xalan_root(root) {
    Some(tests) => xalan_cases(&tests),
    None => Vec::new(),
  }
}

// --- the Xalan layout ------------------------------------------------------------------------

/// Reads the layout Apache's copy uses.
///
/// A case is identified by its *expected result*: `conf-gold/<group>/<name>.out` is the answer to
/// `conf/<group>/<name>.xsl` over `conf/<group>/<name>.xml`. Going from the gold files rather
/// than from the stylesheets matters — `conf` also holds the modules the cases import, which are
/// not cases and would each be counted as one that produces nothing.
fn xalan_cases(tests: &Path) -> Vec<Case> {
  let mut cases = Vec::new();
  for (group, gold) in files_under(&tests.join("conf-gold"), "out") {
    let Some(stem) = gold.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else { continue };
    let directory = tests.join("conf").join(&group);
    let stylesheet = directory.join(format!("{stem}.xsl"));
    if !stylesheet.exists() {
      continue;
    }
    let data = directory.join(format!("{stem}.xml"));
    cases.push(Case {
      name: format!("{group}/{stem}"),
      expectation: Expectation::Runs,
      compare: Compare::ByOutputMethod,
      stylesheet,
      data: data.exists().then_some(data),
      expected: Some(gold),
    });
  }
  for (group, stylesheet) in files_under(&tests.join("conferr"), "xsl") {
    let Some(stem) = stylesheet.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else { continue };
    let data = stylesheet.with_file_name(format!("{stem}.xml"));
    cases.push(Case {
      name: format!("{group}/{stem}"),
      expectation: Expectation::IsRefused,
      compare: Compare::Cannot("the case expects a refusal".to_owned()),
      stylesheet,
      data: data.exists().then_some(data),
      expected: None,
    });
  }
  cases.sort_by(|a, b| a.name.cmp(&b.name));
  cases
}

/// Every file with the given extension one directory down, with the directory's name beside it.
fn files_under(root: &Path, extension: &str) -> Vec<(String, PathBuf)> {
  let mut found = Vec::new();
  let Ok(groups) = std::fs::read_dir(root) else { return found };
  for group in groups.flatten() {
    if !group.path().is_dir() {
      continue;
    }
    let name = group.file_name().to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir(group.path()) else { continue };
    for entry in entries.flatten() {
      let path = entry.path();
      if path.extension().is_some_and(|found| found.eq_ignore_ascii_case(extension)) {
        found.push((name.clone(), path));
      }
    }
  }
  found
}

// --- the catalogued layout -------------------------------------------------------------------

/// Reads the TC's catalogue into the cases it describes.
///
/// The catalogue is read with xenolith itself, which exercises the DOM builder on a megabyte of
/// real XML before a single case is judged.
fn catalogued_cases(root: &Path) -> Vec<Case> {
  let Ok(source) = std::fs::read(root.join("catalog.xml")) else { return Vec::new() };
  let Ok(document) = build::parse(source.as_slice()) else {
    eprintln!("the catalogue could not be parsed");
    return Vec::new();
  };

  let mut cases = Vec::new();
  let mut stack: Vec<(xenolith_dom::NodeId, String)> = Vec::new();
  if let Some(top) = document.document_element() {
    stack.push((top, String::new()));
  }
  while let Some((element, major)) = stack.pop() {
    let mut major = major;
    // Each catalogue gives the directory its cases sit under.
    for child in document.children(element) {
      if document.local_name(child) == Some("major-path") {
        major = document.text_content(child).trim().to_owned();
      }
    }
    for child in document.children(element) {
      match document.local_name(child) {
        Some("test-case") => {
          if let Some(case) = read_case(&document, child, root, &major) {
            cases.push(case);
          }
        }
        Some(_) => stack.push((child, major.clone())),
        None => {}
      }
    }
  }
  cases
}

/// Reads one `test-case`.
fn read_case(
  document: &xenolith_dom::Document,
  element: xenolith_dom::NodeId,
  root: &Path,
  major: &str,
) -> Option<Case> {
  let mut name = String::new();
  let mut file_path = String::new();
  let mut scenario = None;
  for child in document.children(element) {
    match document.local_name(child) {
      Some("name") => name = document.text_content(child).trim().to_owned(),
      Some("file-path") => file_path = document.text_content(child).trim().to_owned(),
      Some("scenario") => scenario = Some(child),
      _ => {}
    }
  }
  let scenario = scenario?;
  let operation = document.attribute(scenario, "operation").unwrap_or("standard");
  let expectation = if operation == "standard" { Expectation::Runs } else { Expectation::IsRefused };

  let directory = root.join(major).join(&file_path);
  let (mut stylesheet, mut data, mut expected, mut compare) = (None, None, None, Compare::Xml);
  for file in document.children(scenario) {
    let value = document.text_content(file);
    let value = value.trim();
    match (document.local_name(file), document.attribute(file, "role")) {
      (Some("input-file"), Some("principal-stylesheet")) => stylesheet = Some(directory.join(value)),
      (Some("input-file"), Some("principal-data")) => data = Some(directory.join(value)),
      (Some("output-file"), Some("principal")) => {
        expected = Some(directory.join(value));
        if let Some(kind) = document.attribute(file, "compare") {
          compare = match kind.to_ascii_lowercase().as_str() {
            "xml" | "fragment" => Compare::Xml,
            "text" => Compare::Text,
            _ => Compare::Cannot(kind.to_owned()),
          };
        }
      }
      _ => {}
    }
  }
  Some(Case { name, expectation, compare, stylesheet: stylesheet?, data, expected })
}

// --- running a case --------------------------------------------------------------------------

/// Loads a stylesheet module from the filesystem, which `xsl:import` and `xsl:include` need.
struct Files;

impl Loader for Files {
  fn load(&mut self, uri: &str) -> xenolith_core::error::Result<Vec<u8>> {
    let path = path_of(uri);
    std::fs::read(&path).map_err(|error| xenolith_core::Error::xslt(format!("{uri}: {error}")))
  }
}

impl UriResolver for Files {
  /// Fetches an external entity or DTD from the filesystem.
  ///
  /// The parser resolves nothing unless it is told how — that is the XXE decision, and it is the
  /// right default. But several cases of the suite declare a DTD that sits beside the data file,
  /// and refusing to fetch it would make them fail for a reason that has nothing to do with
  /// XSLT. Here, where the files are the ones the suite shipped, they are read.
  fn resolve(&mut self, request: &EntityRequest) -> xenolith_core::error::Result<Option<Box<dyn std::io::Read>>> {
    let Some(uri) = request.resolved_uri() else { return Ok(None) };
    Ok(std::fs::File::open(path_of(&uri)).ok().map(|f| Box::new(f) as Box<dyn std::io::Read>))
  }
}

/// The system identifier a path has, as the loader above expects to see it.
///
/// A path that is already rooted keeps its root — `file:///` plus `/tmp/a.xsl` would name
/// `//tmp/a.xsl` — and one that begins with a drive letter gains the slash the URI wants.
fn system_id(path: &Path) -> String {
  let written = path.display().to_string().replace('\\', "/");
  format!("file://{}{written}", if written.starts_with('/') { "" } else { "/" })
}

/// The filesystem path a `file:` URI names: the inverse of [`system_id`].
fn path_of(uri: &str) -> String {
  let rest = uri.strip_prefix("file://").unwrap_or(uri);
  match rest.strip_prefix('/') {
    // `/C:/…`: the slash belongs to the URI, not to the path.
    Some(after) if after.as_bytes().get(1) == Some(&b':') => after.to_owned(),
    _ => rest.to_owned(),
  }
}

/// What running a case produced.
struct Written {
  text: String,
  method: OutputMethod,
  /// Whether `xsl:output` asked for indentation, which is whitespace the processor chose.
  indented: bool,
}

/// Runs one case, giving what it wrote or why it could not.
fn transform_case(case: &Case) -> Result<Written, String> {
  let source = std::fs::read(&case.stylesheet).map_err(|error| error.to_string())?;
  let stylesheet = Stylesheet::compile_with(&source, &system_id(&case.stylesheet), &mut Files)
    .map_err(|error| format!("compiling: {}", error.message()))?;

  // A case with no data file is run over an empty document, which is what the suite means by it.
  let data = match &case.data {
    Some(path) => std::fs::read(path).map_err(|error| error.to_string())?,
    None => b"<empty/>".to_vec(),
  };
  // The system identifier is what a declared DTD beside the data file is resolved against.
  let system_id = case.data.as_deref().map_or_else(|| "urn:empty".to_owned(), system_id);
  let reader = Reader::with_system_id(data.as_slice(), &system_id).with_resolver(Files);
  let document = build::parse_reader(reader).map_err(|error| format!("the data: {}", error.message()))?;

  // `document()` refers to files beside the case's own, so the trees it fetches share the node space
  // the source document is read through.
  let space = Documents::new();
  let model = DomModel::with_documents(&document, &space);
  let available = Rc::new(LoadedDocuments::new(&space, Files));
  let result = Transform::new()
    .run_with_documents(&stylesheet, &model, model.root_node(), Functions::new(), available)
    .map_err(|error| format!("running: {}", error.message()))?;
  Ok(Written { text: result.serialize(), method: result.output().method(), indented: result.output().indent() })
}

/// The tree an XML text denotes, written in a form two conforming processors must agree on.
///
/// What is erased is what the specifications say carries no meaning, and nothing else:
///
/// - **how it is written** — `<a/>` against `<a></a>`, which quotation mark, where the line
///   breaks fall. Both sides are parsed, so none of this survives.
/// - **the order of attributes** — XPath 1.0 §5.3 says an element's attributes have no order,
///   so they are written here sorted by expanded name. Two processors that write the same
///   attributes in a different order have not disagreed about anything.
/// - **which prefix stands for a namespace** — a name is a namespace URI and a local part
///   (Namespaces §2.1); `bdd:a` and `ns0:a` are the same name when both prefixes are bound to
///   the same URI, and XSLT nowhere requires a particular prefix to be chosen. So names are
///   written expanded, and the `xmlns` declarations that establish prefixes are left out.
///
/// A prefix used but *not* declared is not erased by that last rule: such a result does not
/// parse at all, so it is a failure before it reaches here.
///
/// A result need not be one element — a stylesheet may write two, or none — so what will not
/// parse as a document is wrapped and tried again. Both sides go through this, so the wrapper
/// cancels out.
fn normalize(xml: &str) -> Option<String> {
  normalize_indented(xml, false)
}

/// As [`normalize`], and when `indented` also erases the whitespace an indenting processor puts
/// between elements.
///
/// §16.1 lets the XML method "add whitespace" when `indent="yes"` and says nothing about how
/// much: this writes a newline and two spaces a level, Xalan — which produced the suite's
/// expected results — writes a newline and none. Both are conforming, so a case whose output is
/// indented cannot be judged on the whitespace between its elements. It is judged on everything
/// else, and a case that did not ask to be indented is still judged on all of it.
fn normalize_indented(xml: &str, indented: bool) -> Option<String> {
  let document = parse_loosely(xml)?;
  let mut written = String::new();
  for child in document.children(document.document_node()) {
    // Whitespace outside the document element is layout rather than content — XML allows it
    // there and gives it no meaning — so a blank line between the declaration and the root is
    // not a difference of any kind.
    if document.node_type(child) == xenolith_dom::NodeType::TEXT_NODE {
      continue;
    }
    canonical(&document, child, indented, &mut written);
  }
  Some(written)
}

/// Parses XML, wrapping it first if it is a fragment rather than a document.
fn parse_loosely(xml: &str) -> Option<xenolith_dom::Document> {
  if let Ok(document) = build::parse(xml.as_bytes())
    && document.document_element().is_some()
  {
    return Some(document);
  }
  let body = match xml.split_once("?>") {
    // A declaration may only begin a document, so it cannot stay inside the wrapper.
    Some((head, rest)) if head.trim_start().starts_with("<?xml") => rest,
    _ => xml,
  };
  build::parse(format!("<xenolith-wrapper>{body}</xenolith-wrapper>").as_bytes()).ok()
}

/// Writes one node in the canonical form described on [`normalize`].
fn canonical(document: &xenolith_dom::Document, node: xenolith_dom::NodeId, indented: bool, into: &mut String) {
  use std::fmt::Write as _;
  match document.node_type(node) {
    xenolith_dom::NodeType::ELEMENT_NODE => {
      let _ = write!(into, "<{}", expanded(document, node));
      let mut attributes: Vec<String> = document
        .attributes(node)
        .iter()
        // An xmlns declaration is how a prefix is established, not part of what the element
        // says; the names it qualifies are written out in full instead.
        .filter(|&attribute| !is_namespace_declaration(document, attribute))
        .map(|attribute| {
          format!(" {}={:?}", expanded(document, attribute), document.node_value(attribute).unwrap_or_default())
        })
        .collect();
      attributes.sort();
      into.push_str(&attributes.concat());
      into.push('>');
      // Where an indenting processor may have put whitespace: between an element's children,
      // when they are elements. Text of its own is never touched, here or in the writer.
      let among_elements = indented
        && document.children(node).any(|child| document.node_type(child) == xenolith_dom::NodeType::ELEMENT_NODE);
      for child in document.children(node) {
        if among_elements && is_only_whitespace(document, child) {
          continue;
        }
        canonical(document, child, indented, into);
      }
      let _ = write!(into, "</{}>", expanded(document, node));
    }
    xenolith_dom::NodeType::TEXT_NODE | xenolith_dom::NodeType::CDATA_SECTION_NODE => {
      // A CDATA section is a way of writing text, not a different kind of content.
      into.push_str(document.node_value(node).unwrap_or_default());
    }
    xenolith_dom::NodeType::COMMENT_NODE => {
      let _ = write!(into, "<!--{}-->", document.node_value(node).unwrap_or_default());
    }
    xenolith_dom::NodeType::PROCESSING_INSTRUCTION_NODE => {
      let _ = write!(into, "<?{} {}?>", document.node_name(node), document.node_value(node).unwrap_or_default());
    }
    _ => {}
  }
}

/// Whether a node is a text node holding nothing but whitespace.
fn is_only_whitespace(document: &xenolith_dom::Document, node: xenolith_dom::NodeId) -> bool {
  document.node_type(node) == xenolith_dom::NodeType::TEXT_NODE
    && document.node_value(node).unwrap_or_default().trim().is_empty()
}

/// A name as the data model has it: the namespace URI and the local part, with no prefix.
fn expanded(document: &xenolith_dom::Document, node: xenolith_dom::NodeId) -> String {
  let local = document.local_name(node).unwrap_or_default();
  match document.namespace_uri(node) {
    Some(uri) => format!("{{{uri}}}{local}"),
    None => local.to_owned(),
  }
}

/// Whether an attribute is an `xmlns` declaration rather than an attribute of the element.
fn is_namespace_declaration(document: &xenolith_dom::Document, attribute: xenolith_dom::NodeId) -> bool {
  document.prefix(attribute) == Some("xmlns") || document.node_name(attribute) == "xmlns"
}

/// How to compare this case's result, once the stylesheet has said how it writes.
fn comparison(case: &Case, method: OutputMethod) -> Compare {
  match &case.compare {
    Compare::ByOutputMethod => match method {
      OutputMethod::Xml => Compare::Xml,
      OutputMethod::Text => Compare::Text,
      // HTML is neither: `<br>` is not well-formed XML, and two processors indent and quote it
      // differently, so an exact comparison would fail on cases that are right.
      OutputMethod::Html => Compare::Cannot("HTML".to_owned()),
    },
    settled => settled.clone(),
  }
}

/// Whether what was written matches what the case expected.
fn matches(case: &Case, written: &str, how: &Compare) -> Result<(), String> {
  matches_indented(case, written, how, false)
}

/// As [`matches`], saying whether the result was indented — see [`normalize_indented`].
fn matches_indented(case: &Case, written: &str, how: &Compare, indented: bool) -> Result<(), String> {
  let Some(path) = &case.expected else { return Ok(()) };
  let expected = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
  let normalize = |xml: &str| normalize_indented(xml, indented);
  match how {
    Compare::Text => {
      if written.trim_end() == expected.trim_end() {
        Ok(())
      } else {
        Err(format!("text differs\n  expected: {:?}\n  written:  {:?}", expected.trim_end(), written.trim_end()))
      }
    }
    // Both sides through the same serializer, so a difference is one of the tree.
    Compare::Xml => match (normalize(&expected), normalize(written)) {
      (Some(expected), Some(written)) if expected == written => Ok(()),
      (Some(expected), Some(written)) => Err(format!("differs\n  expected: {expected}\n  written:  {written}")),
      (None, _) => Err("the expected result is not well-formed XML".to_owned()),
      (_, None) => Err(format!("what was written is not well-formed XML: {written}")),
    },
    Compare::Cannot(_) | Compare::ByOutputMethod => Ok(()),
  }
}

/// How many failing cases to name in the report.
fn examples_wanted() -> usize {
  std::env::var("XSLTCONF_EXAMPLES").ok().and_then(|value| value.trim().parse().ok()).unwrap_or(25)
}

/// What a run of the suite came to.
#[derive(Default)]
struct Tally {
  passed: usize,
  failed: usize,
  skipped: usize,
  /// Why cases were skipped, by the name the suite or this harness gave the comparison.
  unjudged: BTreeMap<String, usize>,
  /// What went wrong, by the first words of the reason.
  reasons: BTreeMap<String, usize>,
  examples: Vec<String>,
}

impl Tally {
  fn judged(&self) -> usize {
    self.passed + self.failed
  }

  fn rate(&self) -> f64 {
    if self.judged() == 0 { 0.0 } else { self.passed as f64 * 100.0 / self.judged() as f64 }
  }

  fn note(&mut self, case: &Case, outcome: Result<(), String>) {
    match outcome {
      Ok(()) => self.passed += 1,
      Err(why) => {
        self.failed += 1;
        let kind = why.split(['\n', ':']).next().unwrap_or("other").trim().to_owned();
        *self.reasons.entry(kind).or_default() += 1;
        // Enough to see what kind of thing goes wrong. Set XSLTCONF_EXAMPLES to see more — the
        // whole list is what one reads to write the deviations down.
        if self.examples.len() < examples_wanted() {
          self.examples.push(format!("  {}: {}", case.name, why.lines().next().unwrap_or_default()));
        }
      }
    }
  }

  fn report(&self, title: &str) {
    eprintln!("  {title}");
    eprintln!(
      "    judged: {}  passed: {}  failed: {}  skipped: {}",
      self.judged(),
      self.passed,
      self.failed,
      self.skipped
    );
    eprintln!("    pass rate: {:.1}%", self.rate());
    for (kind, count) in &self.unjudged {
      eprintln!("    not judged, {kind}: {count}");
    }
    if !self.reasons.is_empty() {
      eprintln!("\n    what went wrong, by kind:");
      for (kind, count) in &self.reasons {
        eprintln!("      {count:>5}  {kind}");
      }
    }
    if !self.examples.is_empty() {
      eprintln!("\n    the first few:");
      for example in &self.examples {
        eprintln!("  {example}");
      }
    }
    eprintln!();
  }
}

#[test]
fn the_conformance_suite_is_run_and_reported() {
  let Some(root) = suite() else {
    eprintln!("skipped: set XSLTCONF to a copy of the OASIS/Xalan suite (see this file's documentation)");
    return;
  };

  let cases = cases(&root);
  assert!(!cases.is_empty(), "XSLTCONF is set but no cases were found");

  let (mut runs, mut refusals) = (Tally::default(), Tally::default());
  for case in &cases {
    match case.expectation {
      Expectation::Runs => match transform_case(case) {
        Ok(written) => {
          let how = comparison(case, written.method);
          if let Compare::Cannot(why) = &how {
            runs.skipped += 1;
            *runs.unjudged.entry(why.clone()).or_default() += 1;
            continue;
          }
          runs.note(case, matches_indented(case, &written.text, &how, written.indented));
        }
        Err(why) => runs.note(case, Err(why)),
      },
      // A case that must be refused: being refused is the pass, whatever the reason given.
      Expectation::IsRefused => {
        let outcome = match transform_case(case) {
          Err(_) => Ok(()),
          Ok(_) => Err("accepted: it was expected to be refused, and was not".to_owned()),
        };
        refusals.note(case, outcome);
      }
    }
  }

  eprintln!("\nOASIS/Xalan XSLT 1.0 conformance\n  cases found: {}\n", cases.len());
  runs.report("cases that must run and produce a result:");
  if refusals.judged() > 0 {
    refusals.report("cases that must be refused:");
  }

  // A threshold nobody has measured is no threshold at all, so one is asserted only when the
  // caller sets it — having seen the report above and decided what it should be.
  if let Some(budget) = std::env::var_os("XSLTCONF_MAX_FAILURES") {
    let budget: usize = budget.to_string_lossy().trim().parse().expect("XSLTCONF_MAX_FAILURES is a number");
    let failed = runs.failed + refusals.failed;
    assert!(failed <= budget, "{failed} cases failed, more than the {budget} allowed");
  }
}

// --- checking the harness itself ---------------------------------------------------------------

/// A suite of three cases, written to a directory, in the shape the catalogue uses.
///
/// The harness above is only ever exercised by a suite that is not in this repository, which
/// would leave it unchecked in every ordinary run — a harness that had quietly stopped finding
/// cases would look exactly like a clean skip. So the catalogue reading and the judging are put
/// through a small suite built here, where the right answers are known.
fn write_small_suite(root: &Path) -> std::io::Result<()> {
  let cases = root.join("Cases");
  std::fs::create_dir_all(&cases)?;
  std::fs::write(
    root.join("catalog.xml"),
    r#"<test-suite>
         <test-catalog>
           <major-path>Cases</major-path>
           <test-case>
             <name>passes</name><file-path>.</file-path>
             <scenario operation="standard">
               <input-file role="principal-data">in.xml</input-file>
               <input-file role="principal-stylesheet">good.xsl</input-file>
               <output-file role="principal" compare="XML">good.out</output-file>
             </scenario>
           </test-case>
           <test-case>
             <name>differs</name><file-path>.</file-path>
             <scenario operation="standard">
               <input-file role="principal-data">in.xml</input-file>
               <input-file role="principal-stylesheet">good.xsl</input-file>
               <output-file role="principal" compare="XML">wrong.out</output-file>
             </scenario>
           </test-case>
           <test-case>
             <name>skipped</name><file-path>.</file-path>
             <scenario operation="standard">
               <input-file role="principal-data">in.xml</input-file>
               <input-file role="principal-stylesheet">good.xsl</input-file>
               <output-file role="principal" compare="Manual">good.out</output-file>
             </scenario>
           </test-case>
         </test-catalog>
       </test-suite>"#,
  )?;
  std::fs::write(cases.join("in.xml"), "<r><a>text</a></r>")?;
  std::fs::write(cases.join("good.xsl"), GOOD_STYLESHEET)?;
  // Written the other way round from what this serializer would write, so a byte comparison
  // would call it a failure and the normalization above must not.
  std::fs::write(cases.join("good.out"), "<out>text</out>")?;
  std::fs::write(cases.join("wrong.out"), "<out>something else</out>")?;
  Ok(())
}

const GOOD_STYLESHEET: &str = r#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
     <xsl:output omit-xml-declaration="yes"/>
     <xsl:template match="/"><out><xsl:value-of select="//a"/></out></xsl:template>
   </xsl:stylesheet>"#;

/// The same three cases, laid out the way Apache's copy is.
fn write_small_xalan_suite(root: &Path) -> std::io::Result<()> {
  let conf = root.join("tests/conf/group");
  let gold = root.join("tests/conf-gold/group");
  let err = root.join("tests/conferr/grouperr");
  for directory in [&conf, &gold, &err] {
    std::fs::create_dir_all(directory)?;
  }
  std::fs::write(conf.join("case01.xml"), "<r><a>text</a></r>")?;
  std::fs::write(conf.join("case01.xsl"), GOOD_STYLESHEET)?;
  std::fs::write(gold.join("case01.out"), "<out>text</out>")?;
  // A module a case imports. It has no gold file, so it is not itself a case.
  std::fs::write(conf.join("module.xsl"), GOOD_STYLESHEET)?;
  // One that must be refused: `version` is required on xsl:stylesheet.
  std::fs::write(err.join("err01.xml"), "<r/>")?;
  std::fs::write(err.join("err01.xsl"), r#"<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform"/>"#)?;
  Ok(())
}

#[test]
fn the_harness_reads_a_catalogue_and_judges_what_it_finds() {
  let root = std::env::temp_dir().join(format!("xenolith-xsltconf-{}", std::process::id()));
  let _ = std::fs::remove_dir_all(&root);
  write_small_suite(&root).expect("writes a small suite");

  let cases = cases(&root);
  assert_eq!(cases.len(), 3, "the catalogue describes three cases");
  assert_eq!(cases.iter().filter(|case| matches!(case.compare, Compare::Cannot(_))).count(), 1);

  let by_name: BTreeMap<&str, &Case> = cases.iter().map(|case| (case.name.as_str(), case)).collect();

  // The paths are put together from major-path, file-path and the file's own name.
  let passing = by_name["passes"];
  assert!(passing.stylesheet.ends_with("good.xsl"), "{:?}", passing.stylesheet);
  assert!(passing.data.as_ref().is_some_and(|path| path.ends_with("in.xml")));

  // One that agrees, one that does not, and the difference is in the tree rather than the bytes.
  let written = transform_case(passing).expect("runs");
  assert_eq!(matches(passing, &written.text, &Compare::Xml), Ok(()), "the expected result should have matched");
  assert!(matches(by_name["differs"], &written.text, &Compare::Xml).is_err(), "a different result should not match");

  let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_harness_reads_the_layout_apaches_copy_uses() {
  let root = std::env::temp_dir().join(format!("xenolith-xsltconf-xalan-{}", std::process::id()));
  let _ = std::fs::remove_dir_all(&root);
  write_small_xalan_suite(&root).expect("writes a small suite");

  let cases = cases(&root);
  // The imported module has no gold file beside it and so is not counted as a case of its own.
  assert_eq!(cases.len(), 2, "one case that runs and one that must be refused: {cases:#?}");

  let by_name: BTreeMap<&str, &Case> = cases.iter().map(|case| (case.name.as_str(), case)).collect();
  let runs = by_name["group/case01"];
  assert_eq!(runs.expectation, Expectation::Runs);
  assert!(runs.data.as_ref().is_some_and(|path| path.ends_with("case01.xml")));
  assert!(runs.expected.as_ref().is_some_and(|path| path.ends_with("case01.out")));

  let written = transform_case(runs).expect("runs");
  // The stylesheet says nothing about the method, so the comparison comes out as XML.
  assert_eq!(comparison(runs, written.method), Compare::Xml);
  assert_eq!(matches(runs, &written.text, &Compare::Xml), Ok(()));

  let refused = by_name["grouperr/err01"];
  assert_eq!(refused.expectation, Expectation::IsRefused);
  assert!(transform_case(refused).is_err(), "a stylesheet with no version should be refused");

  let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_result_that_is_not_one_element_still_compares() {
  // Two top-level elements, and the same two written differently. Neither parses as a document,
  // so a harness that only compared documents would call every such case a failure.
  assert_eq!(normalize("<a/><b>x</b>"), normalize("<a></a><b>x</b>"));
  assert_ne!(normalize("<a/><b>x</b>"), normalize("<a/><b>y</b>"));
  // A declaration on one side only must not make the difference.
  assert_eq!(normalize("<?xml version=\"1.0\"?><a/><b/>"), normalize("<a/><b/>"));
}

#[test]
fn what_xml_says_is_insignificant_does_not_count_as_a_difference() {
  // The order of an element's attributes is not part of what it says (XPath 1.0 §5.3).
  assert_eq!(normalize("<a x='1' y='2'/>"), normalize("<a y='2' x='1'/>"));
  // Which prefix stands for a namespace is not either — the name is the URI and the local part.
  assert_eq!(normalize("<p:a xmlns:p='urn:n'/>"), normalize("<q:a xmlns:q='urn:n'/>"));
  assert_eq!(normalize("<a xmlns:p='urn:n' p:x='1'/>"), normalize("<a xmlns:q='urn:n' q:x='1'/>"));
  // A CDATA section is a way of writing text.
  assert_eq!(normalize("<a><![CDATA[x<y]]></a>"), normalize("<a>x&lt;y</a>"));
}

#[test]
fn what_xml_says_is_significant_still_counts_as_a_difference() {
  // The guard on the rules above: they must erase the spelling and nothing else.
  assert_ne!(normalize("<a x='1'/>"), normalize("<a x='2'/>"), "an attribute's value");
  assert_ne!(normalize("<a x='1'/>"), normalize("<a y='1'/>"), "which attribute it is");
  assert_ne!(normalize("<a/>"), normalize("<a xmlns='urn:n'/>"), "the namespace itself");
  assert_ne!(normalize("<r><a/><b/></r>"), normalize("<r><b/><a/></r>"), "the order of children");
  assert_ne!(normalize("<a> x </a>"), normalize("<a>x</a>"), "the whitespace in text");
  assert_ne!(normalize("<a><!--c--></a>"), normalize("<a></a>"), "a comment");
}

#[test]
fn how_much_a_processor_indents_by_is_not_a_difference() {
  // §16.1 lets an indenting processor add whitespace between elements and does not say how much.
  let ours = "<r>\n  <a/>\n  <b>x</b>\n</r>";
  let theirs = "<r>\n<a/>\n<b>x</b>\n</r>";
  assert_ne!(normalize_indented(ours, false), normalize_indented(theirs, false), "not asked for");
  assert_eq!(normalize_indented(ours, true), normalize_indented(theirs, true), "asked for");

  // Even then, only whitespace *between elements* is the processor's. Text of the result's own
  // is what the stylesheet wrote, and a case that differs there differs.
  assert_ne!(normalize_indented("<r> x </r>", true), normalize_indented("<r>x</r>", true), "text of its own");
  assert_ne!(normalize_indented("<r>a<b/></r>", true), normalize_indented("<r><b/></r>", true), "mixed content");
}

#[test]
fn a_path_and_its_system_identifier_are_inverses() {
  // Both spellings, so that whichever platform this runs on, the other one is checked too.
  for path in ["/tmp/conf/case01.xsl", "C:/conf/case01.xsl"] {
    assert_eq!(path_of(&system_id(Path::new(path))), path);
  }
  assert_eq!(system_id(Path::new("/tmp/a.xsl")), "file:///tmp/a.xsl");
  assert_eq!(system_id(Path::new("C:/a.xsl")), "file:///C:/a.xsl");
}
