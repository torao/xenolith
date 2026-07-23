//! The W3C XML Conformance Test Suite.
//!
//! The suite is several thousand files and is not vendored here. Point `XMLCONF` at an
//! unpacked copy to run against it:
//!
//! ```text
//! curl -O https://www.w3.org/XML/Test/xmlts20130923.tar.gz
//! tar xf xmlts20130923.tar.gz
//! XMLCONF=xmlconf cargo test -p xylograph-parser --test conformance
//! ```
//!
//! Without it the test prints what it would have needed and passes, so a checkout with no
//! suite is not a checkout that silently tests nothing.
//!
//! What can be judged depends on the phase. Until the DTD arrives in phase 2, a document with
//! a `DOCTYPE` cannot be: an undeclared entity may in fact be declared, and default attribute
//! values change which documents are well-formed. Those cases are counted as skipped.

use std::path::{Path, PathBuf};

use xylograph_parser::Reader;

/// Locates the suite, or explains that it is absent.
fn suite() -> Option<PathBuf> {
  let root = PathBuf::from(std::env::var_os("XMLCONF")?);
  if root.join("xmlconf.xml").exists() {
    return Some(root);
  }
  eprintln!("XMLCONF is set to {}, which holds no xmlconf.xml", root.display());
  None
}

/// Every case in the suite, as (identifier, path, type).
///
/// The catalogue is read with xylograph itself, which exercises the reader on a few hundred
/// kilobytes of real XML before a single case is judged.
fn cases(root: &Path) -> Vec<(String, PathBuf, String)> {
  let mut cases = Vec::new();
  // The top-level catalogue pulls in the per-vendor ones through entity references, which
  // phase 1 cannot expand, so those files are found on disk instead.
  let catalogues = std::fs::read_dir(root)
    .into_iter()
    .flatten()
    .flatten()
    .map(|entry| entry.path())
    .filter(|path| path.is_dir())
    .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()))
    .filter(|path| path.extension().is_some_and(|e| e == "xml"));

  for catalogue in catalogues {
    let Ok(file) = std::fs::File::open(&catalogue) else { continue };
    let base = catalogue.parent().unwrap_or(root).to_path_buf();
    let mut reader = Reader::new(std::io::BufReader::new(file));
    // A catalogue that fails to parse is a bug worth seeing, but not one to stop on here.
    while let Ok(Some(_)) = reader.advance() {
      let parser = reader.parser();
      if parser.local_name() != "TEST" {
        continue;
      }
      let id = parser.attribute_value(None, "ID").unwrap_or_default().to_owned();
      let kind = parser.attribute_value(None, "TYPE").unwrap_or_default().to_owned();
      let namespace_aware = parser.attribute_value(None, "NAMESPACE") != Some("no");
      if let Some(uri) = parser.attribute_value(None, "URI").filter(|_| namespace_aware) {
        cases.push((id, base.join(uri), kind));
      }
    }
  }
  cases
}

/// Parses a document to its end, returning the failure if there was one.
fn parse(path: &Path) -> Result<(), String> {
  let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
  let system_id = format!("file:///{}", path.display().to_string().replace('\\', "/"));
  let mut reader = Reader::with_system_id(std::io::BufReader::new(file), &system_id);
  loop {
    match reader.advance() {
      Ok(Some(_)) => {}
      Ok(None) => return Ok(()),
      Err(e) => return Err(e.to_string()),
    }
  }
}

/// Runs every case of one type, requiring each to be accepted or rejected as `expected`.
fn run(kind: &str, expected: bool) {
  let Some(root) = suite() else {
    eprintln!("skipped: set XMLCONF to a copy of the W3C suite (see this file's documentation)");
    return;
  };
  let (mut checked, mut skipped) = (0, 0);
  let mut failures = Vec::new();

  for (id, path, case_kind) in cases(&root).into_iter().filter(|(_, _, k)| k == kind) {
    let Ok(source) = std::fs::read_to_string(&path) else {
      skipped += 1;
      continue;
    };
    if source.contains("<!DOCTYPE") {
      skipped += 1; // needs the DTD; phase 2
      continue;
    }
    checked += 1;
    match (parse(&path), expected) {
      (Ok(()), true) | (Err(_), false) => {}
      (Err(why), true) => failures.push(format!("{id} should be accepted ({case_kind}): {why}")),
      (Ok(()), false) => failures.push(format!("{id} should be rejected ({case_kind}): {}", path.display())),
    }
  }

  eprintln!("{kind}: {checked} checked, {skipped} skipped for phase 2, {} failed", failures.len());
  assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn valid_documents_are_accepted() {
  run("valid", true);
}

#[test]
fn documents_that_are_not_well_formed_are_rejected() {
  run("not-wf", false);
}
