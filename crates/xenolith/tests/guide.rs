//! The developer guide names the crates, and a table of names goes stale the moment one moves.
//!
//! Most of what that document says is prose about design, which nothing can check. The crate
//! layout is the exception: it is a list of facts the workspace already knows, so it is checked
//! here rather than trusted. Adding a crate without a line in the table, or leaving a line for a
//! crate that has gone, fails this test.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The repository root, from this crate's manifest.
fn root() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root")
}

/// The crates the workspace holds, by directory name.
fn workspace_crates() -> BTreeSet<String> {
  let crates = root().join("crates");
  std::fs::read_dir(&crates)
    .unwrap_or_else(|error| panic!("{}: {error}", crates.display()))
    .flatten()
    .filter(|entry| entry.path().is_dir())
    .map(|entry| entry.file_name().to_string_lossy().into_owned())
    .collect()
}

/// The crate names the guide mentions in backticks.
fn crates_named_in(guide: &str) -> BTreeSet<String> {
  let mut named = BTreeSet::new();
  for piece in guide.split('`').skip(1).step_by(2) {
    if piece == "xenolith" || piece.starts_with("xenolith-") {
      named.insert(piece.to_owned());
    }
  }
  named
}

fn guide() -> String {
  let path: PathBuf = root().join("DEVELOPER-GUIDE.md");
  std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", Path::new(&path).display()))
}

#[test]
fn the_guide_names_every_crate_in_the_workspace() {
  let text = guide();
  let missing: Vec<String> = workspace_crates().into_iter().filter(|name| !text.contains(name.as_str())).collect();
  assert!(missing.is_empty(), "DEVELOPER-GUIDE.md does not mention {missing:?}; add them to the Layout table");
}

#[test]
fn every_crate_the_guide_names_is_one_that_exists() {
  let named = crates_named_in(&guide());
  assert!(!named.is_empty(), "the guide should name the crates in backticks");
  let workspace = workspace_crates();
  let gone: Vec<&String> = named.iter().filter(|name| !workspace.contains(*name)).collect();
  assert!(gone.is_empty(), "DEVELOPER-GUIDE.md lists {gone:?}, which are not crates of this workspace");
}

#[test]
fn the_commands_the_guide_gives_name_crates_that_exist() {
  // A `-p <crate>` in the guide is something a reader will type. One giving a crate that has
  // been renamed fails for them rather than here, which is the wrong way round.
  let text = guide();
  let workspace = workspace_crates();
  for (index, _) in text.match_indices("-p ") {
    let rest = &text[index + 3..];
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    if name.starts_with("xenolith") {
      assert!(workspace.contains(&name), "the guide runs `-p {name}`, which is not a crate of this workspace");
    }
  }
}
