//! The OASIS/Xalan XSLT 1.0 conformance suite.
//!
//! The suite is several thousand files and is not vendored here. Point `XSLTCONF` at an unpacked
//! copy — the directory holding `catalog.xml` — to run against it:
//!
//! ```text
//! XSLTCONF=xslt-conformance cargo test -p xylograph-xslt --test conformance -- --nocapture
//! ```
//!
//! Without it the test prints what it would have needed and passes, so a checkout with no suite
//! is not a checkout that silently tests nothing.
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
//! Cases the suite marks for HTML or manual comparison are skipped and counted, never passed
//! silently.
//!
//! # Failing the run
//!
//! Set `XSLTCONF_MAX_FAILURES` to the number of failures the run may have before it is a test
//! failure. Without it the report is printed and nothing is asserted, because a threshold nobody
//! has measured is a threshold that means nothing — see `ROADMAP.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use xylograph_dom::build;
use xylograph_serialize::Serializer;
use xylograph_xdm::DomModel;
use xylograph_xslt::{Loader, Stylesheet, Transform};

/// One case of the suite.
#[derive(Debug)]
struct Case {
  name: String,
  /// What the case expects to happen.
  operation: String,
  /// How the result is to be compared: `XML`, `Text`, `HTML`, `Manual`, …
  compare: String,
  stylesheet: PathBuf,
  data: Option<PathBuf>,
  expected: Option<PathBuf>,
}

/// Locates the suite, or explains that it is absent.
fn suite() -> Option<PathBuf> {
  let root = PathBuf::from(std::env::var_os("XSLTCONF")?);
  if root.join("catalog.xml").exists() {
    return Some(root);
  }
  eprintln!("XSLTCONF is set to {}, which holds no catalog.xml", root.display());
  None
}

/// Reads the catalogue into the cases it describes.
///
/// The catalogue is read with xylograph itself, which exercises the DOM builder on a megabyte of
/// real XML before a single case is judged.
fn cases(root: &Path) -> Vec<Case> {
  let Ok(source) = std::fs::read(root.join("catalog.xml")) else { return Vec::new() };
  let Ok(document) = build::parse(source.as_slice()) else {
    eprintln!("the catalogue could not be parsed");
    return Vec::new();
  };

  let mut cases = Vec::new();
  let mut stack: Vec<(xylograph_dom::NodeId, String)> = Vec::new();
  if let Some(top) = document.document_element() {
    stack.push((top, String::new()));
  }
  while let Some((element, major)) = stack.pop() {
    let mut major = major;
    // Each catalogue names the directory its cases sit under.
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
  document: &xylograph_dom::Document,
  element: xylograph_dom::NodeId,
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
  let operation = document.attribute(scenario, "operation").unwrap_or("standard").to_owned();

  let directory = root.join(major).join(&file_path);
  let (mut stylesheet, mut data, mut expected, mut compare) = (None, None, None, "XML".to_owned());
  for file in document.children(scenario) {
    let value = document.text_content(file);
    let value = value.trim();
    match (document.local_name(file), document.attribute(file, "role")) {
      (Some("input-file"), Some("principal-stylesheet")) => stylesheet = Some(directory.join(value)),
      (Some("input-file"), Some("principal-data")) => data = Some(directory.join(value)),
      (Some("output-file"), Some("principal")) => {
        expected = Some(directory.join(value));
        if let Some(kind) = document.attribute(file, "compare") {
          compare = kind.to_owned();
        }
      }
      _ => {}
    }
  }
  Some(Case { name, operation, compare, stylesheet: stylesheet?, data, expected })
}

/// Loads a stylesheet module from the filesystem, which `xsl:import` and `xsl:include` need.
struct Files;

impl Loader for Files {
  fn load(&mut self, uri: &str) -> xylograph_core::error::Result<Vec<u8>> {
    let path = uri.strip_prefix("file:///").unwrap_or(uri);
    std::fs::read(path)
      .map_err(|error| xylograph_core::Error::new(xylograph_core::ErrorKind::Xslt, format!("{uri}: {error}")))
  }
}

/// The system identifier a path has, as the loader above expects to see it.
fn system_id(path: &Path) -> String {
  format!("file:///{}", path.display().to_string().replace('\\', "/"))
}

/// Runs one case, giving what it wrote or why it could not.
fn transform_case(case: &Case) -> Result<String, String> {
  let source = std::fs::read(&case.stylesheet).map_err(|error| error.to_string())?;
  let stylesheet = Stylesheet::compile_with(&source, &system_id(&case.stylesheet), &mut Files)
    .map_err(|error| format!("compiling: {}", error.message()))?;

  // A case with no data file is run over an empty document, which is what the suite means by it.
  let data = match &case.data {
    Some(path) => std::fs::read(path).map_err(|error| error.to_string())?,
    None => b"<empty/>".to_vec(),
  };
  let document = build::parse(data.as_slice()).map_err(|error| format!("the data: {}", error.message()))?;
  let model = DomModel::new(&document);
  let result = Transform::new()
    .run(&stylesheet, &model, model.root_node())
    .map_err(|error| format!("running: {}", error.message()))?;
  Ok(result.serialize())
}

/// Writes XML through this serializer, so that two ways of writing one tree compare equal.
fn normalize(xml: &str) -> Option<String> {
  let document = build::parse(xml.as_bytes()).ok()?;
  let root = document.document_element()?;
  Some(Serializer::new().to_string(&document, root))
}

/// Whether what was written matches what the case expected.
fn matches(case: &Case, written: &str) -> Result<(), String> {
  let Some(path) = &case.expected else { return Ok(()) };
  let expected = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
  if case.compare.eq_ignore_ascii_case("text") {
    return if written.trim_end() == expected.trim_end() {
      Ok(())
    } else {
      Err(format!("text differs\n  expected: {:?}\n  written:  {:?}", expected.trim_end(), written.trim_end()))
    };
  }
  // Both sides through the same serializer, so a difference is one of the tree.
  match (normalize(&expected), normalize(written)) {
    (Some(expected), Some(written)) if expected == written => Ok(()),
    (Some(expected), Some(written)) => Err(format!("differs\n  expected: {expected}\n  written:  {written}")),
    (None, _) => Err("the expected result is not well-formed XML".to_owned()),
    (_, None) => Err(format!("what was written is not well-formed XML: {written}")),
  }
}

#[test]
fn the_conformance_suite_is_run_and_reported() {
  let Some(root) = suite() else {
    eprintln!("skipped: set XSLTCONF to a copy of the OASIS/Xalan suite (see this file's documentation)");
    return;
  };

  let cases = cases(&root);
  assert!(!cases.is_empty(), "XSLTCONF is set but the catalogue described no cases");

  let (mut passed, mut failed, mut skipped) = (0, 0, 0);
  // Grouped by the first word of the reason, so the report says what kind of thing goes wrong
  // rather than listing three thousand lines.
  let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
  let mut examples: Vec<String> = Vec::new();

  for case in &cases {
    // A comparison this cannot make is not a judgement either way.
    if !matches!(case.compare.to_ascii_lowercase().as_str(), "xml" | "text" | "fragment") {
      skipped += 1;
      continue;
    }
    let outcome = match (transform_case(case), case.operation.as_str()) {
      // The suite expects it to run and to produce the file beside it.
      (Ok(written), "standard") => matches(case, &written),
      // The suite expects it to be refused, and it was.
      (Err(_), "compile-error" | "execution-error") => Ok(()),
      (Ok(_), operation) => Err(format!("{operation}: it was expected to fail, and did not")),
      (Err(why), _) => Err(why),
    };
    match outcome {
      Ok(()) => passed += 1,
      Err(why) => {
        failed += 1;
        let kind = why.split(['\n', ':']).next().unwrap_or("other").trim().to_owned();
        *reasons.entry(kind).or_default() += 1;
        if examples.len() < 40 {
          examples.push(format!("  {}: {}", case.name, why.lines().next().unwrap_or_default()));
        }
      }
    }
  }

  let judged = passed + failed;
  let rate = if judged == 0 { 0.0 } else { passed as f64 * 100.0 / judged as f64 };
  eprintln!("\nOASIS/Xalan XSLT 1.0 conformance");
  eprintln!("  cases in the catalogue: {}", cases.len());
  eprintln!("  judged: {judged}  passed: {passed}  failed: {failed}  skipped: {skipped}");
  eprintln!("  pass rate: {rate:.1}%");
  if !reasons.is_empty() {
    eprintln!("\n  what went wrong, by kind:");
    for (kind, count) in &reasons {
      eprintln!("    {count:>5}  {kind}");
    }
  }
  if !examples.is_empty() {
    eprintln!("\n  the first few:");
    for example in &examples {
      eprintln!("{example}");
    }
  }
  eprintln!();

  // A threshold nobody has measured is no threshold at all, so one is asserted only when the
  // caller names it — having seen the report above and decided what it should be.
  if let Some(budget) = std::env::var_os("XSLTCONF_MAX_FAILURES") {
    let budget: usize = budget.to_string_lossy().trim().parse().expect("XSLTCONF_MAX_FAILURES is a number");
    assert!(failed <= budget, "{failed} cases failed, more than the {budget} allowed");
  }
}

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
  std::fs::write(
    cases.join("good.xsl"),
    r#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
         <xsl:output omit-xml-declaration="yes"/>
         <xsl:template match="/"><out><xsl:value-of select="//a"/></out></xsl:template>
       </xsl:stylesheet>"#,
  )?;
  // Written the other way round from what this serializer would write, so a byte comparison
  // would call it a failure and the normalization above must not.
  std::fs::write(cases.join("good.out"), "<out>text</out>")?;
  std::fs::write(cases.join("wrong.out"), "<out>something else</out>")?;
  Ok(())
}

#[test]
fn the_harness_reads_a_catalogue_and_judges_what_it_finds() {
  let root = std::env::temp_dir().join(format!("xylograph-xsltconf-{}", std::process::id()));
  let _ = std::fs::remove_dir_all(&root);
  write_small_suite(&root).expect("writes a small suite");

  let cases = cases(&root);
  assert_eq!(cases.len(), 3, "the catalogue describes three cases");
  assert_eq!(cases.iter().filter(|case| case.compare == "Manual").count(), 1);

  let by_name: BTreeMap<&str, &Case> = cases.iter().map(|case| (case.name.as_str(), case)).collect();

  // The paths are put together from major-path, file-path and the file's own name.
  let passing = by_name["passes"];
  assert!(passing.stylesheet.ends_with("good.xsl"), "{:?}", passing.stylesheet);
  assert!(passing.data.as_ref().is_some_and(|path| path.ends_with("in.xml")));

  // One that agrees, one that does not, and the difference is in the tree rather than the bytes.
  let written = transform_case(passing).expect("runs");
  assert_eq!(matches(passing, &written), Ok(()), "the expected result should have matched");
  assert!(matches(by_name["differs"], &written).is_err(), "a different result should not match");

  let _ = std::fs::remove_dir_all(&root);
}
